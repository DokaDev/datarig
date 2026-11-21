//! Whether a session of a read-only profile opens, by its role and what its connection proved
//! (`SessionRules::read_only_open`): every role and every proof on every engine, those no driver
//! gives today included (they are refused). No answer depends on the server's version.

use crate::Case;
use crate::Engine::{self, MariaDb, MySql, Pg};
use datarig_core::driver::SessionRole;
use datarig_core::engine::ReadOnlyProof;

/// A session's role and what its connection proved.
pub struct Opening {
    pub role: SessionRole,
    pub proof: Proof,
}

/// What a connection proved ([`ReadOnlyProof`]; an unconfirmed one says what the server
/// answered).
#[derive(Clone, Copy)]
pub enum Proof {
    Confirmed,
    PerTransaction,
    Unconfirmed,
    NotAsked,
}

/// The verdict of engine `e`'s rules on `o`.
pub fn verdict(e: Engine, o: &Opening) -> String {
    let proof = match o.proof {
        Proof::Confirmed => ReadOnlyProof::Confirmed,
        Proof::PerTransaction => ReadOnlyProof::PerTransaction,
        Proof::Unconfirmed => ReadOnlyProof::Unconfirmed("what the server answered".to_string()),
        Proof::NotAsked => ReadOnlyProof::NotAsked,
    };
    format!("{:?}", e.dialect().engine().spec().session.read_only_open(o.role, proof))
}

pub fn cases() -> Vec<Case<Opening>> {
    vec![
        Case {
            name: "query session, confirmed",
            input: Opening { role: SessionRole::Query, proof: Proof::Confirmed },
            expect: &[(Pg, "Open"), (MySql, "Open"), (MariaDb, "Open")],
        },
        Case {
            name: "query session, read-only per transaction",
            input: Opening { role: SessionRole::Query, proof: Proof::PerTransaction },
            expect: &[
                (Pg, "OpenPerTransaction"),
                (MySql, "Refused(\"the session's read-only setting was not confirmed\")"),
                (MariaDb, "Refused(\"the session's read-only setting was not confirmed\")"),
            ],
        },
        Case {
            name: "query session, unconfirmed",
            input: Opening { role: SessionRole::Query, proof: Proof::Unconfirmed },
            expect: &[
                (Pg, "Refused(\"what the server answered\")"),
                (MySql, "Refused(\"what the server answered\")"),
                (MariaDb, "Refused(\"what the server answered\")"),
            ],
        },
        Case {
            name: "query session, not asked",
            input: Opening { role: SessionRole::Query, proof: Proof::NotAsked },
            expect: &[
                (Pg, "Refused(\"the session's read-only setting was not confirmed\")"),
                (MySql, "Refused(\"the session's read-only setting was not confirmed\")"),
                (MariaDb, "Refused(\"the session's read-only setting was not confirmed\")"),
            ],
        },
        Case {
            name: "metadata session, confirmed",
            input: Opening { role: SessionRole::Meta, proof: Proof::Confirmed },
            expect: &[(Pg, "Open"), (MySql, "Open"), (MariaDb, "Open")],
        },
        Case {
            name: "metadata session, read-only per transaction",
            input: Opening { role: SessionRole::Meta, proof: Proof::PerTransaction },
            expect: &[
                (Pg, "OpenPerTransaction"),
                (MySql, "Refused(\"the session's read-only setting was not confirmed\")"),
                (MariaDb, "Refused(\"the session's read-only setting was not confirmed\")"),
            ],
        },
        Case {
            name: "metadata session, unconfirmed",
            input: Opening { role: SessionRole::Meta, proof: Proof::Unconfirmed },
            expect: &[
                (Pg, "Refused(\"what the server answered\")"),
                (MySql, "OpenUnconfirmed(\"what the server answered\")"),
                (MariaDb, "OpenUnconfirmed(\"what the server answered\")"),
            ],
        },
        Case {
            name: "metadata session, not asked",
            input: Opening { role: SessionRole::Meta, proof: Proof::NotAsked },
            expect: &[
                (Pg, "Refused(\"the session's read-only setting was not confirmed\")"),
                (MySql, "Refused(\"the session's read-only setting was not confirmed\")"),
                (MariaDb, "Refused(\"the session's read-only setting was not confirmed\")"),
            ],
        },
        Case {
            name: "monitor session, confirmed",
            input: Opening { role: SessionRole::Monitor, proof: Proof::Confirmed },
            expect: &[(Pg, "Open"), (MySql, "Open"), (MariaDb, "Open")],
        },
        Case {
            name: "monitor session, read-only per transaction",
            input: Opening { role: SessionRole::Monitor, proof: Proof::PerTransaction },
            expect: &[
                (Pg, "OpenPerTransaction"),
                (MySql, "Refused(\"the session's read-only setting was not confirmed\")"),
                (MariaDb, "Refused(\"the session's read-only setting was not confirmed\")"),
            ],
        },
        Case {
            name: "monitor session, unconfirmed",
            input: Opening { role: SessionRole::Monitor, proof: Proof::Unconfirmed },
            expect: &[
                (Pg, "Refused(\"what the server answered\")"),
                (MySql, "OpenUnconfirmed(\"what the server answered\")"),
                (MariaDb, "OpenUnconfirmed(\"what the server answered\")"),
            ],
        },
        Case {
            name: "monitor session, not asked",
            input: Opening { role: SessionRole::Monitor, proof: Proof::NotAsked },
            expect: &[
                (Pg, "Open"),
                (MySql, "Refused(\"the session's read-only setting was not confirmed\")"),
                (MariaDb, "Refused(\"the session's read-only setting was not confirmed\")"),
            ],
        },
    ]
}
