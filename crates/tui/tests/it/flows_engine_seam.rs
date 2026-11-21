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
            Engine::MariaDb => Some(mariadb()),
        }
    }
}

fn mysql(mode: MySqlMode) -> Language {
    Language::Sql(Dialect::MySql(mode))
}

fn mariadb() -> Language {
    Language::Sql(Dialect::MariaDb(MySqlMode::default()))
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
    h.meta_db("slow", DbEvent::Language(mariadb()));
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
    h.meta_db("slow", DbEvent::Language(mariadb()));
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
    h.meta_db("local-my", DbEvent::Language(mariadb()));
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
            h.meta_db("slow", DbEvent::Language(mariadb()));
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

// ── The engines a profile expects and the language its sessions said ─────────────────────────

/// A connected profile named `driver` (one tab on it) whose sessions run through the fake
/// MySQL driver, and PostgreSQL's fake for a `postgres` profile.
fn family_profile(driver_key: &str) -> Harness {
    let p = ConnectionConfig { name: "fam".into(), driver: driver_key.into(), ..ConnectionConfig::test_db() };
    let cfg = Config { connections: vec![p], ..Config::default() };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver(true));
    let (my, pg) = (h.driver.clone(), driver(false));
    h.app.set_drivers(Arc::new(move |name: &str| match name {
        "mysql" | "mariadb" => Some(Arc::new(my.clone()) as Arc<dyn Driver>),
        "postgres" => Some(Arc::new(pg.clone()) as Arc<dyn Driver>),
        _ => None,
    }));
    h.meta_db("fam", DbEvent::Connected);
    h.app.focus = Focus::Editor;
    h
}

/// MariaDB's language in sql mode `mode`.
fn maria(mode: MySqlMode) -> Language {
    Language::Sql(Dialect::MariaDb(mode))
}

/// On a `mariadb` profile, the MariaDB language a tab's session says is the tab's (its editor,
/// splitter and classifier read text in it, each sql mode of it), and the profile's first word
/// is what a new tab of it reads in.
#[test]
fn a_mariadb_language_is_kept_by_its_tab_and_profile() {
    let mut h = family_profile("mariadb");
    let pid = id(&h, "fam");
    run_all(&mut h, "SELECT 1");
    h.tab_db(0, DbEvent::Language(maria(MySqlMode::default())));
    done(&mut h);
    assert_eq!(tab_reads(&h, 0), (maria(MySqlMode::default()), maria(MySqlMode::default())));
    assert_eq!(h.app.conns.get(pid).and_then(|c| c.language), Some(maria(MySqlMode::default())));
    h.tab_db(0, DbEvent::Language(maria(no_backslash_escapes())));
    assert_eq!(tab_reads(&h, 0), (maria(no_backslash_escapes()), maria(no_backslash_escapes())));
    assert_eq!(split_in_tab(&mut h, 0, "SELECT 'a\\';\nSELECT 2;"), ["SELECT 'a\\'", "SELECT 2"]);
    h.app.dispatch(Action::NewTab);
    assert_eq!(h.app.tab().profile, Some(pid));
    assert_eq!(tab_reads(&h, 1), (maria(MySqlMode::default()), maria(MySqlMode::default())));
}

/// A `mysql` profile whose server is MariaDB keeps the language the server said, as a
/// `mariadb` profile does: the server decides, the profile only names the family.
#[test]
fn a_mysql_profile_on_a_mariadb_server_keeps_its_language() {
    let mut h = family_profile("mysql");
    let pid = id(&h, "fam");
    run_all(&mut h, "SELECT 1");
    h.tab_db(0, DbEvent::Language(maria(ansi_quotes())));
    done(&mut h);
    let tab = h.app.tab().id;
    assert_eq!(h.app.tab_language(tab), maria(ansi_quotes()));
    assert_eq!(tab_reads(&h, 0), (maria(ansi_quotes()), maria(ansi_quotes())));
    assert_eq!(h.app.conns.get(pid).and_then(|c| c.language), Some(maria(ansi_quotes())));
    // Its classifier is MariaDB's: a call MariaDB does not provide is one of a stored function.
    let risk = h.app.tab().exec.prepared.clone().classify("SELECT BIN_TO_UUID(a) FROM t");
    assert!(risk.unchecked_call.is_some(), "{risk:?}");
}

