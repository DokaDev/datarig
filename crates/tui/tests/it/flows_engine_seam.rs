//! Pins of how the app follows a profile's engine today, before and after its server answered:
//! what a statement is judged in before connect (the profile's last known language, else the
//! driver's default), how Startup SQL of a MySQL-family profile is read, how each tab follows its
//! own session's sql mode (and drops it with a lost connection), when a tab's prepared
//! statements are known, what a cancel does on every engine, and which transaction checks and
//! server limits each engine gets. Each test states what happens now, so a later change of any
//! of it shows up here.

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbError, DbEvent, Driver, ExplainFormat, Outcome, SessionRole, TxRule, TxStep};
use datarig_core::policy::{CommitRule, Policy};
use datarig_core::profile::{ConnectionConfig, ProfileId};
use datarig_core::secret::{MemoryStore, PasswordSource};
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_i18n::{Label, Lang, Msg};
use datarig_tui::app::action::Action;
use datarig_tui::app::tabs::ResultView;
use datarig_tui::app::{AppEvent, Focus, NodeState, Startup};
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::KeyCode;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;

const MS: Duration = Duration::from_millis(3);

/// The three engines a profile may reach today: PostgreSQL, and MySQL or MariaDB through the
/// same MySQL driver (the server says which with its first `Language`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Engine {
    Postgres,
    MySql,
    MariaDb,
}

const ENGINES: [Engine; 3] = [Engine::Postgres, Engine::MySql, Engine::MariaDb];

impl Engine {
    /// The profile's `driver` key.
    fn driver(self) -> &'static str {
        match self {
            Engine::Postgres => "postgres",
            Engine::MySql | Engine::MariaDb => "mysql",
        }
    }

    fn mysql_driver(self) -> bool {
        self != Engine::Postgres
    }

    /// The language a session of the engine says it reads its text in, if it says one.
    fn said(self) -> Option<Language> {
        match self {
            Engine::Postgres => None,
            Engine::MySql => Some(mysql(MySqlMode::default())),
            Engine::MariaDb => Some(mysql(mariadb())),
        }
    }
}

fn mysql(mode: MySqlMode) -> Language {
    Language::Sql(Dialect::MySql(mode))
}

fn mariadb() -> MySqlMode {
    MySqlMode { mariadb: true, ..MySqlMode::default() }
}

fn no_backslash_escapes() -> MySqlMode {
    MySqlMode { no_backslash_escapes: true, ..MySqlMode::default() }
}

fn ansi_quotes() -> MySqlMode {
    MySqlMode { ansi_quotes: true, ..MySqlMode::default() }
}

/// A fake driver that is MySQL's when `mysql`.
fn driver(mysql: bool) -> FakeDriver {
    let d = FakeDriver::default();
    d.mysql.store(mysql, Ordering::SeqCst);
    d
}

/// [`driver`], saying it can stop a statement on the server (`Capabilities::cancel`) or not.
fn driver_cancel(mysql: bool, cancel: bool) -> FakeDriver {
    let d = driver(mysql);
    d.no_cancel.store(!cancel, Ordering::SeqCst);
    d
}

fn id(h: &Harness, name: &str) -> ProfileId {
    h.app.profiles.iter().find(|p| p.name == name).expect("profile").id
}

/// The runs sent since the last look: their statements.
fn executes(h: &mut Harness) -> Vec<Vec<String>> {
    h.sent()
        .into_iter()
        .filter_map(|c| if let DbCommand::Execute { statements, .. } = c { Some(statements) } else { None })
        .collect()
}

/// The editor of the active tab holds `sql`, read in the tab's language.
fn edit(h: &mut Harness, sql: &str) {
    let mut editor = Editor::new(sql);
    editor.set_language(h.app.tab_language(h.app.tab().id));
    h.app.tab_mut().editor = editor;
}

/// Run all of `sql` (selected) with Ctrl+E in the active tab.
fn run_all(h: &mut Harness, sql: &str) {
    edit(h, sql);
    h.keys("ggvG$");
    h.ctrl('e');
}

/// The run of the active tab ended without error.
fn done(h: &mut Harness) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Done { id, outcome: Outcome::Command("X".into()), elapsed: MS });
}

fn status(h: &Harness) -> Option<Msg> {
    h.app.status.as_ref().map(|n| n.msg.clone())
}

fn tab_status(h: &Harness) -> Option<Msg> {
    h.app.tab().status.as_ref().map(|n| n.msg.clone())
}

// ── A profile whose connect is slow: what waits for it ──────────────────────────────────────

/// Next background event other than the launch-time keychain probe, or fail after a few
/// seconds.
async fn next_event(rx: &mut UnboundedReceiver<AppEvent>) -> AppEvent {
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a background event")
            .expect("channel open");
        if !matches!(ev, AppEvent::KeychainProbed(_)) {
            return ev;
        }
    }
}

/// One profile `slow` of `engine` (read-only with `read_only`), its password from a command, so
/// that its connect waits until the test hands the command's result over
/// ([`AppEvent::Resolved`]). Nothing is connected yet.
fn slow_harness(engine: Engine, read_only: bool) -> (Harness, UnboundedReceiver<AppEvent>) {
    slow_harness_cancel(engine, read_only, true)
}

/// [`slow_harness`] with a driver that says it can stop a statement on the server or not.
fn slow_harness_cancel(engine: Engine, read_only: bool, cancel: bool) -> (Harness, UnboundedReceiver<AppEvent>) {
    slow_harness_key(engine.driver(), engine.mysql_driver(), read_only, cancel)
}

