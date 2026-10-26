//! Session presets, headless: what a profile's query sessions are opened with (its
//! environment's preset, its own, none, its own statements; metadata sessions never), a profile
//! whose plan has an error (an unknown preset, one for the other database, a statement that is
//! not a setting or that read-only refuses: nothing is opened), a start-up statement the server
//! refused (said with its source and number), and a session behind a pooler that sets them per
//! transaction.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbError, DbEvent, Outcome, SessionRole, SessionStatement};
use datarig_core::i18n::Lang;
use datarig_core::policy::Policy;
use datarig_core::profile::ConnectionConfig;
use datarig_core::profile::session::{Dbms, SessionMap, SessionPreset, SessionRef};
use datarig_core::secret::MemoryStore;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::{NodeState, Startup};
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::KeyCode;
use std::sync::Arc;
use std::time::Duration;

const MS: Duration = Duration::from_millis(3);

fn presets() -> Vec<SessionPreset> {
    vec![
        SessionPreset::new(
            "reporting",
            Dbms::Postgres,
            vec!["SET statement_timeout = '30s'".into(), "SET work_mem = '64MB'".into()],
        ),
        SessionPreset::new("my_ro", Dbms::MySql, vec!["SET SESSION sql_mode = 'ANSI_QUOTES'".into()]),
    ]
}

/// The test profile in environment `prod`, which maps `reporting` for PostgreSQL.
fn config(profile: impl FnOnce(&mut ConnectionConfig)) -> Config {
    let mut p = ConnectionConfig { policy: Some("prod".into()), ..ConnectionConfig::test_db() };
    profile(&mut p);
    let mut cfg = Config { connections: vec![p], sessions: presets(), ..Config::default() };
    let rules = Policy {
        session: SessionMap { postgres: Some("reporting".into()), mysql: Some("my_ro".into()) },
        ..Policy::default()
    };
    cfg.policies.insert("prod", rules);
    cfg
}

fn connected(cfg: &Config) -> Harness {
    let mut h = Harness::with_config(cfg, Lang::En);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["public".into()])));
    h.db(DbEvent::Catalog(Ok(catalog())));
    h.sent();
    h
}

/// Run `sql` in the active tab and return the run's id.
fn run(h: &mut Harness, sql: &str) -> u64 {
    h.app.tab_mut().editor = Editor::new(sql);
    h.keys("ggvG$");
    h.sent();
    h.ctrl('e');
    if h.overlay_kind() == Some(OverlayKind::RunConfirm) {
        h.keys("y");
    }
    h.app.tab().exec.query_id
}

/// The statements session `i` was opened with.
fn opened_with(h: &Harness, i: usize) -> Vec<String> {
    h.driver.sessions.lock().unwrap()[i].opts.session_sql.iter().map(|s| s.sql.clone()).collect()
}

#[test]
fn query_sessions_start_with_the_environments_preset_then_the_profiles_statements() {
    let cfg = config(|p| p.session_sql = vec!["SET work_mem = '256MB'".into()]);
    let mut h = connected(&cfg);
    let id = run(&mut h, "SELECT 1");
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Affected(0), elapsed: MS });
    assert_eq!(h.roles(), [SessionRole::Meta, SessionRole::Query]);
    assert!(opened_with(&h, 0).is_empty(), "never the metadata session");
    assert_eq!(
        opened_with(&h, 1),
        ["SET statement_timeout = '30s'", "SET work_mem = '64MB'", "SET work_mem = '256MB'"]
    );
    // Each with its form behind a pooler.
    let s = h.driver.sessions.lock().unwrap()[1].opts.session_sql.clone();
    assert_eq!(s[2].per_transaction.as_deref(), Some("SET LOCAL work_mem = '256MB'"));
}

