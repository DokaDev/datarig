//! Startup SQL presets, headless: what a profile's query sessions are opened with (its
//! environment's preset, its own, none, its own statements; metadata sessions never), a profile
//! whose plan has an error (an unknown preset, one for the other database, a statement that is
//! not a setting or that read-only refuses: nothing is opened), a start-up statement the server
//! refused (said with its source and number), and a session behind a pooler that sets them per
//! transaction. The explorer's "Startup SQL" section: its rows, the preset form (new,
//! edit, rename, copy, a statement that is not allowed), delete and test. The profile form's
//! Startup SQL list and own startup SQL, and the environment form's lists; `:startup`.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbError, DbEvent, Outcome, SessionRole, SessionStatement};
use datarig_core::i18n::Lang;
use datarig_core::policy::Policy;
use datarig_core::profile::ConnectionConfig;
use datarig_core::profile::session::{Dbms, SessionMap, SessionPreset, SessionRef};
use datarig_core::secret::MemoryStore;
use datarig_tui::app::explorer::RowKind;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::profiles::Field;
use datarig_tui::app::{Focus, NodeState, Startup, TestState};
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::{Event, KeyCode};
use std::path::PathBuf;
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
    let mut editor = Editor::new(sql);
    editor.set_language(h.app.tab_language(h.app.tab().id));
    h.app.tab_mut().editor = editor;
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
            "local-pg names the Startup SQL “gone”, which does not exist: pick one in its form, or none (it never connects without it instead)",
        ),
        (
            Box::new(|p| p.session = SessionRef::Named("my_ro".into())),
            "local-pg is a PostgreSQL profile and its Startup SQL “my_ro” is for MySQL: pick a Startup SQL for its database",
        ),
        (
            Box::new(|p| p.session_sql = vec!["SET work_mem = '1MB'".into(), "DELETE FROM t".into()]),
            "local-pg does not connect: statement 2 of its own startup SQL is not allowed (only SET of the session's own settings): DELETE FROM t",
        ),
        (
            Box::new(|p| p.session_sql = vec!["SET idle_session_timeout = '1s'".into()]),
            "local-pg does not connect: statement 1 of its own startup SQL is not allowed (idle_session_timeout: datarig closes idle sessions itself and warns first; the server's limit is only its safety net — set the environment's idle_disconnect): SET idle_session_timeout = '1s'",
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
            "The environment prod maps the Startup SQL “reporting” for PostgreSQL, which does not exist: local-pg does not connect until it is fixed, or its form picks none"
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
            "local-pg does not connect: its environment is read-only and statement 1 of its own startup SQL is refused (this setting is not on the read-only allowlist): SET session_replication_role = replica"
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
        screen.contains("local-pg: statement 1 of its own startup SQL failed, so the session was closed: ERROR: invalid value for parameter \"work_mem\": \"lots\" (SET work_mem = 'lots')"),
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
        h.screen(220, 40).contains("statement 2 of the Startup SQL “reporting” of the environment prod failed"),
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
            == "local-pg is behind a connection pooler: 1 session setting was not sent, as it cannot be set for one transaction there (:session says why): SET ROLE reporting"),
        "{notes:?}"
    );
    let mut h = connected(&config(|_| {}));
    run(&mut h, "SELECT 1");
    h.tab_db(0, DbEvent::SessionPerTransaction { refused: Vec::new() });
    h.tab_db(0, DbEvent::Connected);
    let notes: Vec<String> = h.app.tab().exec.run.notes.iter().map(|n| h.app.i18n.msg(&n.msg).to_string()).collect();
    assert!(notes.iter().any(|n| n.contains("go first in each transaction (SET LOCAL)")), "{notes:?}");
}

// ── the explorer's section and the preset form ──────────────────────────

