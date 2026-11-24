use super::*;
use datarig_core::engine::PgFacts;

fn my(major: u16, minor: u16, patch: u16) -> Greeting {
    Greeting { mariadb: false, handshake: Version::new(major, minor, patch) }
}

fn maria(major: u16, minor: u16, patch: u16) -> Greeting {
    Greeting { mariadb: true, handshake: Version::new(major, minor, patch) }
}

/// The greeting of MariaDB (`mariadb`) or MySQL at `version`.
fn at(mariadb: bool, (major, minor, patch): (u16, u16, u16)) -> Greeting {
    Greeting { mariadb, handshake: Version::new(major, minor, patch) }
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
fn the_greeting_of_facts_is_the_sessions() {
    for (g, ..) in table() {
        for own in [Some(true), Some(false), None] {
            assert_eq!(Greeting::of(&facts(&g, &VersionReport::agreeing(&g), own)), g, "{g:?}");
        }
    }
    let g = my(8, 4, 6);
    assert_eq!(Greeting::of(&facts(&g, &VersionReport::agreeing(&g), None)).label(), "MySQL 8.4.6");
    let g = maria(11, 4, 13);
    assert_eq!(Greeting::of(&facts(&g, &VersionReport::agreeing(&g), None)).label(), "MariaDB 11.4.13");
    // Facts no session of this driver finds: a server it refuses, with no plan and no edits.
    let pg = ServerFacts {
        version: Fact::Known(Version::new(17, 2, 0)),
        version_text: Some("17.2".into()),
        provider: Fact::Unknown(Why::NotChecked),
        topology: Fact::Known(Topology::Direct),
        detail: EngineFacts::Postgres(PgFacts::default()),
    };
    assert_eq!(Greeting::of(&pg), Greeting::UNREAD);
    assert_eq!(Greeting::UNREAD.handshake, Version::new(0, 0, 0));
    assert!(Greeting::UNREAD.check().is_err());
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
            assert_eq!(greeting, g.label());
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

/// `l` with dollar quotes read as `dollar_quotes` says.
fn with_dollar_quotes(l: Language, dollar_quotes: DollarQuotes) -> Language {
    let Language::Sql(d) = l;
    match d {
        Dialect::MySql(m) => Language::Sql(Dialect::MySql(MySqlMode { dollar_quotes, ..m })),
        Dialect::MariaDb(m) => Language::Sql(Dialect::MariaDb(MySqlMode { dollar_quotes, ..m })),
        Dialect::Postgres => l,
    }
}

/// Every answer follows the greeting, but for dollar quotes when the greeting and the line
/// answer them differently (MySQL 8.1 or later against MariaDB's line, MariaDB against MySQL
/// 8.4's): those are not known.
#[test]
fn legacy_answers_follow_the_greeting_when_the_line_names_the_other_engine() {
    for (g, _, _, greeting_reads, _) in table() {
        let line = if g.mariadb { "8.4.6" } else { "11.4.13-MariaDB-ubu2404" };
        let line_reads = g.mariadb;
        let report = judge(&g, Some(Value::Bytes(line.as_bytes().to_vec()))).expect("a line");
        let line_engine = if g.mariadb { EngineId::MySql } else { EngineId::MariaDb };
        for own in [Some(true), Some(false), None] {
            let f = facts(&g, &report, own);
            assert_eq!(f.engine(), line_engine, "{g:?}");
            assert_eq!(f.topology, Fact::Unknown(Why::EnginesDisagree), "{g:?} {own:?}");
            assert_eq!(f.version, Fact::Unknown(Why::EnginesDisagree), "{g:?}");
            assert_eq!(f.version_text.as_deref(), Some(line), "{g:?}");
            // Everything a session says and sends follows the greeting, as when both agree.
            assert_eq!(Greeting::of(&f), g, "{g:?}");
            let agreeing = facts(&g, &VersionReport::agreeing(&g), own);
            let tracked = Tracked { sql_mode: Some("ANSI_QUOTES".into()), ..Tracked::default() };
            let (d, a) = (derive(&f, &tracked), derive(&agreeing, &tracked));
            let language = match greeting_reads == line_reads {
                true => a.language,
                false => with_dollar_quotes(a.language, DollarQuotes::NotKnown),
            };
            assert_eq!((d.language, d.explain, d.edit), (language, a.explain, a.edit), "{g:?}");
            let capabilities = Capabilities { language, ..a.capabilities };
            assert_eq!(format!("{:?}", d.capabilities), format!("{:?}", capabilities), "{g:?}");
        }
        let i = identity(&g, &report);
        assert_eq!(i.engine, Fact::Known(line_engine), "{g:?}");
        assert_eq!(i.version, Fact::Unknown(Why::EnginesDisagree), "{g:?}");
    }
}

/// The edits a session takes follow MySQL's edit rules on the greeting's number even when the
/// version line names MariaDB, whose own rules would refuse them.
#[test]
fn the_mysql_edit_writer_reads_the_greeting_whatever_the_line_names() {
    let Fact::Known(rules) = EngineId::MySql.spec().edit else { panic!("MySQL takes edits") };
    let line = "11.4.13-MariaDB-ubu2404";
    let cases = [
        (my(8, 0, 17), Fact::Known(EditFormat::MySql(MySqlEdit { nowait: true }))),
        (my(8, 0, 16), Fact::No(Why::VersionTooOld { needed: Version::new(8, 0, 17) })),
    ];
    for (g, want) in cases {
        let report = judge(&g, Some(Value::Bytes(line.as_bytes().to_vec()))).expect("a line");
        for own in [Some(true), Some(false), None] {
            let f = facts(&g, &report, own);
            assert_eq!(f.engine(), EngineId::MariaDb, "{g:?}");
            assert!(matches!(f.engine().spec().edit, Fact::No(Why::NotBuilt)), "{g:?}");
            assert_eq!(rules.writer(&f), want, "{g:?} {own:?}");
            let derived = match &want {
                Fact::Known(format) => Some(*format),
                Fact::No(_) | Fact::Unknown(_) => None,
            };
            let d = derive(&f, &Tracked::default());
            assert_eq!((d.edit, d.capabilities.edit), (derived, derived), "{g:?} {own:?}");
        }
    }
}

/// The servers the session's set-up is recorded for: MySQL and MariaDB around each version its
/// statements depend on.
const SET_UP_SERVERS: [(bool, (u16, u16, u16)); 7] = [
    (false, (8, 0, 0)),
    (false, (8, 0, 40)),
    (false, (8, 4, 6)),
    (true, (10, 6, 0)),
    (true, (11, 0, 6)),
    (true, (11, 1, 0)),
    (true, (11, 4, 13)),
];

/// Every statement and message of a session's set-up on each of [`SET_UP_SERVERS`], one per
/// line: whether the version is supported, the settings by role, read-only policy and limit,
/// the read-only question and its message, and the question for what the server did not report.
fn session_set_up_texts() -> String {
    use crate::session::{Settings, ask_read_only, ask_unreported, init_sql, not_read_only};
    use datarig_core::driver::SessionRole;
    let mut out = String::new();
    let mut line = |what: String, text: String| {
        out.push_str(&what);
        out.push_str(": ");
        out.push_str(&text);
        out.push('\n');
    };
    // The floors and an unread version.
    for (mariadb, version) in [(false, (0, 0, 0)), (false, (5, 7, 44)), (true, (0, 0, 0)), (true, (10, 5, 27))] {
        let server = at(mariadb, version);
        line(format!("check {}", server.label()), format!("{:?}", server.check()));
    }
    for (mariadb, version) in SET_UP_SERVERS {
        let server = at(mariadb, version);
        let name = server.label();
        // A session straight to the server: its reports agree on its version.
        let v = session_version(&facts(&server, &VersionReport::agreeing(&server), None));
        line(format!("check {name}"), format!("{:?}", server.check()));
        for role in [SessionRole::Query, SessionRole::Meta, SessionRole::Monitor] {
            for read_only in [false, true] {
                for select_limit in [None, Some(501)] {
                    for lock_wait in [None, Some(7)] {
                        for wait_timeout in [None, Some(360)] {
                            let s = Settings { role, read_only, select_limit, lock_wait, wait_timeout };
                            line(
                                format!(
                                    "init_sql {name} {role:?} read_only={read_only} select_limit={select_limit:?} \
                                     lock_wait={lock_wait:?} wait_timeout={wait_timeout:?}"
                                ),
                                init_sql(&server, &v, &s),
                            );
                        }
                    }
                }
            }
        }
        line(format!("ask_read_only {name}"), ask_read_only(&server, &v));
        let answered = (Some("0".to_string()), Some("1".to_string()), Some("0".to_string()));
        line(format!("not_read_only {name} reported OFF"), not_read_only(&server, &v, Some("OFF"), &answered));
        line(format!("not_read_only {name} reported none"), not_read_only(&server, &v, None, &(None, None, None)));
        line(format!("ask_unreported {name}"), ask_unreported(&server, &v));
    }
    out
}

#[test]
fn init_sql_is_todays_for_every_role_engine_and_version() {
    let got = session_set_up_texts();
    let want = include_str!("golden/session_set_up.txt");
    let (got, want): (Vec<&str>, Vec<&str>) = (got.lines().collect(), want.lines().collect());
    for (n, (got, want)) in got.iter().zip(&want).enumerate() {
        assert_eq!(got, want, "line {}", n + 1);
    }
    assert_eq!(got.len(), want.len());
}

#[test]
fn the_family_names_session_variables_by_version() {
    let (tx, transaction) = (("tx_read_only", "tx_isolation"), ("transaction_read_only", "transaction_isolation"));
    let v = |a, b, c| Fact::Known(Version::new(a, b, c));
    // What each engine names on a version it was told, and on one it was not: MariaDB takes its
    // older names, which every version it serves has; MySQL has one name on every version.
    let rows = [
        (EngineId::MariaDb, v(10, 6, 0), tx),
        (EngineId::MariaDb, v(11, 0, 6), tx),
        (EngineId::MariaDb, v(11, 1, 0), transaction),
        (EngineId::MariaDb, v(11, 4, 13), transaction),
        (EngineId::MariaDb, Fact::Unknown(Why::VersionUnknown), tx),
        (EngineId::MariaDb, Fact::Unknown(Why::VersionsDisagree), tx),
        (EngineId::MariaDb, Fact::Unknown(Why::EnginesDisagree), tx),
        (EngineId::MariaDb, Fact::Unknown(Why::NotChecked), tx),
        (EngineId::MariaDb, Fact::No(Why::NotOnEngine), tx),
        (EngineId::MySql, v(8, 0, 0), transaction),
        (EngineId::MySql, v(8, 0, 40), transaction),
        (EngineId::MySql, v(8, 4, 6), transaction),
        (EngineId::MySql, Fact::Unknown(Why::VersionUnknown), transaction),
        (EngineId::MySql, Fact::Unknown(Why::VersionsDisagree), transaction),
        (EngineId::MySql, Fact::Unknown(Why::EnginesDisagree), transaction),
        (EngineId::MySql, Fact::Unknown(Why::NotChecked), transaction),
        (EngineId::MySql, Fact::No(Why::NotOnEngine), transaction),
    ];
    for (engine, version, (read_only, isolation)) in rows {
        let f = family(engine);
        assert_eq!(f.engine(), engine);
        assert_eq!(f.read_only_var(&version), read_only, "{engine:?} {version:?}");
        assert_eq!(f.isolation_var(&version), isolation, "{engine:?} {version:?}");
    }
    // The rest of a session's set-up does not depend on the version.
    assert_eq!(family(EngineId::MySql).statement_timeout(5_000), "SESSION max_execution_time = 5000");
    assert_eq!(family(EngineId::MariaDb).statement_timeout(5_000), "SESSION max_statement_time = 5");
    assert_eq!(family(EngineId::MySql).super_read_only(), Some("super_read_only"));
    assert_eq!(family(EngineId::MariaDb).super_read_only(), None);
    assert_eq!(family(EngineId::MySql).min_version(), Version::new(8, 0, 0));
    assert_eq!(family(EngineId::MariaDb).min_version(), Version::new(10, 6, 0));
    // Never this driver's engine: MySQL's rules.
    assert_eq!(family(EngineId::Postgres).engine(), EngineId::MySql);
    // A greeting's server asks the family of its engine, by the greeting's version.
    for (mariadb, version) in SET_UP_SERVERS {
        let server = at(mariadb, version);
        let (major, minor, patch) = version;
        let known = Fact::Known(Version::new(major, minor, patch));
        assert_eq!(server.family().engine(), engine_of(mariadb));
        assert_eq!(server.read_only_var(&known), server.family().read_only_var(&known));
        assert_eq!(server.isolation_var(&known), server.family().isolation_var(&known));
    }
}

/// The version a session's settings are named by: when its two reports name one engine, the
/// version its facts know (none when the numbers differ or the line has none, and then
/// MariaDB's older names, which every version it serves has); when they name two, the
/// greeting's number, so every name is the one the greeting implies, as before. MySQL has one
/// name on every version. Every statement of the set-up and the allowlist's question use them.
#[test]
fn session_variables_follow_the_facts() {
    use crate::session::{Settings, ask_read_only, ask_unreported, init_sql, not_read_only};
    use datarig_core::driver::SessionRole;
    let (tx, transaction) = (("tx_read_only", "tx_isolation"), ("transaction_read_only", "transaction_isolation"));
    let known = |a, b, c| Fact::Known(Version::new(a, b, c));
    let rows = [
        // Reports naming one engine: the facts' version.
        (maria(11, 4, 13), "11.4.13-MariaDB-ubu2404", known(11, 4, 13), transaction),
        (maria(11, 1, 0), "11.1.0-MariaDB", known(11, 1, 0), transaction),
        (maria(11, 0, 6), "11.0.6-MariaDB", known(11, 0, 6), tx),
        (maria(10, 6, 0), "10.6.0-MariaDB", known(10, 6, 0), tx),
        (maria(11, 8, 0), "11.4.13-MariaDB-ubu2404", Fact::Unknown(Why::VersionsDisagree), tx),
        (maria(11, 4, 13), "5.5.5-11.4.13-MariaDB", Fact::Unknown(Why::VersionsDisagree), tx),
        (maria(10, 11, 9), "11.4.13-MariaDB-ubu2404", Fact::Unknown(Why::VersionsDisagree), tx),
        (maria(11, 4, 13), "x-MariaDB", Fact::Unknown(Why::VersionUnknown), tx),
        (my(8, 4, 6), "8.4.6", known(8, 4, 6), transaction),
        (my(8, 0, 0), "8.0.0", known(8, 0, 0), transaction),
        (my(8, 4, 99), "8.4.6", Fact::Unknown(Why::VersionsDisagree), transaction),
        (my(8, 4, 6), "x", Fact::Unknown(Why::VersionUnknown), transaction),
        // An Aurora line has no number the driver reads: unknown, as before.
        (my(8, 0, 32), "8.0.mysql_aurora.3.04.0", Fact::Unknown(Why::VersionUnknown), transaction),
        // Reports naming two engines: the greeting's number, the greeting's names.
        (my(8, 4, 6), "11.4.13-MariaDB-ubu2404", known(8, 4, 6), transaction),
        (my(8, 0, 40), "5.5.5-10.11.9-MariaDB", known(8, 0, 40), transaction),
        (maria(11, 4, 13), "8.4.6", known(11, 4, 13), transaction),
        (maria(10, 11, 9), "8.4.6", known(10, 11, 9), tx),
        (maria(11, 4, 13), "8.0.mysql_aurora.3.04.0", known(11, 4, 13), transaction),
        (maria(11, 0, 6), "x", known(11, 0, 6), tx),
    ];
    let meta =
        Settings { role: SessionRole::Meta, read_only: false, select_limit: None, lock_wait: None, wait_timeout: None };
    let d = Dialect::MySql(MySqlMode::default());
    for (g, line, version, (ro, iso)) in rows {
        let what = format!("{g:?} {line}");
        let report = judge(&g, Some(Value::Bytes(line.as_bytes().to_vec()))).expect("a line");
        for own in [Some(true), Some(false), None] {
            let f = facts(&g, &report, own);
            assert_eq!(session_version(&f), version, "{what} {own:?}");
        }
        let v = session_version(&facts(&g, &report, None));
        assert_eq!((g.read_only_var(&v), g.isolation_var(&v)), (ro, iso), "{what}");
        let set = init_sql(&g, &v, &meta);
        assert!(set.contains(&format!("'sql_mode,{ro},sql_select_limit,")), "{what}: {set}");
        assert!(set.contains(&format!(",{iso}', ")), "{what}: {set}");
        assert!(set.contains(&format!("SESSION {iso} = @@SESSION.{iso}")), "{what}: {set}");
        assert!(set.ends_with(&format!("SESSION {ro} = ON")), "{what}: {set}");
        assert!(ask_read_only(&g, &v).starts_with(&format!("SELECT CAST(@@SESSION.{ro} AS CHAR)")), "{what}");
        assert!(ask_unreported(&g, &v).contains(&format!("CAST(@@SESSION.{ro} AS CHAR)")), "{what}");
        let message = not_read_only(&g, &v, None, &(None, None, None));
        assert!(message.contains(&format!("(reported {ro}: none; asked: {ro} NULL")), "{what}: {message}");
        let check = crate::query::check_sql(&[], d, &g, &v).concat();
        assert!(check.contains(&format!("@@SESSION.{iso}")), "{what}: {check}");
        let other = if iso == "tx_isolation" { "transaction_" } else { "tx_" };
        for text in [set, ask_read_only(&g, &v), ask_unreported(&g, &v), message, check] {
            assert!(!text.contains(other), "{what}: {text}");
        }
    }
}

/// The servers the reads of the app's own are recorded for: those of the session's set-up, and
/// MySQL around each version a structure read depends on.
const OWN_READ_SERVERS: [(bool, (u16, u16, u16)); 11] = [
    (false, (8, 0, 0)),
    (false, (8, 0, 12)),
    (false, (8, 0, 13)),
    (false, (8, 0, 15)),
    (false, (8, 0, 16)),
    (false, (8, 0, 40)),
    (false, (8, 4, 6)),
    (true, (10, 6, 0)),
    (true, (11, 0, 6)),
    (true, (11, 1, 0)),
    (true, (11, 4, 13)),
];

/// Every statement of the app's own a session reads its catalogue with, on each of
/// [`OWN_READ_SERVERS`], one per line: the question whether a statement's tables undo, the
/// allowlist's question, a bounded count's text with the time limit it applies, the session's
/// time limit as read from the server's words, a keyset chunk's text, a table's structure
/// statement and how a column's default is written from the server's words.
fn own_read_texts() -> String {
    use crate::meta::structure::{default_text, statement};
    use crate::query::{ALL_ROWS, Asked, bounded_select, check_sql, count_text, not_undone_sql};
    use std::time::Duration;
    let mut out = String::new();
    let mut line = |what: String, text: String| {
        out.push_str(&what);
        out.push_str(": ");
        out.push_str(&text);
        out.push('\n');
    };
    let modes = [
        ("default", MySqlMode::default()),
        ("ansi_quotes", MySqlMode { ansi_quotes: true, ..MySqlMode::default() }),
        ("no_backslash_escapes", MySqlMode { no_backslash_escapes: true, ..MySqlMode::default() }),
    ];
    let name_sets: [&[&str]; 3] = [&[], &["shop.t", "u"], &["o'k", "a\\b", "`my t`.c", "\"x.y\".z"]];
    for (mariadb, version) in OWN_READ_SERVERS {
        let server = at(mariadb, version);
        let name = server.label();
        let v = session_version(&facts(&server, &VersionReport::agreeing(&server), None));
        for (mode_name, mode) in modes {
            let d = dialect(&server, mode);
            // How the names are written depends on the sql mode alone: plain names are recorded in
            // the default mode only.
            let plain = mode_name == "default";
            for names in name_sets.iter().filter(|n| plain || n.len() > 2) {
                let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
                let text = not_undone_sql(&names, d, server.family());
                line(format!("not_undone {name} {mode_name} {names:?}"), format!("{text:?}"));
                line(format!("check {name} {mode_name} {names:?}"), format!("{:?}", check_sql(&names, d, &server, &v)));
            }
            for (schema, table) in
                [("shop", "items"), ("o'k", "a\\b`c")].into_iter().filter(|(s, _)| plain || *s != "shop")
            {
                let text = statement(server, d, schema, table);
                line(format!("structure {name} {mode_name} {schema:?}.{table:?}"), format!("{text:?}"));
            }
        }
    }
    // What depends on the engine alone.
    for (mariadb, version) in [(false, (8, 4, 6)), (true, (11, 4, 13))] {
        let server = at(mariadb, version);
        let name = server.label();
        let sqls = [
            "SELECT COUNT(*) FROM (\nSELECT * FROM `t` WHERE (a > 1) LIMIT 10\n) AS datarig_count",
            "SELECT\n`a`, COUNT(*) FROM `t` GROUP BY `a`",
            " SELECT 1",
            "select 1",
            "WITH x AS (SELECT 1) SELECT * FROM x",
        ];
        let limits = [Duration::from_micros(500), Duration::from_millis(1500), Duration::from_secs(30)];
        let session = [None, Some(Duration::from_secs(5)), Some(Duration::from_secs(60))];
        for sql in sqls {
            for limit in limits {
                for statement in session {
                    for lock in [None, Some(0), Some(1), Some(50)] {
                        for hints in ["", ALL_ROWS] {
                            let asked = Asked { isolation: String::new(), statement, lock, sort_length: None };
                            line(
                                format!(
                                    "bounded {name} {sql:?} limit={limit:?} session={statement:?} lock={lock:?} \
                                     hints={hints:?}"
                                ),
                                format!("{:?}", bounded_select(sql, limit, &asked, server.family(), hints)),
                            );
                        }
                    }
                }
            }
        }
        // A count's text without a time limit, on a session whose reports agree or not.
        for sql in sqls {
            for lock in [None, Some(0), Some(1), Some(50)] {
                for disagree in [false, true] {
                    let asked = Asked { isolation: String::new(), statement: None, lock, sort_length: None };
                    line(
                        format!("count_off {name} {sql:?} lock={lock:?} disagree={disagree}"),
                        format!("{:?}", count_text(sql, None, &asked, server.family(), disagree)),
                    );
                }
            }
        }
        for v in ["0", "2500", " 2500 ", "0.000000", "1.500000", "0.0005", "x", "", "-1", "1e10", "NaN", "inf"] {
            line(format!("session_limit {name} {v:?}"), format!("{:?}", server.family().session_limit(v)));
        }
        for sql in [
            "SELECT * FROM `s`.`t` WHERE `id` > 7 ORDER BY `id` LIMIT 10000",
            "SELECT\n* FROM `s`.`t`",
            "select * from t",
            " SELECT 1",
            "WITH x AS (SELECT 1) SELECT * FROM x",
        ] {
            line(format!("keyset {name} {sql:?}"), format!("{:?}", server.family().keyset_text(sql)));
        }
        let d = dialect(&server, MySqlMode::default());
        for value in [
            None,
            Some("NULL"),
            Some("0"),
            Some("it's"),
            Some("'it''s'"),
            Some("CURRENT_TIMESTAMP"),
            Some("current_timestamp()"),
            Some("now()"),
            Some("b'101'"),
            Some("0x0A"),
            Some("a\\b"),
        ] {
            for data_type in ["int", "varchar", "timestamp", "bit", "varbinary"] {
                for extra in ["", "DEFAULT_GENERATED", "DEFAULT_GENERATED on update CURRENT_TIMESTAMP"] {
                    line(
                        format!("default {name} {value:?} {data_type} {extra:?}"),
                        format!("{:?}", default_text(value.map(str::to_string), data_type, extra, server.family(), d)),
                    );
                }
            }
        }
    }
    out
}

#[test]
fn own_read_texts_are_todays_on_both_engines() {
    let got = own_read_texts();
    let want = include_str!("golden/own_reads.txt");
    let (got, want): (Vec<&str>, Vec<&str>) = (got.lines().collect(), want.lines().collect());
    for (n, (got, want)) in got.iter().zip(&want).enumerate() {
        assert_eq!(got, want, "line {}", n + 1);
    }
    assert_eq!(got.len(), want.len());
}
