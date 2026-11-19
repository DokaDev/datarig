use super::*;
use datarig_core::engine::PgFacts;

fn my(major: u16, minor: u16, patch: u16) -> Greeting {
    Greeting { mariadb: false, handshake: Version::new(major, minor, patch) }
}

fn maria(major: u16, minor: u16, patch: u16) -> Greeting {
    Greeting { mariadb: true, handshake: Version::new(major, minor, patch) }
}

/// A greeting with what a session on its server says: the plan format, the edits, whether the
/// server reads dollar quotes and whether the text is read as MariaDB's.
type Row = (Greeting, Option<ExplainFormat>, Option<EditFormat>, bool, bool);

/// MySQL and MariaDB greetings around each threshold, with today's answers.
fn table() -> Vec<Row> {
    let tree = Some(ExplainFormat::MySqlTree);
    let edit = Some(EditFormat::MySql(MySqlEdit { nowait: true }));
    vec![
        (my(8, 0, 15), None, None, false, false),
        (my(8, 0, 16), tree, None, false, false),
        (my(8, 0, 17), tree, edit, false, false),
        (my(8, 0, 40), tree, edit, false, false),
        (my(8, 1, 0), tree, edit, true, false),
        (my(8, 4, 6), tree, edit, true, false),
        (maria(10, 6, 0), None, None, false, true),
        (maria(11, 4, 13), None, None, false, true),
    ]
}

#[test]
fn derived_answers_per_engine_and_version() {
    for (g, explain, edit, dollar_quotes, mariadb) in table() {
        for own in [Some(true), Some(false), None] {
            let d = derive(&facts(&g, own), &Tracked::default());
            assert_eq!(d.explain, explain, "{g:?}");
            assert_eq!(d.edit, edit, "{g:?}");
            let mode = MySqlMode { dollar_quotes, mariadb, ..MySqlMode::default() };
            assert_eq!(d.language, Language::Sql(Dialect::MySql(mode)), "{g:?}");
        }
    }
    // The session's sql mode is the session's: the server's answers stay.
    let tracked = Tracked { sql_mode: Some("ANSI_QUOTES,NO_BACKSLASH_ESCAPES".into()), ..Tracked::default() };
    let d = derive(&facts(&maria(11, 4, 13), Some(true)), &tracked);
    let mode = MySqlMode { ansi_quotes: true, no_backslash_escapes: true, mariadb: true, ..MySqlMode::default() };
    assert_eq!(d.language, Language::Sql(Dialect::MySql(mode)));
    assert_eq!((d.explain, d.edit), (None, None));
}

#[test]
fn per_server_capabilities_are_the_drivers_with_the_servers_answers() {
    // The driver's own, as they were before any server answered.
    assert_eq!(CAPABILITIES.language, Language::Sql(Dialect::MySql(MySqlMode::default())));
    assert_eq!(CAPABILITIES.explain, Some(ExplainFormat::MySqlTree));
    assert_eq!(CAPABILITIES.edit, Some(EditFormat::MySql(MySqlEdit { nowait: true })));
    for (g, ..) in table() {
        let d = derive(&facts(&g, None), &Tracked::default());
        let c = d.capabilities;
        assert_eq!((c.language, c.explain, c.edit), (d.language, d.explain, d.edit), "{g:?}");
        let rest = Capabilities {
            language: CAPABILITIES.language,
            explain: CAPABILITIES.explain,
            edit: CAPABILITIES.edit,
            ..c
        };
        assert_eq!(format!("{rest:?}"), format!("{CAPABILITIES:?}"), "{g:?}");
    }
}

#[test]
fn server_of_facts_is_the_greetings() {
    for (g, ..) in table() {
        for own in [Some(true), Some(false), None] {
            assert_eq!(Server::of(&facts(&g, own)), Server::of_greeting(&g), "{g:?}");
        }
    }
    assert_eq!(Server::of_greeting(&my(8, 4, 6)), Server { version: (8, 4, 6), mariadb: false });
    assert_eq!(Server::of_greeting(&maria(11, 4, 13)), Server { version: (11, 4, 13), mariadb: true });
    // Facts no session of this driver finds: a server it refuses, with no plan and no edits.
    let pg = ServerFacts {
        version: Fact::Known(Version::new(17, 2, 0)),
        version_text: Some("17.2".into()),
        provider: Fact::Unknown(Why::NotChecked),
        topology: Fact::Known(Topology::Direct),
        detail: EngineFacts::Postgres(PgFacts::default()),
    };
    assert_eq!(Server::of(&pg), Server::UNREAD);
    assert_eq!(Server::UNREAD.version, (0, 0, 0));
    assert!(Server::UNREAD.check().is_err());
    let d = derive(&pg, &Tracked::default());
    assert_eq!((d.explain, d.edit), (None, None));
}

#[test]
fn the_facts_are_the_greeting_and_the_own_id_check() {
    for (g, ..) in table() {
        for (own, topology) in [
            (Some(true), Fact::Known(Topology::Direct)),
            (Some(false), Fact::Known(Topology::Pooled)),
            (None, Fact::Unknown(Why::NotChecked)),
        ] {
            let f = facts(&g, own);
            assert_eq!(f.topology, topology, "{g:?} {own:?}");
            assert_eq!(f.version, Fact::Known(g.handshake));
            assert_eq!(f.version_text, None);
            assert_eq!(f.provider, Fact::Unknown(Why::NotChecked));
            let engine = if g.mariadb { EngineId::MariaDb } else { EngineId::MySql };
            assert_eq!(f.engine(), engine);
        }
        let i = identity(&g);
        assert_eq!(i.engine, Fact::Known(if g.mariadb { EngineId::MariaDb } else { EngineId::MySql }));
        assert_eq!(i.version, Fact::Known(g.handshake));
    }
}