/// A scratch directory, removed when dropped.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(tag: &str) -> Scratch {
    let d = std::env::temp_dir().join(format!("datarig-sessions-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Scratch(d)
}

impl Scratch {
    fn config(&self) -> PathBuf {
        self.0.join("config.toml")
    }

    fn text(&self) -> String {
        std::fs::read_to_string(self.config()).unwrap_or_default()
    }
}

/// The sample profiles (`local-pg` in `prod`, which maps `reporting`; the replica naming
/// `my_ro`, which is MySQL's), the presets with one that has a statement that is not allowed and
/// one with a risky setting, written to `dir`.
fn section_config(dir: &Scratch) -> Config {
    let mut cfg = sample_config(None);
    cfg.connections[0].policy = Some("prod".into());
    cfg.connections[1].session = SessionRef::Named("my_ro".into());
    cfg.sessions = presets();
    cfg.sessions.push(SessionPreset::new(
        "broken",
        Dbms::Postgres,
        vec!["SET work_mem = '1MB'".into(), "SET LOCAL work_mem = '2MB'".into()],
    ));
    cfg.sessions.push(SessionPreset::new("fk_off", Dbms::MySql, vec!["SET foreign_key_checks = 0".into()]));
    cfg.sessions.sort_by(|a, b| a.name.cmp(&b.name));
    let rules = Policy { session: SessionMap { postgres: Some("reporting".into()), mysql: None }, ..Policy::default() };
    cfg.policies.insert("prod", rules);
    cfg.path = Some(dir.config());
    datarig_core::config::save(
        &dir.config(),
        datarig_core::config::Settings {
            version: 2,
            language: "en",
            icons: Default::default(),
            theme: datarig_core::theme::DEFAULT,
            default_source: Default::default(),
            prefs: Default::default(),
        },
        Some(datarig_core::config::Profiles {
            connections: &cfg.connections,
            tunnels: &[],
            sessions: Some(&cfg.sessions),
            environments: Some(cfg.policies.environments()),
            default_rules: None,
            folders: &cfg.folders,
            last_used: None,
        }),
    )
    .unwrap();
    let (cfg, err) = datarig_core::config::load(Some(dir.config()));
    assert!(err.is_none(), "{err:?}");
    cfg
}

fn section_harness(dir: &Scratch) -> Harness {
    launched(&section_config(dir))
}

fn preset_id(h: &Harness, name: &str) -> datarig_core::profile::session::SessionPresetId {
    h.app.session_presets().iter().find(|p| p.name == name).expect("preset").id
}

fn on_row(h: &mut Harness, kind: RowKind) {
    h.app.focus = Focus::Tree;
    h.app.explorer.select_kind(kind);
}

fn on_preset(h: &mut Harness, name: &str) {
    let id = preset_id(h, name);
    on_row(h, RowKind::Session(id));
}

fn said(h: &Harness) -> String {
    h.app.transient.as_ref().map(|(n, _)| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

/// The section lists the presets by name, each with its database, statements and profiles, the
/// problem of one that has a statement not allowed under it, and, opened, its profiles; the
/// filter finds a preset by a statement.
#[test]
fn the_section_lists_the_presets_with_their_users_and_problems() {
    let dir = scratch("section");
    let mut h = section_harness(&dir);
    on_preset(&mut h, "reporting");
    h.key(KeyCode::Char('l'));
    let rows = h.rows();
    let at = rows.iter().position(|r| r == "[startup sql]").expect("the section");
    assert_eq!(
        rows[at..at + 7],
        ["[startup sql]", "  $broken", "    !", "  $fk_off", "  $my_ro", "  $reporting", "    @local-pg"],
        "{rows:?}"
    );
    on_preset(&mut h, "my_ro");
    let line = h.explorer_line();
    assert!(line.contains("my_ro  MySQL · 1 statement · 1 profile"), "{line}");
    on_preset(&mut h, "broken");
    assert!(h.explorer_line().contains("broken  PostgreSQL · 2 statements"), "{}", h.explorer_line());
    for (w, ht) in [(80, 24), (120, 40)] {
        insta::assert_snapshot!(format!("sessions_section_en_{w}x{ht}"), h.draw(w, ht).backend());
    }
    // Its problem's line, whole in the status bar (the explorer is narrow).
    let broken = preset_id(&h, "broken");
    on_row(&mut h, RowKind::SessionError(broken));
    let status = h.status(120, 40);
    assert!(status.contains("statement 2: SET LOCAL ends with its transaction: write SET"), "{status}");
    // The filter: by a statement.
    on_preset(&mut h, "broken");
    h.keys("/");
    h.type_text("foreign_key");
    let rows = h.rows();
    assert!(rows.contains(&"  $fk_off".to_string()) && !rows.contains(&"  $reporting".to_string()), "{rows:?}");
}

/// `n` in the section opens the preset form; its statements are a list, one per row (`Enter`
/// after one starts the next), saved one per item; the database is picked with ←/→.
#[test]
fn a_new_preset_is_made_in_its_form() {
    let dir = scratch("new");
    let mut h = section_harness(&dir);
    on_row(&mut h, RowKind::SessionsHeader);
    h.keys("n");
    assert!(h.form().is_session(), "the preset form");
    h.type_text("analytics");
    h.key(KeyCode::Tab);
    assert_eq!(h.form().focus, Field::SessionDbms);
    h.key(KeyCode::Right);
    h.key(KeyCode::Right);
    h.key(KeyCode::Tab);
    h.type_text("SET statement_timeout = '1min'");
    h.key(KeyCode::Enter);
    h.type_text("SET jit = off");
    for (w, ht) in [(80, 24), (120, 40)] {
        insta::assert_snapshot!(format!("session_form_en_{w}x{ht}"), h.draw(w, ht).backend());
    }
    h.ctrl('s');
    assert!(!h.form_open(), "saved and closed");
    assert_eq!(said(&h), "Saved the Startup SQL “analytics”");
    let p = h.app.session_presets().iter().find(|p| p.name == "analytics").expect("made").clone();
    assert_eq!(
        (p.dbms, p.sql.as_slice()),
        (Dbms::Postgres, ["SET statement_timeout = '1min'".to_string(), "SET jit = off".to_string()].as_slice())
    );
    assert!(
        dir.text().contains(
            "[startup.analytics]\ndbms = \"postgres\"\nsql = [\"SET statement_timeout = '1min'\", \"SET jit = off\"]\n"
        ),
        "{}",
        dir.text()
    );
    assert_eq!(h.app.explorer.cursor(), &RowKind::Session(p.id), "revealed");
}

/// Each statement that is not allowed says why under its own row and keeps the form from
/// saving (the focus goes to the first field with an error, the first such row in the list);
/// `none` is not a name; on MySQL each row a read-only environment refuses says so.
#[test]
fn the_form_refuses_statements_that_are_not_settings_and_the_reserved_name() {
    let dir = scratch("refuse");
    let mut h = section_harness(&dir);
    on_row(&mut h, RowKind::SessionsHeader);
    h.keys("n");
    h.type_text("none");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    for (i, sql) in ["SET work_mem = '1MB'", "RESET work_mem", "DELETE FROM t"].into_iter().enumerate() {
        if i > 0 {
            h.key(KeyCode::Enter);
        }
        h.type_text(sql);
    }
    let screen = h.screen(120, 40);
    assert!(screen.contains("reserved: it means none"), "{screen}");
    let lines: Vec<&str> = screen.lines().collect();
    let row = |sql: &str| lines.iter().position(|l| l.contains(sql)).unwrap_or_else(|| panic!("{sql}\n{screen}"));
    for sql in ["RESET work_mem", "DELETE FROM t"] {
        assert!(lines[row(sql) + 1].contains("only SET of the session's own settings"), "under {sql}:\n{screen}");
    }
    assert_eq!(row("SET work_mem = '1MB'") + 1, row("RESET work_mem"), "nothing under an allowed one");
    for (w, ht) in [(80, 24), (120, 40)] {
        insta::assert_snapshot!(format!("session_form_errors_en_{w}x{ht}"), h.draw(w, ht).backend());
    }
    h.ctrl('s');
    assert!(h.form_open(), "not saved");
    assert_eq!(h.form().focus, Field::Name, "the first field with an error");
    for _ in 0.."none".len() {
        h.key(KeyCode::Backspace);
    }
    h.type_text("ro");
    h.ctrl('s');
    assert!(h.form_open(), "not saved");
    assert_eq!((h.form().focus, h.form().session_sql.row), (Field::SessionSql, 1), "the first refused row");
    h.ctrl('d');
    assert_eq!(h.form().session_sql.statements(), ["SET work_mem = '1MB'", "DELETE FROM t"]);
    h.ctrl('d');
    assert_eq!(h.form().session_sql.statements(), ["SET work_mem = '1MB'"]);
    h.key(KeyCode::Enter);
    h.type_text("SET foreign_key_checks = 0");
    // A risky MySQL setting: allowed, but read-only environments refuse it.
    h.key(KeyCode::BackTab);
    assert_eq!(h.form().focus, Field::SessionDbms);
    h.key(KeyCode::Right);
    // On MySQL neither is on the read-only allowlist: each says so.
    let screen = h.screen(120, 40);
    assert_eq!(screen.matches("refused in read-only environments").count(), 2, "{screen}");
    h.ctrl('s');
    assert!(!h.form_open(), "a warning does not keep it from saving");
    assert!(
        dir.text().contains(
            "[startup.ro]\ndbms = \"mysql\"\nsql = [\"SET work_mem = '1MB'\", \"SET foreign_key_checks = 0\"]\n"
        ),
        "{}",
        dir.text()
    );
    assert!(!dir.text().contains("[startup.none]"));
}

/// The list's keys: `↑`/`↓` go from row to row and leave the list at its ends (coming back
/// from above on the first row, from below on the last), `Tab` leaves it, `Enter` on an empty
/// row removes it and moves on, `Backspace` on an empty row removes it.
#[test]
fn the_statement_lists_keys_add_move_and_remove_rows() {
    let dir = scratch("list-keys");
    let mut h = section_harness(&dir);
    on_row(&mut h, RowKind::SessionsHeader);
    h.keys("n");
    h.type_text("keys");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.type_text("SET a = 1");
    h.key(KeyCode::Enter);
    h.type_text("SET b = 2");
    h.key(KeyCode::Enter);
    let at = |h: &Harness| (h.form().focus, h.form().session_sql.row, h.form().session_sql.row_count());
    assert_eq!(at(&h), (Field::SessionSql, 2, 3));
    h.key(KeyCode::Up);
    h.key(KeyCode::Up);
    assert_eq!(at(&h), (Field::SessionSql, 0, 3));
    h.key(KeyCode::Up);
    assert_eq!(h.form().focus, Field::SessionDbms, "past the first row");
    h.key(KeyCode::Down);
    assert_eq!(at(&h), (Field::SessionSql, 0, 3), "back on the first row");
    h.key(KeyCode::Down);
    h.key(KeyCode::Down);
    h.key(KeyCode::Backspace);
    assert_eq!(at(&h), (Field::SessionSql, 1, 2), "the empty row went");
    assert_eq!(h.form().session_sql.text(1), "SET b = 2", "nothing of the row before was deleted");
    h.key(KeyCode::Down);
    assert_eq!(h.form().focus, Field::Test, "past the last row");
    h.key(KeyCode::BackTab);
    assert_eq!(at(&h), (Field::SessionSql, 1, 2), "back on the last row");
    h.key(KeyCode::Enter);
    assert_eq!(at(&h), (Field::SessionSql, 2, 3));
    h.key(KeyCode::Enter);
    assert_eq!((h.form().focus, h.form().session_sql.row_count()), (Field::Test, 2), "an empty row's Enter moves on");
    h.key(KeyCode::BackTab);
    h.key(KeyCode::Tab);
    assert_eq!(h.form().focus, Field::Test, "Tab leaves the list");
    assert_eq!(h.form().session_sql.statements(), ["SET a = 1", "SET b = 2"]);
    // A paste of several statements: a row each, after the focused one.
    h.key(KeyCode::BackTab);
    h.key(KeyCode::Enter);
    h.app.handle_event(Event::Paste("SET c = 3;\n-- the next\nSET d = 4; SET e = 5\n".into()));
    assert_eq!(h.form().session_sql.statements(), ["SET a = 1", "SET b = 2", "SET c = 3", "SET d = 4", "SET e = 5"]);
    assert_eq!(h.form().session_sql.row, 4);
    h.ctrl('s');
    assert!(!h.form_open(), "saved");
    assert!(
        dir.text().contains("sql = [\"SET a = 1\", \"SET b = 2\", \"SET c = 3\", \"SET d = 4\", \"SET e = 5\"]"),
        "{}",
        dir.text()
    );
}

/// The position of the `n`-th `text` on a 100x30 screen.
fn find_on(h: &mut Harness, text: &str, n: usize) -> (u16, u16) {
    let screen = h.screen(100, 30);
    screen
        .lines()
        .enumerate()
        .flat_map(|(y, row)| row.match_indices(text).map(move |(i, _)| (row, i, y)))
        .nth(n)
        .map(|(row, i, y)| (datarig_tui::text::width(&row[..i]) as u16, y as u16))
        .unwrap_or_else(|| panic!("no {text:?} #{n}:\n{screen}"))
}

/// A click at (x, y) after the dialog's arming delay.
fn click_at(h: &mut Harness, (x, y): (u16, u16)) {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    h.advance(Duration::from_millis(500));
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

/// The mouse on the list: "+ Add statement" adds a row at the end, `×` removes its row, a click
/// on a row puts the cursor under the pointer, and the wheel scrolls a list longer than its
/// room (the lines out of view counted under the label).
#[test]
fn the_statement_list_takes_the_mouse() {
    use ratatui::crossterm::event::MouseEventKind;
    let dir = scratch("list-mouse");
    let mut h = section_harness(&dir);
    on_preset(&mut h, "reporting");
    h.keys("e");
    // A row's number focuses it too.
    let at = find_on(&mut h, "2 SET work_mem", 0);
    click_at(&mut h, at);
    assert_eq!((h.form().focus, h.form().session_sql.row), (Field::SessionSql, 1));
    let at = find_on(&mut h, "+ Add statement", 0);
    click_at(&mut h, at);
    assert_eq!((h.form().focus, h.form().session_sql.row, h.form().session_sql.row_count()), (Field::SessionSql, 2, 3));
    h.type_text("SET jit = off");
    let screen = h.screen(100, 30);
    let (y, line) = screen.lines().enumerate().find(|(_, l)| l.contains("SET statement_timeout")).expect("the row");
    let x = datarig_tui::text::width(&line[..line.rfind('×').expect("its ×")]) as u16;
    click_at(&mut h, (x, y as u16));
    assert_eq!(h.form().session_sql.statements(), ["SET work_mem = '64MB'", "SET jit = off"]);
    let at = find_on(&mut h, "jit = off", 0);
    click_at(&mut h, at);
    h.type_text("_");
    assert_eq!((h.form().session_sql.row, h.form().session_sql.text(1)), (1, "SET _jit = off"));
    // Twenty rows: more than the room at 80x24.
    h.key(KeyCode::End);
    h.key(KeyCode::Enter);
    let many: Vec<String> = (1..=18).map(|i| format!("SET work_mem = '{i}MB'")).collect();
    h.app.handle_event(Event::Paste(many.join(";\n")));
    assert_eq!(h.form().session_sql.row_count(), 20);
    let screen = h.screen(80, 24);
    assert!(
        screen.contains("↑ 13 more lines")
            && screen.matches("more line").count() == 1
            && screen.contains("+ Add statement"),
        "the focused last row in view, and the line that adds one:\n{screen}"
    );
    let (x, y) = {
        let lines: Vec<&str> = screen.lines().collect();
        let y = lines.iter().position(|l| l.contains("'15MB'")).expect("a row in view");
        (40, y as u16)
    };
    let before = h.form().session_sql.scroll;
    h.mouse(MouseEventKind::ScrollUp, x, y);
    h.mouse(MouseEventKind::ScrollUp, x, y);
    let screen = h.screen(80, 24);
    assert_eq!(h.form().session_sql.scroll, before - 2, "{screen}");
    assert!(screen.contains("↓ 2 more lines"), "{screen}");
    insta::assert_snapshot!("session_form_list_scrolled_en_80x24", h.draw(80, 24).backend());
}

/// Rows not edited are saved as the file has them (their comments and line breaks too, though
/// a row shows them on one line), in a preset and in a profile's own list; an edited one is
/// saved as its one line.
#[test]
fn unedited_statements_are_saved_as_the_file_has_them() {
    let dir = scratch("verbatim");
    section_config(&dir);
    let own = "name = \"local-pg\"\nstartup_sql = [\"SET work_mem = '1MB' -- mine\", \"\"\"SET jit\n  = off\"\"\"]\n";
    let text = dir.text().replacen("name = \"local-pg\"\n", own, 1)
        + "\n# kept\n[startup.zz_commented]\ndbms = \"postgres\"\nsql = [\n  \"SET statement_timeout = '30s' -- a comment\",  # why\n  \"\"\"SET work_mem\n  = '64MB'\"\"\",\n]\n";
    std::fs::write(dir.config(), &text).unwrap();
    let (cfg, err) = datarig_core::config::load(Some(dir.config()));
    assert!(err.is_none(), "{err:?}");
    let mut h = launched(&cfg);
    on_preset(&mut h, "zz_commented");
    h.keys("e");
    let screen = h.screen(120, 40);
    assert!(screen.contains("SET statement_timeout = '30s'  ") && screen.contains("SET work_mem = '64MB'"), "{screen}");
    h.ctrl('s');
    assert!(!h.form_open(), "saved");
    assert_eq!(dir.text(), text, "unedited: as written");
    profile_field(&mut h, "local-pg", Field::SessionSql);
    assert_eq!(h.form().session_sql.text(1), "SET jit = off");
    h.ctrl('s');
    assert!(!h.form_open(), "saved");
    assert_eq!(dir.text(), text, "unedited: as written");
    // An edited row is its one line; the other keeps its text.
    on_preset(&mut h, "zz_commented");
    h.keys("e");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.key(KeyCode::Down);
    for _ in 0.."64MB'".len() {
        h.key(KeyCode::Backspace);
    }
    h.type_text("32MB'");
    h.ctrl('s');
    assert!(!h.form_open(), "saved");
    assert!(
        dir.text().contains("# kept\n[startup.zz_commented]\ndbms = \"postgres\"\nsql = [\"SET statement_timeout = '30s' -- a comment\", \"SET work_mem = '32MB'\"]\n"),
        "{}",
        dir.text()
    );
}

/// A rename takes the profiles and environments that name the preset along, in the file too; a
/// copy gets a free name and the same statements.
#[test]
fn a_rename_follows_into_profiles_and_environments_and_a_copy_gets_a_free_name() {
    let dir = scratch("rename");
    let mut h = section_harness(&dir);
    on_preset(&mut h, "my_ro");
    h.keys("e");
    assert_eq!(h.form().original_name.as_deref(), Some("my_ro"));
    for _ in 0.."my_ro".len() {
        h.key(KeyCode::Backspace);
    }
    h.type_text("mysql_ro");
    h.ctrl('s');
    assert_eq!(h.app.profiles[1].session, SessionRef::Named("mysql_ro".into()));
    on_preset(&mut h, "reporting");
    h.keys("e");
    for _ in 0.."reporting".len() {
        h.key(KeyCode::Backspace);
    }
    h.type_text("reports");
    h.ctrl('s');
    let rules = &h.app.policies.environment("prod").unwrap().policy.session;
    assert_eq!(rules.postgres.as_deref(), Some("reports"));
    let text = dir.text();
    assert!(text.contains("[startup.mysql_ro]") && !text.contains("[startup.my_ro]"), "{text}");
    assert!(text.contains("startup = \"mysql_ro\""), "{text}");
    assert!(text.contains("startup = { postgres = \"reports\" }"), "{text}");
    // A copy.
    on_preset(&mut h, "reports");
    h.keys("c");
    assert_eq!(h.form().name.text(), "reports-copy");
    h.ctrl('s');
    let copy = h.app.session_presets().iter().find(|p| p.name == "reports-copy").expect("copied");
    assert_eq!(copy.sql, presets()[0].sql);
}

/// Deleting asks first and names the profiles and environments that use it; their references
/// stay, and the profile then does not connect (never without it).
#[test]
fn a_deleted_preset_leaves_its_names_and_its_profiles_do_not_connect() {
    let dir = scratch("delete");
    let mut h = section_harness(&dir);
    on_preset(&mut h, "reporting");
    h.keys("d");
    let screen = h.screen(120, 40);
    assert!(screen.contains("Delete the Startup SQL “reporting”?"), "{screen}");
    assert!(screen.contains("local-pg uses it: it will not connect until you pick"), "{screen}");
    assert!(screen.contains("Environments map it: prod; their profiles will not"), "{screen}");
    insta::assert_snapshot!("session_delete_en_80x24", h.draw(80, 24).backend());
    assert!(matches!(
        h.app.overlays.top(),
        Some(datarig_tui::app::overlay::Overlay::Confirm(c)) if matches!(c.action, ConfirmAction::DeleteSession(_))
    ));
    h.keys("y");
    assert!(!h.app.session_presets().iter().any(|p| p.name == "reporting"));
    assert_eq!(
        said(&h),
        "Deleted the Startup SQL “reporting”; the profile that used it needs another Startup SQL or none"
    );
    assert!(!dir.text().contains("[startup.reporting]"), "{}", dir.text());
    assert!(dir.text().contains("startup = { postgres = \"reporting\" }"), "the environment keeps the name");
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    assert!(h.driver.sessions.lock().unwrap().is_empty(), "nothing is opened");
    assert!(h.node_error("local-pg").is_some_and(|e| e.contains("“reporting” for PostgreSQL, which does not exist")));
}

/// The context menu of a preset has its own words.
#[test]
fn the_menu_of_a_preset_says_what_it_does() {
    let dir = scratch("menu");
    let mut h = section_harness(&dir);
    h.right_click_row("$reporting");
    let labels = h.menu_labels();
    for want in ["Edit Startup SQL", "Duplicate Startup SQL", "Test Startup SQL", "Delete Startup SQL"] {
        assert!(labels.iter().any(|l| l == want), "{want}: {labels:?}");
    }
    assert!(!labels.iter().any(|l| l.contains("SSH tunnel") || l.contains("console")), "{labels:?}");
}

// ── test ────────────────────────────────────────────────────────────────

/// A test needs a connected profile: none of the database, or none connected, is said; on the
/// one connected it opens a query session that starts with the preset alone, and once that
/// connected it says so and closes it.
#[test]
fn a_test_runs_on_a_connected_profile_and_closes_its_session() {
    let mut cfg = config(|p| p.session_sql = vec!["SET work_mem = '256MB'".into()]);
    cfg.sessions.push(SessionPreset::new("pg_only", Dbms::MySql, vec!["SET @a = 1".into()]));
    let mut h = connected(&cfg);
    // MySQL: no profile of it.
    on_preset(&mut h, "pg_only");
    h.keys("t");
    assert_eq!(said(&h), "No profile uses MySQL: nothing to test the Startup SQL with");
    assert!(h.driver.sessions.lock().unwrap().len() == 1, "nothing opened");
    // On the connected profile that uses it.
    on_preset(&mut h, "reporting");
    h.keys("t");
    assert_eq!(h.roles(), [SessionRole::Meta, SessionRole::Query], "a throwaway query session");
    assert_eq!(opened_with(&h, 1), ["SET statement_timeout = '30s'", "SET work_mem = '64MB'"], "the preset alone");
    assert_eq!(h.app.conn_test.as_ref().map(|t| t.state.clone()), Some(TestState::Running));
    let seq = h.app.conn_test.as_ref().unwrap().seq;
    h.app.on_app_event(datarig_tui::app::AppEvent::Db {
        target: datarig_tui::app::EventTarget::SessionTest(seq),
        generation: seq,
        ev: DbEvent::Connected,
    });
    assert!(h.session_closed(1), "closed once it connected");
    assert_eq!(said(&h), "Startup SQL “reporting” applied on local-pg (2 statements); the test session was closed");
    // Its failure, in the server's words.
    h.keys("t");
    let seq = h.app.conn_test.as_ref().unwrap().seq;
    let error = DbError::SessionSql {
        index: 0,
        statement: "SET statement_timeout = '30s'".into(),
        error: Box::new(DbError::Server("ERROR: nope".into())),
    };
    h.app.on_app_event(datarig_tui::app::AppEvent::Db {
        target: datarig_tui::app::EventTarget::SessionTest(seq),
        generation: seq,
        ev: DbEvent::ConnectFailed { error, auth: false },
    });
    assert!(
        said(&h).starts_with(
            "The Startup SQL could not be applied: statement 1 the session starts with failed: ERROR: nope"
        ),
        "{}",
        said(&h)
    );
}

/// A profile that is not connected is not connected for a test: the test says which to connect.
#[test]
fn a_test_without_a_connection_says_which_to_connect() {
    let dir = scratch("test-off");
    let mut h = section_harness(&dir);
    on_preset(&mut h, "reporting");
    h.keys("t");
    assert_eq!(said(&h), "Connect local-pg first: the test opens a session on that connection");
    assert!(h.driver.sessions.lock().unwrap().is_empty());
}

// ── the profile form's and the environment form's lists ────────────────

/// The session preset list of the form on top (its rows' labels).
fn session_rows(h: &Harness) -> Vec<String> {
    let Some(datarig_tui::app::picker::Picker::Session { list, .. }) = h.app.overlays.picker() else {
        panic!("the session preset list is open")
    };
    list.rows().iter().map(|r| list.label(*r).to_string()).collect()
}

/// Pick the row labelled `label` in the open session preset list.
fn pick_session(h: &mut Harness, label: &str) {
    h.type_text(label);
    let rows = session_rows(h);
    assert!(rows.iter().any(|r| r == label), "{label}: {rows:?}");
    while {
        let Some(datarig_tui::app::picker::Picker::Session { list, .. }) = h.app.overlays.picker() else { panic!() };
        list.selected().map(|r| list.label(r).to_string()).as_deref() != Some(label)
    } {
        h.key(KeyCode::Down);
    }
    h.key(KeyCode::Enter);
}

/// The profile form of `name` on field `field` (Advanced).
fn profile_field(h: &mut Harness, name: &str, field: Field) {
    h.explore(name);
    h.keys("e");
    h.ctrl('n');
    h.ctrl('n');
    while h.form().focus != field {
        h.key(KeyCode::Tab);
    }
}

/// The profile form's list: its environment's preset (named), none (even when the environment
/// maps one), the presets of its database, a new one; none picked is written as `"none"`.
#[test]
fn the_profile_form_picks_its_environments_preset_none_or_one_of_its_database() {
    let dir = scratch("profile-list");
    let mut h = section_harness(&dir);
    profile_field(&mut h, "local-pg", Field::SessionPreset);
    assert!(h.screen(120, 40).contains("Environment's (reporting) ▾"), "{}", h.screen(120, 40));
    h.key(KeyCode::Enter);
    assert_eq!(
        session_rows(&h),
        ["Environment's (reporting)", "None", "broken", "reporting", "New Startup SQL…"],
        "PostgreSQL's only"
    );
    for (w, ht) in [(80, 24), (120, 40)] {
        insta::assert_snapshot!(format!("profile_form_session_list_en_{w}x{ht}"), h.draw(w, ht).backend());
    }
    pick_session(&mut h, "None");
    assert_eq!(h.form().session_ref, SessionRef::Off);
    assert!(h.screen(120, 40).contains("Startup SQL       None ▾"), "{}", h.screen(120, 40));
    h.ctrl('s');
    assert!(!h.form_open());
    assert_eq!(h.app.profiles[0].session, SessionRef::Off);
    assert!(dir.text().contains("startup = \"none\""), "{}", dir.text());
    // The plan says the environment's was skipped.
    let plan = h.app.session_plan_of(&h.app.profiles[0].clone()).unwrap();
    assert!(
        plan.items
            .iter()
            .any(|i| i.what == datarig_core::profile::session::Planned::Skipped { preset: "reporting".into() })
    );
}

/// The profile's own startup SQL is the same list: a statement that is not allowed says why
/// under its row and keeps the form from saving (the focus goes to that row); saved, one
/// statement per item.
#[test]
fn the_profile_forms_session_sql_is_checked_and_saved_one_per_item() {
    let dir = scratch("profile-sql");
    let mut h = section_harness(&dir);
    profile_field(&mut h, "local-pg", Field::SessionSql);
    for (i, sql) in ["SET work_mem = '1MB'", "SET search_path = x", "SET jit = off"].into_iter().enumerate() {
        if i > 0 {
            h.key(KeyCode::Enter);
        }
        h.type_text(sql);
    }
    let screen = h.screen(160, 45);
    let lines: Vec<&str> = screen.lines().collect();
    let row = lines.iter().position(|l| l.contains("SET search_path = x")).expect("the row");
    assert!(lines[row + 1].contains("search_path: the tab"), "{screen}");
    let detail = h.app.overlays.form().and_then(|f| f.session_problem());
    assert!(matches!(detail, Some((2, datarig_core::sql::risk::SetProblem::AppOwned { .. }))), "{detail:?}");
    // With the room, a long reason takes a second line.
    for (w, ht) in [(80, 24), (80, 30), (120, 40)] {
        insta::assert_snapshot!(format!("profile_form_startup_list_en_{w}x{ht}"), h.draw(w, ht).backend());
    }
    h.ctrl('s');
    assert!(h.form_open(), "not saved");
    h.key(KeyCode::Up);
    assert_eq!((h.form().focus, h.form().session_sql.row), (Field::SessionSql, 1));
    h.ctrl('d');
    h.ctrl('s');
    assert!(!h.form_open(), "saved");
    assert_eq!(h.app.profiles[0].session_sql, ["SET work_mem = '1MB'", "SET jit = off"]);
    assert!(dir.text().contains("startup_sql = [\"SET work_mem = '1MB'\", \"SET jit = off\"]"), "{}", dir.text());
}

/// A preset of the other database (a driver changed in the form): marked, and not saved.
#[test]
fn a_preset_of_the_other_database_is_marked_in_the_profile_form() {
    let dir = scratch("profile-dbms");
    let mut h = section_harness(&dir);
    profile_field(&mut h, "local-pg", Field::SessionPreset);
    h.key(KeyCode::Enter);
    pick_session(&mut h, "reporting");
    h.ctrl('p');
    h.ctrl('p');
    h.key(KeyCode::BackTab);
    assert_eq!(h.form().focus, Field::Driver);
    h.key(KeyCode::Right); // MySQL
    h.ctrl('n');
    h.ctrl('n');
    let screen = h.screen(120, 40);
    assert!(screen.contains("reporting (for PostgreSQL) ▾"), "{screen}");
    assert!(screen.contains("a Startup SQL for another database"), "{screen}");
    h.ctrl('s');
    assert!(h.form_open(), "not saved");
    assert_eq!(h.app.profiles[0].driver, "postgres", "unchanged");
}

/// "New Startup SQL…" opens the preset form for the profile's database over the profile
/// form; saved, the profile form comes back with it picked.
#[test]
fn a_new_preset_from_the_profile_forms_list_comes_back_picked() {
    let dir = scratch("profile-new");
    let mut h = section_harness(&dir);
    profile_field(&mut h, "local-pg", Field::SessionPreset);
    h.key(KeyCode::Enter);
    h.key(KeyCode::End);
    pick_session(&mut h, "New Startup SQL…");
    assert!(h.form().is_session());
    assert_eq!(h.form().session_dbms, Dbms::Postgres);
    h.type_text("own");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.type_text("SET jit = off");
    h.ctrl('s');
    assert!(!h.form().is_session(), "back in the profile form");
    assert_eq!(h.form().session_ref, SessionRef::Named("own".into()));
    assert!(dir.text().contains("[startup.own]"), "{}", dir.text());
    h.ctrl('s');
    assert_eq!(h.app.profiles[0].session, SessionRef::Named("own".into()));
}

/// The environment form has a list per database: none and that database's presets; picked,
/// the environment maps it (in the file as `session = { … }`).
#[test]
fn the_environment_form_maps_a_preset_per_database() {
    let dir = scratch("env-list");
    let mut h = section_harness(&dir);
    let prod = h.app.environments().iter().find(|e| e.name == "prod").unwrap().id;
    on_row(&mut h, RowKind::Environment(prod));
    h.keys("e");
    while h.form().focus != Field::EnvSessionMy {
        h.key(KeyCode::Tab);
    }
    let screen = h.screen(120, 40);
    assert!(screen.contains("Startup SQL (PG)     reporting ▾"), "{screen}");
    assert!(screen.contains("Startup SQL (MySQL)  None ▾"), "{screen}");
    h.key(KeyCode::Enter);
    assert_eq!(session_rows(&h), ["None", "fk_off", "my_ro", "New Startup SQL…"], "MySQL's only");
    for (w, ht) in [(80, 24), (120, 40)] {
        insta::assert_snapshot!(format!("environment_form_session_list_en_{w}x{ht}"), h.draw(w, ht).backend());
    }
    pick_session(&mut h, "my_ro");
    h.key(KeyCode::BackTab);
    h.key(KeyCode::Enter);
    pick_session(&mut h, "None");
    h.ctrl('s');
    let map = &h.app.policies.environment("prod").unwrap().policy.session;
    assert_eq!((map.postgres.as_deref(), map.mysql.as_deref()), (None, Some("my_ro")));
    assert!(dir.text().contains("startup = { mysql = \"my_ro\" }"), "{}", dir.text());
}

/// Names are matched exactly; a preset of the same name but for case is named as the one
/// likely meant.
#[test]
fn an_unknown_preset_of_another_case_says_which_one_was_meant() {
    let cfg = config(|p| p.session = SessionRef::Named("Reporting".into()));
    let mut h = launched(&cfg);
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    assert!(h.driver.sessions.lock().unwrap().is_empty());
    let e = h.node_error("local-pg").unwrap_or_default();
    assert!(
        e.contains("“Reporting”, which does not exist")
            && e.ends_with("(did you mean “reporting”? names are matched exactly)"),
        "{e}"
    );
}

/// A MySQL profile's Advanced section (its longest) keeps a blank line above the buttons, at the
/// smallest screen too.
#[test]
fn a_mysql_profiles_advanced_section_keeps_a_line_above_the_buttons() {
    let mut cfg = config(|p| p.driver = "mysql".into());
    cfg.policies = Default::default();
    let mut h = launched(&cfg);
    profile_field(&mut h, "local-pg", Field::Folder);
    let screen = h.screen(80, 24);
    let lines: Vec<&str> = screen.lines().collect();
    let folder = lines.iter().position(|l| l.contains("Folder")).expect("Folder");
    let buttons = lines.iter().position(|l| l.contains("Test    Save    Cancel")).expect("buttons");
    assert_eq!(buttons, folder + 2, "{screen}");
    assert!(lines[folder + 1].trim_matches(|c| c == '│' || c == ' ').is_empty(), "{screen}");
}

// ── the session view ────────────────────────────────────────────────────

/// The `prod` environment of [`config`] with a lock wait of its own and the profile's own
/// statements (one with no form for one transaction).
fn view_config() -> Config {
    let mut cfg = config(|p| p.session_sql = vec!["SET work_mem = '256MB'".into(), "SET ROLE reporting".into()]);
    let mut rules = cfg.policies.get(Some("prod")).unwrap();
    rules.lock_wait = Some(Some(Duration::from_secs(5)));
    cfg.policies.insert("prod", rules);
    cfg
}

/// `:session` on the active tab's PostgreSQL profile: the app's limits with their sources,
/// the preset's statements and then the profile's own, each under where it comes from, with
/// how it goes behind a pooler; in order, and said once that a later setting wins.
#[test]
fn the_session_view_lists_what_a_profiles_sessions_start_with_and_from_where() {
    let mut h = connected(&view_config());
    h.command("session");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::SessionView));
    let screen = h.screen(120, 40);
    for line in [
        "local-pg · PostgreSQL · environment prod",
        "Lock wait        5s     environment prod",
        "Idle disconnect  never  app setting",
        "Idle rollback    30m    environment prod",
        "From the Startup SQL “reporting” of the environment prod:",
        "  SET statement_timeout = '30s'",
        "    behind a pooler: per transaction as SET LOCAL statement_timeout = '30s'",
        "From its own startup SQL:",
        "  SET work_mem = '256MB'",
        "    behind a pooler: per transaction as SET LOCAL work_mem = '256MB'",
        "  SET ROLE reporting",
        "    behind a pooler: not sent — it changes who the session is",
        "Applied in this order: a later setting of the same name wins",
    ] {
        assert!(screen.contains(line), "{line}\n{screen}");
    }
    for (w, ht) in [(80, 24), (120, 40)] {
        insta::assert_snapshot!(format!("session_view_en_{w}x{ht}"), h.draw(w, ht).backend());
    }
    remember_english("session_view_en_120x40", h.draw(120, 40).backend().buffer());
    h.key(KeyCode::Esc);
    assert_eq!(h.overlay_kind(), None);
    // A click on Close closes it too.
    h.command("session");
    click_on(&mut h, "[ Close ]");
    assert_eq!(h.overlay_kind(), None);
}

/// `:startup` is another name of `:session`: the same view of the same profile.
#[test]
fn startup_opens_the_session_view() {
    let mut h = connected(&view_config());
    h.command("session");
    let by_session = h.screen(120, 40);
    h.key(KeyCode::Esc);
    h.command("startup");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::SessionView));
    assert_eq!(h.screen(120, 40), by_session);
}

