use super::*;
use crate::sql::risk::{NotLocal, ReadOnlyBlock, SetProblem};

fn profile(driver: &str) -> ConnectionConfig {
    ConnectionConfig { name: "orders".into(), driver: driver.into(), ..ConnectionConfig::default() }
}

fn presets() -> Vec<SessionPreset> {
    vec![
        SessionPreset::new(
            "reporting",
            Dbms::Postgres,
            vec!["SET statement_timeout = '30s'".into(), "SET work_mem = '64MB'".into()],
        ),
        SessionPreset::new("my_ro", Dbms::MySql, vec!["SET SESSION sql_mode = 'ANSI_QUOTES'".into()]),
        SessionPreset::new("role", Dbms::Postgres, vec!["SET ROLE reporting".into()]),
    ]
}

fn mapped() -> Policy {
    Policy {
        session: SessionMap { postgres: Some("reporting".into()), mysql: Some("my_ro".into()) },
        ..Policy::default()
    }
}

fn sql(p: &SessionPlan) -> Vec<String> {
    p.statements().into_iter().map(|s| s.sql).collect()
}

#[test]
fn nothing_set_is_the_intents_only() {
    let p = plan(&profile("postgres"), &Policy::default(), Defaults::default(), &presets()).unwrap();
    assert!(p.statements().is_empty());
    let whats: Vec<_> = p.items.iter().map(|i| (&i.what, &i.source)).collect();
    assert_eq!(
        whats,
        [
            (&Planned::Intent(Intent::LockWait(None)), &Source::Global),
            (&Planned::Intent(Intent::IdleDisconnect(None)), &Source::Global),
            (&Planned::Intent(Intent::TxIdleRollback(Some(crate::policy::TX_IDLE_ROLLBACK))), &Source::DefaultRules),
        ]
    );
}

#[test]
fn intents_say_whose_they_are() {
    let mut c = profile("postgres");
    c.policy = Some("prod".into());
    let rules = Policy { lock_wait: Some(Some(Duration::from_secs(5))), ..Policy::default() };
    let defaults = Defaults { lock_wait: None, idle_disconnect: Some(Duration::from_secs(600)) };
    let p = plan(&c, &rules, defaults, &[]).unwrap();
    assert_eq!(p.items[0].what, Planned::Intent(Intent::LockWait(Some(Duration::from_secs(5)))));
    assert_eq!(p.items[0].source, Source::Environment("prod".into()));
    assert_eq!(p.items[1].what, Planned::Intent(Intent::IdleDisconnect(Some(Duration::from_secs(600)))));
    assert_eq!(p.items[1].source, Source::Global);
}

#[test]
fn order_is_intents_then_preset_then_profile() {
    let mut c = profile("postgres");
    c.policy = Some("prod".into());
    c.session_sql = vec!["SET work_mem = '256MB'".into()];
    let p = plan(&c, &mapped(), Defaults::default(), &presets()).unwrap();
    assert_eq!(sql(&p), ["SET statement_timeout = '30s'", "SET work_mem = '64MB'", "SET work_mem = '256MB'"]);
    let by_env = Source::Preset { name: "reporting".into(), by: PickedBy::Environment(Some("prod".into())) };
    assert_eq!(p.statement_source(0), Some((&by_env, 1)));
    assert_eq!(p.statement_source(1), Some((&by_env, 2)));
    assert_eq!(p.statement_source(2), Some((&Source::Profile, 1)));
    assert_eq!(p.statement_source(3), None);
    // Each with its form behind a pooler.
    assert_eq!(p.statements()[1].per_transaction.as_deref(), Some("SET LOCAL work_mem = '64MB'"));
    // Intents first.
    assert!(matches!(p.items[0].what, Planned::Intent(_)));
}

#[test]
fn the_profiles_own_preset_replaces_the_environments() {
    let mut c = profile("postgres");
    c.session = SessionRef::Named("role".into());
    let p = plan(&c, &mapped(), Defaults::default(), &presets()).unwrap();
    assert_eq!(sql(&p), ["SET ROLE reporting"]);
    assert_eq!(p.statement_source(0), Some((&Source::Preset { name: "role".into(), by: PickedBy::Profile }, 1)));
    // An identity has no form for one transaction: refused behind a pooler, and said so.
    let item = p.items.iter().find(|i| matches!(i.what, Planned::Sql(_))).unwrap();
    assert_eq!(item.pooled, Some(OnPooler::Refused(NotLocal::Identity)));
    assert_eq!(p.statements()[0].per_transaction, None);
}