/// A profile that now names another engine's driver drops the language its sessions said,
/// MySQL's and MariaDB's alike: its tabs read text in the new driver's language.
#[test]
fn a_profile_that_changed_its_driver_drops_the_other_engines_language() {
    for said in [mysql(ansi_quotes()), maria(ansi_quotes())] {
        let mut h = family_profile("mysql");
        run_all(&mut h, "SELECT 1");
        h.tab_db(0, DbEvent::Language(said));
        done(&mut h);
        let tab = h.app.tab().id;
        assert_eq!(h.app.tab_language(tab), said);
        h.app.profiles[0].driver = "postgres".into();
        assert_eq!(h.app.tab_language(tab), Language::Sql(Dialect::Postgres), "{said:?}");
    }
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
        Engine::MySql => {
            (Version::new(8, 4, 6), EngineFacts::MySql(MyFacts::new(Version::new(8, 4, 6), EngineId::MySql)))
        }
        Engine::MariaDb => {
            (Version::new(11, 4, 13), EngineFacts::MariaDb(MyFacts::new(Version::new(11, 4, 13), EngineId::MariaDb)))
        }
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

// ── What the app asks of a tab's server ─────────────────────────────────────────────────────

/// The facts a session of `engine` at `version` says first, reached directly.
fn facts_at(engine: Engine, version: datarig_core::engine::Version) -> Arc<datarig_core::engine::ServerFacts> {
    use datarig_core::engine::{EngineFacts, EngineId, Fact, MyFacts, PgFacts, ServerFacts, Topology, Why};
    let detail = match engine {
        Engine::Postgres => EngineFacts::Postgres(PgFacts::default()),
        Engine::MySql => EngineFacts::MySql(MyFacts::new(version, EngineId::MySql)),
        Engine::MariaDb => EngineFacts::MariaDb(MyFacts::new(version, EngineId::MariaDb)),
    };
    Arc::new(ServerFacts {
        version: Fact::Known(version),
        version_text: None,
        provider: Fact::Unknown(Why::NotChecked),
        topology: Fact::Known(Topology::Direct),
        detail,
    })
}

/// Version `major.minor.patch`.
fn v(major: u16, minor: u16, patch: u16) -> datarig_core::engine::Version {
    datarig_core::engine::Version::new(major, minor, patch)
}

/// Tab `index`'s rules, which must answer the language its editor and classifier read text in
/// (today's), with a fresh classifier of that language.
fn rules_of(h: &Harness, index: usize) -> datarig_core::engine::Rules {
    let tab = h.app.tabs.iter().nth(index).expect("tab").id;
    let rules = h.app.rules(tab);
    let lang = rules.language();
    assert_eq!(tab_reads(h, index), (lang, lang), "{rules:?}");
    assert_eq!(h.app.tab_language(tab), lang);
    assert_eq!(rules.classifier(), datarig_core::sql::risk::Classifier::new(lang));
    rules
}

/// The facts tab `index`'s rules carry: what a session said its server is, if one did.
fn rules_facts(h: &Harness, index: usize) -> Option<Arc<datarig_core::engine::ServerFacts>> {
    use datarig_core::engine::Rules;
    match rules_of(h, index) {
        Rules::Server { facts, .. } => Some(facts),
        Rules::Unbound | Rules::Expected { .. } => None,
    }
}

/// A profile `seam` whose driver is named `key` (the fake MySQL driver's when `mysql`), its
/// first tab bound to it, not connected.
fn seam_profile(key: &str, mysql: bool) -> Harness {
    let p = ConnectionConfig { name: "seam".into(), driver: key.into(), ..ConnectionConfig::test_db() };
    let cfg = Config { connections: vec![p], ..Config::default() };
    let h = Harness::with_driver(&cfg, Lang::En, driver(mysql));
    assert_eq!(h.app.tab().profile, Some(id(&h, "seam")), "the first tab is on it");
    h
}

/// Connect profile `seam` from the explorer: its metadata session opens (it has said nothing
/// yet).
fn open_meta(h: &mut Harness) {
    h.explore("seam");
    h.key(KeyCode::Enter);
    let pid = id(h, "seam");
    assert!(h.app.conns.get(pid).is_some_and(|c| c.meta.is_some()), "the metadata session opens");
}

/// The rules answer each tab the language it reads its text in today: without a profile, on a
/// profile no driver serves, before a session said anything, once the profile's session and
/// then the tab's said their server and language (MySQL, MariaDB, PostgreSQL), and again after a
/// reconnect, which drops what the old sessions found and keeps the last language said.
#[test]
fn rules_answer_todays_language_before_and_after_connect() {
    use datarig_core::engine::{Expected, Rules, expected};
    let pg = Language::Sql(Dialect::Postgres);
    let default = mysql(MySqlMode::default());
    // No profile: the tab bound to none, its language pushed again (as a new registry does).
    let mut h = seam_profile("mysql", true);
    let tab = h.app.tab().id;
    h.app.tabs.bind(tab, None);
    let h = h.with_fake_driver();
    assert_eq!(h.app.tab().profile, None);
    assert_eq!(rules_of(&h, 0), Rules::Unbound);
    assert_eq!(rules_of(&h, 0).language(), pg);
    // A driver name no engine (and no driver) serves.
    let h = seam_profile("redis", true);
    let unknown = Rules::Expected { expected: Expected::Unknown("redis".into()), last: None, driver: pg };
    assert_eq!(rules_of(&h, 0), unknown);
    for engine in ENGINES {
        let mut h = seam_profile(engine.driver(), engine.mysql_driver());
        let pid = id(&h, "seam");
        let driver_lang = if engine.mysql_driver() { default } else { pg };
        let said = engine.said().unwrap_or(pg);
        // Never connected: the driver's language.
        let never = Rules::Expected { expected: expected(engine.driver()), last: None, driver: driver_lang };
        assert_eq!(rules_of(&h, 0), never, "{engine:?}");
        // The profile's metadata session said its server and language.
        open_meta(&mut h);
        let meta = facts_at(engine, v(8, 4, 6));
        h.meta_db("seam", DbEvent::Server(meta.clone()));
        for ev in connect_words(engine, false) {
            h.meta_db("seam", ev);
        }
        assert_eq!(rules_of(&h, 0), Rules::Server { facts: meta.clone(), language: said }, "{engine:?}");
        // The tab's own session said its server, then a sql mode of its own (MySQL family).
        h.app.focus = Focus::Editor;
        run_all(&mut h, "SELECT 1");
        let own = facts_at(engine, v(8, 4, 7));
        h.tab_db(0, DbEvent::Server(own.clone()));
        assert_eq!(rules_facts(&h, 0), Some(own.clone()), "{engine:?}: the tab's session's");
        let mine = match engine {
            Engine::Postgres => None,
            Engine::MySql => Some(mysql(ansi_quotes())),
            Engine::MariaDb => Some(maria(ansi_quotes())),
        };
        if let Some(lang) = mine {
            h.tab_db(0, DbEvent::Language(lang));
        }
        done(&mut h);
        let mine = mine.unwrap_or(pg);
        assert_eq!(rules_of(&h, 0), Rules::Server { facts: own, language: mine }, "{engine:?}");
        // A reconnect: nothing the old sessions found is used; the last language said is kept.
        h.app.dispatch(Action::ReconnectCurrent);
        if h.overlay_kind().is_some() {
            h.keys("y");
        }
        // The tab keeps the language its session said last, as the profile does (the tab
        // session's first word was the profile's).
        let last = engine.mysql_driver().then_some(mine);
        let reconnecting = Rules::Expected { expected: expected(engine.driver()), last, driver: driver_lang };
        assert_eq!(rules_of(&h, 0), reconnecting, "{engine:?}");
        let again = facts_at(engine, v(8, 4, 8));
        h.meta_db("seam", DbEvent::Server(again.clone()));
        h.meta_db("seam", DbEvent::Connected);
        assert_eq!(rules_of(&h, 0), Rules::Server { facts: again, language: last.unwrap_or(driver_lang) });
        assert!(h.app.conns.get(pid).is_some_and(|c| c.connected), "{engine:?}");
    }
}

/// What a session found its server to be is used only while that session is open: a lost tab
/// session drops its facts (the profile's metadata session's answer again), a new tab session
/// counts only once it says its own, a late word of an older session is dropped, a lost
/// metadata session and a disconnect drop the profile's, an older attempt's word after a
/// reconnect is dropped, and facts of an engine the profile no longer expects are not used.
#[test]
fn facts_are_dropped_with_the_connection() {
    let mut h = seam_profile("mysql", true);
    let pid = id(&h, "seam");
    open_meta(&mut h);
    let meta = facts_at(Engine::MySql, v(8, 4, 6));
    h.meta_db("seam", DbEvent::Server(meta.clone()));
    h.meta_db("seam", DbEvent::Connected);
    assert_eq!(rules_facts(&h, 0), Some(meta.clone()));
    h.app.focus = Focus::Editor;
    run_all(&mut h, "SELECT 1");
    let first = facts_at(Engine::MySql, v(8, 4, 7));
    h.tab_db(0, DbEvent::Server(first.clone()));
    h.tab_db(0, DbEvent::Language(mysql(MySqlMode::default())));
    done(&mut h);
    assert_eq!(rules_facts(&h, 0), Some(first));
    let old = h.app.tab().exec.generation;
    // The tab's session is lost: its facts go with it.
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    assert_eq!(h.app.tab().exec.facts, None);
    assert_eq!(rules_facts(&h, 0), Some(meta.clone()), "the profile's session's");
    // A new session: the profile's until it says its own; an older session's late word is
    // dropped.
    run_all(&mut h, "SELECT 1");
    assert_ne!(h.app.tab().exec.generation, old);
    assert_eq!(rules_facts(&h, 0), Some(meta.clone()));
    let late = facts_at(Engine::MySql, v(8, 0, 36));
    let target = datarig_tui::app::EventTarget::Tab(h.app.tab().id);
    h.app.on_app_event(AppEvent::Db { target, generation: old, ev: DbEvent::Server(late) });
    assert_eq!(rules_facts(&h, 0), Some(meta.clone()), "an older session's word");
    let second = facts_at(Engine::MySql, v(8, 4, 8));
    h.tab_db(0, DbEvent::Server(second.clone()));
    done(&mut h);
    assert_eq!(rules_facts(&h, 0), Some(second.clone()));
    // The profile no longer expects the engine: no facts of it are used.
    h.app.profiles[0].driver = "postgres".into();
    assert_eq!(rules_facts(&h, 0), None);
    h.app.profiles[0].driver = "mysql".into();
    // The metadata session is lost: the profile's facts go; the tab's open session keeps its own.
    h.meta_db("seam", DbEvent::Lost { error: DbError::Closed });
    assert_eq!(h.app.conns.get(pid).and_then(|c| c.facts.clone()), None);
    assert_eq!(rules_facts(&h, 0), Some(second));
    // A disconnect drops everything the sessions found; the language said is kept.
    h.app.dispatch(Action::DisconnectCurrent);
    if h.overlay_kind().is_some() {
        h.keys("y");
    }
    assert_eq!(h.app.conns.state(pid), NodeState::Disconnected);
    assert_eq!(rules_facts(&h, 0), None);
    assert_eq!(h.app.conns.get(pid).and_then(|c| c.language), Some(mysql(MySqlMode::default())));
    // A reconnect: the older attempt's late word is dropped, the new attempt's counts.
    let attempt = h.app.conns.get(pid).map_or(0, |c| c.generation);
    h.app.dispatch(Action::ReconnectCurrent);
    if h.overlay_kind().is_some() {
        h.keys("y");
    }
    let target = datarig_tui::app::EventTarget::Meta(pid);
    let stale = DbEvent::Server(facts_at(Engine::MySql, v(8, 0, 36)));
    h.app.on_app_event(AppEvent::Db { target, generation: attempt, ev: stale });
    assert_eq!(rules_facts(&h, 0), None, "an older attempt's word");
    let fresh = facts_at(Engine::MySql, v(8, 4, 9));
    h.meta_db("seam", DbEvent::Server(fresh.clone()));
    assert_eq!(rules_facts(&h, 0), Some(fresh));
}

// ── A text the driver refuses to send ───────────────────────────────────────────────────────

/// A run of several statements the MySQL driver refused because its session's server reports
/// disagree on dollar quotes (nothing of it was sent) is shown with why, in English and Korean,
/// and the run ends there: no statement of it is sent again, and the tab can run again.
#[test]
fn a_dollar_quotes_unknown_refusal_is_worded_and_nothing_else_runs() {
    for lang in [Lang::En, Lang::Ko] {
        let p = ConnectionConfig { name: "local-my".into(), driver: "mysql".into(), ..ConnectionConfig::test_db() };
        let cfg = Config { connections: vec![p], ..Config::default() };
        let mut h = Harness::with_driver(&cfg, lang, driver(true));
        h.db(DbEvent::Connected);
        h.app.focus = Focus::Editor;
        run_all(&mut h, "SELECT $$a; SELECT 2");
        let sent = executes(&mut h);
        assert_eq!(sent.len(), 1, "{sent:?}");
        let id = h.app.tab().exec.query_id;
        h.tab_db(0, DbEvent::Failed { id, error: DbError::DollarQuotesUnknown, cancelled: false });
        assert!(h.app.tab().exec.running.is_none(), "{lang:?}: the run ended");
        let want = h.app.i18n.label(Label::EngineDollarQuotesUnknown).to_string();
        let shown = h.status(400, 45);
        let start: String = want.chars().take(30).collect();
        assert!(shown.contains(&start), "{lang:?}: {shown}");
        assert!(executes(&mut h).is_empty(), "{lang:?}: nothing else runs");
        run_all(&mut h, "SELECT 1");
        assert_eq!(executes(&mut h), [["SELECT 1"]], "{lang:?}: the tab runs again");
    }
}

// ── A session whose greeting and VERSION() name two engines ─────────────────────────────────

/// The facts a MySQL-driver session says whose greeting names `greeting` (MySQL 8.4.6 or
/// MariaDB 11.4.13) and whose `VERSION()` line names the other engine (a proxy wrote the
/// greeting).
fn disagreeing_facts(greeting: Engine) -> DbEvent {
    use datarig_core::engine::{EngineFacts, EngineId, Fact, MyFacts, ServerFacts, Version, Why};
    let (detail, line) = match greeting {
        Engine::MySql => {
            (EngineFacts::MariaDb(MyFacts::new(Version::new(8, 4, 6), EngineId::MySql)), "11.4.13-MariaDB")
        }
        Engine::MariaDb | Engine::Postgres => {
            (EngineFacts::MySql(MyFacts::new(Version::new(11, 4, 13), EngineId::MariaDb)), "8.4.6")
        }
    };
    DbEvent::Server(Arc::new(ServerFacts {
        version: Fact::Unknown(Why::EnginesDisagree),
        version_text: Some(line.into()),
        provider: Fact::Unknown(Why::NotChecked),
        topology: Fact::Unknown(Why::EnginesDisagree),
        detail,
    }))
}

// ── MariaDB's own executable comments (`/*M! … */`) ─────────────────────────────────────────

/// A run holding `/*M! … */` was refused before anything was sent: no question, a reason on the
/// tab.
fn refused_m_comment(h: &mut Harness, what: &str) {
    assert!(h.app.overlays.run_confirm().is_none(), "{what}: not asked about");
    assert!(executes(h).is_empty(), "{what}: nothing sent");
    assert_eq!(tab_status(h), Some(Msg::Label(Label::EngineCommentUnsupported)), "{what}: said why");
}

/// On a MariaDB session, and on one whose reports name both engines (either greeting), text
/// holding MariaDB's own executable comment is refused before it is sent, from the selection and
/// from the statement under the cursor, a whole script with it.
#[test]
fn a_mariadb_only_comment_is_refused_where_mariadb_may_run_it() {
    type Session = (&'static str, fn() -> DbEvent, Language);
    let sessions: [Session; 3] = [
        ("MariaDB", || facts_of(Engine::MariaDb), mariadb()),
        ("MySQL greeting, MariaDB line", || disagreeing_facts(Engine::MySql), mysql(MySqlMode::default())),
        ("MariaDB greeting, MySQL line", || disagreeing_facts(Engine::MariaDb), mariadb()),
    ];
    for (what, facts, lang) in sessions {
        for sql in ["SELECT 1 /*M! , 2 */", "/*M!100500 SET @a = 1 */", "SELECT 1; SELECT 2 /*M! , 3 */"] {
            let mut h = two_engines(false, facts(), lang);
            run_all(&mut h, sql);
            refused_m_comment(&mut h, &format!("{what}: selection: {sql}"));
            let mut h = two_engines(false, facts(), lang);
            // The cursor on the statement with the comment (the last one).
            edit(&mut h, sql);
            h.keys("gg$");
            h.ctrl('e');
            refused_m_comment(&mut h, &format!("{what}: cursor: {sql}"));
        }
        // The statement under the cursor without one runs: the other is not part of the run.
        let mut h = two_engines(false, facts(), lang);
        edit(&mut h, "SELECT 1; SELECT 2 /*M! , 3 */");
        h.keys("gg");
        h.ctrl('e');
        assert_eq!(executes(&mut h), [["SELECT 1"]], "{what}");
    }
}

/// A run queued before its profile connects, on a profile with the MySQL driver key whose server
/// turns out to be MariaDB, is refused once the session says so, after the user's yes to its
/// executable comment before connect: nothing of it is sent.
#[test]
fn a_queued_run_with_a_mariadb_only_comment_is_refused_once_the_server_is_mariadb() {
    let sql = "SELECT 1 /*M! , 2 */";
    let mut h = connecting_two();
    run_all(&mut h, sql);
    assert!(h.app.overlays.run_confirm().is_some(), "asked about before connect");
    h.keys("y");
    assert!(h.app.is_queued(h.app.tab().id), "queued until the profile connects");
    h.meta_db("two", facts_of(Engine::MariaDb));
    h.meta_db("two", DbEvent::Language(mariadb()));
    h.meta_db("two", DbEvent::Connected);
    refused_m_comment(&mut h, "queued");
}

/// Where only MySQL runs the text (reports that agree on MySQL), `/*M! … */` is a plain comment
/// and the text is asked about and runs as before; MySQL's own `/*! … */` is asked about and runs
/// on MariaDB as before.
#[test]
fn mariadb_only_comments_on_mysql_and_mysql_comments_on_mariadb_are_unchanged() {
    for (what, facts, lang, sql) in [
        ("MySQL", facts_of(Engine::MySql), mysql(MySqlMode::default()), "SELECT 1 /*M! , 2 */"),
        ("MariaDB", facts_of(Engine::MariaDb), mariadb(), "SELECT 1 /*! , 2 */"),
    ] {
        let mut h = two_engines(false, facts, lang);
        run_all(&mut h, sql);
        assert!(h.app.overlays.run_confirm().is_some(), "{what}: asked about, as before");
        h.keys("y");
        assert_eq!(executes(&mut h), [[sql]], "{what}: it runs");
    }
}

/// A connected MySQL-driver profile `two` (read-only when `read_only`) whose metadata session
/// said `lang`, and whose first tab's session said `facts`, then `lang`.
fn two_engines(read_only: bool, facts: DbEvent, lang: Language) -> Harness {
    let mut p = ConnectionConfig { name: "two".into(), driver: "mysql".into(), ..ConnectionConfig::test_db() };
    if read_only {
        p.policy = Some("prod".into());
    }
    let mut cfg = Config { connections: vec![p], ..Config::default() };
    cfg.policies.insert("prod", Policy { read_only: true, ..Policy::default() });
    let mut h = Harness::with_driver(&cfg, Lang::En, driver(true));
    h.meta_db("two", DbEvent::Language(lang));
    h.db(DbEvent::Connected);
    h.app.focus = Focus::Editor;
    run_all(&mut h, "SELECT 1");
    h.tab_db(0, facts);
    h.tab_db(0, DbEvent::Language(lang));
    done(&mut h);
    h.sent();
    h
}

/// Statements one engine refuses on a read-only profile and the other runs, each behind the
/// greeting of the engine that runs it today (its language).
fn one_engine_refuses() -> Vec<(Engine, &'static str)> {
    vec![
        (Engine::MySql, "SELECT BIN_TO_UUID(a) FROM t"),
        (Engine::MySql, "SELECT POINT()"),
        (Engine::MySql, "SELECT LATERAL(1)"),
        (Engine::MySql, "SELECT OF(1)"),
        (Engine::MariaDb, "SELECT SYSDATE ()"),
        (Engine::MariaDb, "SELECT `DATABASE`()"),
        (Engine::MariaDb, "SELECT `COALESCE`(a, 1) FROM t"),
    ]
}

/// On a read-only profile, a session whose greeting names one engine and whose `VERSION()` line
/// the other refuses what either engine refuses, before anything is sent: behind a MySQL
/// greeting the calls MariaDB does not read as built-ins, behind a MariaDB greeting those MySQL
/// does not.
#[test]
fn a_disagreeing_session_refuses_on_read_only_what_either_engine_refuses() {
    for (greeting, sql) in one_engine_refuses() {
        let lang = greeting.said().expect("a MySQL-family language");
        let mut h = two_engines(true, disagreeing_facts(greeting), lang);
        run_all(&mut h, sql);
        assert!(executes(&mut h).is_empty(), "{greeting:?} greeting: {sql}: nothing sent");
        assert!(matches!(tab_status(&h), Some(Msg::SafetyReadOnlyBlocked { .. })), "{sql}: {:?}", tab_status(&h));
    }
}

/// A session whose two reports name one engine keeps that engine's rules: each statement the
/// other engine refuses runs on a read-only profile, as before.
#[test]
fn an_agreeing_session_keeps_one_engines_rules() {
    for (engine, sql) in one_engine_refuses() {
        let lang = engine.said().expect("a MySQL-family language");
        let mut h = two_engines(true, facts_of(engine), lang);
        run_all(&mut h, sql);
        assert_eq!(executes(&mut h), [[sql]], "{engine:?}: it runs");
    }
}

/// On a profile that asks before a dangerous statement, a session whose reports name two
/// engines asks when either engine's rules ask: `DO` of a call one engine reads as the user's
/// code (MariaDB has no `BIN_TO_UUID`; MySQL reads a quoted `DATABASE` as a stored function).
/// Nothing is sent before the answer. A session whose reports agree runs it as before.
#[test]
fn a_disagreeing_session_asks_before_what_either_engine_asks_for() {
    for (greeting, sql) in [(Engine::MySql, "DO BIN_TO_UUID(a)"), (Engine::MariaDb, "DO `DATABASE`()")] {
        let lang = greeting.said().expect("a MySQL-family language");
        let mut h = two_engines(false, disagreeing_facts(greeting), lang);
        run_all(&mut h, sql);
        assert!(executes(&mut h).is_empty(), "{greeting:?} greeting: {sql}: asked first");
        assert!(h.app.overlays.run_confirm().is_some(), "{greeting:?} greeting: {sql}: the confirm is open");
        let mut h = two_engines(false, facts_of(greeting), lang);
        run_all(&mut h, sql);
        assert_eq!(executes(&mut h), [[sql]], "{greeting:?}, agreeing: it runs");
    }
}

/// A tab that reads MySQL's language before its session said anything, whose session then says
/// facts whose two reports name different engines and the same language, gets a classifier
/// that judges on both engines: the language alone does not decide when it is rebuilt.
#[test]
fn the_same_language_with_disagreeing_facts_rebuilds_the_classifier() {
    use datarig_core::sql::risk::Classifier;
    let lang = mysql(MySqlMode::default());
    let h = two_engines(false, disagreeing_facts(Engine::MySql), lang);
    let tab = h.app.tab().id;
    assert_eq!(h.app.tab_language(tab), lang, "the same language");
    let prepared = h.app.tab().exec.prepared.clone();
    assert_eq!(prepared, h.app.rules(tab).classifier(), "the tab's rules' classifier");
    assert_ne!(prepared, Classifier::new(lang), "not the greeting's engine's alone");
    let risk = prepared.clone().classify("SELECT BIN_TO_UUID(a) FROM t");
    assert!(risk.read_only().is_err(), "judged on MariaDB's rules too: {risk:?}");
}

/// PostgreSQL's classifier keeps the statements its session prepared when the session's facts
/// arrive: they rebuild nothing there.
#[test]
fn a_pg_tab_keeps_its_prepared_statements_when_its_facts_arrive() {
    let mut h = Harness::connected(Lang::En);
    let knows = |h: &Harness, name: &str| h.app.tab().exec.prepared.knows(name);
    run_all(&mut h, "PREPARE ok AS SELECT 1");
    done(&mut h);
    assert!(knows(&h, "ok"));
    h.tab_db(0, facts_of(Engine::Postgres));
    assert!(knows(&h, "ok"), "kept through the facts");
    let tab = h.app.tab().id;
    let meta_facts = facts_at(Engine::Postgres, v(17, 2, 0));
    let name = h.app.profiles.first().expect("a profile").name.clone();
    h.meta_db(&name, DbEvent::Server(meta_facts));
    assert!(knows(&h, "ok"), "kept through the profile's facts");
    assert_eq!(h.app.rules(tab).language(), Language::Sql(Dialect::Postgres));
}

// ── The engines a tab's text is judged on after its session closed ─────────────────────────

/// The idle limit of the tabs' and the profile's sessions in these tests.
const IDLE: Duration = Duration::from_secs(300);

/// A connected MySQL-driver profile `two` (read-only when `read_only`) whose sessions close
/// after [`IDLE`] unused: its metadata session said `meta`, then `lang`; the first tab's
/// session said `tab`, then `lang`, and ran a statement.
fn idle_two(read_only: bool, meta: DbEvent, tab: DbEvent, lang: Language) -> Harness {
    let mut p = ConnectionConfig { name: "two".into(), driver: "mysql".into(), ..ConnectionConfig::test_db() };
    if read_only {
        p.policy = Some("prod".into());
    }
    let mut cfg = Config { connections: vec![p], ..Config::default() };
    cfg.prefs.idle_disconnect = Some(IDLE);
    cfg.policies.insert("prod", Policy { read_only: true, ..Policy::default() });
    let mut h = Harness::with_driver(&cfg, Lang::En, driver(true));
    h.meta_db("two", meta);
    h.meta_db("two", DbEvent::Language(lang));
    h.db(DbEvent::Connected);
    h.app.focus = Focus::Editor;
    run_all(&mut h, "SELECT 1");
    h.tab_db(0, tab);
    h.tab_db(0, DbEvent::Language(lang));
    done(&mut h);
    h.sent();
    h
}

/// The tab's session closes for being idle while the profile's metadata session stays open.
fn close_tab_idle(h: &mut Harness) {
    h.advance(Duration::from_secs(200));
    h.db(DbEvent::Schemas(Ok(vec!["shop".into()])));
    h.advance(Duration::from_secs(100));
    assert!(h.app.tab().exec.session.is_none(), "the tab's session closed");
    assert!(h.app.conns.get(id(h, "two")).is_some_and(|c| c.meta.is_some()), "the profile's is open");
}

/// A tab whose own session said its reports agree (judged on one engine) and closed for being
/// idle is judged, with no session, on the profile's: its metadata session's reports name two
/// engines, so a dangerous call either engine asks about asks before it runs, and nothing is
/// sent before the answer.
#[test]
fn a_closed_sessions_engines_do_not_judge_the_next_run() {
    let lang = mysql(MySqlMode::default());
    let mut h = idle_two(false, disagreeing_facts(Engine::MySql), facts_of(Engine::MySql), lang);
    close_tab_idle(&mut h);
    run_all(&mut h, "DO BIN_TO_UUID(a)");
    assert!(h.app.overlays.run_confirm().is_some(), "asked: the profile's facts name two engines");
    assert!(executes(&mut h).is_empty(), "nothing sent before the answer");
}

/// The other way round: a tab whose own session's reports named two engines and closed for
/// being idle, on a profile whose metadata session's reports agree. Within the profile's
/// connection the engines a statement is judged on never shrink: the refusal stays while that
/// connection lasts. Once the profile closed and connected again (a new connection, whose
/// reports agree), no refusal of the closed one stays.
#[test]
fn a_closed_sessions_refusal_does_not_stay() {
    let lang = mysql(MySqlMode::default());
    let sql = "SELECT BIN_TO_UUID(a) FROM t";
    let mut h = idle_two(true, facts_of(Engine::MySql), disagreeing_facts(Engine::MySql), lang);
    run_all(&mut h, sql);
    assert!(executes(&mut h).is_empty(), "refused while the session that named two engines is open");
    close_tab_idle(&mut h);
    run_all(&mut h, sql);
    assert!(executes(&mut h).is_empty(), "still refused within the same connection");
    h.advance(IDLE);
    assert!(h.app.conns.get(id(&h, "two")).is_none_or(|c| c.meta.is_none()), "the profile closed");
    h.app.focus = Focus::Editor;
    run_all(&mut h, sql);
    meta_says(&mut h, facts_of(Engine::MySql));
    assert_eq!(executes(&mut h), [[sql]], "it runs on the new connection: {:?}", tab_status(&h));
}

/// A tab whose own session's reports named two engines and closed for being idle, on a profile
/// whose metadata session's reports agree, keeps being judged on both engines while the
/// profile's connection lasts: a dangerous call either engine asks about asks before it runs.
#[test]
fn a_closed_sessions_two_engines_keep_judging_the_profile() {
    let lang = mysql(MySqlMode::default());
    let mut h = idle_two(false, facts_of(Engine::MySql), disagreeing_facts(Engine::MySql), lang);
    close_tab_idle(&mut h);
    run_all(&mut h, "DO BIN_TO_UUID(a)");
    assert!(executes(&mut h).is_empty(), "nothing sent before the answer");
    assert!(h.app.overlays.run_confirm().is_some(), "asked: the profile's connection named two engines");
}

/// A run queued while the profile connects again (its sessions closed for being idle) is
/// judged before connect on one engine; when the profile's new metadata session says its
/// reports name two engines, a dangerous call either engine asks about asks before it is
/// sent, not after.
#[test]
fn a_queued_run_is_asked_about_again_when_the_reconnected_server_names_two_engines() {
    let lang = mysql(MySqlMode::default());
    let mut h = idle_two(false, facts_of(Engine::MySql), facts_of(Engine::MySql), lang);
    close_tab_idle(&mut h);
    h.advance(IDLE);
    assert!(h.app.conns.get(id(&h, "two")).is_none_or(|c| c.meta.is_none()), "the profile closed");
    h.app.focus = Focus::Editor;
    run_all(&mut h, "DO BIN_TO_UUID(a)");
    assert!(h.app.overlays.run_confirm().is_none(), "one engine before connect: not asked");
    assert!(h.app.is_queued(h.app.tab().id), "queued until the profile connects");
    h.meta_db("two", disagreeing_facts(Engine::MySql));
    h.meta_db("two", DbEvent::Language(lang));
    h.meta_db("two", DbEvent::Connected);
    assert!(executes(&mut h).is_empty(), "not sent once the server named two engines");
    assert!(h.app.overlays.run_confirm().is_some(), "asked before it runs");
}

/// A MySQL-driver profile `two` that connected once and was disconnected: the next run
/// connects it again and waits for it, the editor focused.
fn connecting_two() -> Harness {
    let p = ConnectionConfig { name: "two".into(), driver: "mysql".into(), ..ConnectionConfig::test_db() };
    let cfg = Config { connections: vec![p], ..Config::default() };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver(true));
    h.db(DbEvent::Connected);
    h.app.dispatch(Action::DisconnectCurrent);
    h.app.focus = Focus::Editor;
    h.sent();
    h
}

/// The profile's metadata session says `facts`, MySQL's language, then that it connected.
fn meta_says(h: &mut Harness, facts: DbEvent) {
    h.meta_db("two", facts);
    h.meta_db("two", DbEvent::Language(mysql(MySqlMode::default())));
    h.meta_db("two", DbEvent::Connected);
}

/// A statement queued before the profile connects is judged again when it is sent: once the
/// server's reports name two engines, `EXPLAIN ANALYZE` of a call MariaDB runs as the user's
/// code asks before it runs (it is not sent), where before connect, on MySQL's rules alone, it
/// did not ask.
#[test]
fn a_run_queued_before_connect_is_judged_again_when_the_server_names_two_engines() {
    let sql = "EXPLAIN ANALYZE SELECT BIN_TO_UUID(a) FROM t";
    let mut h = connecting_two();
    run_all(&mut h, sql);
    assert!(h.app.overlays.run_confirm().is_none(), "one engine before connect: not asked");
    assert!(h.app.is_queued(h.app.tab().id), "queued until the profile connects");
    meta_says(&mut h, disagreeing_facts(Engine::MySql));
    assert!(executes(&mut h).is_empty(), "not sent once the server named two engines");
    assert!(h.app.overlays.run_confirm().is_some(), "asked before it runs");
}

/// Judged again under the same rules, a waiting run gets the same answer: an ordinary
/// statement queued before connect runs once connected without a question, and a dangerous
/// one the user confirmed before connect is not asked about again.
#[test]
fn a_waiting_run_judged_again_under_the_same_rules_asks_nothing_more() {
    let mut h = connecting_two();
    run_all(&mut h, "SELECT 1");
    assert!(h.app.is_queued(h.app.tab().id));
    meta_says(&mut h, facts_of(Engine::MySql));
    assert!(h.app.overlays.run_confirm().is_none(), "no question");
    assert_eq!(executes(&mut h), [["SELECT 1"]]);
    let mut h = connecting_two();
    run_all(&mut h, "DROP TABLE t");
    assert!(h.app.overlays.run_confirm().is_some(), "asked before connect");
    h.keys("y");
    assert!(h.app.is_queued(h.app.tab().id), "queued once confirmed");
    meta_says(&mut h, facts_of(Engine::MySql));
    assert!(h.app.overlays.run_confirm().is_none(), "not asked again");
    assert_eq!(executes(&mut h), [["DROP TABLE t"]]);
}

/// A run whose question was asked under one engine's rules, answered after the tab's session
/// said its server's reports name two engines, asks again for what only the new rules ask
/// about, and nothing is sent before that answer.
#[test]
fn a_question_answered_after_the_rules_changed_is_asked_again_for_what_is_new() {
    let p = ConnectionConfig { name: "two".into(), driver: "mysql".into(), ..ConnectionConfig::test_db() };
    let cfg = Config { connections: vec![p], ..Config::default() };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver(true));
    h.meta_db("two", facts_of(Engine::MySql));
    h.meta_db("two", DbEvent::Language(mysql(MySqlMode::default())));
    h.db(DbEvent::Connected);
    h.app.focus = Focus::Editor;
    // The tab's session opens and answers before it says what its server is.
    run_all(&mut h, "SELECT 1");
    done(&mut h);
    h.sent();
    run_all(&mut h, "DROP TABLE t;\nDO BIN_TO_UUID(a);");
    let asked = |h: &Harness| -> Vec<usize> {
        h.app.overlays.run_confirm().map(|c| c.items.iter().map(|d| d.index).collect()).unwrap_or_default()
    };
    assert_eq!(asked(&h), [0], "MySQL's rules ask about the DROP only");
    h.tab_db(0, disagreeing_facts(Engine::MySql));
    h.tab_db(0, DbEvent::Language(mysql(MySqlMode::default())));
    h.keys("y");
    assert!(executes(&mut h).is_empty(), "nothing sent");
    assert_eq!(asked(&h), [0, 1], "asked again, the DO too");
    h.keys("y");
    assert_eq!(executes(&mut h), [["DROP TABLE t", "DO BIN_TO_UUID(a)"]]);
}

