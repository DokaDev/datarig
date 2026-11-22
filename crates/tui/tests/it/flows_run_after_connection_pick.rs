//! A run in a tab without a connection is split before the database is known. When the profile
//! picked for it in quick connect is of another kind of database, it is not sent: the tab says
//! to run it again, and statements are split again when the database kind changes. A
//! PostgreSQL profile runs it as before. From a console without a connection and from a saved
//! query that has none alike; and a profile whose driver changes is connected again.

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_core::paths::Paths;
use datarig_core::profile::ConnectionConfig;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_core::sql::dialect::{Dialect, MySqlMode};
use datarig_core::sql::risk::{Danger, Why};
use datarig_core::sql::split::split_in;
use datarig_i18n::Lang;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::{Focus, Startup, TabKind};
use ratatui::crossterm::event::KeyCode;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Texts with comments that hold executable comments.
const TEXTS: [&str; 5] = [
    "SELECT 1 /* a /* b */ /*M! , 2 */",
    "SELECT 1 /* a /* b */ /*! , 2 */",
    "SELECT 1 /* a /* b */ /*M!100500 , 2 */",
    "SELECT 5--1 /*M! , 2 */",
    "SELECT 1 /* a /* b */ /*M! , 2 */; SELECT 3;",
];

struct Dirs(PathBuf);

impl Dirs {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("datarig-pick-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("data").join("scripts")).unwrap();
        Dirs(root)
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Launched with one profile `target` of `driver` (no tab has a connection yet), on `dirs`.
fn launch(driver: &str, dirs: &Dirs) -> Harness {
    let p = ConnectionConfig { name: "target".into(), driver: driver.into(), ..ConnectionConfig::test_db() };
    let cfg = Config { connections: vec![p], ..Config::default() };
    let (mut app, clock) = new_app_with_clock(&cfg, Lang::En);
    let store = Arc::new(MemoryStore::new());
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.set_paths(Paths { data: Some(dirs.0.join("data")), state: Some(dirs.0.join("state")) });
    let fake = FakeDriver::default();
    fake.mysql.store(driver != "postgres", Ordering::SeqCst);
    let f = fake.clone();
    app.set_drivers(Arc::new(move |name: &str| {
        matches!(name, "postgres" | "mysql" | "mariadb")
            .then(|| Arc::new(f.clone()) as Arc<dyn datarig_core::driver::Driver>)
    }));
    app.launch(Startup::Normal);
    Harness { app, cancelled: fake.any_cancel.clone(), driver: fake, store, clock }
}

/// Launched on `dirs` whose workspace has one console without a connection holding `sql`
/// (restored as written; the cursor at its start).
fn restored(driver: &str, dirs: &Dirs, sql: &str) -> Harness {
    let id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1";
    let consoles = dirs.0.join("state").join("consoles");
    std::fs::create_dir_all(&consoles).unwrap();
    std::fs::write(consoles.join(format!("{id}.sql")), sql).unwrap();
    let ws =
        format!("version = 1\nactive = 0\n[[tabs]]\nid = \"{id}\"\nkind = \"console\"\ncursor = [0, 0]\ntop = 0\n");
    std::fs::write(dirs.0.join("state").join("workspace.toml"), ws).unwrap();
    let h = launch(driver, dirs);
    assert_eq!(
        (h.app.tab().profile, h.app.tab().editor.text()),
        (None, sql.to_string()),
        "a console without a connection"
    );
    h
}

/// Ctrl+E on the active tab: all of its text selected, or the statement under the cursor.
fn run(h: &mut Harness, cursor: bool) {
    h.app.focus = Focus::Editor;
    if !cursor {
        h.keys("ggvG$");
    }
    h.sent();
    h.ctrl('e');
}

/// Ctrl+E in a tab without a connection, `target` picked in quick connect, and it connects.
fn run_and_pick(h: &mut Harness, cursor: bool) {
    run(h, cursor);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::QuickConnect), "the run asks for a connection");
    h.type_text("target");
    h.key(KeyCode::Enter);
    h.meta_db("target", DbEvent::Connected);
}