#[test]
fn none_skips_the_environments_preset_and_says_so() {
    let mut c = profile("postgres");
    c.session = SessionRef::Off;
    c.session_sql = vec!["SET work_mem = '1MB'".into()];
    let p = plan(&c, &mapped(), Defaults::default(), &presets()).unwrap();
    assert_eq!(sql(&p), ["SET work_mem = '1MB'"]);
    assert!(p.items.contains(&PlanItem {
        what: Planned::Skipped { preset: "reporting".into() },
        source: Source::Profile,
        pooled: None,
    }));
    // Nothing to skip without a mapping: no item.
    let p = plan(&c, &Policy::default(), Defaults::default(), &presets()).unwrap();
    assert!(!p.items.iter().any(|i| matches!(i.what, Planned::Skipped { .. })));
}

#[test]
fn only_own_statements_without_a_preset() {
    let mut c = profile("mysql");
    c.session_sql = vec!["SET SESSION group_concat_max_len = 4096".into()];
    let p = plan(&c, &Policy::default(), Defaults::default(), &[]).unwrap();
    assert_eq!(sql(&p), ["SET SESSION group_concat_max_len = 4096"]);
    // MySQL statements are never set per transaction.
    assert_eq!(p.items.last().unwrap().pooled, None);
}

#[test]
fn the_environment_maps_by_database() {
    let p = plan(&profile("mariadb"), &mapped(), Defaults::default(), &presets()).unwrap();
    assert_eq!(sql(&p), ["SET SESSION sql_mode = 'ANSI_QUOTES'"]);
}

#[test]
fn an_unknown_preset_is_an_error_never_none() {
    let mut c = profile("postgres");
    c.session = SessionRef::Named("gone".into());
    assert_eq!(
        plan(&c, &Policy::default(), Defaults::default(), &presets()),
        Err(SessionError::NotFound { preset: "gone".into(), by: PickedBy::Profile })
    );
    let rules = Policy { session: SessionMap { postgres: Some("gone".into()), mysql: None }, ..Policy::default() };
    assert_eq!(
        plan(&profile("postgres"), &rules, Defaults::default(), &presets()),
        Err(SessionError::NotFound { preset: "gone".into(), by: PickedBy::Environment(None) })
    );
}

#[test]
fn a_preset_of_the_other_database_is_an_error() {
    let mut c = profile("mysql");
    c.session = SessionRef::Named("reporting".into());
    assert_eq!(
        plan(&c, &Policy::default(), Defaults::default(), &presets()),
        Err(SessionError::WrongDbms {
            preset: "reporting".into(),
            by: PickedBy::Profile,
            dbms: Dbms::Postgres,
            driver: "mysql".into()
        })
    );
    // An environment's mapping pointing at the other database's preset too.
    let rules = Policy { session: SessionMap { postgres: Some("my_ro".into()), mysql: None }, ..Policy::default() };
    assert!(matches!(
        plan(&profile("postgres"), &rules, Defaults::default(), &presets()),
        Err(SessionError::WrongDbms { dbms: Dbms::MySql, .. })
    ));
}