/// An approval covers what the user saw: a run whose `DROP` was confirmed under MariaDB's rules,
/// queued while its profile connects again, asks again once the server is MySQL, where its
/// `DO` of a quoted `DATABASE` (a stored function to MySQL) asks too, and nothing is sent.
#[test]
fn an_approval_under_one_engine_does_not_cover_what_another_asks_about() {
    // The profile's server said it is MariaDB, then the profile was disconnected: its last
    // language is MariaDB's.
    let p = ConnectionConfig { name: "two".into(), driver: "mysql".into(), ..ConnectionConfig::test_db() };
    let cfg = Config { connections: vec![p], ..Config::default() };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver(true));
    h.meta_db("two", facts_of(Engine::MariaDb));
    h.meta_db("two", DbEvent::Language(mariadb()));
    h.db(DbEvent::Connected);
    h.app.dispatch(Action::DisconnectCurrent);
    h.app.focus = Focus::Editor;
    h.sent();
    assert_eq!(h.app.tab_language(h.app.tab().id), mariadb());
    let asked = |h: &Harness| -> Vec<usize> {
        h.app.overlays.run_confirm().map(|c| c.items.iter().map(|d| d.index).collect()).unwrap_or_default()
    };
    run_all(&mut h, "DROP TABLE t;\nDO `DATABASE`();");
    assert_eq!(asked(&h), [0], "MariaDB's rules ask about the DROP only");
    h.keys("y");
    assert!(h.app.is_queued(h.app.tab().id), "queued once confirmed");
    meta_says(&mut h, facts_of(Engine::MySql));
    assert!(executes(&mut h).is_empty(), "nothing sent");
    assert_eq!(asked(&h), [0, 1], "asked again: MySQL asks about the DO");
}

