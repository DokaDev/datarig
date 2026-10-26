//! Session presets, headless: what a profile's query sessions are opened with (its
//! environment's preset, its own, none, its own statements; metadata sessions never), a profile
//! whose plan has an error (an unknown preset, one for the other database, a statement that is
//! not a setting or that read-only refuses: nothing is opened), a start-up statement the server
//! refused (said with its source and number), and a session behind a pooler that sets them per
//! transaction. The explorer's "Session presets" section: its rows, the preset form (new,
//! edit, rename, copy, a statement that is not allowed), delete and test. The profile form's
//! session preset list and session SQL, and the environment form's lists.

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
use ratatui::crossterm::event::KeyCode;
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
    let at = rows.iter().position(|r| r == "[session presets]").expect("the section");
    assert_eq!(
        rows[at..at + 7],
        ["[session presets]", "  $broken", "    !", "  $fk_off", "  $my_ro", "  $reporting", "    @local-pg"],
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

/// `n` in the section opens the preset form; its statements are one field (`;` between them),
/// saved one per item; the database is picked with ←/→.
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
    h.type_text("SET statement_timeout = '1min'; SET jit = off");
    for (w, ht) in [(80, 24), (120, 40)] {
        insta::assert_snapshot!(format!("session_form_en_{w}x{ht}"), h.draw(w, ht).backend());
    }
    h.ctrl('s');
    assert!(!h.form_open(), "saved and closed");
    assert_eq!(said(&h), "Saved the session preset “analytics”");
    let p = h.app.session_presets().iter().find(|p| p.name == "analytics").expect("made").clone();
    assert_eq!(
        (p.dbms, p.sql.as_slice()),
        (Dbms::Postgres, ["SET statement_timeout = '1min'".to_string(), "SET jit = off".to_string()].as_slice())
    );
    assert!(
        dir.text().contains(
            "[sessions.analytics]\ndbms = \"postgres\"\nsql = [\"SET statement_timeout = '1min'\", \"SET jit = off\"]\n"
        ),
        "{}",
        dir.text()
    );
    assert_eq!(h.app.explorer.cursor(), &RowKind::Session(p.id), "revealed");
}

/// A statement that is not allowed is said under the field (which and why) and keeps the form
/// from saving (the focus goes there); `none` is not a name.
#[test]
fn the_form_refuses_statements_that_are_not_settings_and_the_reserved_name() {
    let dir = scratch("refuse");
    let mut h = section_harness(&dir);
    on_row(&mut h, RowKind::SessionsHeader);
    h.keys("n");
    h.type_text("none");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.type_text("SET work_mem = '1MB'; RESET work_mem");
    let screen = h.screen(120, 40);
    assert!(screen.contains("reserved: it means no preset"), "{screen}");
    assert!(screen.contains("statement 2: only SET of the session's own settings"), "{screen}");
    insta::assert_snapshot!("session_form_errors_en_80x24", h.draw(80, 24).backend());
    h.ctrl('s');
    assert!(h.form_open(), "not saved");
    assert_eq!(h.form().focus, Field::Name, "the first field with an error");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    for _ in 0.." RESET work_mem".len() {
        h.key(KeyCode::Backspace);
    }
    h.type_text(" SET foreign_key_checks = 0");
    // A risky MySQL setting: allowed, but read-only environments refuse it.
    h.key(KeyCode::BackTab);
    h.key(KeyCode::Right);
    // On MySQL neither is on the read-only allowlist: the first is said.
    let screen = h.screen(120, 40);
    assert!(screen.contains("Statement 1 would be refused in read-only environments"), "{screen}");
    assert!(!dir.text().contains("[sessions.none]"));
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
    assert!(text.contains("[sessions.mysql_ro]") && !text.contains("[sessions.my_ro]"), "{text}");
    assert!(text.contains("session = \"mysql_ro\""), "{text}");
    assert!(text.contains("session = { postgres = \"reports\" }"), "{text}");
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
    assert!(screen.contains("Delete the session preset “reporting”?"), "{screen}");
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
        "Deleted the session preset “reporting”; the profile that used it needs another preset or none"
    );
    assert!(!dir.text().contains("[sessions.reporting]"), "{}", dir.text());
    assert!(dir.text().contains("session = { postgres = \"reporting\" }"), "the environment keeps the name");
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
    for want in ["Edit session preset", "Duplicate session preset", "Test session preset", "Delete session preset"] {
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
    assert_eq!(said(&h), "No profile uses MySQL: nothing to test the preset with");
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
    assert_eq!(said(&h), "Session preset “reporting” applied on local-pg (2 statements); the test session was closed");
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
        said(&h)
            .starts_with("The preset could not be applied: statement 1 the session starts with failed: ERROR: nope"),
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
        ["Environment's (reporting)", "None", "broken", "reporting", "New session preset…"],
        "PostgreSQL's only"
    );
    for (w, ht) in [(80, 24), (120, 40)] {
        insta::assert_snapshot!(format!("profile_form_session_list_en_{w}x{ht}"), h.draw(w, ht).backend());
    }
    pick_session(&mut h, "None");
    assert_eq!(h.form().session_ref, SessionRef::Off);
    assert!(h.screen(120, 40).contains("Session preset    None ▾"), "{}", h.screen(120, 40));
    h.ctrl('s');
    assert!(!h.form_open());
    assert_eq!(h.app.profiles[0].session, SessionRef::Off);
    assert!(dir.text().contains("session = \"none\""), "{}", dir.text());
    // The plan says the environment's was skipped.
    let plan = h.app.session_plan_of(&h.app.profiles[0].clone()).unwrap();
    assert!(
        plan.items
            .iter()
            .any(|i| i.what == datarig_core::profile::session::Planned::Skipped { preset: "reporting".into() })
    );
}