/// [`slow_harness_cancel`] with the profile's `driver` key `key` and the MySQL driver when
/// `mysql_driver`.
fn slow_harness_key(
    key: &str,
    mysql_driver: bool,
    read_only: bool,
    cancel: bool,
) -> (Harness, UnboundedReceiver<AppEvent>) {
    let mut p = ConnectionConfig {
        name: "slow".into(),
        driver: key.into(),
        password: String::new(),
        ..ConnectionConfig::test_db()
    };
    p.set_source(PasswordSource::Command("printf datarig".into()));
    if read_only {
        p.policy = Some("prod".into());
    }
    let mut cfg = Config { connections: vec![p], ..Config::default() };
    cfg.policies.insert("prod", Policy { read_only: true, ..Policy::default() });
    let (h, rx) = Harness::started(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal);
    h.driver.mysql.store(mysql_driver, Ordering::SeqCst);
    h.driver.no_cancel.store(!cancel, Ordering::SeqCst);
    (h.with_fake_driver(), rx)
}

/// `o` on `slow`: a console on it while its password command runs; the command's result is
/// held back (returned) and the editor focused.
async fn console_on_slow(h: &mut Harness, rx: &mut UnboundedReceiver<AppEvent>) -> AppEvent {
    h.explore("slow");
    h.keys("o");
    assert_eq!(h.app.conns.state(id(h, "slow")), NodeState::Connecting);
    let resolved = next_event(rx).await;
    assert!(matches!(resolved, AppEvent::Resolved { .. }), "{resolved:?}");
    h.app.focus = Focus::Editor;
    assert_eq!(h.app.tab().profile, Some(id(h, "slow")), "the console is on slow");
    resolved
}

/// The roles of the sessions the fake driver opened.
fn query_sessions(h: &Harness) -> usize {
    h.roles().into_iter().filter(|r| *r == SessionRole::Query).count()
}

/// The statements waiting in the active tab for its profile to connect.
fn waiting(h: &Harness) -> Vec<Vec<String>> {
    let tab = h.app.tab().id;
    h.app
        .conns
        .iter()
        .flat_map(|(_, c)| c.pending.iter().filter(|q| q.tab == tab).map(|q| q.statements.clone()))
        .collect()
}

/// A tree captured from MySQL 8.4 (the core's fixtures).
fn tree_fixture(name: &str) -> String {
    let path = format!("{}/../core/src/sql/plan/fixtures/mysql/8.4/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// A plan asked for before a MySQL profile connected waits for the connection, in the plan
/// format of the driver (MySQL's tree), and is shown as a plan once the server answered.
#[tokio::test(flavor = "multi_thread")]
async fn a_plan_asked_before_connect_on_a_mysql_profile_connects_then_explains() {
    let (mut h, mut rx) = slow_harness(Engine::MySql, false);
    let resolved = console_on_slow(&mut h, &mut rx).await;
    edit(&mut h, "SELECT * FROM c;");
    h.sent();
    h.keys(" ex");
    assert!(h.app.is_queued(h.app.tab().id), "it waits for the connection");
    assert_eq!(waiting(&h), [["EXPLAIN FORMAT=TREE SELECT * FROM c"]]);
    assert!(executes(&mut h).is_empty(), "nothing is sent before the connect");
    h.app.on_app_event(resolved);
    h.meta_db("slow", DbEvent::Language(mysql(MySqlMode::default())));
    h.meta_db("slow", DbEvent::Explain(Some(ExplainFormat::MySqlTree)));
    assert!(executes(&mut h).is_empty(), "not before `Connected`");
    h.meta_db("slow", DbEvent::Connected);
    assert_eq!(executes(&mut h), [["EXPLAIN FORMAT=TREE SELECT * FROM c"]]);
    let id = h.app.tab().exec.query_id;
    h.tab_db(
        h.app.tabs.active_index(),
        DbEvent::Page {
            id,
            columns: Some(vec![meta("EXPLAIN", "TEXT", false, false)]),
            rows: vec![vec![Some(tree_fixture("scan.tree.txt"))]],
            more: false,
            elapsed: MS,
        },
    );
    assert_eq!(h.app.tab().exec.view, ResultView::Plan, "the tree is shown as a plan");
}

/// A MariaDB server says it gives no plan the app reads (`Explain(None)`) before `Connected`.
/// A plan asked for before that word was asked in the driver's format and waits: today it is
/// still sent once connected (the server answers it with an error). Once the word came, a plan
/// is no longer asked for: before `Connected` and after it, nothing is queued or sent.
#[tokio::test(flavor = "multi_thread")]
async fn a_plan_asked_before_connect_on_a_mariadb_server_is_refused_after_explain_none() {
    let (mut h, mut rx) = slow_harness(Engine::MariaDb, false);
    let resolved = console_on_slow(&mut h, &mut rx).await;
    edit(&mut h, "SELECT * FROM c;");
    h.sent();
    h.keys(" ex");
    assert_eq!(waiting(&h), [["EXPLAIN FORMAT=TREE SELECT * FROM c"]], "asked before the server said");
    h.app.on_app_event(resolved);
    h.meta_db("slow", DbEvent::Language(mysql(mariadb())));
    h.meta_db("slow", DbEvent::Explain(None));
    h.meta_db("slow", DbEvent::Edit(None));
    let pid = id(&h, "slow");
    assert_eq!(h.app.conns.get(pid).and_then(|c| c.explain), Some(None), "stored before `Connected`");
    h.meta_db("slow", DbEvent::Connected);
    assert_eq!(executes(&mut h), [["EXPLAIN FORMAT=TREE SELECT * FROM c"]], "today: sent as it was queued");
    let id = h.app.tab().exec.query_id;
    let error = DbError::Server("ERROR 1064 (42000): You have an error in your SQL syntax".into());
    h.tab_db(h.app.tabs.active_index(), DbEvent::Failed { id, error, cancelled: false });
    assert!(h.app.tab().exec.running.is_none());
    // Connected: no plan is asked for any more.
    h.keys(" ex");
    h.keys(" ea");
    assert!(executes(&mut h).is_empty() && !h.app.is_queued(h.app.tab().id));
    // A new attempt: the word comes again before `Connected`, and a plan asked for between the
    // two is refused before anything waits.
    h.app.dispatch(Action::DisconnectCurrent);
    h.explore("slow");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.conns.state(pid), NodeState::Connecting);
    let resolved = next_event(&mut rx).await;
    h.app.on_app_event(resolved);
    assert_eq!(h.app.conns.get(pid).and_then(|c| c.explain), None, "forgotten at the new attempt");
    h.meta_db("slow", DbEvent::Language(mysql(mariadb())));
    h.meta_db("slow", DbEvent::Explain(None));
    h.app.focus = Focus::Editor;
    assert_eq!(h.app.tab().profile, Some(pid), "the console is still on slow");
    assert!(!h.app.tab_busy(h.app.tab().id), "nothing runs or waits in it");
    h.sent();
    h.keys(" ex");
    assert!(!h.app.is_queued(h.app.tab().id), "nothing waits");
    h.meta_db("slow", DbEvent::Connected);
    assert!(executes(&mut h).is_empty(), "nothing is sent");
}

/// `Edit(None)` said before `Connected` is the profile's at once: the first result read after
/// the connect is not editable, with the server's reason.
#[test]
fn edit_none_is_stored_before_connected() {
    let mut p = ConnectionConfig { name: "local-my".into(), driver: "mysql".into(), ..ConnectionConfig::test_db() };
    p.password = "datarig".into();
    let cfg = Config { connections: vec![p], ..Config::default() };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver(true));
    let pid = id(&h, "local-my");
    h.meta_db("local-my", DbEvent::Language(mysql(mariadb())));
    h.meta_db("local-my", DbEvent::Explain(None));
    h.meta_db("local-my", DbEvent::Edit(None));
    assert_eq!(h.app.conns.get(pid).and_then(|c| c.edit), Some(None), "stored before `Connected`");
    h.meta_db("local-my", DbEvent::Connected);
    assert_eq!(h.app.conns.get(pid).and_then(|c| c.edit), Some(None), "kept by `Connected`");
    h.meta_db("local-my", DbEvent::Keys(Ok(shop_keys())));
    h.app.run(vec!["SELECT id, email FROM shop.users".into()]);
    let id = h.app.tab().exec.query_id;
    let columns = vec![meta_from("id", "int8", true, USERS, 1), meta_from("email", "text", false, USERS, 2)];
    let rows = vec![vec![Some("1".into()), Some("a@x".into())]];
    h.tab_db(0, DbEvent::Page { id, columns: Some(columns), rows, more: false, elapsed: MS });
    h.app.focus = Focus::Results;
    h.sent();
    h.keys("e");
    let said = h.app.transient.take().map(|(n, _)| n.msg);
    assert_eq!(said, Some(Msg::EditReadOnly { reason: "this connection type can't edit data".into() }));
    assert!(h.sent().is_empty(), "nothing asked of the server");
}

