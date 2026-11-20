use super::*;
use crate::profile::dsn;

fn scheme_of(spec: &EngineSpec) -> Option<Scheme> {
    match spec.transport {
        Transport::Network { scheme, .. } => Some(scheme),
        Transport::File => None,
    }
}

#[test]
fn every_key_and_alias_names_its_engine() {
    for &id in EngineId::ALL {
        let spec = id.spec();
        assert_eq!(spec.id, id);
        let names = std::iter::once(spec.key).chain(spec.aliases.iter().copied());
        for name in names {
            for written in [name.to_string(), name.to_ascii_uppercase(), mixed_case(name)] {
                match expected(&written) {
                    Expected::One(found) => {
                        assert_eq!(found, id, "{written}");
                        assert_eq!(spec.family, &[id], "{written}");
                    }
                    Expected::Family(family) => {
                        assert!(family.contains(&id), "{written}: {family:?}");
                        assert_eq!(family, spec.family, "{written}");
                        assert!(family.len() > 1, "{written}");
                    }
                    Expected::Unknown(s) => panic!("{written}: unknown {s:?}"),
                }
            }
        }
    }
}

/// `name` with every other ASCII letter upper case.
fn mixed_case(name: &str) -> String {
    name.chars().enumerate().map(|(i, c)| if i % 2 == 0 { c.to_ascii_uppercase() } else { c }).collect()
}

#[test]
fn unknown_names_are_kept_as_written() {
    for name in ["redis", "Redis", "elasticsearch", "sqlite", "", " mysql", "mysql ", "postgres9", "my sql"] {
        assert_eq!(expected(name), Expected::Unknown(name.to_string()), "{name:?}");
    }
}

#[test]
fn the_scheme_and_preset_of_every_driver_string_are_todays() {
    use dsn::Scheme::{MySql as MyScheme, Postgres as PgScheme};
    let pg = Some(Dbms::Postgres);
    let my = Some(Dbms::MySql);
    let table: [(&str, Scheme, Option<Dbms>, Option<&str>); 14] = [
        ("postgres", PgScheme, pg, Some("postgres")),
        ("PostgreSQL", PgScheme, pg, Some("postgres")),
        ("pg", PgScheme, pg, Some("postgres")),
        ("PG", PgScheme, pg, Some("postgres")),
        ("mysql", MyScheme, my, Some("mysql")),
        ("MySQL", MyScheme, my, Some("mysql")),
        ("mariadb", MyScheme, my, Some("mysql")),
        ("MARIADB", MyScheme, my, Some("mysql")),
        ("redis", PgScheme, None, None),
        ("Redis", PgScheme, None, None),
        ("elasticsearch", PgScheme, None, None),
        ("sqlite", PgScheme, None, None),
        ("", PgScheme, None, None),
        (" mysql", PgScheme, None, None),
    ];
    for (driver, scheme, dbms, form_key) in table {
        let e = expected(driver);
        assert_eq!(e.form_scheme(), scheme, "{driver:?}");
        assert_eq!(e.preset(), dbms, "{driver:?}");
        assert_eq!(e.form_key(), form_key, "{driver:?}");
    }
    assert_eq!(LEGACY_FORM_SCHEME, PgScheme);
}

#[test]
fn a_familys_engines_share_form_and_presets() {
    for &id in EngineId::ALL {
        let spec = id.spec();
        assert!(spec.family.contains(&id), "{id:?}");
        assert!(Dbms::parse(spec.preset_key).is_some(), "{id:?}: preset {}", spec.preset_key);
        for &other in spec.family {
            let o = other.spec();
            assert_eq!(o.family, spec.family, "{id:?} / {other:?}");
            assert_eq!(scheme_of(o), scheme_of(spec), "{id:?} / {other:?}");
            assert_eq!(o.preset_key, spec.preset_key, "{id:?} / {other:?}");
            assert_eq!(o.form_key, spec.form_key, "{id:?} / {other:?}");
            assert_eq!(o.hierarchy, spec.hierarchy, "{id:?} / {other:?}");
        }
    }
}

#[test]
fn url_schemes_match_the_dsn_parser() {
    let mut all = Vec::new();
    for &id in EngineId::ALL {
        let spec = id.spec();
        for s in spec.url_schemes {
            let parsed = dsn::parse(&format!("{s}://u@h/db")).unwrap_or_else(|e| panic!("{s}: {e:?}"));
            assert_eq!(Some(parsed.scheme), scheme_of(spec), "{s}");
            all.push(*s);
        }
    }
    all.sort_unstable();
    // The schemes `dsn::parse` reads, no more and no fewer.
    assert_eq!(all, ["mariadb", "mysql", "postgres", "postgresql"]);
    assert_eq!(dsn::parse("pg://u@h/db"), Err(dsn::DsnError::Scheme));
}