/// Click the first `text` on a 100x30 screen (after the dialog's arming delay).
fn click_on(h: &mut Harness, text: &str) {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let screen = h.screen(100, 30);
    let (x, y) = screen
        .lines()
        .enumerate()
        .find_map(|(y, row)| row.find(text).map(|i| (datarig_tui::text::width(&row[..i]) as u16 + 1, y as u16)))
        .unwrap_or_else(|| panic!("no {text:?}:\n{screen}"));
    h.advance(Duration::from_millis(500));
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

/// A plan longer than the screen: `j` and the wheel scroll it, never past its last line, and
/// the lines out of view above and below are counted; a statement on several lines is shown
/// whole, wrapped.
#[test]
fn a_long_session_view_scrolls_and_shows_statements_whole() {
    let cfg = config(|p| {
        p.session_sql = (1..=12).map(|i| format!("SET work_mem = '{i}MB'")).collect();
        p.session_sql.push("SET statement_timeout\n  TO '5min'".into());
    });
    let mut h = launched(&cfg);
    h.explore("local-pg");
    h.command("session");
    let first = h.screen(80, 24);
    assert!(first.contains("SET LOCAL work_mem = '1MB'"), "{first}");
    assert!(!first.contains("TO '5min'"), "below: {first}");
    let hidden = h.app.overlays.session_view().expect("open").max_scroll;
    assert!(hidden > 1, "{first}");
    assert!(first.contains(&format!("↓ {hidden} more lines")) && !first.contains('↑'), "{first}");
    insta::assert_snapshot!("session_view_long_en_80x24", h.draw(80, 24).backend());
    h.keys("j");
    let second = h.screen(80, 24);
    assert!(second.contains("↑ 1 more line ") || second.contains("↑ 1 more line│"), "{second}");
    assert!(second.contains(&format!("↓ {} more lines", hidden - 1)), "{second}");
    for _ in 0..60 {
        h.keys("j");
    }
    h.mouse(ratatui::crossterm::event::MouseEventKind::ScrollDown, 40, 12);
    let last = h.screen(80, 24);
    assert!(last.contains("  SET statement_timeout") && last.contains("    TO '5min'"), "{last}");
    assert!(
        last.contains(&format!("↑ {hidden} more lines")) && !last.contains("more lines ↓") && !last.contains('↓'),
        "{last}"
    );
    // At the end already: one `k` shows one line more above.
    h.keys("k");
    assert_ne!(h.screen(80, 24), last, "the scroll stopped at the last line");
    h.mouse(ratatui::crossterm::event::MouseEventKind::ScrollUp, 40, 12);
    h.mouse(ratatui::crossterm::event::MouseEventKind::ScrollDown, 40, 12);
    h.mouse(ratatui::crossterm::event::MouseEventKind::ScrollDown, 40, 12);
    assert_eq!(h.screen(80, 24), last);
}

/// A profile with no environment: the default rules are where its limits come from.
#[test]
fn a_profile_without_an_environment_shows_the_default_rules() {
    let cfg = Config { connections: vec![ConnectionConfig::test_db()], sessions: presets(), ..Config::default() };
    let mut h = launched(&cfg);
    h.explore("local-pg");
    h.command("session");
    let screen = h.screen(120, 40);
    assert!(screen.contains("local-pg · PostgreSQL · default rules"), "{screen}");
    assert!(screen.contains("Idle rollback    30m                   default rules"), "{screen}");
    assert!(screen.contains("No startup statements"), "{screen}");
}

/// An environment that is not defined: what the connect refuses with, from `:session` and from
/// the form (never the plan of other rules).
#[test]
fn an_undefined_environment_says_what_the_connect_refuses_with() {
    let cfg = config(|p| p.policy = Some("gone".into()));
    let mut h = launched(&cfg);
    h.explore("local-pg");
    h.command("session");
    let refused = "Environment “gone” not found for local-pg";
    let screen = h.screen(120, 40);
    assert!(screen.contains(refused), "{screen}");
    assert!(screen.contains("The profile does not connect until this is fixed"), "{screen}");
    assert!(!screen.contains("Lock wait") && !screen.contains("SET "), "{screen}");
    h.key(KeyCode::Esc);
    profile_field(&mut h, "local-pg", Field::SessionSql);
    h.type_text("SET jit = off");
    h.ctrl('e');
    let screen = h.screen(120, 40);
    assert!(screen.contains(refused) && !screen.contains("SET jit"), "{screen}");
}

#[test]
fn the_session_view_is_in_korean() {
    let mut h = connected(&view_config());
    h.command("session");
    remember_english("session_view_en_120x40", h.draw(120, 40).backend().buffer());
    let mut h = Harness::with_config(&view_config(), Lang::Ko);
    h.db(DbEvent::Connected);
    h.command("session");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::SessionView));
    let screen = h.screen(120, 40);
    assert!(screen.contains(ko(datarig_core::i18n::Label::SessionViewOrder)), "{screen}");
    check_localized("session_view_ko_120x40", Lang::Ko, h.draw(120, 40).backend().buffer());
}