/// The profile's own session SQL: a statement that is not allowed is said after the field and
/// keeps the form from saving; saved, one statement per item.
#[test]
fn the_profile_forms_session_sql_is_checked_and_saved_one_per_item() {
    let dir = scratch("profile-sql");
    let mut h = section_harness(&dir);
    profile_field(&mut h, "local-pg", Field::SessionSql);
    h.type_text("SET work_mem = '1MB'; SET lock_timeout = '1s'");
    let screen = h.screen(160, 45);
    assert!(screen.contains("statement 2: lock_timeout is set by"), "{screen}");
    let detail = h.app.overlays.form().and_then(|f| f.session_problem());
    assert!(matches!(detail, Some((2, datarig_core::sql::risk::SetProblem::AppOwned { .. }))), "{detail:?}");
    h.ctrl('s');
    assert!(h.form_open(), "not saved");
    assert_eq!(h.form().focus, Field::SessionSql);
    for _ in 0.."; SET lock_timeout = '1s'".len() {
        h.key(KeyCode::Backspace);
    }
    h.type_text("; SET jit = off");
    h.ctrl('s');
    assert!(!h.form_open(), "saved");
    assert_eq!(h.app.profiles[0].session_sql, ["SET work_mem = '1MB'", "SET jit = off"]);
    assert!(dir.text().contains("session_sql = [\"SET work_mem = '1MB'\", \"SET jit = off\"]"), "{}", dir.text());
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
    assert!(screen.contains("a preset for another database"), "{screen}");
    h.ctrl('s');
    assert!(h.form_open(), "not saved");
    assert_eq!(h.app.profiles[0].driver, "postgres", "unchanged");
}

/// "New session preset…" opens the preset form for the profile's database over the profile
/// form; saved, the profile form comes back with it picked.
#[test]
fn a_new_preset_from_the_profile_forms_list_comes_back_picked() {
    let dir = scratch("profile-new");
    let mut h = section_harness(&dir);
    profile_field(&mut h, "local-pg", Field::SessionPreset);
    h.key(KeyCode::Enter);
    h.key(KeyCode::End);
    pick_session(&mut h, "New session preset…");
    assert!(h.form().is_session());
    assert_eq!(h.form().session_dbms, Dbms::Postgres);
    h.type_text("own");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.type_text("SET jit = off");
    h.ctrl('s');
    assert!(!h.form().is_session(), "back in the profile form");
    assert_eq!(h.form().session_ref, SessionRef::Named("own".into()));
    assert!(dir.text().contains("[sessions.own]"), "{}", dir.text());
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
    assert!(screen.contains("Preset (PostgreSQL)  reporting ▾"), "{screen}");
    assert!(screen.contains("Preset (MySQL)       None ▾"), "{screen}");
    h.key(KeyCode::Enter);
    assert_eq!(session_rows(&h), ["None", "fk_off", "my_ro", "New session preset…"], "MySQL's only");
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
    assert!(dir.text().contains("session = { mysql = \"my_ro\" }"), "{}", dir.text());
}
