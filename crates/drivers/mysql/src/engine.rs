//! What the driver knows of the engines it serves, MySQL and MariaDB: what it offers before any
//! server answered ([`CAPABILITIES`]), the one reader of a server's greeting ([`greeting`]), the
//! read of the server's own version line, which names the engine ([`confirm`]), the facts a
//! session's connection found ([`facts`]) and the one place that turns them into what a session
//! says of its server ([`derive`]), with the dialect its text is read in ([`dialect`]); and,
//! per engine of the family ([`family::Family`]), how a session is set up on it and how the
//! app's own reads are written.

mod family;
mod mariadb;
mod mysql;

pub(crate) use family::{Family, family};

use crate::session::{Tracked, my_error};
use datarig_core::driver::{Capabilities, DbError, EditFormat, ExplainFormat, Hierarchy, MySqlEdit};
use datarig_core::engine::mysql_family::line_names_mariadb;
use datarig_core::engine::{
    EngineFacts, EngineId, Fact, MyFacts, PingIdentity, ServerFacts, Support, Topology, Version, Why,
};
use datarig_core::sql::dialect::{Dialect, DollarQuotes, Language, MySqlMode};
use datarig_core::sql::risk::mysql::{Judge, Judged};
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
        dollar_quotes: DollarQuotes::NotRead,
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
/// server's `init_connect`) does not empty the answer. A results character set the session
/// starts with that is not ASCII-compatible (`utf16`, `utf32`, `ucs2`) makes the answer's column
/// definitions unreadable to the client; the read then fails with its own error, as every read
/// of such a session always has (the session's settings keep the character set it starts with).
pub(crate) const VERSION_SQL: &str = "SELECT VERSION() LIMIT 1";

/// The server's version line and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VersionReport {
    /// The line as the server sent it.
    pub(crate) line: String,
    /// The engine the line names.
    pub(crate) engine: EngineId,
    /// Its number when it is the greeting's; unknown when the line has none
    /// ([`Why::VersionUnknown`]), another one ([`Why::VersionsDisagree`]) or names another
    /// engine than the greeting ([`Why::EnginesDisagree`]).
    pub(crate) version: Fact<Version>,
}

/// Ask the server of greeting `g` for its version line ([`VERSION_SQL`]) and read it ([`judge`]).
/// A failed read is its own error.
pub(crate) async fn confirm(conn: &mut Conn, g: &Greeting) -> Result<VersionReport, DbError> {
    let row: Option<Row> = conn.query_first(VERSION_SQL).await.map_err(|e| my_error(&e))?;
    // A row of one value; any other shape is no answer.
    judge(g, row.map(|row| mysql_async::from_row_opt::<Value>(row).unwrap_or(Value::NULL)))
}

/// What the answer to [`VERSION_SQL`] (`None`: no row) says of the server of greeting `g`. No
/// line (no row, `NULL`, bytes that are not UTF-8, a value that is not text) or a blank one
/// confirms no engine ([`DbError::EngineUnconfirmed`]). Otherwise the engine is the one the line
/// names; the version is the greeting's number when the line names the greeting's engine and
/// starts with that number, and unknown otherwise.
pub(crate) fn judge(g: &Greeting, answer: Option<Value>) -> Result<VersionReport, DbError> {
    let greeting = || g.label();
    let line = match answer {
        Some(Value::Bytes(bytes)) => String::from_utf8(bytes).ok(),
        _ => None,
    };
    let Some(line) = line.filter(|l| !l.trim().is_empty()) else {
        return Err(DbError::EngineUnconfirmed { greeting: greeting() });
    };
    let mariadb = line_names_mariadb(&line);
    let version = match Version::parse(&line) {
        _ if mariadb != g.mariadb => Fact::Unknown(Why::EnginesDisagree),
        Some(v) if v == g.handshake => Fact::Known(v),
        Some(_) => Fact::Unknown(Why::VersionsDisagree),
        None => Fact::Unknown(Why::VersionUnknown),
    };
    Ok(VersionReport { line, engine: engine_of(mariadb), version })
}