/// The Startup SQL of a MySQL-family profile is judged before any server answered, so it is
/// read as on MySQL and on MariaDB: a call MariaDB does not provide (`BIN_TO_UUID`,
/// `ANY_VALUE`) and one MySQL does not (`` `POINT`(1, 2) ``, quoted: a stored function's name
/// there, MariaDB's own constructor) each refuse the connect on a `mysql` and on a `mariadb`
/// profile alike; a call both provide opens the session with it.
#[test]
fn startup_sql_of_a_mysql_family_profile_is_read_as_mysql_and_mariadb() {
    for key in ["mysql", "mariadb"] {
        for (sql, connects) in [
            ("SET @x = BIN_TO_UUID(UUID_TO_BIN(UUID()))", false),
            ("SET @x = ANY_VALUE(1)", false),
            ("SET @p = `POINT`(1, 2)", false),
            ("SET @x = UUID()", true),
        ] {
            let p = ConnectionConfig {
                name: "local-my".into(),
                driver: key.into(),
                session_sql: vec![sql.into()],
                ..ConnectionConfig::test_db()
            };
            let cfg = Config { connections: vec![p], ..Config::default() };
            let mut h =
                Harness::launched(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal).with_fake_driver();
            h.driver.mysql.store(true, Ordering::SeqCst);
            h.explore("local-my");
            h.key(KeyCode::Enter);
            let pid = id(&h, "local-my");
            if connects {
                assert_ne!(h.app.conns.state(pid), NodeState::Failed, "{key}: {sql}");
                h.meta_db("local-my", DbEvent::Connected);
                h.app.focus = Focus::Editor;
                run_all(&mut h, "SELECT 1");
                let opened: Vec<Vec<String>> = h
                    .driver
                    .sessions
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|s| s.role == SessionRole::Query)
                    .map(|s| s.opts.session_sql.iter().map(|s| s.sql.clone()).collect())
                    .collect();
                assert_eq!(opened, [[sql]], "{key}");
            } else {
                assert!(h.driver.sessions.lock().unwrap().is_empty(), "{key}: nothing is opened: {sql}");
                assert_eq!(h.app.conns.state(pid), NodeState::Failed, "{key}: {sql}");
                let error = h.node_error("local-my").unwrap_or_default();
                assert!(
                    error.starts_with("local-my does not connect: statement 1 of its own startup SQL is not allowed"),
                    "{key}: {error}"
                );
                assert!(error.ends_with(sql), "{key}: {error}");
            }
        }
    }
}