/// Questions are asked one at a time: two tabs whose runs waited for their profile to connect
/// (its password still being read), each asking once the server names two engines, are asked
/// about in turn; each yes sends its own run, and no question replaces another.
#[tokio::test(flavor = "multi_thread")]
async fn waiting_runs_of_two_tabs_are_asked_about_in_turn() {
    let (mut h, mut rx) = slow_harness(Engine::MySql, false);
    let resolved = console_on_slow(&mut h, &mut rx).await;
    h.sent();
    run_all(&mut h, "DO BIN_TO_UUID(a)");
    assert!(h.app.is_queued(h.app.tab().id));
    h.app.dispatch(Action::NewTab);
    assert_eq!(h.app.tab().profile, Some(id(&h, "slow")));
    h.app.focus = Focus::Editor;
    run_all(&mut h, "DO BIN_TO_UUID(b)");
    assert!(h.app.is_queued(h.app.tab().id), "{:?}", tab_status(&h));
    h.app.on_app_event(resolved);
    h.meta_db("slow", disagreeing_facts(Engine::MySql));
    h.meta_db("slow", DbEvent::Language(mysql(MySqlMode::default())));
    h.meta_db("slow", DbEvent::Connected);
    assert!(executes(&mut h).is_empty(), "nothing sent before the answers");
    let first = h.app.overlays.run_confirm().map(|c| c.statements.clone()).expect("the first question");
    assert_eq!(first, ["DO BIN_TO_UUID(a)"]);
    h.keys("y");
    assert_eq!(executes(&mut h), [["DO BIN_TO_UUID(a)"]]);
    let second = h.app.overlays.run_confirm().map(|c| c.statements.clone()).expect("the second question");
    assert_eq!(second, ["DO BIN_TO_UUID(b)"]);
    h.keys("y");
    assert_eq!(executes(&mut h), [["DO BIN_TO_UUID(b)"]]);
}