#[cfg(test)]
impl VersionReport {
    /// What the server of greeting `g` answers to [`VERSION_SQL`] when nothing is in between:
    /// its number, and `-MariaDB` on MariaDB.
    pub(crate) fn agreeing(g: &Greeting) -> VersionReport {
        let v = g.handshake;
        let line = format!("{}.{}.{}{}", v.major, v.minor, v.patch, if g.mariadb { "-MariaDB" } else { "" });
        judge(g, Some(Value::Bytes(line.into_bytes()))).expect("a line")
    }
}

/// MariaDB or MySQL.
fn engine_of(mariadb: bool) -> EngineId {
    if mariadb { EngineId::MariaDb } else { EngineId::MySql }
}

impl Greeting {
    /// A server of no version the driver reads: refused when checked, with no plan format and
    /// no edits.
    pub(crate) const UNREAD: Greeting = Greeting { mariadb: false, handshake: Version::new(0, 0, 0) };

    /// The rules of the engine it names.
    pub(crate) fn family(&self) -> &'static dyn Family {
        family(engine_of(self.mariadb))
    }

    /// `MySQL 8.4.6`, `MariaDB 11.4.2`.
    pub(crate) fn label(&self) -> String {
        family::label(self.family(), self.handshake)
    }

    /// Refused when it is older than the driver supports (or says no version it can read).
    pub(crate) fn check(&self) -> Result<(), DbError> {
        family::check(self.family(), self.handshake)
    }

    /// The session variable that makes its server's transactions read-only, on its engine at
    /// version `v` ([`session_version`]): `tx_read_only` on MariaDB before 11.1 or of a version
    /// not known.
    pub(crate) fn read_only_var(&self, v: &Fact<Version>) -> &'static str {
        self.family().read_only_var(v)
    }

    /// The session variable of its server's transactions' isolation level, on its engine at
    /// version `v` ([`session_version`]): `tx_isolation` on MariaDB before 11.1 or of a
    /// version not known.
    pub(crate) fn isolation_var(&self, v: &Fact<Version>) -> &'static str {
        self.family().isolation_var(v)
    }

    /// The greeting the facts of a session keep (its engine too, when the version line names
    /// the other one).
    pub(crate) fn of(facts: &ServerFacts) -> Greeting {
        match &facts.detail {
            EngineFacts::MySql(my) | EngineFacts::MariaDb(my) => greeting_of(my),
            // Never what this driver's sessions find.
            EngineFacts::Postgres(_) => Greeting::UNREAD,
        }
    }
}

/// MySQL's rules of the family, for what only MySQL's plans are asked with (an estimate reads
/// MySQL's tree plan).
pub(crate) fn mysql_family() -> &'static dyn Family {
    family(EngineId::MySql)
}

/// The rules of MariaDB (`true`) or MySQL, for tests that name an engine by the greeting's flag.
#[cfg(test)]
pub(crate) fn family_of(mariadb: bool) -> &'static dyn Family {
    family(engine_of(mariadb))
}

/// The greeting the facts `my` keep.
fn greeting_of(my: &MyFacts) -> Greeting {
    let mariadb = match my.greeting {
        EngineId::MariaDb => true,
        EngineId::MySql | EngineId::Postgres => false,
    };
    Greeting { mariadb, handshake: my.handshake }
}

/// The facts of a session's connection to the server of greeting `g` and version line `report`,
/// once it opened: the engine is the one the line names. Whether its connection id is the
/// server's own (`own`, read as it opened; `None`: not read) says whether it reaches the server
/// directly, unless the greeting names the other engine: then something in between wrote it,
/// and how the session reaches the server is not known. Its `performance_schema` not read.
pub(crate) fn facts(g: &Greeting, report: &VersionReport, own: Option<bool>) -> Arc<ServerFacts> {
    opened_facts(g, report, own, Fact::Unknown(Why::NotChecked), Fact::Unknown(Why::NotChecked))
}

