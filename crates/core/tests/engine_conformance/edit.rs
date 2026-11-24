//! Whether one or all of rows the same in every column may be offered
//! (`EditRules::duplicates_choice`), by what the session that read the table's structure and the
//! session that applies the change found their server to be, and the format the change is
//! written in. An engine that takes no edits answers its reason for every case.

use crate::Case;
use crate::Engine::{self, MariaDb, MySql, Pg};
use datarig_core::driver::{EditFormat, MySqlEdit};
use datarig_core::engine::{EngineFacts, EngineId, Fact, MyFacts, PgFacts, ServerFacts, Topology, Version, Why};

/// The two sessions and the format.
pub struct Sessions {
    /// The version the session that read the structure reports (`None`: it said nothing).
    pub read: Option<Version>,
    /// The version the applying session reports (`None`: it said nothing).
    pub applies: Option<Version>,
    /// The format is the one the applying session takes.
    pub its_format: bool,
}

/// What a session of engine `e` at `v` found its server to be, its reports agreeing.
fn facts(e: EngineId, v: Version) -> ServerFacts {
    let my = MyFacts::new(v, e);
    ServerFacts {
        version: Fact::Known(v),
        version_text: None,
        provider: Fact::Unknown(Why::NotChecked),
        topology: Fact::Known(Topology::Direct),
        detail: match e {
            EngineId::Postgres => EngineFacts::Postgres(PgFacts::default()),
            EngineId::MySql => EngineFacts::MySql(my),
            EngineId::MariaDb => EngineFacts::MariaDb(my),
        },
    }
}

/// The answer of engine `e`'s rules for `s`.
pub fn choice(e: Engine, s: &Sessions) -> String {
    let id = e.dialect().engine();
    let rules = match &id.spec().edit {
        Fact::Known(r) => *r,
        Fact::No(why) => return format!("No({why:?})"),
        Fact::Unknown(why) => return format!("Unknown({why:?})"),
    };
    let (read, applies) = (s.read.map(|v| facts(id, v)), s.applies.map(|v| facts(id, v)));
    let format = match (id, s.its_format) {
        (EngineId::Postgres, _) => EditFormat::Postgres,
        (EngineId::MySql | EngineId::MariaDb, true) => EditFormat::MySql(MySqlEdit { nowait: true }),
        (EngineId::MySql | EngineId::MariaDb, false) => EditFormat::MySql(MySqlEdit { nowait: false }),
    };
    format!("{:?}", rules.duplicates_choice(read.as_ref(), applies.as_ref(), format))
}

const V846: Version = Version::new(8, 4, 6);
const NOT_BUILT: (Engine, &str) = (MariaDb, "No(NotBuilt)");

pub fn cases() -> Vec<Case<Sessions>> {
    vec![
        Case {
            name: "one version, its format",
            input: Sessions { read: Some(V846), applies: Some(V846), its_format: true },
            expect: &[(Pg, "Known(())"), (MySql, "Known(())"), NOT_BUILT],
        },
        Case {
            name: "the reading session said nothing",
            input: Sessions { read: None, applies: Some(V846), its_format: true },
            expect: &[(Pg, "Known(())"), (MySql, "Unknown(NotChecked)"), NOT_BUILT],
        },
        Case {
            name: "the applying session said nothing",
            input: Sessions { read: Some(V846), applies: None, its_format: true },
            expect: &[(Pg, "Known(())"), (MySql, "Unknown(NotChecked)"), NOT_BUILT],
        },
        Case {
            name: "two versions",
            input: Sessions { read: Some(Version::new(8, 0, 15)), applies: Some(V846), its_format: true },
            expect: &[(Pg, "Known(())"), (MySql, "No(SessionsDiffer)"), NOT_BUILT],
        },
        Case {
            name: "one old version",
            input: Sessions {
                read: Some(Version::new(8, 0, 15)),
                applies: Some(Version::new(8, 0, 15)),
                its_format: true,
            },
            expect: &[
                (Pg, "Known(())"),
                (MySql, "No(VersionTooOld { needed: Version { major: 8, minor: 0, patch: 17 } })"),
                NOT_BUILT,
            ],
        },
        Case {
            name: "another format",
            input: Sessions { read: Some(V846), applies: Some(V846), its_format: false },
            expect: &[(Pg, "Known(())"), (MySql, "No(EditFormatDiffers)"), NOT_BUILT],
        },
    ]
}