#[test]
fn versions_parse_only_a_leading_number() {
    let v = |a, b, c| Some(Version::new(a, b, c));
    let table = [
        ("17.2 (Debian 17.2-1.pgdg120+1)", v(17, 2, 0)),
        ("17.2", v(17, 2, 0)),
        ("11.4.13-MariaDB-ubu2404", v(11, 4, 13)),
        ("8.0.36-28", v(8, 0, 36)),
        ("8.4.6", v(8, 4, 6)),
        ("5.5.5-10.6.12-MariaDB", v(5, 5, 5)),
        ("16.4_1", v(16, 4, 0)),
        ("8.0.36+build", v(8, 0, 36)),
        ("16.1(x)", v(16, 1, 0)),
        ("65535.65535.65535", v(65535, 65535, 65535)),
        ("18beta1", None),
        ("17devel", None),
        ("17.2beta1", None),
        ("", None),
        ("17", None),
        ("17.", None),
        ("17.2.", None),
        ("8.0.36.1", None),
        (" 8.0.1", None),
        ("v8.0.1", None),
        ("99999.0.0", None),
        ("8.65536.0", None),
        ("+8.0.1", None),
        ("8.+0.1", None),
        ("8.0.+1", None),
        ("8..1", None),
        ("-8.0.1", None),
        ("\u{FF18}.0.1", None),
    ];
    for (text, want) in table {
        assert_eq!(Version::parse(text), want, "{text:?}");
    }
}

#[test]
fn versions_compare_by_number() {
    let order = [Version::new(8, 0, 9), Version::new(8, 0, 16), Version::new(8, 1, 0), Version::new(10, 0, 0)];
    for pair in order.windows(2) {
        assert!(pair[0] < pair[1], "{:?} < {:?}", pair[0], pair[1]);
    }
    assert_eq!(Version::parse("8.0.16"), Some(Version::new(8, 0, 16)));
}

#[test]
fn the_engine_of_facts_is_its_detail() {
    let facts = |detail| ServerFacts {
        version: Fact::Unknown(Why::VersionUnknown),
        version_text: None,
        provider: Fact::Unknown(Why::NotChecked),
        topology: Fact::Unknown(Why::NotChecked),
        detail,
    };
    let my = MyFacts { handshake: Version::new(8, 4, 6), greeting: EngineId::MySql };
    assert_eq!(facts(EngineFacts::Postgres(PgFacts::default())).engine(), EngineId::Postgres);
    assert_eq!(facts(EngineFacts::MySql(my.clone())).engine(), EngineId::MySql);
    assert_eq!(facts(EngineFacts::MariaDb(my)).engine(), EngineId::MariaDb);
}

/// The facts of a server reached directly whose greeting and version line agree: `engine` at
/// `version`.
fn agreeing(engine: EngineId, version: Version) -> ServerFacts {
    let my = MyFacts { handshake: version, greeting: engine };
    ServerFacts {
        version: Fact::Known(version),
        version_text: None,
        provider: Fact::Unknown(Why::NotChecked),
        topology: Fact::Known(Topology::Direct),
        detail: match engine {
            EngineId::Postgres => EngineFacts::Postgres(PgFacts::default()),
            EngineId::MySql => EngineFacts::MySql(my),
            EngineId::MariaDb => EngineFacts::MariaDb(my),
        },
    }
}

#[test]
fn every_spec_answers_dollar_quotes_as_the_greeting_did() {
    use crate::sql::dialect::DollarQuotes::{NotRead, Read};
    // Each server, and what its greeting said: MySQL from 8.1.0 on reads them, MySQL 8.0 and
    // MariaDB do not; PostgreSQL always does.
    let table = [
        (EngineId::MySql, Version::new(8, 0, 36), NotRead),
        (EngineId::MySql, Version::new(8, 1, 0), Read),
        (EngineId::MySql, Version::new(8, 4, 6), Read),
        (EngineId::MariaDb, Version::new(11, 4, 13), NotRead),
        (EngineId::Postgres, Version::new(17, 2, 0), Read),
    ];
    for (engine, version, want) in table {
        let facts = agreeing(engine, version);
        assert_eq!(engine.spec().text.dollar_quotes(&facts), want, "{engine:?} {version:?}");
        // The family's engines share the rule: the greeting decides, not the spec asked.
        if engine != EngineId::Postgres {
            for other in [EngineId::MySql, EngineId::MariaDb] {
                assert_eq!(other.spec().text.dollar_quotes(&facts), want, "{other:?} on {engine:?} {version:?}");
            }
        }
    }
}