/// MySQL's statements are sent as written: no pooler lines.
#[test]
fn a_mysql_profiles_session_view_has_no_pooler_lines() {
    let cfg = config(|p| p.driver = "mysql".into());
    let mut h = launched(&cfg);
    h.explore("local-pg");
    h.command("session");
    let screen = h.screen(120, 40);
    assert!(screen.contains("local-pg · MySQL · environment prod"), "{screen}");
    assert!(screen.contains("From the Startup SQL “my_ro” of the environment prod:"), "{screen}");
    assert!(screen.contains("  SET SESSION sql_mode = 'ANSI_QUOTES'"), "{screen}");
    assert!(!screen.contains("behind a pooler"), "{screen}");
}

/// `session = "none"`: the environment's preset is said to be skipped; with no statements left,
/// that is said too (the app's limits still apply).
#[test]
fn the_session_view_says_a_skipped_preset_and_no_statements() {
    let cfg = config(|p| p.session = SessionRef::Off);
    let mut h = launched(&cfg);
    h.explore("local-pg");
    h.command("session");
    let screen = h.screen(120, 40);
    assert!(
        screen.contains(
            "Not used: the Startup SQL “reporting” of the environment prod (the profile says startup = \"none\")"
        ),
        "{screen}"
    );
    assert!(screen.contains("No startup statements"), "{screen}");
    assert!(screen.contains("Lock wait"), "{screen}");
}