/// A question about one profile's waiting run holds no other profile's: a profile that
/// connects while the question is open keeps its run waiting (one question at a time), and it
/// is sent once the question is answered, without a question of its own.
#[tokio::test(flavor = "multi_thread")]
async fn a_question_for_one_profile_holds_another_profiles_run_only_until_it_is_answered() {
    let slow = |name: &str| {
        let mut p = ConnectionConfig {
            name: name.into(),
            driver: "mysql".into(),
            password: String::new(),
            ..ConnectionConfig::test_db()
        };
        p.set_source(PasswordSource::Command("printf datarig".into()));
        p
    };
    let cfg = Config { connections: vec![slow("slow"), slow("other")], ..Config::default() };
    let (h, mut rx) = Harness::started(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal);
    h.driver.mysql.store(true, Ordering::SeqCst);
    let mut h = h.with_fake_driver();
    let resolved = console_on_slow(&mut h, &mut rx).await;
    run_all(&mut h, "DO BIN_TO_UUID(a)");
    assert!(h.app.is_queued(h.app.tab().id));
    h.explore("other");
    h.keys("o");
    let other_resolved = next_event(&mut rx).await;
    h.app.focus = Focus::Editor;
    assert_eq!(h.app.tab().profile, Some(id(&h, "other")));
    let other_tab = h.app.tab().id;
    run_all(&mut h, "SELECT 1");
    assert!(h.app.is_queued(other_tab));
    h.sent();
    h.app.on_app_event(resolved);
    h.meta_db("slow", disagreeing_facts(Engine::MySql));
    h.meta_db("slow", DbEvent::Language(mysql(MySqlMode::default())));
    h.meta_db("slow", DbEvent::Connected);
    assert!(h.app.overlays.run_confirm().is_some(), "slow's run asks");
    h.app.on_app_event(other_resolved);
    h.meta_db("other", facts_of(Engine::MySql));
    h.meta_db("other", DbEvent::Language(mysql(MySqlMode::default())));
    h.meta_db("other", DbEvent::Connected);
    assert!(executes(&mut h).is_empty(), "nothing sent while the question is open");
    h.keys("n");
    assert_eq!(executes(&mut h), [["SELECT 1"]], "other's run is sent once the question is answered");
    assert!(!h.app.is_queued(other_tab));
}