/// A statement that waits for a MySQL profile to connect is split and checked in the language
/// the profile's last session said (here `NO_BACKSLASH_ESCAPES`), kept while it is not
/// connected: two statements, where the default mode would read one.
#[tokio::test(flavor = "multi_thread")]
async fn a_queued_run_is_judged_in_the_profiles_last_language_before_connect() {
    let (mut h, mut rx) = slow_harness(Engine::MySql, false);
    let resolved = console_on_slow(&mut h, &mut rx).await;
    let pid = id(&h, "slow");
    let tab = h.app.tab().id;
    assert_eq!(h.app.tab_language(tab), mysql(MySqlMode::default()), "the driver's default before any word");
    h.app.on_app_event(resolved);
    h.meta_db("slow", DbEvent::Language(mysql(no_backslash_escapes())));
    h.meta_db("slow", DbEvent::Connected);
    h.app.dispatch(Action::DisconnectCurrent);
    assert_eq!(h.app.conns.state(pid), NodeState::Disconnected);
    assert_eq!(h.app.conns.get(pid).and_then(|c| c.language), Some(mysql(no_backslash_escapes())), "kept");
    assert_eq!(h.app.tab_language(tab), mysql(no_backslash_escapes()));
    let sql = "SELECT 'a\\';\nSELECT 2;";
    h.app.focus = Focus::Editor;
    h.sent();
    run_all(&mut h, sql);
    assert!(h.app.is_queued(tab), "it waits for the connection");
    assert_eq!(waiting(&h), [vec!["SELECT 'a\\'".to_string(), "SELECT 2".to_string()]]);
    assert_eq!(h.app.tab().exec.prepared.language(), mysql(no_backslash_escapes()), "checked in it");
    let resolved = next_event(&mut rx).await;
    h.app.on_app_event(resolved);
    h.meta_db("slow", DbEvent::Connected);
    assert_eq!(executes(&mut h), [vec!["SELECT 'a\\'".to_string(), "SELECT 2".to_string()]]);
}

/// `SELECT BIN_TO_UUID(a) FROM t` on a read-only MySQL profile is judged in the driver's
/// default (MySQL) before the server said anything: it waits and runs once connected. When the
/// server says it is MariaDB before `Connected`, the check made again right before it is sent
/// refuses it (MariaDB has no such built-in).
#[tokio::test(flavor = "multi_thread")]
async fn bin_to_uuid_runs_on_a_mysql_read_only_profile_before_connect() {
    let sql = "SELECT BIN_TO_UUID(a) FROM t";
    for mariadb_said in [false, true] {
        let (mut h, mut rx) = slow_harness(Engine::MySql, true);
        let resolved = console_on_slow(&mut h, &mut rx).await;
        h.sent();
        run_all(&mut h, sql);
        assert!(h.app.is_queued(h.app.tab().id), "not refused before connect");
        assert_eq!(waiting(&h), [[sql]]);
        h.app.on_app_event(resolved);
        if mariadb_said {
            h.meta_db("slow", DbEvent::Language(mysql(mariadb())));
        }
        h.meta_db("slow", DbEvent::Connected);
        if mariadb_said {
            assert!(executes(&mut h).is_empty(), "refused once the server said MariaDB");
            assert!(matches!(tab_status(&h), Some(Msg::SafetyReadOnlyBlocked { .. })), "{:?}", tab_status(&h));
        } else {
            assert_eq!(executes(&mut h), [[sql]], "it runs");
        }
    }
}

/// What became of a run on a read-only profile, at each step.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Verdict {
    /// Sent to the server (or, before connect, waiting to be).
    Runs,
    /// Refused before anything was sent.
    Refused,
}

/// The profile's `driver` key (`mysql` or `mariadb`) and the server the MySQL driver reaches
/// (it says MySQL or MariaDB with its first `Language`) are independent: on a read-only
/// profile a call only one of them provides (`BIN_TO_UUID` MySQL's, `` `POINT`(1, 2) ``
/// quoted MariaDB's) is judged before connect in the driver's default (MySQL) whatever the
/// key, judged again right before it is sent in what the server said, and judged in what the
/// server said once connected. The key changes none of it.
#[tokio::test(flavor = "multi_thread")]
async fn the_profile_key_and_the_servers_language_decide_independently() {
    use Verdict::{Refused, Runs};
    // (statement, on MySQL, on MariaDB)
    let calls = [("SELECT BIN_TO_UUID(a) FROM t", Runs, Refused), ("SELECT `POINT`(1, 2)", Refused, Runs)];
    for key in ["mysql", "mariadb"] {
        for server in [Engine::MySql, Engine::MariaDb] {
            for (sql, on_mysql, on_mariadb) in calls {
                let on_server = if server == Engine::MariaDb { on_mariadb } else { on_mysql };
                let case = format!("{key} profile, {server:?} server: {sql}");
                let (mut h, mut rx) = slow_harness_key(key, true, true, true);
                let resolved = console_on_slow(&mut h, &mut rx).await;
                let tab = h.app.tab().id;
                assert_eq!(h.app.tab_language(tab), mysql(MySqlMode::default()), "{case}: the driver's default");
                h.sent();
                run_all(&mut h, sql);
                let before = if h.app.is_queued(tab) { Runs } else { Refused };
                assert_eq!(before, on_mysql, "{case}: before connect");
                if before == Refused {
                    assert!(matches!(tab_status(&h), Some(Msg::SafetyReadOnlyBlocked { .. })), "{case}");
                }
                h.app.on_app_event(resolved);
                h.meta_db("slow", DbEvent::Language(server.said().expect("a MySQL-family server")));
                h.meta_db("slow", DbEvent::Connected);
                let sent = executes(&mut h);
                if before == Runs {
                    let at_send = if sent.is_empty() { Refused } else { Runs };
                    assert_eq!(at_send, on_server, "{case}: right before it is sent");
                    if at_send == Runs {
                        assert_eq!(sent, [[sql]], "{case}");
                        done(&mut h);
                    } else {
                        assert!(matches!(tab_status(&h), Some(Msg::SafetyReadOnlyBlocked { .. })), "{case}");
                    }
                } else {
                    assert!(sent.is_empty(), "{case}: a refused run is not sent later");
                }
                // Connected: run once more.
                h.app.focus = Focus::Editor;
                run_all(&mut h, sql);
                let connected = if executes(&mut h).is_empty() { Refused } else { Runs };
                assert_eq!(connected, on_server, "{case}: connected");
            }
        }
    }
}

// ── Each tab follows its own session's sql mode ─────────────────────────────────────────────

