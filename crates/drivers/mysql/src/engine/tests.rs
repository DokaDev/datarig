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
            let d = derive(&facts(&g, &VersionReport::agreeing(&g), own), &Tracked::default());
            assert_eq!(d.explain, explain, "{g:?}");
            assert_eq!(d.edit, edit, "{g:?}");
            let dollar_quotes = if dollar_quotes { DollarQuotes::Read } else { DollarQuotes::NotRead };
            let mode = MySqlMode { dollar_quotes, ..MySqlMode::default() };
            let dialect = if mariadb { Dialect::MariaDb(mode) } else { Dialect::MySql(mode) };
            assert_eq!(d.language, Language::Sql(dialect), "{g:?}");
        }
    }
    // The session's sql mode is the session's: the server's answers stay.
    let tracked = Tracked { sql_mode: Some("ANSI_QUOTES,NO_BACKSLASH_ESCAPES".into()), ..Tracked::default() };
    let g = maria(11, 4, 13);
    let d = derive(&facts(&g, &VersionReport::agreeing(&g), Some(true)), &tracked);
    let mode = MySqlMode { ansi_quotes: true, no_backslash_escapes: true, ..MySqlMode::default() };
    assert_eq!(d.language, Language::Sql(Dialect::MariaDb(mode)));
    assert_eq!((d.explain, d.edit), (None, None));
}

#[test]
fn per_server_capabilities_are_the_drivers_with_the_servers_answers() {
    // The driver's own, as they were before any server answered.
    assert_eq!(CAPABILITIES.language, Language::Sql(Dialect::MySql(MySqlMode::default())));
    assert_eq!(CAPABILITIES.explain, Some(ExplainFormat::MySqlTree));
    assert_eq!(CAPABILITIES.edit, Some(EditFormat::MySql(MySqlEdit { nowait: true })));
    for (g, ..) in table() {
        let d = derive(&facts(&g, &VersionReport::agreeing(&g), None), &Tracked::default());
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
            assert_eq!(Server::of(&facts(&g, &VersionReport::agreeing(&g), own)), Server::of_greeting(&g), "{g:?}");
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
            let f = facts(&g, &VersionReport::agreeing(&g), own);
            assert_eq!(f.topology, topology, "{g:?} {own:?}");
            assert_eq!(f.version, Fact::Known(g.handshake));
            assert_eq!(f.version_text, Some(VersionReport::agreeing(&g).line));
            assert_eq!(f.provider, Fact::Unknown(Why::NotChecked));
            let engine = if g.mariadb { EngineId::MariaDb } else { EngineId::MySql };
            assert_eq!(f.engine(), engine);
        }
        let i = identity(&g, &VersionReport::agreeing(&g));
        assert_eq!(i.engine, Fact::Known(if g.mariadb { EngineId::MariaDb } else { EngineId::MySql }));
        assert_eq!(i.version, Fact::Known(g.handshake));
    }
}

#[test]
fn the_version_query_survives_a_zero_select_limit() {
    // A `sql_select_limit` of 0 left on the session empties an unbounded `SELECT`; a `LIMIT` of
    // the statement's own wins over it.
    assert_eq!(VERSION_SQL, "SELECT VERSION() LIMIT 1");
}