/// The engines each command of user text sent to session `i` was judged on, in order.
fn judged_sent(h: &mut Harness, i: usize) -> Vec<Option<datarig_core::sql::risk::mysql::Judged>> {
    h.sent_to(i);
    h.driver.sessions.lock().unwrap()[i].judged.clone()
}

/// Every command of user text goes with the engines whose rules it was judged on. The first
/// run on a query session that has not said what its server is goes judged on the profile's
/// (one engine, its metadata session's reports agree); the driver refuses it once the session
/// found two engines, the refusal is worded, and run again it is judged on both: it asks
/// first, and goes with both.
#[test]
fn a_run_judged_before_its_session_named_two_engines_is_refused_and_judged_anew() {
    use datarig_core::sql::risk::mysql::Judged;
    let p = ConnectionConfig { name: "two".into(), driver: "mysql".into(), ..ConnectionConfig::test_db() };
    let cfg = Config { connections: vec![p], ..Config::default() };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver(true));
    meta_says(&mut h, facts_of(Engine::MySql));
    h.app.focus = Focus::Editor;
    let sql = "DO BIN_TO_UUID(a)";
    run_all(&mut h, sql);
    assert!(h.app.overlays.run_confirm().is_none(), "one engine: not asked");
    let query = h.roles().iter().rposition(|r| *r == SessionRole::Query).expect("a query session");
    assert_eq!(judged_sent(&mut h, query), [Some(Judged::Own)]);
    // The session says its server's reports name two engines; the driver refused the run.
    h.tab_db(0, disagreeing_facts(Engine::MySql));
    h.tab_db(0, DbEvent::Language(mysql(MySqlMode::default())));
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::Failed { id, error: DbError::ReportsChanged, cancelled: false });
    assert!(h.app.tab().exec.running.is_none(), "the run ended");
    let want = h.app.i18n.label(Label::EngineReportsChanged).to_string();
    let shown = h.status(400, 45);
    let start: String = want.chars().take(30).collect();
    assert!(shown.contains(&start), "{shown}");
    // Run again: judged on both, it asks, and goes with both.
    run_all(&mut h, sql);
    assert!(h.app.overlays.run_confirm().is_some(), "asked: two engines");
    h.keys("y");
    assert_eq!(judged_sent(&mut h, query), [Some(Judged::Own), Some(Judged::WithSibling)]);
}