/// The facts of a MySQL-family session whose greeting names `greeting` at `handshake` and whose
/// `VERSION()` line is `line` (`None`: not read), reached as `topology` says: the engine is the
/// one the line names, as the driver reads it.
fn reported(greeting: EngineId, handshake: Version, line: Option<&str>, topology: Fact<Topology>) -> ServerFacts {
    let my = MyFacts { handshake, greeting };
    let engine = match line {
        Some(l) if mysql_family::line_names_mariadb(l) => EngineId::MariaDb,
        Some(_) => EngineId::MySql,
        None => greeting,
    };
    ServerFacts {
        version: Version::parse(line.unwrap_or_default()).map_or(Fact::Unknown(Why::VersionUnknown), Fact::Known),
        version_text: line.map(str::to_string),
        provider: Fact::Unknown(Why::NotChecked),
        topology,
        detail: match engine {
            EngineId::MariaDb => EngineFacts::MariaDb(my),
            EngineId::MySql | EngineId::Postgres => EngineFacts::MySql(my),
        },
    }
}

/// Each of the server's two reports gives a dollar-quote answer: the greeting (its engine and
/// number) and the `VERSION()` line (the engine it names and the number it starts with). MySQL
/// from 8.1.0 on reads them; MySQL before 8.1.0 and MariaDB (whatever its number) do not; a
/// MySQL line without a number gives no answer. The session's answer is the one they give when
/// they agree (or the greeting's, when only it gives one and the line names its engine), and
/// not known when the two differ or a line of the other engine gives none. How the
/// session reaches the server plays no part, and every spec of the family answers alike.
#[test]
fn dollar_quotes_follow_the_facts() {
    use crate::sql::dialect::DollarQuotes::{self, NotKnown, NotRead, Read};
    let greetings = [
        (EngineId::MySql, Version::new(8, 0, 36)),
        (EngineId::MySql, Version::new(8, 1, 0)),
        (EngineId::MySql, Version::new(8, 4, 6)),
        (EngineId::MariaDb, Version::new(10, 6, 12)),
        (EngineId::MariaDb, Version::new(11, 4, 13)),
    ];
    // Each line, and its answer.
    let lines: [(&str, Option<DollarQuotes>); 11] = [
        ("8.0.36", Some(NotRead)),
        ("8.0.40-log", Some(NotRead)),
        ("8.1.0", Some(Read)),
        ("8.4.6", Some(Read)),
        ("9.1.0-commercial", Some(Read)),
        ("10.6.12-MariaDB", Some(NotRead)),
        ("11.4.13-MariaDB-ubu2404", Some(NotRead)),
        ("5.5.5-10.6.12-MariaDB", Some(NotRead)),
        ("1", None),
        ("8.0.mysql_aurora.3.04.0", None),
        // MariaDB reads none whatever its number: its line answers without one.
        ("MariaDB-custom", Some(NotRead)),
    ];
    let greeted = |engine: EngineId, v: Version| match engine {
        EngineId::MySql if v >= Version::new(8, 1, 0) => Read,
        EngineId::MySql | EngineId::MariaDb | EngineId::Postgres => NotRead,
    };
    let topologies = [
        Fact::Known(Topology::Direct),
        Fact::Known(Topology::Pooled),
        Fact::Unknown(Why::NotChecked),
        Fact::Unknown(Why::EnginesDisagree),
    ];
    for (engine, handshake) in greetings {
        let by_greeting = greeted(engine, handshake);
        for (line, by_line) in lines {
            let line_engine = if mysql_family::line_names_mariadb(line) { EngineId::MariaDb } else { EngineId::MySql };
            let want = match by_line {
                Some(answer) if answer != by_greeting => NotKnown,
                Some(_) => by_greeting,
                // A MySQL line without a number behind a MariaDB greeting: no answer to set
                // against the greeting's, which is another engine's.
                None if line_engine != engine => NotKnown,
                None => by_greeting,
            };
            for topology in &topologies {
                let facts = reported(engine, handshake, Some(line), topology.clone());
                for spec in [EngineId::MySql, EngineId::MariaDb] {
                    let got = spec.spec().text.dollar_quotes(&facts);
                    assert_eq!(got, want, "{spec:?}: greeting {engine:?} {handshake:?}, line {line:?}, {topology:?}");
                }
            }
        }
        // No line read: the greeting's answer.
        let facts = reported(engine, handshake, None, Fact::Unknown(Why::NotChecked));
        assert_eq!(facts.engine().spec().text.dollar_quotes(&facts), by_greeting, "{engine:?} {handshake:?}");
    }
    // The cases by name. A pooler whose reports agree, and a direct session whose own-id
    // check failed: the greeting's answer, as before.
    let v84 = Version::new(8, 4, 6);
    for topology in [Fact::Known(Topology::Pooled), Fact::Unknown(Why::NotChecked)] {
        let f = reported(EngineId::MySql, v84, Some("8.4.6"), topology);
        assert_eq!(f.engine().spec().text.dollar_quotes(&f), Read);
    }
    // A line without a number (version unknown): the greeting's answer.
    let f = reported(EngineId::MySql, v84, Some("8.0.mysql_aurora.3.04.0"), Fact::Known(Topology::Direct));
    assert_eq!(f.engine().spec().text.dollar_quotes(&f), Read);
    // A line of the other engine without a number: MariaDB's answers all the same, MySQL's
    // gives no answer, so the greeting's is not taken for it.
    let f = reported(EngineId::MySql, v84, Some("MariaDB-custom"), Fact::Unknown(Why::EnginesDisagree));
    assert_eq!(f.engine().spec().text.dollar_quotes(&f), NotKnown);
    let f =
        reported(EngineId::MySql, Version::new(8, 0, 36), Some("MariaDB-custom"), Fact::Unknown(Why::EnginesDisagree));
    assert_eq!(f.engine().spec().text.dollar_quotes(&f), NotRead);
    let f = reported(
        EngineId::MariaDb,
        Version::new(11, 4, 13),
        Some("8.0.mysql_aurora.3.04.0"),
        Fact::Unknown(Why::EnginesDisagree),
    );
    assert_eq!(f.engine().spec().text.dollar_quotes(&f), NotKnown);
    // Numbers that disagree: not known only across 8.1.0.
    let f = reported(EngineId::MySql, Version::new(8, 0, 36), Some("8.4.6"), Fact::Known(Topology::Direct));
    assert_eq!(f.engine().spec().text.dollar_quotes(&f), NotKnown);
    let f = reported(EngineId::MySql, Version::new(8, 1, 0), Some("8.4.6"), Fact::Known(Topology::Direct));
    assert_eq!(f.engine().spec().text.dollar_quotes(&f), Read);
    // Engines that disagree: not known only when their answers differ.
    let f = reported(EngineId::MariaDb, Version::new(11, 4, 13), Some("8.4.6"), Fact::Unknown(Why::EnginesDisagree));
    assert_eq!(f.engine().spec().text.dollar_quotes(&f), NotKnown);
    let f = reported(EngineId::MySql, v84, Some("11.4.13-MariaDB"), Fact::Unknown(Why::EnginesDisagree));
    assert_eq!(f.engine().spec().text.dollar_quotes(&f), NotKnown);
    let f = reported(EngineId::MariaDb, Version::new(11, 4, 13), Some("8.0.36"), Fact::Unknown(Why::EnginesDisagree));
    assert_eq!(f.engine().spec().text.dollar_quotes(&f), NotRead);
    let f =
        reported(EngineId::MySql, Version::new(8, 0, 36), Some("11.4.13-MariaDB"), Fact::Unknown(Why::EnginesDisagree));
    assert_eq!(f.engine().spec().text.dollar_quotes(&f), NotRead);
    // PostgreSQL reads them, whatever it says.
    let pg = agreeing(EngineId::Postgres, Version::new(17, 2, 0));
    assert_eq!(EngineId::Postgres.spec().text.dollar_quotes(&pg), Read);
}