fn executes(h: &mut Harness) -> Vec<Vec<String>> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { statements, .. } => Some(statements),
            _ => None,
        })
        .collect()
}

fn asked(h: &Harness) -> Vec<(Option<Why>, String)> {
    h.app.overlays.run_confirm().map(|c| c.items.iter().map(|d| (d.why, d.sql.clone())).collect()).unwrap_or_default()
}

/// The statements of `sql` as MySQL splits it: all of them, or the first (the cursor at the
/// start of the text).
fn mysql_reading(sql: &str, cursor: bool) -> Vec<String> {
    let all: Vec<String> =
        split_in(sql, Dialect::MySql(MySqlMode::default())).iter().map(|s| s.body(sql).to_string()).collect();
    if cursor { all.into_iter().take(1).collect() } else { all }
}

/// After the pick nothing is sent, nothing asks, and the tab says to run it again; the next run
/// sends the statements as MySQL splits them, once confirmed.
fn assert_not_sent_then_run_again(h: &mut Harness, sql: &str, cursor: bool, case: &str) {
    assert!(asked(h).is_empty(), "{case}: nothing asks: {:?}", asked(h));
    assert!(h.overlay_kind().is_none(), "{case}: {:?}", h.overlay_kind());
    assert!(executes(h).is_empty(), "{case}: nothing is sent");
    assert!(!h.app.tab().editor.has_staged_run(), "{case}: nothing stays staged");
    let note = h.app.tab().status.as_ref().map(|n| format!("{:?}", n.msg));
    assert!(note.as_deref().is_some_and(|n| n.contains("QueryReadForOtherDatabase")), "{case}: {note:?}");
    assert!(h.app.tab().profile.is_some(), "{case}: the tab is on the picked profile");
    // The next run splits the text as MySQL does, and asks for the executable comment.
    let want = mysql_reading(sql, cursor);
    run(h, cursor);
    let shown = asked(h);
    assert!(shown.iter().any(|(why, _)| *why == Some(Why::Danger(Danger::ExecutableComment))), "{case}: {shown:?}");
    assert!(executes(h).is_empty(), "{case}: nothing before the yes");
    h.key(KeyCode::Char('y'));
    assert_eq!(executes(h), [want], "{case}");
}

#[test]
fn a_run_read_before_a_mysql_family_profile_is_picked_is_not_sent() {
    for driver in ["mariadb", "mysql"] {
        for sql in TEXTS {
            for cursor in [false, true] {
                // The cursor runs one statement; the last text has two.
                if cursor && sql.matches(';').count() > 1 {
                    continue;
                }
                let case = format!("{driver}, cursor {cursor}: {sql}");
                let dirs = Dirs::new(&format!("ed-{driver}-{cursor}"));
                let mut h = restored(driver, &dirs, sql);
                run_and_pick(&mut h, cursor);
                assert_not_sent_then_run_again(&mut h, sql, cursor, &case);
            }
        }
    }
}

/// A saved query that has no connection opens in a tab without one: its run is the editor's.
#[test]
fn a_saved_query_without_a_connection_is_not_sent_on_a_mysql_family_profile() {
    for driver in ["mariadb", "mysql"] {
        for sql in [TEXTS[0], TEXTS[4]] {
            let case = format!("{driver}: {sql}");
            let dirs = Dirs::new(&format!("saved-{driver}"));
            std::fs::write(dirs.0.join("data").join("scripts").join("kept.sql"), sql).unwrap();
            let mut h = launch(driver, &dirs);
            h.command("e kept");
            assert_eq!(h.app.tab().kind, TabKind::Script, "{case}");
            assert!(h.app.tab().profile.is_none(), "{case}: the saved query has no connection");
            run_and_pick(&mut h, false);
            assert_not_sent_then_run_again(&mut h, sql, false, &case);
        }
    }
}

