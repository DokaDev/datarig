//! What each engine's sessions may be given and how they stand to a transaction, asked of the
//! engine of a session's facts (`TxRules`, `SessionRules`). None of these answers depends on
//! the server's version: a session whose version is not known gets the same answer as one
//! whose version is.

use crate::Case;
use crate::Engine::{
    self, MariaDb, MariaDbVersionUnknown, MariaDbVersionsDisagree, MySql, MySqlVersionUnknown, MySqlVersionsDisagree,
    Pg, PgVersionUnknown,
};
use datarig_core::engine::{EngineFacts, EngineId, Fact, MyFacts, PgFacts, ServerFacts, Topology, Version, Why};

/// A question of the engine's rules.
pub enum Question {
    /// `SessionRules::server_idle_limit`.
    ServerIdleLimit,
    /// `TxRules::rollback_wait`.
    RollbackWait,
    /// `TxRules::count_in_tx`.
    CountInTx,
    /// `TxRules::reads_lock_rows`.
    ReadsLockRows,
}

/// The facts of a session of column `e`: PostgreSQL 17.2, MySQL 8.4.6, MariaDB 11.4.13 reached
/// directly, or of the same engine with a version that is not known.
fn facts(e: Engine) -> ServerFacts {
    let my = |engine, v| MyFacts { handshake: v, greeting: engine };
    let (version, detail) = match e {
        Pg => (Fact::Known(Version::new(17, 2, 0)), EngineFacts::Postgres(PgFacts::default())),
        PgVersionUnknown => (Fact::Unknown(Why::VersionUnknown), EngineFacts::Postgres(PgFacts::default())),
        MySql => (Fact::Known(Version::new(8, 4, 6)), EngineFacts::MySql(my(EngineId::MySql, Version::new(8, 4, 6)))),
        MySqlVersionUnknown => {
            (Fact::Unknown(Why::VersionUnknown), EngineFacts::MySql(my(EngineId::MySql, Version::new(8, 4, 6))))
        }
        MySqlVersionsDisagree => {
            (Fact::Unknown(Why::VersionsDisagree), EngineFacts::MySql(my(EngineId::MySql, Version::new(8, 0, 36))))
        }
        MariaDb => {
            (Fact::Known(Version::new(11, 4, 13)), EngineFacts::MariaDb(my(EngineId::MariaDb, Version::new(11, 4, 13))))
        }
        MariaDbVersionUnknown => {
            (Fact::Unknown(Why::VersionUnknown), EngineFacts::MariaDb(my(EngineId::MariaDb, Version::new(11, 4, 13))))
        }
        MariaDbVersionsDisagree => {
            (Fact::Unknown(Why::VersionsDisagree), EngineFacts::MariaDb(my(EngineId::MariaDb, Version::new(11, 8, 0))))
        }
        other => panic!("{other:?} is not a column of the session rules"),
    };
    ServerFacts {
        version,
        version_text: None,
        provider: Fact::Unknown(Why::NotChecked),
        topology: Fact::Known(Topology::Direct),
        detail,
    }
}

/// The answer to `q` of the rules of the engine of a session of column `e`.
pub fn answer(e: Engine, q: &Question) -> String {
    let spec = facts(e).engine().spec();
    match q {
        Question::ServerIdleLimit => format!("{:?}", spec.session.server_idle_limit()),
        Question::RollbackWait => format!("{:?}", spec.tx.rollback_wait()),
        Question::CountInTx => format!("{:?}", spec.tx.count_in_tx()),
        Question::ReadsLockRows => format!("{}", spec.tx.reads_lock_rows()),
    }
}

pub fn cases() -> Vec<Case<Question>> {
    vec![
        Case {
            name: "server idle limit",
            input: Question::ServerIdleLimit,
            expect: &[
                (Pg, "Known(())"),
                (PgVersionUnknown, "Known(())"),
                (MySql, "No(NotOnEngine)"),
                (MySqlVersionUnknown, "No(NotOnEngine)"),
                (MySqlVersionsDisagree, "No(NotOnEngine)"),
                (MariaDb, "No(NotOnEngine)"),
                (MariaDbVersionUnknown, "No(NotOnEngine)"),
                (MariaDbVersionsDisagree, "No(NotOnEngine)"),
            ],
        },
        Case {
            name: "rollback wait",
            input: Question::RollbackWait,
            expect: &[
                (Pg, "10s"),
                (PgVersionUnknown, "10s"),
                (MySql, "60s"),
                (MySqlVersionUnknown, "60s"),
                (MySqlVersionsDisagree, "60s"),
                (MariaDb, "60s"),
                (MariaDbVersionUnknown, "60s"),
                (MariaDbVersionsDisagree, "60s"),
            ],
        },
        Case {
            name: "count in a transaction",
            input: Question::CountInTx,
            expect: &[
                (Pg, "LeftOut"),
                (PgVersionUnknown, "LeftOut"),
                (MySql, "Limited"),
                (MySqlVersionUnknown, "Limited"),
                (MySqlVersionsDisagree, "Limited"),
                (MariaDb, "Limited"),
                (MariaDbVersionUnknown, "Limited"),
                (MariaDbVersionsDisagree, "Limited"),
            ],
        },
        Case {
            name: "reads lock rows",
            input: Question::ReadsLockRows,
            expect: &[
                (Pg, "false"),
                (PgVersionUnknown, "false"),
                (MySql, "true"),
                (MySqlVersionUnknown, "true"),
                (MySqlVersionsDisagree, "true"),
                (MariaDb, "true"),
                (MariaDbVersionUnknown, "true"),
                (MariaDbVersionsDisagree, "true"),
            ],
        },
    ]
}