#[test]
fn rules_answer_the_language_before_and_after_the_server_said() {
    use crate::sql::dialect::{Dialect, Language, MySqlMode};
    use crate::sql::risk::Classifier;
    use std::sync::Arc;
    let pg = Language::Sql(Dialect::Postgres);
    let my = Language::Sql(Dialect::MySql(MySqlMode::default()));
    let maria = Language::Sql(Dialect::MariaDb(MySqlMode { ansi_quotes: true, ..MySqlMode::default() }));
    let family = expected("mysql");
    assert_eq!(Rules::Unbound.language(), pg);
    assert_eq!(Rules::Unbound.language().dialect(), UNBOUND_TAB_DIALECT);
    // Before the server said: the last language said of an expected engine, else the driver's.
    let before = |expected: &Expected, last| Rules::Expected { expected: expected.clone(), last, driver: my };
    assert_eq!(before(&family, None).language(), my);
    assert_eq!(before(&family, Some(maria)).language(), maria);
    assert_eq!(before(&expected("postgres"), Some(maria)).language(), my, "not an engine it expects");
    assert_eq!(before(&expected("redis"), Some(pg)).language(), my, "no engine serves it");
    // Once it said: the language it was given, whatever the facts.
    let facts = Arc::new(agreeing(EngineId::MySql, Version::new(8, 4, 6)));
    assert_eq!(Rules::Server { facts, language: maria }.language(), maria);
    // A fresh classifier of that language, as `Classifier::new` builds it.
    for lang in [pg, my, maria] {
        let rules = Rules::Expected { expected: family.clone(), last: None, driver: lang };
        assert_eq!(rules.classifier(), Classifier::new(lang), "{lang:?}");
    }
}