/// A PostgreSQL profile picked: the statements read before run as they are, the comments that
/// hold executable ones with them, and a plain comment is still left out.
#[test]
fn a_run_read_before_a_postgresql_profile_is_picked_runs() {
    let dirs = Dirs::new("pg");
    let mut h = restored("postgres", &dirs, "SELECT 1 /* plain */");
    run_and_pick(&mut h, true);
    assert!(h.overlay_kind().is_none());
    assert_eq!(executes(&mut h), [["SELECT 1"]]);

    let dirs = Dirs::new("pg2");
    let sql = "SELECT 1 /* outer /* inner */ /*M! , 2 */ */";
    let mut h = restored("postgres", &dirs, sql);
    run_and_pick(&mut h, true);
    assert!(h.overlay_kind().is_none());
    assert_eq!(executes(&mut h), [[sql]]);
}

/// A run that waits for its profile to connect, whose driver is changed in the profile's form
/// meanwhile: the profile is disconnected, the run is not sent, and the next run goes to the
/// connection made with the new driver.
#[test]
fn a_waiting_run_is_not_sent_when_its_profiles_driver_changes() {
    let mut h = Harness::connected(Lang::En);
    let id = h.app.tab().id;
    let name = h.app.conn().map(|c| c.name.clone()).expect("a profile");
    let sql = "SELECT 1 /* a /* b */ /*M! , 2 */ */";
    h.app.tab_mut().editor = datarig_tui::widgets::editor::Editor::new(sql);
    h.keys(" cx");
    h.ctrl('e');
    assert!(h.app.is_queued(id), "waits for the connection");
    h.explore(&name);
    h.keys("e");
    assert!(h.form_open());
    h.app.overlays.form_mut().expect("the form").driver = 1;
    h.driver.mysql.store(true, Ordering::SeqCst);
    h.ctrl('s');
    assert!(!h.form_open(), "saved");
    assert!(matches!(h.app.tab_language(id).dialect(), Dialect::MySql(_)), "the tab follows the driver");
    h.sent();
    h.db(DbEvent::Connected);
    assert!(executes(&mut h).is_empty(), "nothing is sent");
    assert!(!h.app.is_queued(id));
    assert!(!h.app.tabs.get(id).expect("the tab").editor.has_staged_run());
}

/// Launched with one MySQL profile `target`, each driver its own fake (PostgreSQL's, MySQL's),
/// connected from the explorer: its console has the focus, with no session of its own yet.
fn connected_apart(dirs: &Dirs) -> (Harness, FakeDriver) {
    let p = ConnectionConfig { name: "target".into(), driver: "mysql".into(), ..ConnectionConfig::test_db() };
    let cfg = Config { connections: vec![p], ..Config::default() };
    let (mut app, clock) = new_app_with_clock(&cfg, Lang::En);
    let store = Arc::new(MemoryStore::new());
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.set_paths(Paths { data: Some(dirs.0.join("data")), state: Some(dirs.0.join("state")) });
    let (pg, my) = (FakeDriver::default(), FakeDriver::default());
    my.mysql.store(true, Ordering::SeqCst);
    let (p2, m2) = (pg.clone(), my.clone());
    app.set_drivers(Arc::new(move |name: &str| match name {
        "postgres" => Some(Arc::new(p2.clone()) as Arc<dyn datarig_core::driver::Driver>),
        "mysql" | "mariadb" => Some(Arc::new(m2.clone()) as Arc<dyn datarig_core::driver::Driver>),
        _ => None,
    }));
    app.launch(Startup::Normal);
    let mut h = Harness { app, cancelled: pg.any_cancel.clone(), driver: pg, store, clock };
    h.explore("target");
    h.key(KeyCode::Enter);
    h.meta_db("target", DbEvent::Connected);
    assert!(h.app.tab().profile.is_some(), "its console");
    h.app.focus = Focus::Editor;
    (h, my)
}

