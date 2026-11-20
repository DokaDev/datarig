//! What the driver knows of the engines it serves, MySQL and MariaDB: what it offers before any
//! server answered ([`CAPABILITIES`]), the one reader of a server's greeting ([`greeting`]), the
//! facts a session's connection found ([`facts`]) and the one place that turns them into what a
//! session says of its server ([`derive`]).

use crate::session::{Server, Tracked};
use datarig_core::driver::{Capabilities, EditFormat, ExplainFormat, Hierarchy, MySqlEdit};
use datarig_core::engine::{EngineFacts, EngineId, Fact, MyFacts, PingIdentity, ServerFacts, Topology, Version, Why};
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use mysql_async::Conn;
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

/// The facts of a session's connection to the server of greeting `g`, once it opened: whether
/// its connection id is the server's own (`own`, read as it opened; `None`: not read) says
/// whether it reaches the server directly.
pub(crate) fn facts(g: &Greeting, own: Option<bool>) -> Arc<ServerFacts> {
    let my = MyFacts { handshake: g.handshake };
    let topology = match own {
        Some(true) => Fact::Known(Topology::Direct),
        Some(false) => Fact::Known(Topology::Pooled),
        None => Fact::Unknown(Why::NotChecked),
    };
    Arc::new(ServerFacts {
        version: Fact::Known(g.handshake),
        version_text: None,
        provider: Fact::Unknown(Why::NotChecked),
        topology,
        detail: match g.mariadb {
            true => EngineFacts::MariaDb(my),
            false => EngineFacts::MySql(my),
        },
    })
}

/// What a test connection to the server of greeting `g` says of it.
pub(crate) fn identity(g: &Greeting) -> PingIdentity {
    let facts = facts(g, None);
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
