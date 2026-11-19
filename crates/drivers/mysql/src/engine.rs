//! What the driver knows of the engines it serves, MySQL and MariaDB: what it offers before any
//! server answered ([`CAPABILITIES`]), the one reader of a server's greeting ([`greeting`]), the
//! check that the server's own version line names the engine the greeting does ([`confirm`]),
//! the facts a session's connection found ([`facts`]) and the one place that turns them into
//! what a session says of its server ([`derive`]).

use crate::session::{Server, Tracked, my_error};
use datarig_core::driver::{Capabilities, DbError, EditFormat, ExplainFormat, Hierarchy, MySqlEdit};
use datarig_core::engine::mysql_family::line_names_mariadb;
use datarig_core::engine::{EngineFacts, EngineId, Fact, MyFacts, PingIdentity, ServerFacts, Topology, Version, Why};
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Row, Value};
use std::sync::Arc;

/// The engines the driver serves.
pub(crate) const ENGINES: &[EngineId] = &[EngineId::MySql, EngineId::MariaDb];

/// What the driver offers before a session said what its server takes.
pub(crate) const CAPABILITIES: Capabilities = Capabilities {
    server_paging: true,
    cancel: true,
    introspection: true,
    key_metadata: true,
    contexts: false,
    structure: true,
    ddl: true,
    language: Language::Sql(Dialect::MySql(MySqlMode {
        ansi_quotes: false,
        no_backslash_escapes: false,
        dollar_quotes: false,
        mariadb: false,
        ignore_space: false,
    })),
    hierarchy: Hierarchy::SchemaOnly,
    explain: Some(ExplainFormat::MySqlTree),
    reads_held_to_end: true,
    export: true,
    manual_commit: true,
    edit: Some(EditFormat::MySql(MySqlEdit { nowait: true })),
    monitor: true,
};

/// What a server's greeting says of it: whether it is MariaDB, and its version number (MariaDB's
/// own, past the `5.5.5-` it starts with; 0.0.0 when the client could not read one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Greeting {
    pub(crate) mariadb: bool,
    pub(crate) handshake: Version,
}

/// The greeting of the server `conn` logged in to.
pub(crate) fn greeting(conn: &Conn) -> Greeting {
    let (major, minor, patch) = conn.server_version();
    Greeting { mariadb: conn.is_mariadb(), handshake: Version::new(major, minor, patch) }
}

/// The server's own version line, asked first on every session and in a test connection. With
/// `LIMIT 1`, a `sql_select_limit` of 0 the session starts with (left by a proxy or the
/// server's `init_connect`) does not empty the answer.
pub(crate) const VERSION_SQL: &str = "SELECT VERSION() LIMIT 1";

/// The server's version line, once it names the engine the greeting does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VersionReport {
    /// The line as the server sent it.
    pub(crate) line: String,
    /// Its number when it is the greeting's; unknown when the line has none
    /// ([`Why::VersionUnknown`]) or another one ([`Why::VersionsDisagree`]).
    pub(crate) version: Fact<Version>,
}

/// Ask the server of greeting `g` for its version line ([`VERSION_SQL`]) and check it names the
/// same engine ([`judge`]). A failed read is its own error.
pub(crate) async fn confirm(conn: &mut Conn, g: &Greeting) -> Result<VersionReport, DbError> {
    let row: Option<Row> = conn.query_first(VERSION_SQL).await.map_err(|e| my_error(&e))?;
    // A row of one value; any other shape is no answer.
    judge(g, row.map(|row| mysql_async::from_row_opt::<Value>(row).unwrap_or(Value::NULL)))
}

/// What the answer to [`VERSION_SQL`] (`None`: no row) says of the server of greeting `g`. No
/// line (no row, `NULL`, bytes that are not UTF-8, a value that is not text) or a blank one
/// confirms no engine ([`DbError::EngineUnconfirmed`]); a line naming the other engine than the
/// greeting is refused ([`DbError::EnginesDisagree`]). Otherwise the version is the greeting's
/// number when the line starts with it.
pub(crate) fn judge(g: &Greeting, answer: Option<Value>) -> Result<VersionReport, DbError> {
    let greeting = || Server::of_greeting(g).label();
    let line = match answer {
        Some(Value::Bytes(bytes)) => String::from_utf8(bytes).ok(),
        _ => None,
    };
    let Some(line) = line.filter(|l| !l.trim().is_empty()) else {
        return Err(DbError::EngineUnconfirmed { greeting: greeting() });
    };
    if line_names_mariadb(&line) != g.mariadb {
        return Err(DbError::EnginesDisagree { greeting: greeting(), version: line });
    }
    let version = match Version::parse(&line) {
        Some(v) if v == g.handshake => Fact::Known(v),
        Some(_) => Fact::Unknown(Why::VersionsDisagree),
        None => Fact::Unknown(Why::VersionUnknown),
    };
    Ok(VersionReport { line, version })
}