/// The statements each session of `fake` was sent since the last look.
fn executes_on(fake: &FakeDriver) -> Vec<Vec<String>> {
    let mut sessions = fake.sessions.lock().unwrap();
    let mut v = Vec::new();
    for s in sessions.iter_mut() {
        s.closed();
        for c in std::mem::take(&mut s.seen) {
            if let DbCommand::Execute { statements, .. } = c {
                v.push(statements);
            }
        }
    }
    v
}

/// Ctrl+E on `sql` in the active tab (the statement under the cursor).
fn run_text(h: &mut Harness, sql: &str) {
    let mut editor = datarig_tui::widgets::editor::Editor::new(sql);
    editor.set_language(h.app.tab_language(h.app.tab().id));
    h.app.tab_mut().editor = editor;
    h.app.focus = Focus::Editor;
    h.ctrl('e');
}

/// The profile `target` saved with PostgreSQL as its driver (its form, from the explorer).
fn save_as_postgresql(h: &mut Harness) {
    h.explore("target");
    h.keys("e");
    assert!(h.form_open());
    h.app.overlays.form_mut().expect("the form").driver = 0;
    h.ctrl('s');
    assert!(!h.form_open(), "saved");
    let id = h.app.tab().id;
    assert_eq!(h.app.tab_language(id).dialect(), Dialect::Postgres, "the tab follows the driver");
}

/// The run reaches no session of the MySQL driver: it waits for the connection made with the
/// PostgreSQL driver, and is sent there once it is made.
fn assert_sent_to_the_new_connection(h: &mut Harness, my: &FakeDriver, sql: &str, case: &str) {
    assert!(h.overlay_kind().is_none(), "{case}: {:?}", h.overlay_kind());
    assert!(executes_on(my).is_empty(), "{case}: nothing reaches the MySQL driver");
    assert!(executes(h).is_empty(), "{case}: nothing before it connects");
    h.meta_db("target", DbEvent::Connected);
    assert!(executes_on(my).is_empty(), "{case}: nothing reaches the MySQL driver");
    assert_eq!(executes(h), [[sql]], "{case}: the PostgreSQL connection runs it");
}

/// A MySQL profile's tab with its session open, then the profile saved with the PostgreSQL
/// driver: the profile is disconnected, and the next run goes to the new connection.
#[test]
fn a_profile_whose_driver_changes_closes_its_open_session() {
    let dirs = Dirs::new("open-session");
    let (mut h, my) = connected_apart(&dirs);
    run_text(&mut h, "SELECT 1");
    assert_eq!(executes_on(&my), [["SELECT 1"]], "the MySQL session ran it");
    let id = h.app.tab().exec.query_id;
    h.tab_db(
        0,
        DbEvent::Done {
            id,
            outcome: datarig_core::driver::Outcome::Command("SELECT 1".into()),
            elapsed: std::time::Duration::from_millis(1),
        },
    );
    assert!(h.app.tab().exec.running.is_none());
    save_as_postgresql(&mut h);
    assert!(h.app.tab().exec.session.is_none(), "its session is closed");
    run_text(&mut h, "SELECT 2");
    assert_sent_to_the_new_connection(&mut h, &my, "SELECT 2", "open session");
}

/// The same with no session of the tab yet: no session opens for the connection the MySQL
/// driver made.
#[test]
fn a_profile_whose_driver_changes_is_connected_again_before_its_next_run() {
    let dirs = Dirs::new("old-connection");
    let (mut h, my) = connected_apart(&dirs);
    assert!(h.app.tab().exec.session.is_none(), "no session of the tab yet");
    save_as_postgresql(&mut h);
    run_text(&mut h, "SELECT 2");
    assert_sent_to_the_new_connection(&mut h, &my, "SELECT 2", "no session");
}