/// A connected MySQL profile with two tabs on it, each with a session that said the default
/// mode first.
fn two_mysql_tabs() -> Harness {
    let p = ConnectionConfig { name: "local-my".into(), driver: "mysql".into(), ..ConnectionConfig::test_db() };
    let cfg = Config { connections: vec![p], ..Config::default() };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver(true));
    h.db(DbEvent::Connected);
    h.app.focus = Focus::Editor;
    run_all(&mut h, "SELECT 1");
    h.tab_db(0, DbEvent::Language(mysql(MySqlMode::default())));
    done(&mut h);
    h.app.dispatch(Action::NewTab);
    assert_eq!(h.app.tabs.len(), 2);
    assert_eq!(h.app.tab().profile, Some(id(&h, "local-my")));
    h.app.focus = Focus::Editor;
    run_all(&mut h, "SELECT 1");
    h.tab_db(1, DbEvent::Language(mysql(MySqlMode::default())));
    done(&mut h);
    h.sent();
    h
}

/// The language tab `index`'s editor, splitter and classifier read its text in.
fn tab_reads(h: &Harness, index: usize) -> (Language, Language) {
    let t = h.app.tabs.iter().nth(index).expect("tab");
    (t.editor.language(), t.exec.prepared.language())
}

/// How tab `index`'s editor splits `sql`, all of it selected (the tab is made the active one,
/// its editor keeps its language).
fn split_in_tab(h: &mut Harness, index: usize, sql: &str) -> Vec<String> {
    h.app.tabs.activate(index);
    h.app.focus = Focus::Editor;
    let lang = h.app.tab().editor.language();
    let mut editor = Editor::new(sql);
    editor.set_language(lang);
    h.app.tab_mut().editor = editor;
    h.keys("ggvG$");
    let (statements, _) = h.app.tab_mut().editor.run_statements();
    h.key(KeyCode::Esc);
    statements
}

/// `SET sql_mode` in one tab (its session says the new mode) changes how that tab's editor,
/// splitter and classifier read text; the other tab and the profile keep the mode their
/// sessions started in.
#[test]
fn a_sql_mode_change_in_one_tab_changes_only_that_tabs_editor_splitter_and_classifier() {
    let mut h = two_mysql_tabs();
    let pid = id(&h, "local-my");
    let default = mysql(MySqlMode::default());
    let nbe = mysql(no_backslash_escapes());
    h.tab_db(0, DbEvent::Language(nbe));
    assert_eq!(tab_reads(&h, 0), (nbe, nbe));
    assert_eq!(tab_reads(&h, 1), (default, default), "the other tab keeps its session's");
    assert_eq!(h.app.conns.get(pid).and_then(|c| c.language), Some(default), "a later word is the session's own");
    let sql = "SELECT 'a\\';\nSELECT 2;";
    assert_eq!(split_in_tab(&mut h, 0, sql), ["SELECT 'a\\'", "SELECT 2"]);
    assert_eq!(split_in_tab(&mut h, 1, sql).len(), 1);
    // `"x"` is a name under ANSI_QUOTES in tab 1 only: its classifier reads it so.
    h.tab_db(1, DbEvent::Language(mysql(ansi_quotes())));
    assert_eq!(tab_reads(&h, 1), (mysql(ansi_quotes()), mysql(ansi_quotes())));
    assert_eq!(tab_reads(&h, 0), (nbe, nbe));
}

/// A lost connection drops the tab's session mode: the tab reads its text in the profile's
/// last known language again (the first word of its sessions), which the profile keeps while
/// it is not connected. The next session's first word is the tab's and the profile's.
#[test]
fn a_lost_connection_drops_the_session_mode_and_a_reconnect_takes_the_new_one() {
    let mut h = two_mysql_tabs();
    let pid = id(&h, "local-my");
    let default = mysql(MySqlMode::default());
    let nbe = mysql(no_backslash_escapes());
    let ansi = mysql(ansi_quotes());
    h.tab_db(0, DbEvent::Language(nbe));
    h.app.tabs.activate(0);
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    assert_eq!(h.app.tabs.iter().next().unwrap().exec.language, None, "dropped with the session");
    assert_eq!(tab_reads(&h, 0), (default, default), "the profile's last known language");
    assert_eq!(tab_reads(&h, 1), (default, default));
    // The next run opens a new session; its first word is the tab's and the profile's.
    h.app.focus = Focus::Editor;
    run_all(&mut h, "SELECT 1");
    h.tab_db(0, DbEvent::Language(ansi));
    assert_eq!(tab_reads(&h, 0), (ansi, ansi));
    assert_eq!(h.app.conns.get(pid).and_then(|c| c.language), Some(ansi), "the first word of a session");
    assert_eq!(tab_reads(&h, 1), (default, default), "a tab whose session said its own keeps it");
    done(&mut h);
    // Disconnected: the profile keeps its last known language, the tabs read in it.
    h.app.dispatch(Action::DisconnectCurrent);
    if h.overlay_kind().is_some() {
        h.keys("y");
    }
    assert_eq!(h.app.conns.state(pid), NodeState::Disconnected);
    assert_eq!(h.app.conns.get(pid).and_then(|c| c.language), Some(ansi), "kept for the next connect");
    assert_eq!(h.app.tab_language(h.app.tabs.iter().next().unwrap().id), ansi);
}

// ── A tab's prepared statements ─────────────────────────────────────────────────────────────