/// What a server answers to the version read: a failed read, no row, or one value.
#[derive(Clone, Debug)]
enum Answer {
    Failed,
    NoRow,
    Null,
    Bytes(&'static [u8]),
    Number,
}

/// How the version read ends.
#[derive(Clone, Debug, PartialEq)]
enum Outcome {
    /// The read's own error.
    Kept,
    Unconfirmed,
    /// A line: the engine it names and the version.
    Line(EngineId, Fact<Version>),
}

/// What the version read of a server of greeting `g` that answers `answer` ends in, as
/// [`confirm`] does once the server answered (a failed read is its own error).
fn outcome(g: &Greeting, answer: &Answer) -> Outcome {
    let value = match answer {
        Answer::Failed => return Outcome::Kept,
        Answer::NoRow => None,
        Answer::Null => Some(Value::NULL),
        Answer::Bytes(b) => Some(Value::Bytes(b.to_vec())),
        Answer::Number => Some(Value::Int(1)),
    };
    match judge(g, value) {
        Ok(r) => {
            assert_eq!(Some(r.line.as_bytes()), if let Answer::Bytes(b) = answer { Some(*b) } else { None });
            Outcome::Line(r.engine, r.version)
        }
        Err(DbError::EngineUnconfirmed { greeting }) => {
            assert_eq!(greeting, Server::of_greeting(g).label());
            Outcome::Unconfirmed
        }
        Err(e) => panic!("{g:?} {answer:?}: {e:?}"),
    }
}

#[test]
fn the_engine_is_the_one_the_version_line_names() {
    use EngineId::{MariaDb, MySql};
    use Outcome::*;
    let mysql = my(8, 4, 6);
    let mariadb = maria(11, 4, 13);
    let known = |g: &Greeting| Fact::Known(g.handshake);
    let unknown = Fact::Unknown(Why::VersionUnknown);
    let differ = Fact::Unknown(Why::VersionsDisagree);
    let other = Fact::Unknown(Why::EnginesDisagree);
    let rows: Vec<(Answer, Outcome, Outcome)> = vec![
        // (answer, under MySQL 8.4.6's greeting, under MariaDB 11.4.13's)
        (Answer::Failed, Kept, Kept),
        (Answer::NoRow, Unconfirmed, Unconfirmed),
        (Answer::Null, Unconfirmed, Unconfirmed),
        (Answer::Number, Unconfirmed, Unconfirmed),
        (Answer::Bytes(b""), Unconfirmed, Unconfirmed),
        (Answer::Bytes(b" "), Unconfirmed, Unconfirmed),
        (Answer::Bytes(b"\t"), Unconfirmed, Unconfirmed),
        (Answer::Bytes(b"\xFF"), Unconfirmed, Unconfirmed),
        (Answer::Bytes(b"8.4.6"), Line(MySql, known(&mysql)), Line(MySql, other.clone())),
        (Answer::Bytes(b"8.4.6-commercial"), Line(MySql, known(&mysql)), Line(MySql, other.clone())),
        (Answer::Bytes(b"8.0.40"), Line(MySql, differ.clone()), Line(MySql, other.clone())),
        (Answer::Bytes(b"1"), Line(MySql, unknown.clone()), Line(MySql, other.clone())),
        (Answer::Bytes(b"11.4.13-MariaDB-ubu2404"), Line(MariaDb, other.clone()), Line(MariaDb, known(&mariadb))),
        (Answer::Bytes(b"10.11.9-MariaDB"), Line(MariaDb, other.clone()), Line(MariaDb, differ.clone())),
        (Answer::Bytes(b"5.5.5-11.4.13-MariaDB"), Line(MariaDb, other.clone()), Line(MariaDb, differ)),
        (Answer::Bytes(b"x-MariaDB"), Line(MariaDb, other), Line(MariaDb, unknown)),
    ];
    for (answer, under_mysql, under_mariadb) in rows {
        assert_eq!(outcome(&mysql, &answer), under_mysql, "MySQL greeting, {answer:?}");
        assert_eq!(outcome(&mariadb, &answer), under_mariadb, "MariaDB greeting, {answer:?}");
    }
}

#[test]
fn legacy_answers_follow_the_greeting_when_the_line_names_the_other_engine() {
    for (g, ..) in table() {
        let line = if g.mariadb { "8.4.6" } else { "11.4.13-MariaDB-ubu2404" };
        let report = judge(&g, Some(Value::Bytes(line.as_bytes().to_vec()))).expect("a line");
        let line_engine = if g.mariadb { EngineId::MySql } else { EngineId::MariaDb };
        for own in [Some(true), Some(false), None] {
            let f = facts(&g, &report, own);
            assert_eq!(f.engine(), line_engine, "{g:?}");
            assert_eq!(f.topology, Fact::Unknown(Why::EnginesDisagree), "{g:?} {own:?}");
            assert_eq!(f.version, Fact::Unknown(Why::EnginesDisagree), "{g:?}");
            assert_eq!(f.version_text.as_deref(), Some(line), "{g:?}");
            // Everything a session says and sends follows the greeting, as when both agree.
            assert_eq!(Server::of(&f), Server::of_greeting(&g), "{g:?}");
            let agreeing = facts(&g, &VersionReport::agreeing(&g), own);
            let tracked = Tracked { sql_mode: Some("ANSI_QUOTES".into()), ..Tracked::default() };
            let (d, a) = (derive(&f, &tracked), derive(&agreeing, &tracked));
            assert_eq!((d.language, d.explain, d.edit), (a.language, a.explain, a.edit), "{g:?}");
            assert_eq!(format!("{:?}", d.capabilities), format!("{:?}", a.capabilities), "{g:?}");
        }
        let i = identity(&g, &report);
        assert_eq!(i.engine, Fact::Known(line_engine), "{g:?}");
        assert_eq!(i.version, Fact::Unknown(Why::EnginesDisagree), "{g:?}");
    }
}