/// A plan with an error: the words the connect refusal says, and that it does not connect.
#[test]
fn the_session_view_of_a_profile_that_does_not_connect_says_why() {
    let cfg = config(|p| p.session = SessionRef::Named("gone".into()));
    let mut h = launched(&cfg);
    h.explore("local-pg");
    h.command("session");
    let screen = h.screen(120, 40);
    assert!(screen.contains("local-pg names the Startup SQL “gone”, which does not exist"), "{screen}");
    assert!(screen.contains("The profile does not connect until this is fixed"), "{screen}");
    assert!(!screen.contains("Lock wait"), "{screen}");
    insta::assert_snapshot!("session_view_error_en_80x24", h.draw(80, 24).backend());
}

/// From the profile form, `Ctrl+E` shows the form's values, unsaved ones too; `Esc` goes back
/// to the form as it was.
#[test]
fn the_profile_forms_key_shows_its_unsaved_values() {
    let dir = scratch("view-form");
    let mut h = section_harness(&dir);
    profile_field(&mut h, "local-pg", Field::SessionSql);
    h.type_text("SET jit = off");
    h.ctrl('e');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::SessionView));
    let screen = h.screen(120, 40);
    assert!(screen.contains("Session settings — local-pg (the form's unsaved values)"), "{screen}");
    assert!(screen.contains("From its own startup SQL:") && screen.contains("  SET jit = off"), "{screen}");
    insta::assert_snapshot!("session_view_form_en_80x24", h.draw(80, 24).backend());
    h.key(KeyCode::Esc);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ProfileForm));
    assert_eq!(h.form().focus, Field::SessionSql);
    assert_eq!(h.form().session_sql.statements(), ["SET jit = off"]);
    assert!(h.app.profiles[0].session_sql.is_empty(), "nothing saved");
}