#[test]
fn none_and_no_keys_start_with_nothing_of_a_preset() {
    // `none`: the environment's preset is skipped, the profile's own statements stay.
    let cfg = config(|p| {
        p.session = SessionRef::Off;
        p.session_sql = vec!["SET work_mem = '1MB'".into()];
    });
    let mut h = connected(&cfg);
    run(&mut h, "SELECT 1");
    assert_eq!(opened_with(&h, 1), ["SET work_mem = '1MB'"]);
    // No environment, no keys: the database's defaults.
    let cfg = Config { connections: vec![ConnectionConfig::test_db()], sessions: presets(), ..Config::default() };
    let mut h = connected(&cfg);
    run(&mut h, "SELECT 1");
    assert!(opened_with(&h, 1).is_empty());
    // Its own preset, whatever the environment maps.
    let mut cfg = config(|p| p.session = SessionRef::Named("own".into()));
    cfg.sessions.push(SessionPreset::new("own", Dbms::Postgres, vec!["SET jit = off".into()]));
    let mut h = connected(&cfg);
    run(&mut h, "SELECT 1");
    assert_eq!(opened_with(&h, 1), ["SET jit = off"]);
}

fn launched(cfg: &Config) -> Harness {
    Harness::launched(cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal).with_fake_driver()
}

/// A profile whose preset is not there, or is for the other database, or that has a statement
/// that is not a setting, does not connect: nothing is opened, and its node says why (never a
/// connection without the preset instead).
#[test]
fn a_profile_whose_plan_has_an_error_does_not_connect() {
    type Change = Box<dyn Fn(&mut ConnectionConfig)>;
    let cases: Vec<(Change, &str)> = vec![
        (
            Box::new(|p| p.session = SessionRef::Named("gone".into())),
            "local-pg names the session preset “gone”, which does not exist: pick one in its form, or none (it never connects without it instead)",
        ),
        (
            Box::new(|p| p.session = SessionRef::Named("my_ro".into())),
            "local-pg is a PostgreSQL profile and its session preset “my_ro” is for MySQL: pick a preset for its database",
        ),
        (
            Box::new(|p| p.session_sql = vec!["SET work_mem = '1MB'".into(), "DELETE FROM t".into()]),
            "local-pg does not connect: statement 2 of its own session SQL is not allowed (only SET of the session's own settings): DELETE FROM t",
        ),
        (
            Box::new(|p| p.session_sql = vec!["SET lock_timeout = '1s'".into()]),
            "local-pg does not connect: statement 1 of its own session SQL is not allowed (lock_timeout is set by the environment's lock_wait): SET lock_timeout = '1s'",
        ),
    ];
    for (change, want) in cases {
        let cfg = config(change);
        let mut h = launched(&cfg);
        h.explore("local-pg");
        h.key(KeyCode::Enter);
        assert!(h.driver.sessions.lock().unwrap().is_empty(), "nothing is opened: {want}");
        assert_eq!(h.app.conns.state(h.app.profiles[0].id), NodeState::Failed);
        assert_eq!(h.node_error("local-pg").as_deref(), Some(want));
    }
    // The environment's mapping of a preset that is not there.
    let mut cfg = config(|_| {});
    cfg.sessions.retain(|p| p.name != "reporting");
    let mut h = launched(&cfg);
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    assert!(h.driver.sessions.lock().unwrap().is_empty());
    assert_eq!(
        h.node_error("local-pg").as_deref(),
        Some(
            "The environment prod maps the session preset “reporting” for PostgreSQL, which does not exist: local-pg does not connect until it is fixed, or its form picks none"
        )
    );
    // A test of it tries nothing either.
    h.explore("local-pg");
    h.keys("t");
    let state = h.app.conn_test.as_ref().map(|t| format!("{:?}", t.state)).unwrap_or_default();
    assert!(state.contains("which does not exist"), "{state}");
    assert!(h.driver.sessions.lock().unwrap().is_empty());
}

/// In a read-only environment only the settings a read-only session may change: another
/// refuses the connect with the read-only reason.
#[test]
fn read_only_refuses_a_setting_off_its_allowlist() {
    let mut cfg = config(|p| p.session_sql = vec!["SET session_replication_role = replica".into()]);
    let mut rules = cfg.policies.environment("prod").unwrap().policy.clone();
    rules.read_only = true;
    cfg.policies.insert("prod", rules);
    let mut h = launched(&cfg);
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    assert!(h.driver.sessions.lock().unwrap().is_empty());
    assert_eq!(
        h.node_error("local-pg").as_deref(),
        Some(
            "local-pg does not connect: its environment is read-only and statement 1 of its own session SQL is refused (this setting is not on the read-only allowlist): SET session_replication_role = replica"
        )
    );
}