/// [`facts`], with whether the server's `performance_schema` is on and whether it listed the
/// session's marker on its connection, as the connection read them.
pub(crate) fn opened_facts(
    g: &Greeting,
    report: &VersionReport,
    own: Option<bool>,
    performance_schema: Support,
    marker: Support,
) -> Arc<ServerFacts> {
    let my = MyFacts { performance_schema, marker, ..MyFacts::new(g.handshake, engine_of(g.mariadb)) };
    let topology = match own {
        _ if report.engine != my.greeting => Fact::Unknown(Why::EnginesDisagree),
        Some(true) => Fact::Known(Topology::Direct),
        Some(false) => Fact::Known(Topology::Pooled),
        None => Fact::Unknown(Why::NotChecked),
    };
    Arc::new(ServerFacts {
        version: report.version.clone(),
        version_text: Some(report.line.clone()),
        provider: Fact::Unknown(Why::NotChecked),
        topology,
        detail: match report.engine {
            EngineId::MariaDb => EngineFacts::MariaDb(my),
            EngineId::MySql | EngineId::Postgres => EngineFacts::MySql(my),
        },
    })
}

/// The version a session on the server of `facts` names its settings by, on its greeting's
/// engine ([`Greeting::of`]). When the greeting and the version line name one engine, the
/// version the facts know, which may be none (the two numbers differ, or the line has none):
/// then the names are those every version of the engine has. When they name two engines, the
/// greeting's number, so the names are the ones the greeting implies.
pub(crate) fn session_version(facts: &ServerFacts) -> Fact<Version> {
    match facts.reports_disagree() {
        true => Fact::Known(Greeting::of(facts).handshake),
        false => facts.version.clone(),
    }
}

/// The version a table's structure on the server of `facts` is read for
/// ([`Family::structure`]): the version the facts know, which is then the greeting's number;
/// when they know none (the greeting and the version line give two numbers, the line has none,
/// or they name two engines), the greeting's number, so the read is the one the greeting
/// implies. Its limit, behind something that greets with another MySQL than the server's: with
/// an older greeting, below 8.0.13 a table with a functional key part fails the read (the part
/// reads as a key part of nothing), and below 8.0.16 no CHECK constraints are listed, a global
/// grant is counted though `partial_revokes` may take it back, and schema grants are compared
/// by pattern; with a newer greeting, the read fails with the server's error only when the
/// greeting's form names a catalogue part the server lacks (`EXPRESSION` from 8.0.13;
/// `CHECK_CONSTRAINTS`, `ENFORCED` and `partial_revokes` from 8.0.16).
pub(crate) fn structure_version(facts: &ServerFacts) -> Version {
    match &facts.version {
        Fact::Known(v) => *v,
        Fact::No(_) | Fact::Unknown(_) => Greeting::of(facts).handshake,
    }
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
    /// The plan format the server gives, as the plan rules of the greeting's engine answer
    /// ([`datarig_core::engine::PlanRules`]): MySQL's tree from 8.0.16 on (`EXPLAIN FORMAT=TREE`;
    /// `EXPLAIN ANALYZE` from 8.0.18, an older one's error is the server's), none on MariaDB
    /// (its plans are shaped otherwise).
    pub(crate) explain: Option<ExplainFormat>,
    /// The edits the server takes, as the edit rules of the greeting's engine answer
    /// ([`datarig_core::engine::EditRules`]): MySQL's from 8.0.17 on (`utf8mb4_0900_bin`
    /// compares text exactly, `FOR UPDATE NOWAIT` locks without waiting), none on MariaDB (its
    /// collations and row locks differ).
    pub(crate) edit: Option<EditFormat>,
    /// The driver's capabilities with the server's answers in them.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) capabilities: Capabilities,
}

