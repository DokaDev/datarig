//! Executable comments in what a run sends: on a MySQL-family profile, a statement is sent with
//! every executable comment (`/*! … */`, `/*M! … */`, versioned too) it was written with, at
//! its start and its end too, and one that is only such a comment is a statement. The run asks
//! first, with the text it sends, and a read-only profile refuses it. From the editor (the
//! selection or the statement under the cursor) and from a saved query's tab alike.

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbEvent, Outcome};
use datarig_core::paths::Paths;
use datarig_core::policy::Policy;
use datarig_core::profile::ConnectionConfig;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_core::sql::risk::{Danger, Why};
use datarig_i18n::{Lang, Msg};
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::script_tree::TreeRow;
use datarig_tui::app::{Focus, Startup, TabKind};
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::KeyCode;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// What the tab's session has said of its server.
#[derive(Clone, Copy, Debug)]
enum Said {
    /// Nothing yet: the driver's reading (a MySQL server's) until a session says.
    Nothing,
    MySql,
    MariaDb,
}

/// `(profile driver, what its session said)`: every reading a MySQL-family tab may have.
const READINGS: [(&str, Said); 5] = [
    ("mysql", Said::Nothing),
    ("mysql", Said::MySql),
    ("mysql", Said::MariaDb),
    ("mariadb", Said::Nothing),
    ("mariadb", Said::MariaDb),
];

/// Statements as written, and the text each run sends (the `;` and the blanks around it go).
const CASES: [(&str, &str); 12] = [
    ("UPDATE t SET v = 0 WHERE v >= 0 /*M! AND id = 1 */;", "UPDATE t SET v = 0 WHERE v >= 0 /*M! AND id = 1 */"),
    ("UPDATE t SET v=0 /*M! WHERE id=1 */;", "UPDATE t SET v=0 /*M! WHERE id=1 */"),
    ("DELETE FROM t /*M!100500 WHERE id=1 */", "DELETE FROM t /*M!100500 WHERE id=1 */"),
    ("/*M! SET @a=1 */;", "/*M! SET @a=1 */"),
    ("/*M!100500 SET @a=1 */ ;", "/*M!100500 SET @a=1 */"),
    ("/*M! SET @a=1 */ SELECT 1;", "/*M! SET @a=1 */ SELECT 1"),
    ("SELECT 1 /*M! , 2 */", "SELECT 1 /*M! , 2 */"),
    ("UPDATE t SET v=0 /*! WHERE id=1 */;", "UPDATE t SET v=0 /*! WHERE id=1 */"),
    ("DELETE FROM t /*!80000 WHERE id=1 */", "DELETE FROM t /*!80000 WHERE id=1 */"),
    ("/*! SET @a=1 */;", "/*! SET @a=1 */"),
    ("/*!80000 SET @a=1 */ SELECT 1", "/*!80000 SET @a=1 */ SELECT 1"),
    ("SELECT 1 /*!80000 , 2 */", "SELECT 1 /*!80000 , 2 */"),
];

fn mode(said: Said) -> MySqlMode {
    MySqlMode { mariadb: matches!(said, Said::MariaDb), ..MySqlMode::default() }
}

fn profile(driver: &str, read_only: bool) -> (Config, FakeDriver) {
    let p = ConnectionConfig {
        name: "local-my".into(),
        driver: driver.into(),
        policy: Some(if read_only { "prod" } else { "plain" }.into()),
        ..ConnectionConfig::test_db()
    };
    let mut cfg = Config { connections: vec![p], ..Config::default() };
    cfg.policies.insert("plain", Policy::default());
    cfg.policies.insert("prod", Policy { read_only: true, ..Policy::default() });
    let fake = FakeDriver::default();
    fake.mysql.store(true, Ordering::SeqCst);
    (cfg, fake)
}