/// On a profile whose reports agree every command of user text goes judged on its one engine,
/// and nothing else changes.
#[test]
fn an_agreeing_profiles_commands_go_judged_on_its_engine() {
    use datarig_core::sql::risk::mysql::Judged;
    let lang = mysql(MySqlMode::default());
    let mut h = two_engines(false, facts_of(Engine::MySql), lang);
    run_all(&mut h, "SELECT 1");
    let query = h.roles().iter().rposition(|r| *r == SessionRole::Query).expect("a query session");
    assert!(judged_sent(&mut h, query).iter().all(|j| *j == Some(Judged::Own)));
}

/// The kinds of session a profile's server may say what it is through.
#[derive(Clone, Copy, Debug)]
enum Kind {
    /// A tab's query session.
    Query,
    /// The profile's metadata session.
    Meta,
    /// A metadata session of the profile in another database.
    Aux,
    /// A Monitor tab's connection.
    Monitor,
}

/// Every kind of session of a profile feeds what the profile's statements are judged on through
/// one path: a server whose reports name two engines, said by any of them, has every tab of the
/// profile judged on both from then on (`DO BIN_TO_UUID(a)` asks first in the profile's
/// console), and reports that agree, said by any of them, change nothing (it runs).
#[test]
fn every_kind_of_session_feeds_the_profiles_engines_alike() {
    let mut wrong = Vec::new();
    for kind in [Kind::Query, Kind::Meta, Kind::Aux, Kind::Monitor] {
        for disagree in [true, false] {
            let case = format!("{kind:?}, reports disagree: {disagree}");
            let p = ConnectionConfig { name: "two".into(), driver: "mysql".into(), ..ConnectionConfig::test_db() };
            let cfg = Config { connections: vec![p], ..Config::default() };
            let mut h = Harness::with_driver(&cfg, Lang::En, driver(true));
            h.driver.monitor.store(true, Ordering::SeqCst);
            meta_says(&mut h, facts_of(Engine::MySql));
            h.app.focus = Focus::Editor;
            run_all(&mut h, "SELECT 1");
            h.tab_db(0, facts_of(Engine::MySql));
            h.tab_db(0, DbEvent::Language(mysql(MySqlMode::default())));
            done(&mut h);
            let facts = if disagree { disagreeing_facts(Engine::MySql) } else { facts_of(Engine::MySql) };
            match kind {
                Kind::Query => h.tab_db(0, facts),
                Kind::Meta => h.meta_db("two", facts),
                Kind::Aux => {
                    let pid = id(&h, "two");
                    let now = h.clock.now();
                    let aux = h.app.conns.add_aux(pid, "other", now);
                    h.app.on_app_event(AppEvent::Db {
                        target: datarig_tui::app::EventTarget::Aux(aux),
                        generation: aux,
                        ev: facts,
                    });
                }
                Kind::Monitor => {
                    h.command("monitor");
                    h.monitor_db(DbEvent::Connected);
                    h.monitor_db(facts);
                    assert!(h.app.tabs.activate(0), "{case}");
                    h.app.focus = Focus::Editor;
                }
            }
            h.sent();
            run_all(&mut h, "DO BIN_TO_UUID(a)");
            let asked = h.app.overlays.run_confirm().is_some();
            let sent = executes(&mut h);
            let ran = sent == [["DO BIN_TO_UUID(a)"]];
            if asked != disagree || ran == disagree {
                wrong.push(format!("{case}: asked {asked}, sent {sent:?}"));
            }
        }
    }
    assert!(wrong.is_empty(), "not judged alike:\n{}", wrong.join("\n"));
}

// ── A cancel on a server that cannot stop a running statement ──────────────────────────────

/// `performance_schema` off on the server.
fn ps_off() -> datarig_core::engine::Support {
    use datarig_core::engine::{Fact, Feature, Why};
    Fact::No(Why::FeatureOff(Feature::PerformanceSchema))
}

/// What a session of a MariaDB 11.4.13 server reached directly says first, its
/// `performance_schema` as `ps` says, its marker confirmed.
fn mariadb_facts(ps: datarig_core::engine::Support) -> DbEvent {
    mariadb_facts_marked(ps, datarig_core::engine::Fact::Known(()))
}

/// [`mariadb_facts`], its marker as `marker` says.
fn mariadb_facts_marked(ps: datarig_core::engine::Support, marker: datarig_core::engine::Support) -> DbEvent {
    use datarig_core::engine::{EngineFacts, EngineId, Fact, MyFacts, ServerFacts, Topology, Version, Why};
    let v = Version::new(11, 4, 13);
    DbEvent::Server(Arc::new(ServerFacts {
        version: Fact::Known(v),
        version_text: None,
        provider: Fact::Unknown(Why::NotChecked),
        topology: Fact::Known(Topology::Direct),
        detail: EngineFacts::MariaDb(MyFacts { performance_schema: ps, marker, ..MyFacts::new(v, EngineId::MariaDb) }),
    }))
}

/// A MariaDB profile `seam` that holds results open, connected; its first tab runs `sql`, whose query session said
/// `facts` first (`None`: it has said nothing yet), and the run's first statement started.
fn running_on_mariadb(sql: &str, facts: Option<DbEvent>) -> (Harness, u64) {
    let (mut h, id) = sent_on_mariadb(sql, facts, true);
    h.tab_db(0, DbEvent::Started { id, index: 0 });
    (h, id)
}

/// [`running_on_mariadb`], before the driver said the statement started, through the fake
/// MySQL driver (`reads_to_end`: it reads a held result to its end at once) or one that leaves
/// a held result to be paged.
fn sent_on_mariadb(sql: &str, facts: Option<DbEvent>, reads_to_end: bool) -> (Harness, u64) {
    let p = ConnectionConfig {
        name: "seam".into(),
        driver: "mysql".into(),
        policy: Some("hold".into()),
        ..ConnectionConfig::test_db()
    };
    let mut cfg = Config { connections: vec![p], ..Config::default() };
    // Results held open, so the rest of one can be fetched.
    cfg.policies.insert("hold", Policy { paging: datarig_core::driver::PagingMode::Hold, ..Policy::default() });
    let mut h = Harness::with_driver(&cfg, Lang::En, driver(reads_to_end));
    h.db(DbEvent::Language(mariadb()));
    h.db(DbEvent::Explain(None));
    h.db(DbEvent::Edit(None));
    h.db(DbEvent::Connected);
    h.app.focus = Focus::Editor;
    h.sent();
    run_all(&mut h, sql);
    let id = h.app.tab().exec.query_id;
    if let Some(f) = facts {
        h.tab_db(0, f);
    }
    (h, id)
}