/// PostgreSQL: a name `PREPARE` made counts once the server confirmed the statement; a run
/// whose end is not known (cancelled, failed before it, the connection lost) forgets the names
/// it may have touched. A word of the server's facts (its plan format, its edits) keeps them;
/// after a lost connection a run is checked as on a new session, which prepared nothing.
#[test]
fn a_prepared_statement_is_kept_after_a_confirmed_success_and_forgotten_after_an_uncertain_result() {
    let mut h = Harness::connected(Lang::En);
    let knows = |h: &Harness, name: &str| h.app.tab().exec.prepared.knows(name);
    run_all(&mut h, "PREPARE ok AS SELECT 1");
    assert!(!knows(&h, "ok"), "sent is not succeeded");
    done(&mut h);
    assert!(knows(&h, "ok"), "confirmed");
    // Facts of the server do not rebuild the classifier.
    h.db(DbEvent::Explain(Some(ExplainFormat::PostgresJson)));
    h.db(DbEvent::Edit(Some(datarig_core::driver::EditFormat::Postgres)));
    assert!(knows(&h, "ok"), "kept through the server's facts");
    // Cancelled: whether it was prepared is not known.
    run_all(&mut h, "PREPARE c AS SELECT 2");
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::Failed { id, error: DbError::Cancelled, cancelled: true });
    assert!(!knows(&h, "c"));
    // A run that failed before its DEALLOCATE: whether `ok` is still there is not known.
    run_all(&mut h, "SELECT 1/0;\nDEALLOCATE ok;");
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::Failed { id, error: DbError::Server("ERROR: division by zero".into()), cancelled: false });
    assert!(!knows(&h, "ok"), "forgotten after an uncertain result");
    // The connection lost in the middle of a run: a new session prepared nothing.
    run_all(&mut h, "PREPARE again AS SELECT 3");
    done(&mut h);
    assert!(knows(&h, "again"));
    run_all(&mut h, "PREPARE lost AS SELECT 4");
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    assert!(!knows(&h, "lost"), "the lost run's name is not known");
    // Without a session the run is checked with a fresh classifier: an EXECUTE of a name the
    // old session prepared asks, and nothing is sent before the answer.
    h.sent();
    run_all(&mut h, "EXECUTE again");
    assert!(executes(&mut h).is_empty(), "asked first");
    let asked: Option<Vec<usize>> = h.app.overlays.run_confirm().map(|c| c.items.iter().map(|d| d.index).collect());
    assert_eq!(asked, Some(vec![0]), "a new session starts with none");
    // Not run: a new session opens for the next run, and its classifier starts with no name
    // either (the tab's own, not a fresh one made for the check).
    h.key(KeyCode::Esc);
    assert_eq!(h.overlay_kind(), None);
    assert!(executes(&mut h).is_empty(), "nothing was sent");
    let before = query_sessions(&h);
    run_all(&mut h, "SELECT 1");
    assert_eq!(executes(&mut h), [["SELECT 1"]]);
    assert_eq!(query_sessions(&h), before + 1, "a new query session");
    done(&mut h);
    assert!(!knows(&h, "again"), "the new session prepared nothing");
    run_all(&mut h, "EXECUTE again");
    assert!(executes(&mut h).is_empty(), "asked first on the new session too");
    let asked: Option<Vec<usize>> = h.app.overlays.run_confirm().map(|c| c.items.iter().map(|d| d.index).collect());
    assert_eq!(asked, Some(vec![0]), "the new session's classifier starts with none");
}

// ── Cancel is local first ───────────────────────────────────────────────────────────────────

/// On every engine, a cancel of a statement that waits for its profile to connect drops it:
/// it never runs, no query session is opened for it, and no session is asked to cancel. The
/// same with a driver that says it cannot stop a statement on the server: the cancel is the
/// app's own.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_drops_a_queued_run_without_starting_a_connection() {
    for (engine, cancel) in ENGINES.into_iter().flat_map(|e| [(e, true), (e, false)]) {
        let (mut h, mut rx) = slow_harness_cancel(engine, false, cancel);
        assert_eq!(h.driver.capabilities().cancel, cancel, "{engine:?}, cancel {cancel}");
        let resolved = console_on_slow(&mut h, &mut rx).await;
        run_all(&mut h, "SELECT 1");
        let tab = h.app.tab().id;
        assert!(h.app.is_queued(tab), "{engine:?}, cancel {cancel}");
        h.app.dispatch(Action::CancelQuery);
        assert!(!h.app.is_queued(tab), "{engine:?}, cancel {cancel}");
        assert_eq!(tab_status(&h), Some(Msg::Label(Label::QueryCancelled)), "{engine:?}, cancel {cancel}");
        h.app.on_app_event(resolved);
        if let Some(lang) = engine.said() {
            h.meta_db("slow", DbEvent::Language(lang));
        }
        h.meta_db("slow", DbEvent::Connected);
        assert!(executes(&mut h).is_empty(), "{engine:?}, cancel {cancel}: nothing ran");
        assert_eq!(query_sessions(&h), 0, "{engine:?}, cancel {cancel}: no query session was opened");
        assert!(!h.cancelled.load(Ordering::SeqCst), "{engine:?}, cancel {cancel}: no session was asked to cancel");
    }
}

/// On every engine, a cancel of a run of several statements reaches the session's canceller
/// (the engine never stops it before the driver): what the driver ran stays as it answered,
/// and the statements after the one it stopped at are not run. The same with a driver that
/// says it cannot stop a statement on the server: the cancel still reaches the session, which
/// stops the run between its statements.
#[test]
fn a_cancel_between_statements_reaches_the_session_on_every_engine() {
    use datarig_tui::app::runlog::StatementOutcome;
    for (engine, cancel) in ENGINES.into_iter().flat_map(|e| [(e, true), (e, false)]) {
        let p = ConnectionConfig { driver: engine.driver().into(), ..ConnectionConfig::test_db() };
        let cfg = Config { connections: vec![p], ..Config::default() };
        let mut h = Harness::with_driver(&cfg, Lang::En, driver_cancel(engine.mysql_driver(), cancel));
        assert_eq!(h.driver.capabilities().cancel, cancel, "{engine:?}, cancel {cancel}");
        if let Some(lang) = engine.said() {
            h.db(DbEvent::Language(lang));
        }
        if engine == Engine::MariaDb {
            h.db(DbEvent::Explain(None));
            h.db(DbEvent::Edit(None));
        }
        h.db(DbEvent::Connected);
        h.app.focus = Focus::Editor;
        h.sent();
        run_all(&mut h, "SELECT 1;\nSELECT 2;\nSELECT 3;");
        assert_eq!(executes(&mut h).len(), 1, "{engine:?}, cancel {cancel}");
        let id = h.app.tab().exec.query_id;
        h.tab_db(0, DbEvent::Started { id, index: 0 });
        h.tab_db(0, DbEvent::Finished { id, index: 0, outcome: Outcome::Command("SELECT".into()), elapsed: MS });
        h.tab_db(0, DbEvent::Started { id, index: 1 });
        h.app.dispatch(Action::CancelQuery);
        let session = h.roles().iter().position(|r| *r == SessionRole::Query).expect("a query session");
        assert!(h.session_cancelled(session), "{engine:?}, cancel {cancel}: the cancel reached the session");
        h.tab_db(0, DbEvent::Failed { id, error: DbError::Cancelled, cancelled: true });
        assert!(h.app.tab().exec.running.is_none(), "{engine:?}, cancel {cancel}");
        let outcomes: Vec<StatementOutcome> =
            h.app.tab().exec.run.statements.iter().map(|s| s.outcome.clone()).collect();
        assert!(matches!(outcomes[0], StatementOutcome::Command(_)), "{engine:?}, cancel {cancel}: {outcomes:?}");
        assert!(matches!(outcomes[1], StatementOutcome::Cancelled), "{engine:?}, cancel {cancel}: {outcomes:?}");
        assert!(matches!(outcomes[2], StatementOutcome::NotRun), "{engine:?}, cancel {cancel}: {outcomes:?}");
    }
}