/// The tab's session says `said` of its server, after a run of its own that ends.
fn say(h: &mut Harness, said: Said) {
    if matches!(said, Said::Nothing) {
        return;
    }
    run_all(h, "SELECT 1");
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::Language(Language::Sql(Dialect::MySql(mode(said)))));
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Command("SELECT 1".into()), elapsed: Duration::from_millis(1) });
    assert!(h.app.tab().exec.running.is_none(), "the first run ended");
    h.sent();
}

/// Connected to a profile of `driver`, read-only when `read_only`, its tab's session having
/// said `said`.
fn connected(driver: &str, said: Said, read_only: bool) -> Harness {
    let (cfg, fake) = profile(driver, read_only);
    let mut h = Harness::with_driver(&cfg, Lang::En, fake);
    h.db(DbEvent::Connected);
    h.app.focus = Focus::Editor;
    say(&mut h, said);
    h
}

/// The editor of the tab holds `sql`, read in the tab's language, the cursor at its start.
fn edit(h: &mut Harness, sql: &str) {
    let mut editor = Editor::new(sql);
    editor.set_language(h.app.tab_language(h.app.tab().id));
    h.app.tab_mut().editor = editor;
}

/// Run all of `sql` (selected) with Ctrl+E.
fn run_all(h: &mut Harness, sql: &str) {
    edit(h, sql);
    h.keys("ggvG$");
    h.sent();
    h.ctrl('e');
}

/// Run `sql` the way `cursor` says: the statement under the cursor, or all of it selected.
fn run(h: &mut Harness, sql: &str, cursor: bool) {
    if cursor {
        edit(h, sql);
        h.sent();
        h.ctrl('e');
    } else {
        run_all(h, sql);
    }
}

/// The statements of each run sent since the last look.
fn executes(h: &mut Harness) -> Vec<Vec<String>> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { statements, .. } => Some(statements),
            _ => None,
        })
        .collect()
}

/// Why the run asks, and the text it shows, per statement that asks.
fn asked(h: &Harness) -> Vec<(Option<Why>, String)> {
    h.app.overlays.run_confirm().map(|c| c.items.iter().map(|d| (d.why, d.sql.clone())).collect()).unwrap_or_default()
}

fn status(h: &Harness) -> Option<Msg> {
    let tab = h.app.tab().status.as_ref().map(|n| n.msg.clone());
    tab.or_else(|| h.app.status.as_ref().map(|n| n.msg.clone()))
}

/// The run sends `sent` once asked and confirmed; a read-only profile refuses it and sends
/// nothing.
fn assert_sent_as_written(mut h: Harness, mut ro: Harness, sql: &str, sent: &str, cursor: bool, case: &str) {
    run(&mut h, sql, cursor);
    assert_eq!(asked(&h), [(Some(Why::Danger(Danger::ExecutableComment)), sent.to_string())], "{case}");
    assert!(executes(&mut h).is_empty(), "{case}: nothing before the yes");
    h.key(KeyCode::Char('y'));
    assert_eq!(executes(&mut h), [[sent]], "{case}");
    run(&mut ro, sql, cursor);
    assert!(matches!(status(&ro), Some(Msg::SafetyReadOnlyBlocked { .. })), "{case}: {:?}", status(&ro));
    assert!(executes(&mut ro).is_empty(), "{case}: refused, nothing sent");
}

/// A run from the editor, of the selection or of the statement under the cursor, sends the
/// statement with the executable comments it starts or ends with, on every reading of a
/// MySQL-family tab (before its session says which server it is on too).
#[test]
fn a_run_sends_the_executable_comments_a_statement_starts_or_ends_with() {
    for (driver, said) in READINGS {
        for (sql, sent) in CASES {
            for cursor in [false, true] {
                let case = format!("{driver} {said:?}, cursor {cursor}: {sql}");
                let h = connected(driver, said, false);
                let ro = connected(driver, said, true);
                assert_sent_as_written(h, ro, sql, sent, cursor, &case);
            }
        }
    }
}