/// The notice a cancel gets on a server whose `performance_schema` is off, in words.
const CLOSES: &str = "This server cannot stop a running statement (performance_schema is off on the server): \
                      cancelling a statement that is running closes the connection";

/// The run's Messages that say a cancel closes the connection, in words.
fn closes_notes(h: &Harness) -> Vec<String> {
    notes(h)
        .iter()
        .filter(|m| matches!(m, Msg::QueryCancelClosesConnection { .. }))
        .map(|m| h.app.i18n.msg(m).to_string())
        .collect()
}

/// A cancel of a statement that runs on a server whose tab's own session said
/// `performance_schema` is off says, in the run's Messages and before the session answers, that
/// cancelling a running statement closes the connection; the cancel still reaches the session.
/// The status line's word of a copy that was fetching every row is kept too. A server with it
/// on, or not known, and PostgreSQL say nothing of it.
#[test]
fn a_cancel_on_a_server_without_performance_schema_says_up_front_when_it_closes_the_connection() {
    use datarig_core::engine::{Fact, Why};
    let (mut h, id) = running_on_mariadb("SELECT SLEEP(5)", Some(mariadb_facts(ps_off())));
    h.app.dispatch(Action::CancelQuery);
    assert_eq!(closes_notes(&h), [CLOSES], "said before the session answers");
    let session = h.roles().iter().position(|r| *r == SessionRole::Query).expect("a query session");
    assert!(h.session_cancelled(session), "the cancel reached the session");
    // A second cancel while it is cancelling says nothing more.
    h.app.dispatch(Action::CancelQuery);
    assert_eq!(closes_notes(&h), [CLOSES]);
    h.tab_db(0, DbEvent::Failed { id, error: DbError::Cancelled, cancelled: true });
    assert_eq!(closes_notes(&h), [CLOSES], "kept with the run");
    for ps in [Fact::Known(()), Fact::Unknown(Why::NotChecked)] {
        let (mut h, _) = running_on_mariadb("SELECT SLEEP(5)", Some(mariadb_facts(ps.clone())));
        h.app.dispatch(Action::CancelQuery);
        assert!(closes_notes(&h).is_empty(), "{ps:?}");
        let session = h.roles().iter().position(|r| *r == SessionRole::Query).expect("a query session");
        assert!(h.session_cancelled(session), "{ps:?}");
    }
    // `performance_schema` on, but the session's marker not confirmed (behind a proxy, or an
    // account that cannot read the connection attributes): no statement is stopped on the server
    // either, and the words say why.
    let unconfirmed = Fact::No(Why::MarkerUnconfirmed);
    let (mut h, _) = running_on_mariadb("SELECT SLEEP(5)", Some(mariadb_facts_marked(Fact::Known(()), unconfirmed)));
    h.app.dispatch(Action::CancelQuery);
    assert_eq!(
        closes_notes(&h),
        ["This server cannot stop a running statement (this tab's own session could not be confirmed on \
          the server): cancelling a statement that is running closes the connection"]
    );
    let mut h = Harness::connected(Lang::En);
    h.app.focus = Focus::Editor;
    run_all(&mut h, "SELECT pg_sleep(5)");
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, facts_of(Engine::Postgres));
    h.tab_db(0, DbEvent::Started { id, index: 0 });
    h.app.dispatch(Action::CancelQuery);
    assert!(closes_notes(&h).is_empty(), "PostgreSQL");

    // A copy fetching every row of a held result: both words, the copy's in the status line.
    // A MySQL-family session reads a held result to its end at once, so no copy can wait for
    // its rows there today: the words are checked on a driver that leaves it to be paged.
    let (mut h, id) = sent_on_mariadb("SELECT * FROM t", Some(mariadb_facts(ps_off())), false);
    let rows = (0..600).map(|i| vec![Some(i.to_string())]).collect::<Vec<_>>();
    let columns = Some(vec![meta("n", "int", true, false)]);
    h.tab_db(0, DbEvent::Page { id, columns, rows, more: true, elapsed: MS });
    assert!(h.app.tab().exec.running.is_none(), "the result rests: {:?}", h.sent());
    h.key(KeyCode::Tab);
    h.keys("gg");
    h.draw(160, 45);
    let (x, y) = (h.app.tab().grid.hit_cols[0].0 + 2, h.app.tab().grid.data_y);
    h.app.handle_event(ratatui::crossterm::event::Event::Mouse(ratatui::crossterm::event::MouseEvent {
        kind: ratatui::crossterm::event::MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Right),
        column: x,
        row: y,
        modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
    }));
    let menu = h.screen(160, 45);
    assert!(menu.contains("Fetch all, then copy ▸"), "{menu}");
    (0..6).for_each(|_| h.key(KeyCode::Down));
    h.key(KeyCode::Enter);
    h.keys("c");
    h.sent();
    h.keys("y");
    let sent = h.sent();
    assert!(matches!(&sent[..], [DbCommand::FetchMore { .. }]), "fetching the rest: {sent:?}\n{}", h.screen(160, 45));
    h.ctrl('c');
    assert_eq!(tab_status(&h), Some(Msg::Label(Label::CopyFetchCancelled)));
    assert_eq!(closes_notes(&h), [CLOSES]);
}

/// While the tab's query session opens (it has said nothing of its server yet), a cancel says
/// nothing of `performance_schema`, even when the profile's metadata session said it is off:
/// the words follow the tab's own session only. The cancel still reaches the session.
#[test]
fn a_cancel_while_the_query_session_opens_says_nothing_of_performance_schema() {
    let (mut h, _) = running_on_mariadb("SELECT SLEEP(5)", None);
    h.meta_db("seam", mariadb_facts(ps_off()));
    h.app.dispatch(Action::CancelQuery);
    assert!(closes_notes(&h).is_empty(), "{:?}", notes(&h));
    let session = h.roles().iter().position(|r| *r == SessionRole::Query).expect("a query session");
    assert!(h.session_cancelled(session));
}

/// A cancel between two statements of a run on a server whose `performance_schema` is off says
/// the same words (whether a statement still runs cannot be told), and the session the driver
/// keeps stays: the run ends cancelled, the next run goes to the same session.
#[test]
fn a_cancel_between_statements_without_performance_schema_keeps_the_session() {
    let (mut h, id) = running_on_mariadb("SELECT 1;\nSELECT SLEEP(5);\nSELECT 2;", Some(mariadb_facts(ps_off())));
    h.tab_db(0, DbEvent::Finished { id, index: 0, outcome: Outcome::Command("SELECT".into()), elapsed: MS });
    h.app.dispatch(Action::CancelQuery);
    assert_eq!(closes_notes(&h), [CLOSES]);
    let session = h.roles().iter().position(|r| *r == SessionRole::Query).expect("a query session");
    assert!(h.session_cancelled(session));
    h.tab_db(0, DbEvent::Failed { id, error: DbError::Cancelled, cancelled: true });
    assert!(h.app.tab().exec.running.is_none());
    assert!(h.app.tab().exec.session.is_some(), "the session stays");
    let sessions = query_sessions(&h);
    executes(&mut h);
    run_all(&mut h, "SELECT 3");
    assert_eq!(executes(&mut h), [vec!["SELECT 3".to_string()]], "sent to the session");
    assert_eq!(query_sessions(&h), sessions, "no new session");
}