#[cfg(test)]
impl VersionReport {
    /// What the server of greeting `g` answers to [`VERSION_SQL`] when nothing is in between:
    /// its number, and `-MariaDB` on MariaDB.
    pub(crate) fn agreeing(g: &Greeting) -> VersionReport {
        let v = g.handshake;
        let line = format!("{}.{}.{}{}", v.major, v.minor, v.patch, if g.mariadb { "-MariaDB" } else { "" });
        judge(g, Some(Value::Bytes(line.into_bytes()))).expect("the line names the greeting's engine")
    }
}

impl Server {
    /// A server of no version the driver reads: refused when checked, with no plan format and
    /// no edits.
    pub(crate) const UNREAD: Server = Server { version: (0, 0, 0), mariadb: false };

    /// The server as its greeting says.
    pub(crate) fn of_greeting(g: &Greeting) -> Server {
        let v = g.handshake;
        Server { version: (v.major, v.minor, v.patch), mariadb: g.mariadb }
    }

    /// The server the facts of a session found, as the greeting said it.
    pub(crate) fn of(facts: &ServerFacts) -> Server {
        let of = |my: &MyFacts, mariadb| Server::of_greeting(&Greeting { mariadb, handshake: my.handshake });
        match &facts.detail {
            EngineFacts::MySql(my) => of(my, false),
            EngineFacts::MariaDb(my) => of(my, true),
            // Never what this driver's sessions find.
            EngineFacts::Postgres(_) => Server::UNREAD,
        }
    }
}

/// The facts of a session's connection to the server of greeting `g` and version line `report`,
/// once it opened: whether its connection id is the server's own (`own`, read as it opened;
/// `None`: not read) says whether it reaches the server directly.
pub(crate) fn facts(g: &Greeting, report: &VersionReport, own: Option<bool>) -> Arc<ServerFacts> {
    let my = MyFacts { handshake: g.handshake };
    let topology = match own {
        Some(true) => Fact::Known(Topology::Direct),
        Some(false) => Fact::Known(Topology::Pooled),
        None => Fact::Unknown(Why::NotChecked),
    };
    Arc::new(ServerFacts {
        version: report.version.clone(),
        version_text: Some(report.line.clone()),
        provider: Fact::Unknown(Why::NotChecked),
        topology,
        detail: match g.mariadb {
            true => EngineFacts::MariaDb(my),
            false => EngineFacts::MySql(my),
        },
    })
}

/// What a test connection to the server of greeting `g` and version line `report` says of it.
pub(crate) fn identity(g: &Greeting, report: &VersionReport) -> PingIdentity {
    let facts = facts(g, report, None);
    PingIdentity { engine: Fact::Known(facts.engine()), version: facts.version.clone() }
}

/// What a session says of its server and of how it reads text now.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Derived {
    /// The language the session's text is read in: the server's, in the session's sql mode.
    pub(crate) language: Language,
    /// The plan format the server gives: MySQL's tree from 8.0.16 on (`EXPLAIN FORMAT=TREE`;
    /// `EXPLAIN ANALYZE` from 8.0.18, an older one's error is the server's), none on MariaDB
    /// (its plans are shaped otherwise).
    pub(crate) explain: Option<ExplainFormat>,
    /// The edits the server takes: MySQL's from 8.0.17 on (`utf8mb4_0900_bin` compares text
    /// exactly, `FOR UPDATE NOWAIT` locks without waiting), none on MariaDB (its collations and
    /// row locks differ).
    pub(crate) edit: Option<EditFormat>,
    /// The driver's capabilities with the server's answers in them.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) capabilities: Capabilities,
}

/// What a session on the server of `facts` says of it, with the session in the state
/// `tracked`: the one place these answers are made.
pub(crate) fn derive(facts: &ServerFacts, tracked: &Tracked) -> Derived {
    let language = Language::Sql(Dialect::MySql(tracked.mode(&Server::of(facts))));
    let (explain, edit) = match &facts.detail {
        EngineFacts::MySql(my) => (
            (my.handshake >= Version::new(8, 0, 16)).then_some(ExplainFormat::MySqlTree),
            (my.handshake >= Version::new(8, 0, 17)).then_some(EditFormat::MySql(MySqlEdit { nowait: true })),
        ),
        EngineFacts::MariaDb(_) | EngineFacts::Postgres(_) => (None, None),
    };
    Derived { language, explain, edit, capabilities: Capabilities { language, explain, edit, ..CAPABILITIES } }
}

#[cfg(test)]
mod tests;