/// Without a profile to show (none selected, no tab), the view opens and says so, with what
/// it would show; `:startup` too. A status line that fades was easy to miss.
#[test]
fn the_session_view_without_a_profile_says_so() {
    let mut h = Harness::with_config(&Config::default(), Lang::En);
    for command in ["session", "startup"] {
        h.command(command);
        assert_eq!(h.overlay_kind(), Some(OverlayKind::SessionView), ":{command}");
        let screen = h.screen(120, 40);
        assert!(screen.contains("Session settings"), "{screen}");
        assert!(
            screen.contains("No profile to show: select one in the explorer, or go to one of its tabs"),
            "{screen}"
        );
        h.key(KeyCode::Esc);
        assert_eq!(h.overlay_kind(), None);
    }
    h.command("session");
    for (w, ht) in [(80, 24), (120, 40)] {
        insta::assert_snapshot!(format!("session_view_no_profile_en_{w}x{ht}"), h.draw(w, ht).backend());
    }
}

/// A statement is shown exactly as written, wrapped where it is too wide: its blanks and every
/// character are there, the wrapped pieces put together give it back.
#[test]
fn a_wrapped_statement_keeps_every_character() {
    let sql = "SET myapp.note = 'a  b, then   a tail long enough to wrap past an eighty column screen'";
    let cfg = config(|p| p.session_sql = vec![sql.into()]);
    let mut h = launched(&cfg);
    h.explore("local-pg");
    h.command("session");
    for _ in 0..20 {
        h.keys("j");
    }
    let screen = h.screen(80, 24);
    let rows: Vec<Vec<char>> = screen.lines().map(|l| l.chars().collect()).collect();
    let (y, x) = rows
        .iter()
        .enumerate()
        .find_map(|(y, r)| r.windows(9).position(|w| w.iter().collect::<String>() == "SET myapp").map(|x| (y, x)))
        .unwrap_or_else(|| panic!("{screen}"));
    let end = x + rows[y][x..].iter().position(|c| *c == '│').expect("the right border");
    let mut shown: String = rows[y][x..end].iter().collect::<String>().trim_end().to_string();
    for r in &rows[y + 1..] {
        if shown.len() >= sql.len() {
            break;
        }
        shown.push_str(r[x + 2..end].iter().collect::<String>().trim_end());
    }
    assert_eq!(shown, sql, "{screen}");
}