// ── Transaction checks and the server's idle limit per engine ───────────────────────────────

/// Connected to a profile of `engine` in environment `env` (`stg`: manual commit with
/// `tx_idle_rollback` of 90 s; `plain`: automatic commit); a MySQL-family session said its
/// language.
fn connected_in(engine: Engine, env: &str) -> Harness {
    let p = ConnectionConfig {
        driver: engine.driver().into(),
        policy: Some(env.to_string()),
        ..ConnectionConfig::test_db()
    };
    let mut cfg = Config { connections: vec![p], ..Config::default() };
    let stg =
        Policy { commit: CommitRule::Manual, tx_idle_rollback: Some(Duration::from_secs(90)), ..Policy::default() };
    cfg.policies.insert("stg", stg);
    cfg.policies.insert("plain", Policy::default());
    let mut h = Harness::with_driver(&cfg, Lang::En, driver(engine.mysql_driver()));
    h.db(DbEvent::Connected);
    h.app.focus = Focus::Editor;
    if let Some(lang) = engine.said() {
        run_all(&mut h, "SELECT 1");
        h.tab_db(0, DbEvent::Language(lang));
        done(&mut h);
    }
    h.sent();
    h
}

fn notes(h: &Harness) -> Vec<Msg> {
    h.app.tab().exec.run.notes.iter().map(|n| n.msg.clone()).collect()
}

/// Run `sql` that opens the transaction for manual commit, answered: the transaction is open.
fn open_tx(h: &mut Harness, sql: &str) {
    run_all(h, sql);
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Affected(1), elapsed: MS });
    h.tab_db(0, DbEvent::ManualBegun { id, index: 0 });
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    h.sent();
}

/// The transaction checks before a run, per engine: PostgreSQL refuses a statement that cannot
/// run in a transaction block while one is open, and notes an enum value added and the lock a
/// DDL statement holds in one; MySQL and MariaDB ask before a statement that commits the open
/// transaction, note DDL with manual commit and none open, and refuse `LOCK TABLES` with manual
/// commit. Neither family gets the other's checks.
#[test]
fn tx_check_verdicts_per_dialect() {
    use datarig_tui::app::overlay::OverlayKind;
    use datarig_tui::app::tx::TxReason;
    for engine in ENGINES {
        let pg = engine == Engine::Postgres;
        // A statement that cannot run in a transaction block, while one is open.
        let mut h = connected_in(engine, "stg");
        open_tx(&mut h, "UPDATE users SET name = 'x' WHERE id = 1");
        run_all(&mut h, "VACUUM users");
        let refused = matches!(status(&h), Some(Msg::TxNoTxBlockRefused { .. }));
        assert_eq!(refused, pg, "{engine:?}: {:?}", status(&h));
        if pg {
            assert!(executes(&mut h).is_empty(), "{engine:?}");
        }
        // DDL with the transaction open.
        let mut h = connected_in(engine, "stg");
        open_tx(&mut h, "UPDATE users SET name = 'x' WHERE id = 1");
        run_all(&mut h, "ALTER TABLE users ADD COLUMN zz_c int");
        let asked: Vec<(usize, Option<TxReason>)> =
            h.app.overlays.run_confirm().map(|c| c.items.iter().map(|d| (d.index, d.tx)).collect()).unwrap_or_default();
        if pg {
            assert_eq!(h.overlay_kind(), None, "{engine:?}");
            let lock = notes(&h).into_iter().any(|m| matches!(m, Msg::TxDdlLockExclusive { .. }));
            assert!(lock, "{engine:?}: {:?}", notes(&h));
        } else {
            assert_eq!(h.overlay_kind(), Some(OverlayKind::RunConfirm), "{engine:?}");
            assert_eq!(asked, [(0, Some(TxReason::ImplicitCommit { statements: 1, rows: 1 }))], "{engine:?}");
            h.key(KeyCode::Esc);
        }
        // DDL with manual commit and no transaction open.
        let mut h = connected_in(engine, "stg");
        run_all(&mut h, "CREATE TABLE zz_t (id int)");
        let n = notes(&h);
        assert_eq!(n.contains(&Msg::TxMysqlDdl { step: "1".into() }), !pg, "{engine:?}: {n:?}");
        // An enum value added in the transaction this run opens.
        let mut h = connected_in(engine, "stg");
        run_all(&mut h, "ALTER TYPE mood ADD VALUE 'meh'");
        let n = notes(&h);
        assert_eq!(n.contains(&Msg::TxEnumValue { step: "1".into() }), pg, "{engine:?}: {n:?}");
        // LOCK TABLES with manual commit, and with automatic commit.
        let mut h = connected_in(engine, "stg");
        run_all(&mut h, "LOCK TABLES users WRITE");
        let refused = status(&h) == Some(Msg::Label(Label::TxLockTablesRefused));
        assert_eq!(refused, !pg, "{engine:?}: {:?}", status(&h));
        let mut h = connected_in(engine, "plain");
        run_all(&mut h, "LOCK TABLES users WRITE");
        assert_ne!(status(&h), Some(Msg::Label(Label::TxLockTablesRefused)), "{engine:?}");
    }
}