/// Several statements selected: each is sent with its own executable comments, and the cursor
/// on one that is only such a comment runs it (not the statement before it, nor nothing).
#[test]
fn several_statements_keep_their_executable_comments() {
    let sql = "UPDATE t SET v=0 /*M! WHERE id=1 */; SELECT 2 /*M! , 3 */;\n/*M! SET @a=1 */;";
    let sent = ["UPDATE t SET v=0 /*M! WHERE id=1 */", "SELECT 2 /*M! , 3 */", "/*M! SET @a=1 */"];
    for (driver, said) in READINGS {
        let case = format!("{driver} {said:?}");
        let mut h = connected(driver, said, false);
        run_all(&mut h, sql);
        let shown: Vec<String> = asked(&h).into_iter().map(|(_, s)| s).collect();
        assert_eq!(shown, sent, "{case}");
        h.key(KeyCode::Char('y'));
        assert_eq!(executes(&mut h), [sent], "{case}");
        let mut h = connected(driver, said, false);
        edit(&mut h, sql);
        h.keys("j$");
        h.sent();
        h.ctrl('e');
        assert_eq!(asked(&h), [(Some(Why::Danger(Danger::ExecutableComment)), sent[2].to_string())], "{case}");
        h.key(KeyCode::Char('y'));
        assert_eq!(executes(&mut h), [[sent[2]]], "{case}");
    }
}

/// Text without an executable comment is sent as before: blanks, plain comments and the `;`
/// around a statement are not.
#[test]
fn plain_comments_around_a_statement_are_still_left_out() {
    for (driver, said) in READINGS {
        let case = format!("{driver} {said:?}");
        let mut h = connected(driver, said, false);
        run_all(&mut h, "  /* a */ SELECT 1 /* b */ -- c\n; /*m! d */");
        assert!(h.app.overlays.run_confirm().is_none(), "{case}");
        assert_eq!(executes(&mut h), [["SELECT 1"]], "{case}");
    }
}

/// A saved query's tab runs as a console does: the statement goes with its executable comments.
#[test]
fn a_saved_query_sends_its_executable_comments() {
    let root = std::env::temp_dir().join(format!("datarig-exec-comments-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    for (driver, said) in READINGS {
        for (sql, sent) in CASES {
            for cursor in [false, true] {
                let case = format!("{driver} {said:?}, cursor {cursor}: {sql}");
                let dir = root.join(format!("{driver}-{said:?}-{cursor}"));
                let _ = std::fs::remove_dir_all(&dir);
                let h = scripted(driver, said, false, &dir);
                let ro = scripted(driver, said, true, &dir.join("ro"));
                assert_sent_as_written(h, ro, sql, sent, cursor, &case);
            }
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// [`connected`], launched on data and state directories under `dir`, its console saved as a
/// query (the tab is that query's).
fn scripted(driver: &str, said: Said, read_only: bool, dir: &std::path::Path) -> Harness {
    let (cfg, fake) = profile(driver, read_only);
    let (mut app, clock) = new_app_with_clock(&cfg, Lang::En);
    let store = Arc::new(MemoryStore::new());
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.set_paths(Paths { data: Some(dir.join("data")), state: Some(dir.join("state")) });
    let driver_fake = fake.clone();
    app.set_drivers(Arc::new(move |name: &str| {
        matches!(name, "mysql" | "mariadb")
            .then(|| Arc::new(driver_fake.clone()) as Arc<dyn datarig_core::driver::Driver>)
    }));
    app.launch(Startup::Normal);
    let mut h = Harness { app, cancelled: fake.any_cancel.clone(), driver: fake, store, clock };
    h.explore("local-my");
    h.key(KeyCode::Enter);
    h.meta_db("local-my", DbEvent::Connected);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Editor);
    h.ctrl('s');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree), "a console asks for a name");
    let t = h.app.overlays.script_tree_mut().expect("the tree dialog");
    t.row = TreeRow::Top;
    t.input.set("kept");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().kind, TabKind::Script, "the tab is the saved query's");
    say(&mut h, said);
    h
}
