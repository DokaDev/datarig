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