/// A preset deleted after the profile connected: the next query session is not opened, and the
/// tab says why.
#[test]
fn a_preset_gone_since_the_connect_opens_no_query_session() {
    let cfg = config(|_| {});
    let mut h = connected(&cfg);
    h.app.session_presets.clear();
    run(&mut h, "SELECT 1");
    assert_eq!(h.roles(), [SessionRole::Meta], "no query session");
    let status = h.app.tab().status.as_ref().map(|n| h.app.i18n.msg(&n.msg).to_string()).unwrap_or_default();
    assert!(status.contains("“reporting” for PostgreSQL, which does not exist"), "{status}");
}

/// A start-up statement the server refused: the tab says which, of what, and the server's
/// words; the run did not happen.
#[test]
fn a_refused_start_up_statement_is_said_with_its_source_and_number() {
    let cfg = config(|p| p.session_sql = vec!["SET work_mem = 'lots'".into()]);
    let mut h = connected(&cfg);
    run(&mut h, "SELECT 1");
    let error = DbError::SessionSql {
        index: 2,
        statement: "SET work_mem = 'lots'".into(),
        error: Box::new(DbError::Server("ERROR: invalid value for parameter \"work_mem\": \"lots\"".into())),
    };
    h.tab_db(0, DbEvent::ConnectFailed { error, auth: false });
    let screen = h.screen(220, 40);
    assert!(
        screen.contains("local-pg: statement 1 of its own session SQL failed, so the session was closed: ERROR: invalid value for parameter \"work_mem\": \"lots\" (SET work_mem = 'lots')"),
        "{screen}"
    );
    assert!(h.app.tab().exec.session.is_none(), "closed");
    // From the preset: its name and who picked it.
    let mut h = connected(&cfg);
    run(&mut h, "SELECT 1");
    let error = DbError::SessionSql {
        index: 1,
        statement: "SET work_mem = '64MB'".into(),
        error: Box::new(DbError::Server("ERROR: no".into())),
    };
    h.tab_db(0, DbEvent::ConnectFailed { error, auth: false });
    assert!(
        h.screen(220, 40).contains("statement 2 of the session preset “reporting” of the environment prod failed"),
        "{}",
        h.screen(220, 40)
    );
}

/// Behind a pooler the session says its settings go in each transaction, and names those it
/// did not send.
#[test]
fn behind_a_pooler_the_tab_says_how_its_settings_go() {
    let cfg = config(|p| p.session_sql = vec!["SET ROLE reporting".into()]);
    let mut h = connected(&cfg);
    run(&mut h, "SELECT 1");
    let sql: Vec<SessionStatement> = h.driver.sessions.lock().unwrap()[1].opts.session_sql.clone();
    assert_eq!(sql[2].per_transaction, None, "an identity has no transaction form");
    // Said before `Connected`.
    h.tab_db(0, DbEvent::SessionPerTransaction { refused: vec![2] });
    h.tab_db(0, DbEvent::Connected);
    let notes: Vec<String> = h.app.tab().exec.run.notes.iter().map(|n| h.app.i18n.msg(&n.msg).to_string()).collect();
    assert!(
        notes.iter().any(|n| n
            == "local-pg is behind a connection pooler: 1 session setting was not sent, as it has no form for one transaction: SET ROLE reporting"),
        "{notes:?}"
    );
    let mut h = connected(&config(|_| {}));
    run(&mut h, "SELECT 1");
    h.tab_db(0, DbEvent::SessionPerTransaction { refused: Vec::new() });
    h.tab_db(0, DbEvent::Connected);
    let notes: Vec<String> = h.app.tab().exec.run.notes.iter().map(|n| h.app.i18n.msg(&n.msg).to_string()).collect();
    assert!(notes.iter().any(|n| n.contains("go first in each transaction (SET LOCAL)")), "{notes:?}");
}