/// What a session on the server of `facts` says of it, with the session in the state
/// `tracked`: the one place these answers are made. They follow the server's greeting, as they
/// did before the version line was read, whichever engine the line names.
pub(crate) fn derive(facts: &ServerFacts, tracked: &Tracked) -> Derived {
    // The greeting's engine and number, whichever engine the version line names.
    let greeting = Greeting::of(facts);
    let language = Language::Sql(dialect(&greeting, tracked.mode(dollar_quotes(facts))));
    // The plan rules of the greeting's engine, as `engine::plan_of` asks them for every facts a
    // session of this driver finds; on facts it never finds (PostgreSQL's), MySQL's say none.
    let explain = match engine_of(greeting.mariadb).spec().plan {
        Fact::Known(rules) => match rules.format(facts) {
            Fact::Known(format) => Some(format),
            Fact::No(_) | Fact::Unknown(_) => None,
        },
        Fact::No(_) | Fact::Unknown(_) => None,
    };
    // The edit rules of the greeting's engine, which read the greeting's number.
    let edit = match engine_of(greeting.mariadb).spec().edit {
        Fact::Known(rules) => match rules.writer(facts) {
            Fact::Known(format) => Some(format),
            Fact::No(_) | Fact::Unknown(_) => None,
        },
        Fact::No(_) | Fact::Unknown(_) => None,
    };
    Derived { language, explain, edit, capabilities: Capabilities { language, explain, edit, ..CAPABILITIES } }
}

/// Whether a session on the server of `facts` reads dollar quotes: as the rules of the engine
/// its version line names say ([`datarig_core::engine::TextRules`]).
pub(crate) fn dollar_quotes(facts: &ServerFacts) -> DollarQuotes {
    facts.engine().spec().text.dollar_quotes(facts)
}

/// The dialect a session of greeting `g` reads its text in, in sql mode `mode`: MariaDB's when
/// the greeting names MariaDB, else MySQL's.
pub(crate) fn dialect(g: &Greeting, mode: MySqlMode) -> Dialect {
    match engine_of(g.mariadb) {
        EngineId::MariaDb => Dialect::MariaDb(mode),
        EngineId::MySql | EngineId::Postgres => Dialect::MySql(mode),
    }
}

/// How the classifier judges text of a session's dialect `d`: on the lists of its engine, and
/// of the other engine of the family too when `judged` says so (the session's facts,
/// [`datarig_core::engine::ServerFacts::judged`]), in its sql mode.
pub(crate) fn risk_judge(d: Dialect, judged: Judged) -> Judge {
    match d {
        Dialect::MariaDb(mode) => Judge::mariadb(mode).judged(judged),
        Dialect::MySql(mode) => Judge::mysql(mode).judged(judged),
        // Never a dialect of this driver's sessions: MySQL's default reading.
        Dialect::Postgres => Judge::mysql(MySqlMode::default()).judged(judged),
    }
}

/// The dialects a session of dialect `d` judged on `judged` reads its text in for what only an
/// engine's reading tells (whether an executable comment runs): its own, and with its sibling
/// the other engine's of the family in the same sql mode.
pub(crate) fn judged_dialects(d: Dialect, judged: Judged) -> Vec<Dialect> {
    match judged {
        Judged::Own => vec![d],
        Judged::WithSibling => match d {
            Dialect::MySql(mode) => vec![d, Dialect::MariaDb(mode)],
            Dialect::MariaDb(mode) => vec![d, Dialect::MySql(mode)],
            // Never a dialect of this driver's sessions.
            Dialect::Postgres => vec![d],
        },
    }
}

/// The ways a session of dialect `d` may read its text: its own, or, when whether its server
/// reads dollar quotes is not known, without them and with them.
pub(crate) fn dollar_readings(d: Dialect) -> Vec<Dialect> {
    let mode = match d {
        Dialect::MySql(mode) | Dialect::MariaDb(mode) => mode,
        Dialect::Postgres => return vec![d],
    };
    match mode.dollar_quotes {
        DollarQuotes::NotKnown => [DollarQuotes::NotRead, DollarQuotes::Read]
            .map(|dollar_quotes| d.with_mode(MySqlMode { dollar_quotes, ..mode }))
            .to_vec(),
        DollarQuotes::NotRead | DollarQuotes::Read => vec![d],
    }
}

/// Whether a session of dialect `d` reads an executable comment with a MySQL 5.7-or-later
/// version (`/*!80000 … */`) as a plain comment: MariaDB's. Such a text alone sets nothing
/// there.
pub(crate) fn skips_mysql_version_comments(d: Dialect) -> bool {
    match d {
        Dialect::MariaDb(_) => true,
        Dialect::MySql(_) | Dialect::Postgres => false,
    }
}

#[cfg(test)]
pub(crate) mod tests;