/// The form names an environment it makes when it is saved (a preset picked): the view shows
/// that environment's rules, not an error.
#[test]
fn the_form_shows_the_rules_of_the_environment_it_makes() {
    let cfg = Config { connections: vec![ConnectionConfig::test_db()], sessions: presets(), ..Config::default() };
    let mut h = launched(&cfg);
    profile_field(&mut h, "local-pg", Field::SessionSql);
    let f = h.app.overlays.form_mut().expect("form");
    f.set_env_choice(datarig_tui::app::profiles::EnvChoice::Preset("prod".into()));
    assert!(h.form().made_environment().is_some());
    h.ctrl('e');
    let screen = h.screen(120, 40);
    assert!(screen.contains("local-pg · PostgreSQL · environment prod"), "{screen}");
    assert!(screen.contains("Read-only environment"), "{screen}");
    assert!(screen.contains("Idle rollback") && !screen.contains("not found"), "{screen}");
}

// ── a lock wait of a preset's or a profile's own ───────────────────────────

/// [`config`] with `prod`'s lock wait 5s, the profile on `driver` with `sql` of its own and the
/// preset `fast` (`fast` statements, for that database) picked by the profile.
fn lock_config(driver: &str, fast: &[&str], sql: &[&str]) -> Config {
    let mut cfg = config(|p| {
        p.driver = driver.into();
        p.session = SessionRef::Named("fast".into());
        p.session_sql = sql.iter().map(|s| s.to_string()).collect();
    });
    let dbms = Dbms::of_driver(driver).unwrap();
    cfg.sessions.push(SessionPreset::new("fast", dbms, fast.iter().map(|s| s.to_string()).collect()));
    let mut rules = cfg.policies.get(Some("prod")).unwrap();
    rules.lock_wait = Some(Some(Duration::from_secs(5)));
    cfg.policies.insert("prod", rules);
    cfg
}

/// Connected to `cfg`'s profile, on MySQL's driver when it is a MySQL profile.
fn connected_on(cfg: &Config) -> Harness {
    let driver = FakeDriver::default();
    driver.mysql.store(cfg.connections[0].driver == "mysql", std::sync::atomic::Ordering::SeqCst);
    let mut h = Harness::with_driver(cfg, Lang::En, driver);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["public".into()])));
    h.db(DbEvent::Catalog(Ok(catalog())));
    h.sent();
    h
}

/// What a statement cut short by the lock wait limit says in the tab.
fn lock_timeout_text(cfg: &Config) -> String {
    let mut h = connected_on(cfg);
    let id = run(&mut h, "UPDATE shop.users SET name = 'x' WHERE id = 1");
    assert_eq!(h.roles(), [SessionRole::Meta, SessionRole::Query], "the query session opened");
    let server = "ERROR: canceling statement due to lock timeout";
    h.tab_db(0, DbEvent::Failed { id, error: DbError::LockTimeout(server.into()), cancelled: false });
    h.screen(200, 45)
}

/// A preset or a profile's own statements may set the lock wait for its sessions: it overrides
/// the environment's, and a wait cut short names the value the server used (the last one set),
/// never the environment's. A value the app cannot read (`DEFAULT`, the server's own), or on
/// MySQL the two limits set apart, names none.
#[test]
fn a_lock_wait_set_by_a_preset_is_the_one_a_cut_wait_names() {
    let named =
        |t: &str, wait: &str| t.contains(&format!("Waited longer than {wait} for a lock another session holds"));
    let generic = |t: &str| t.contains("waited too long for a lock another session holds");
    let pg = lock_config("postgres", &["SET lock_timeout = '3s'"], &[]);
    let t = lock_timeout_text(&pg);
    assert!(named(&t, "3s") && !named(&t, "5s"), "{t}");
    // The profile's own statement comes after the preset's: it wins.
    let t = lock_timeout_text(&lock_config("postgres", &["SET lock_timeout = '3s'"], &["SET lock_timeout = 1500"]));
    assert!(named(&t, "1500ms"), "{t}");
    let t = lock_timeout_text(&lock_config("postgres", &["SET lock_timeout TO '2 min'"], &[]));
    assert!(named(&t, "2m"), "{t}");
    // `DEFAULT` is the server's own value, 0 no limit: neither names a time.
    for unknown in ["SET lock_timeout TO DEFAULT", "SET lock_timeout = 0"] {
        let t = lock_timeout_text(&lock_config("postgres", &[unknown], &[]));
        assert!(generic(&t) && !named(&t, "5s"), "{unknown}: {t}");
    }
    // MySQL: a number only when both limits are known and the same.
    let both = ["SET SESSION lock_wait_timeout = 3, innodb_lock_wait_timeout = 3"];
    let t = lock_timeout_text(&lock_config("mysql", &both, &[]));
    assert!(named(&t, "3s"), "{t}");
    let apart = lock_config("mysql", &["SET SESSION innodb_lock_wait_timeout = 3"], &[]);
    let t = lock_timeout_text(&apart);
    assert!(generic(&t) && !named(&t, "5s"), "{t}");
    let joined =
        lock_config("mysql", &["SET SESSION innodb_lock_wait_timeout = 3"], &["SET @@session.lock_wait_timeout = 3"]);
    assert!(named(&lock_timeout_text(&joined), "3s"));
}

/// `:session` shows the lock wait the sessions get: the preset's, where it comes from, and the
/// environment's value it overrides; one the app cannot read says so.
#[test]
fn the_session_view_shows_a_lock_wait_a_preset_overrides() {
    let mut h = connected_on(&lock_config("postgres", &["SET lock_timeout = '3s'"], &[]));
    h.command("session");
    let screen = h.screen(120, 40);
    for line in ["Lock wait        3s", "its Startup SQL “fast” — overrides environment prod: 5s"] {
        assert!(screen.contains(line), "{line}\n{screen}");
    }
    for (w, ht) in [(80, 24), (120, 40)] {
        insta::assert_snapshot!(format!("session_view_lock_override_en_{w}x{ht}"), h.draw(w, ht).backend());
    }
    // Within 80 columns the source wraps under itself.
    let screen = h.screen(80, 24);
    assert!(
        screen.contains("“fast” — overrides environment") && screen.contains("                         prod: 5s"),
        "{screen}"
    );
    let mut h = connected_on(&lock_config("postgres", &["SET lock_timeout TO DEFAULT"], &[]));
    h.command("session");
    let screen = h.screen(120, 40);
    assert!(screen.contains("Lock wait        not known"), "{screen}");
    assert!(screen.contains("overrides environment prod: 5s"), "{screen}");
}

/// A setting the app keeps refused says why, in plain words, and where to set it instead.
#[test]
fn a_refused_setting_says_why_and_where_to_set_it() {
    let cases = [
        ("postgres", "SET idle_session_timeout = 0", "set the environment's idle_disconnect"),
        ("mysql", "SET SESSION wait_timeout = 10", "set the environment's idle_disconnect"),
        ("mysql", "SET SESSION interactive_timeout = 10", "no effect on an open session"),
        ("postgres", "SET idle_in_transaction_session_timeout = 0", "set the environment's tx_idle_rollback"),
        ("mysql", "SET autocommit = 0", "set the environment's commit"),
        ("mysql", "SET SESSION completion_type = 1", "set the environment's commit"),
        ("postgres", "SET default_transaction_read_only = off", "set the environment's read_only"),
        ("mysql", "SET SESSION TRANSACTION READ WRITE", "set the environment's read_only"),
        ("postgres", "SET search_path = x", ":use"),
        ("postgres", "SET client_encoding = 'LATIN1'", "UTF-8"),
        ("mysql", "SET NAMES latin1", "UTF-8"),
        ("postgres", "SET DateStyle = 'German'", "default text format"),
        ("postgres", "SET standard_conforming_strings = off", "string literals"),
        ("postgres", "SET application_name = 'x'", "names its sessions"),
        ("mysql", "SET sql_select_limit = 10", "pages results"),
        ("mysql", "SET resultset_metadata = NONE", "reads what the server reports"),
    ];
    for (driver, sql, why) in cases {
        let cfg = config(|p| {
            p.driver = driver.into();
            p.session = SessionRef::Off;
            p.session_sql = vec![sql.to_string()];
        });
        let mut h = launched(&cfg);
        h.explore("local-pg");
        h.key(KeyCode::Enter);
        assert!(h.driver.sessions.lock().unwrap().is_empty(), "{sql}: nothing is opened");
        let e = h.node_error("local-pg").unwrap_or_default();
        assert!(e.contains(why) && !e.contains("is set by"), "{sql}: {e}");
    }
}