/// With manual commit and `tx_idle_rollback`, only PostgreSQL's server is given an idle limit
/// for the transaction (the rollback limit plus 30 s); MySQL and MariaDB get none (the app's
/// own timer rolls back on every engine).
#[test]
fn server_idle_limit_is_offered_on_postgres_only() {
    for engine in ENGINES {
        let mut h = connected_in(engine, "stg");
        run_all(&mut h, "UPDATE users SET name = 'x' WHERE id = 1");
        let rules: Vec<TxRule> = h
            .sent()
            .into_iter()
            .filter_map(|c| if let DbCommand::Execute { tx, .. } = c { Some(tx) } else { None })
            .collect();
        let idle = (engine == Engine::Postgres).then_some(Duration::from_secs(90 + 30));
        assert_eq!(rules, [TxRule::Manual { steps: vec![TxStep::Open], idle_timeout: idle }], "{engine:?}");
    }
}

// ── What the server is ──────────────────────────────────────────────────────────────────────

/// The facts a session of `engine` says first: MySQL 8.4.6, MariaDB 11.4.13 or PostgreSQL 17.2,
/// reached directly.
fn facts_of(engine: Engine) -> DbEvent {
    use datarig_core::engine::{EngineFacts, EngineId, Fact, MyFacts, PgFacts, ServerFacts, Topology, Version, Why};
    let (version, detail) = match engine {
        Engine::Postgres => (Version::new(17, 2, 0), EngineFacts::Postgres(PgFacts::default())),
        Engine::MySql => (
            Version::new(8, 4, 6),
            EngineFacts::MySql(MyFacts { handshake: Version::new(8, 4, 6), greeting: EngineId::MySql }),
        ),
        Engine::MariaDb => (
            Version::new(11, 4, 13),
            EngineFacts::MariaDb(MyFacts { handshake: Version::new(11, 4, 13), greeting: EngineId::MariaDb }),
        ),
    };
    DbEvent::Server(Arc::new(ServerFacts {
        version: Fact::Known(version),
        version_text: None,
        provider: Fact::Unknown(Why::NotChecked),
        topology: Fact::Known(Topology::Direct),
        detail,
    }))
}

/// What a session of `engine` says before `Connected`, as its driver says it, with the facts
/// first (`server`) or without them.
fn connect_words(engine: Engine, server: bool) -> Vec<DbEvent> {
    let mut words: Vec<DbEvent> = server.then(|| facts_of(engine)).into_iter().collect();
    words.push(DbEvent::Encryption(datarig_core::tls::Encryption::Plain));
    let format = (engine == Engine::MySql).then_some(ExplainFormat::MySqlTree);
    let edits = (engine == Engine::MySql)
        .then_some(datarig_core::driver::EditFormat::MySql(datarig_core::driver::MySqlEdit { nowait: true }));
    if let Some(lang) = engine.said() {
        words.extend([DbEvent::Language(lang), DbEvent::Explain(format), DbEvent::Edit(edits)]);
    }
    words.push(DbEvent::Connected);
    words
}

/// A session's first word, what its server is, changes nothing yet: on every engine a profile
/// whose metadata session and a tab whose query session say it connect, run and draw exactly as
/// without it (the same screens, plan format, edits and languages, and the same commands sent).
#[test]
fn a_server_word_changes_nothing_yet() {
    let run = |engine: Engine, server: bool| {
        let p = ConnectionConfig { name: "seam".into(), driver: engine.driver().into(), ..ConnectionConfig::test_db() };
        let cfg = Config { connections: vec![p], ..Config::default() };
        let mut h = Harness::with_driver(&cfg, Lang::En, driver(engine.mysql_driver()));
        let pid = id(&h, "seam");
        let mut seen = Vec::new();
        for ev in connect_words(engine, server) {
            h.meta_db("seam", ev);
        }
        seen.push(h.screen(120, 40));
        h.app.focus = Focus::Editor;
        run_all(&mut h, "SELECT 1");
        for ev in connect_words(engine, server) {
            h.tab_db(0, ev);
        }
        done(&mut h);
        seen.push(h.screen(120, 40));
        let c = h.app.conns.get(pid).expect("the profile's connection");
        seen.push(format!("{:?} {:?} {:?} {:?}", c.explain, c.edit, c.connected, tab_reads(&h, 0)));
        seen.push(format!("{:?} {:?} {:?}", status(&h), tab_status(&h), h.sent()));
        seen
    };
    for engine in ENGINES {
        assert_eq!(run(engine, true), run(engine, false), "{engine:?}");
    }
}

/// A MySQL-family session refused because the server's version line confirmed no engine leaves
/// the profile unconnected, and its error says what the greeting named and that nothing else ran
/// on the session.
#[test]
fn an_unconfirmed_engine_says_the_greeting_and_stays_unconnected() {
    let error = DbError::EngineUnconfirmed { greeting: "MySQL 8.4.6".into() };
    let text = "Connection failed: the server's engine could not be confirmed: VERSION() gave no answer \
                (MySQL 8.4.6 by its greeting), so the session was closed before anything else ran on it";
    let p = ConnectionConfig { name: "seam".into(), driver: "mysql".into(), ..ConnectionConfig::test_db() };
    let cfg = Config { connections: vec![p], ..Config::default() };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver(true));
    let pid = id(&h, "seam");
    h.meta_db("seam", DbEvent::ConnectFailed { error: error.clone(), auth: false });
    assert_eq!(h.node_error("seam").as_deref(), Some(text));
    assert!(h.prompt().is_none(), "not a password problem");
    assert!(!h.app.conns.get(pid).is_some_and(|c| c.connected));
}