#[test]
fn statements_are_checked_with_their_source_and_number() {
    let mut c = profile("postgres");
    c.session_sql = vec!["SET work_mem = '1MB'".into(), "SET lock_timeout = '1s'".into()];
    match plan(&c, &Policy::default(), Defaults::default(), &[]) {
        Err(SessionError::Statement {
            source: Source::Profile,
            index: 2,
            problem: SetProblem::AppOwned { .. },
            ..
        }) => {}
        other => panic!("{other:?}"),
    }
    let bad = SessionPreset::new("bad", Dbms::Postgres, vec!["DELETE FROM t".into()]);
    c.session = SessionRef::Named("bad".into());
    c.session_sql.clear();
    match plan(&c, &Policy::default(), Defaults::default(), std::slice::from_ref(&bad)) {
        Err(SessionError::Statement { source: Source::Preset { name, .. }, index: 1, problem, .. }) => {
            assert_eq!(name, "bad");
            assert_eq!(problem, SetProblem::NotSet);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(bad.problem(), Some((1, SetProblem::NotSet)));
}

#[test]
fn read_only_allows_only_safe_settings() {
    let rules = Policy { read_only: true, ..Policy::default() };
    let mut c = profile("postgres");
    c.session_sql = vec!["SET work_mem = '1MB'".into()];
    assert!(plan(&c, &rules, Defaults::default(), &[]).is_ok());
    c.session_sql = vec!["SET session_replication_role = replica".into()];
    assert_eq!(
        plan(&c, &rules, Defaults::default(), &[]),
        Err(SessionError::ReadOnly {
            source: Source::Profile,
            index: 1,
            statement: "SET session_replication_role = replica".into(),
            block: ReadOnlyBlock::Setting,
        })
    );
    // The same statement off read-only is fine.
    assert!(plan(&c, &Policy::default(), Defaults::default(), &[]).is_ok());
    let risky = SessionPreset::new("fk", Dbms::MySql, vec!["SET foreign_key_checks = 0".into()]);
    assert_eq!(risky.read_only_refused(), Some((1, ReadOnlyBlock::Setting)));
}

#[test]
fn statements_split_and_join_as_a_field_shows_them() {
    let text = "SET a.b = 'x;y'; SET c.d = 1 ;; ";
    assert_eq!(split_statements(Dbms::Postgres, text), ["SET a.b = 'x;y'", "SET c.d = 1"]);
    assert_eq!(
        join_statements(Dbms::Postgres, &split_statements(Dbms::Postgres, text)),
        "SET a.b = 'x;y'; SET c.d = 1"
    );
    // A line comment would hide the statements after it on the field's one line: it goes.
    let file = ["SET a.b = 1 -- one".to_string(), "SET c.d = 2".to_string()];
    let field = join_statements(Dbms::Postgres, &file);
    assert_eq!(field, "SET a.b = 1; SET c.d = 2");
    assert_eq!(split_statements(Dbms::Postgres, &field), ["SET a.b = 1", "SET c.d = 2"]);
    assert_eq!(join_statements(Dbms::Postgres, &["SET a.b = 'x  y'\n  -- c\n".to_string()]), "SET a.b = 'x  y'");
    let my = ["SET @a = 1 # one".to_string(), "SET @b = '#'".to_string()];
    assert_eq!(split_statements(Dbms::MySql, &join_statements(Dbms::MySql, &my)), ["SET @a = 1", "SET @b = '#'"]);
    assert_eq!(split_statements(Dbms::MySql, "SET @a = \"q;\"; SET @b = 2"), ["SET @a = \"q;\"", "SET @b = 2"]);
}

#[test]
fn session_values_as_written() {
    assert_eq!(SessionRef::of("none"), SessionRef::Off);
    assert_eq!(SessionRef::of("x"), SessionRef::Named("x".into()));
    assert_eq!(SessionRef::Off.as_value(), Some("none"));
    assert_eq!(SessionRef::Inherit.as_value(), None);
    assert_eq!(Dbms::of_driver("PG"), Some(Dbms::Postgres));
    assert_eq!(Dbms::of_driver("redis"), None);
}

/// Who the session is: `SET ROLE` is on the read-only allowlist (every transaction stays
/// `READ ONLY` on the server whatever the role), `SET SESSION AUTHORIZATION` is not.
#[test]
fn read_only_takes_a_role_but_not_another_authorization() {
    let rules = Policy { read_only: true, ..Policy::default() };
    let mut c = profile("postgres");
    c.session_sql = vec!["SET ROLE reporting".into()];
    assert!(plan(&c, &rules, Defaults::default(), &[]).is_ok());
    c.session_sql = vec!["SET SESSION AUTHORIZATION reporting".into()];
    assert!(matches!(plan(&c, &rules, Defaults::default(), &[]), Err(SessionError::ReadOnly { .. })));
}