/// A lock wait the server's own limit cut short (a statement that ran without the session's,
/// behind a pooler) names no time, even with a limit of the environment's or a preset's.
#[test]
fn a_lock_wait_cut_by_the_servers_own_limit_names_no_time() {
    let mut h = connected_on(&lock_config("postgres", &["SET lock_timeout = '3s'"], &[]));
    let id = run(&mut h, "DO $$ BEGIN PERFORM 1; END $$");
    let server = "ERROR: canceling statement due to lock timeout";
    h.tab_db(0, DbEvent::Failed { id, error: DbError::ServerLockTimeout(server.into()), cancelled: false });
    let t = h.screen(200, 45);
    assert!(t.contains("waited too long for a lock another session holds") && !t.contains("Waited longer than"), "{t}");
}

/// In a list that scrolls, the terminal cursor is on the row being edited (its line, after its
/// text), whichever row that is.
#[test]
fn the_cursor_is_on_the_edited_row_of_a_scrolled_list() {
    let dir = scratch("list-cursor");
    let mut h = section_harness(&dir);
    on_preset(&mut h, "my_ro");
    h.keys("e");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    let check = |h: &mut Harness, what: &str| {
        let mut t = h.draw(80, 24);
        let c = t.get_cursor_position().unwrap();
        let row = h.form().session_sql.row;
        let text = format!("{:>3} {}", row + 1, h.form().session_sql.text(row));
        let screen = h.screen(80, 24);
        let line = screen.lines().nth(usize::from(c.y)).unwrap_or_default().to_string();
        assert!(line.contains(&text), "{what}: cursor at {c:?} on {line:?}, editing {text:?}\n{screen}");
    };
    // Refused rows say why under them, on one line or two.
    for i in 1..=8 {
        h.key(KeyCode::Enter);
        let sql = if i % 2 == 1 { "SET autocommit = 0".to_string() } else { format!("SET @x{i} = 1") };
        h.type_text(&sql);
        check(&mut h, &format!("row {i}"));
    }
    for i in 0..8 {
        h.key(KeyCode::Up);
        check(&mut h, &format!("up {i}"));
    }
    for i in 0..6 {
        h.key(KeyCode::Down);
        check(&mut h, &format!("down {i}"));
    }
    h.key(KeyCode::Enter);
    check(&mut h, "a new row in the middle");
    h.type_text("SET x = 1");
    check(&mut h, "typed");
}

/// Two statements in one item without a `;` between them are refused, as one line or on two,
/// in a read-only environment too: the profile does not connect.
#[test]
fn two_statements_in_one_item_do_not_connect() {
    for read_only in [false, true] {
        for sql in ["SET @c=3 SET @d=4", "SET @c=3\nSET @d=4"] {
            let mut cfg = config(|p| {
                p.driver = "mysql".into();
                p.port = 3306;
                p.session = SessionRef::Off;
                p.session_sql = vec![sql.into()];
            });
            let mut rules = cfg.policies.environment("prod").unwrap().policy.clone();
            rules.read_only = read_only;
            cfg.policies.insert("prod", rules);
            let mut h = launched(&cfg);
            h.explore("local-pg");
            h.key(KeyCode::Enter);
            assert!(h.driver.sessions.lock().unwrap().is_empty(), "{sql}");
            let e = h.node_error("local-pg").unwrap_or_default();
            assert!(e.contains("statement 1 of its own startup SQL is not allowed (one statement per item)"), "{e}");
        }
    }
}

/// Ctrl+S that a refused statement keeps from saving says so, in the preset form and in the
/// profile form; the row being edited says all of its reason, even where the others are cut.
#[test]
fn a_save_a_refused_statement_stops_says_why() {
    let dir = scratch("refused-save");
    let mut h = section_harness(&dir);
    on_preset(&mut h, "my_ro");
    h.keys("e");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.key(KeyCode::End);
    h.key(KeyCode::Enter);
    h.type_text("SET autocommit = 0");
    let screen = h.screen(80, 24);
    assert!(screen.contains("(manual or auto)"), "the whole reason:\n{screen}");
    h.ctrl('s');
    assert!(h.form_open(), "not saved");
    assert_eq!(said(&h), "Not saved: a statement is not allowed (said under it)");
    h.key(KeyCode::Esc);
    profile_field(&mut h, "local-pg", Field::SessionSql);
    h.type_text("SET search_path = x");
    h.ctrl('s');
    assert!(h.form_open(), "not saved");
    assert_eq!(said(&h), "Not saved: a statement is not allowed (said under it)");
}

/// A Startup SQL row cut at a separator ends with `…` like any other cut row, not with a bare
/// ` · `.
#[test]
fn a_cut_preset_row_ends_with_an_ellipsis() {
    let dir = scratch("cut-row");
    section_config(&dir);
    let text = dir.text() + "\n[startup.pg_fast]\ndbms = \"postgres\"\nsql = [\"SET work_mem = '1MB'\"]\n";
    std::fs::write(dir.config(), &text).unwrap();
    let (cfg, err) = datarig_core::config::load(Some(dir.config()));
    assert!(err.is_none(), "{err:?}");
    let mut h = launched(&cfg);
    for (w, ht) in [(80, 24), (100, 30), (120, 40), (160, 45)] {
        let screen = h.screen(w, ht);
        let line = screen.lines().find(|l| l.contains("pg_fast")).unwrap_or_else(|| panic!("{screen}"));
        let row = line.split('│').nth(1).unwrap_or_default().trim_end();
        assert!(!row.ends_with('·'), "{w}x{ht}: {row:?}");
        assert!(row.ends_with('…') || row.ends_with("statement"), "{w}x{ht}: {row:?}");
    }
}

/// Saves from the Startup SQL form and the profile form change only what they edit: in a file
/// with `[editor]`, `[keymap.*]`, two profiles and `[sessions.*]` (the 0.31 name), the preset
/// tables move to `[startup.*]` and an edited profile's keys are renamed or added, and everything
/// else (settings, key map, the other profile, comments) stays byte for byte.
#[test]
fn form_saves_leave_the_rest_of_the_file_as_written() {
    let dir = scratch("rest-of-file");
    let head = "version = 2\nlanguage = \"en\"\n\n# my editor\n[editor]\nclipboard = \"off\" # never the system clipboard\n\n[keymap.explorer]\n\"x\" = \"conn.disconnect\"\n\n";
    let profile = |id: u8, name: &str, keys: &str| {
        format!(
            "[[connections]]\nid = \"00000000-0000-4000-8000-00000000000{id}\"\nname = \"{name}\"\ndriver = \"postgres\"\n{keys}host = \"127.0.0.1\"\nport = 55432\nuser = \"datarig\"\ndatabase = \"datarig\"\nsslmode = \"disable\"\n"
        )
    };
    let a = profile(1, "a", "# its preset\nsession = \"x\"\n");
    let b = profile(2, "b", "");
    let preset = |table: &str, sql: &str| format!("# the preset\n[{table}.x]\ndbms = \"postgres\"\nsql = [{sql}]\n");
    let text = format!("{head}{a}\n{b}\n{}", preset("sessions", "\"SET work_mem = '1MB'\""));
    std::fs::write(dir.config(), &text).unwrap();
    let (cfg, err) = datarig_core::config::load(Some(dir.config()));
    assert!(err.is_none(), "{err:?}");
    let mut h = launched(&cfg);
    assert_eq!(dir.text(), text, "launching writes nothing");
    // The preset form: a statement more, so `[sessions.x]` moves to `[startup.x]`.
    on_preset(&mut h, "x");
    h.keys("e");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.key(KeyCode::Enter);
    h.type_text("SET jit = off");
    h.ctrl('s');
    assert!(!h.form_open(), "saved");
    let presets = preset("startup", "\"SET work_mem = '1MB'\", \"SET jit = off\"");
    let want = format!("{head}{a}\n{b}\n{presets}");
    assert_eq!(dir.text(), want);
    // Profile `b`'s own statements: its new key, at the end of its table.
    profile_field(&mut h, "b", Field::SessionSql);
    h.type_text("SET jit = off");
    h.ctrl('s');
    assert!(!h.form_open(), "saved");
    let b2 = format!("{b}startup_sql = [\"SET jit = off\"]\n");
    let want = format!("{head}{a}\n{b2}\n{presets}");
    assert_eq!(dir.text(), want);
    // Profile `a`: its `session` is renamed in place (its comment kept), the new key added.
    profile_field(&mut h, "a", Field::SessionSql);
    h.type_text("SET work_mem = '2MB'");
    h.ctrl('s');
    assert!(!h.form_open(), "saved");
    let a2 = profile(1, "a", "# its preset\nstartup = \"x\"\n") + "startup_sql = [\"SET work_mem = '2MB'\"]\n";
    assert_eq!(dir.text(), format!("{head}{a2}\n{b2}\n{presets}"));
}
