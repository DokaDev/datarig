//! A MySQL profile on a real server, driven through `App` like the binary does. Needs a MySQL
//! loaded with `dev/init-mysql/*.sql`: `DATARIG_TEST_MYSQL_URL` (e.g.
//! `mysql://datarig:datarig@127.0.0.1:53306/shop`) and, for the objects the tests make,
//! `DATARIG_TEST_MYSQL_ADMIN_URL` (e.g. `mysql://root:datarig-root@127.0.0.1:53306/`).
//! * unset locally  -> each test prints a visible `SKIPPED` line to stderr and returns;
//! * unset with `DATARIG_REQUIRE_MYSQL=1` (CI's `integration` job) -> the test fails.
//!
//! Every object a test makes is named `zz_it_…` and dropped again, also when the test fails.

use datarig_core::config::Config;
use datarig_core::driver::PagingMode;
use datarig_core::profile::{ConnectionConfig, ProfileId};
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_core::sql::dialect::{Dialect, Language};
use datarig_i18n::{Lang, Msg};
use datarig_tui::app::{App, AppEvent, Focus, Paging, Results, Startup};
use mysql_async::prelude::Queryable;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

fn env_url(var: &str, test: &str, example: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            if std::env::var("DATARIG_REQUIRE_MYSQL").is_ok_and(|v| v == "1") {
                panic!("{var} must be set when DATARIG_REQUIRE_MYSQL=1");
            }
            let _ = writeln!(std::io::stderr(), "SKIPPED {test}: set {var}={example}");
            None
        }
    }
}

/// The server's URL and an administrator's (root) URL.
fn urls(test: &str) -> Option<(String, String)> {
    let url = env_url("DATARIG_TEST_MYSQL_URL", test, "mysql://datarig:datarig@127.0.0.1:53306/shop")?;
    let admin = env_url("DATARIG_TEST_MYSQL_ADMIN_URL", test, "mysql://root:datarig-root@127.0.0.1:53306/")?;
    Some((url, admin))
}

/// A MySQL profile with the fields of `url` (its password goes to the store).
fn profile(url: &str) -> ConnectionConfig {
    let d = datarig_core::profile::dsn::parse(url).expect("a mysql:// URL");
    ConnectionConfig {
        name: "it-my".into(),
        driver: "mysql".into(),
        host: d.host.clone(),
        port: d.port.unwrap_or(3306),
        user: d.user.clone(),
        database: d.database.clone(),
        ..ConnectionConfig::default()
    }
}

/// An app with the one MySQL profile of `url` (under `policy`: `it-ro` read-only, `it-hold`
/// held results, `it-manual` manual commit, `it-rollback` manual commit with a `tx_idle_rollback`
/// of 10s and no warning), connected.
async fn app_on(url: &str, tag: &str, policy: Option<&str>) -> (App, UnboundedReceiver<AppEvent>) {
    use datarig_core::policy::Policy;
    let pw = datarig_core::profile::dsn::parse(url).unwrap().password.unwrap_or_default();
    let mut p = profile(url);
    p.policy = policy.map(str::to_string);
    let store = Arc::new(MemoryStore::new());
    store.set(&p.id.account(), &pw).unwrap();
    let mut cfg = Config { connections: vec![p], ..Config::default() };
    cfg.policies.insert("it-ro", Policy { read_only: true, ..Policy::default() });
    cfg.policies.insert("it-hold", Policy { paging: PagingMode::Hold, ..Policy::default() });
    let manual = Policy { commit: datarig_core::policy::CommitRule::Manual, ..Policy::default() };
    cfg.policies.insert("it-manual", manual.clone());
    let rollback = Policy { tx_idle_warning: None, tx_idle_rollback: Some(Duration::from_secs(10)), ..manual };
    cfg.policies.insert("it-rollback", rollback);
    let mut app = App::new(&cfg, None, Lang::En);
    app.set_secret_store(store as Arc<dyn SecretStore>);
    app.set_instance_tag(tag);
    let (tx, mut rx) = unbounded_channel();
    app.start(tx, Startup::Normal);
    let id = first(&app);
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    pump(&mut app, &mut rx, 15, |a| a.conns.is_connected(id)).await;
    (app, rx)
}

fn first(app: &App) -> ProfileId {
    app.profiles[0].id
}

/// Feed background events into the app until `done` (or fail after `secs`).
async fn pump(app: &mut App, rx: &mut UnboundedReceiver<AppEvent>, secs: u64, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !done(app) {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(ev)) => app.on_app_event(ev),
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out after {secs}s (status {:?}, notices {:?})", app.status, app.notices),
        }
    }
}

fn idle(app: &App) -> bool {
    app.tab().exec.running.is_none() && !app.tab_busy(app.tab().id)
}

/// Run `sql` in the active tab (yes to a run confirmation) and wait until it ended.
async fn run(app: &mut App, rx: &mut UnboundedReceiver<AppEvent>, sql: &str) {
    app.run(vec![sql.to_string()]);
    if app.overlays.run_confirm().is_some() {
        press(app, 'y');
    }
    pump(app, rx, 30, idle).await;
}

fn press(app: &mut App, c: char) {
    let m = if c.is_ascii_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(c), m)));
}

/// The first cell of the first row the grid shows, and the rows fetched.
fn shown_page(app: &App) -> (Option<String>, usize) {
    let t = app.tab();
    let Results::Rows(rs) = &t.results else { return (None, 0) };
    let start = t.grid.window(rs.rows.len()).start;
    let first = match rs.cell(start, 0) {
        datarig_tui::widgets::grid::CellRef::Here(c) => c.clone(),
        _ => None,
    };
    (first, rs.rows.len())
}

/// The run's Messages, as the app words them.
fn notes(app: &App) -> Vec<String> {
    app.tab().exec.run.notes.iter().map(|n| n.render(&app.i18n).to_string()).collect()
}

/// Statements `sql` on a side connection (`{p}` is the test's tag); `undo` runs when the guard
/// drops, also when the test fails.
struct Made {
    admin: String,
    undo: Vec<String>,
}

impl Made {
    async fn make(admin: &str, sql: &[String], undo: &[String]) -> Made {
        let made = Made { admin: admin.to_string(), undo: undo.to_vec() };
        let mut c = side(admin).await;
        for s in &made.undo {
            c.query_drop(s.as_str()).await.unwrap();
        }
        for s in sql {
            c.query_drop(s.as_str()).await.unwrap();
        }
        c.disconnect().await.unwrap();
        made
    }
}

impl Drop for Made {
    fn drop(&mut self) {
        let (admin, undo) = (self.admin.clone(), self.undo.clone());
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let mut c = side(&admin).await;
                for s in undo {
                    let _ = c.query_drop(s).await;
                }
                let _ = c.disconnect().await;
            });
        })
        .join();
    }
}

async fn side(url: &str) -> mysql_async::Conn {
    let opts = mysql_async::OptsBuilder::from_opts(mysql_async::Opts::from_url(url).unwrap()).prefer_socket(false);
    mysql_async::Conn::new(opts).await.unwrap()
}

/// A table `shop.zz_it_<what>_<pid>` of `n` rows (`id` 1..=n).
async fn numbers(admin: &str, what: &str, n: u32) -> (Made, String) {
    let t = format!("shop.zz_it_{what}_{}", std::process::id());
    let made = Made::make(
        admin,
        &[
            format!("CREATE TABLE {t} (id INT PRIMARY KEY)"),
            format!(
                "INSERT INTO {t} WITH RECURSIVE d (i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM d WHERE i < 99) \
                 SELECT g.n FROM (SELECT a.i + b.i * 100 + c.i * 10000 + 1 AS n FROM d a, d b, d c) g WHERE g.n <= {n}"
            ),
        ],
        &[format!("DROP TABLE IF EXISTS {t}")],
    )
    .await;
    (made, t)
}

/// The default: the first page holds nothing on the server; the next page runs the statement
/// again (said so), past the rows it has.
#[tokio::test(flavor = "multi_thread")]
async fn pages_are_not_held_and_the_next_one_runs_again() {
    let Some((url, admin)) = urls("pages_are_not_held_and_the_next_one_runs_again") else { return };
    let (_t, t) = numbers(&admin, "tuipages", 1234).await;
    let (mut app, mut rx) = app_on(&url, &format!("mp{}", std::process::id()), None).await;
    run(&mut app, &mut rx, &format!("SELECT id FROM {t} ORDER BY id")).await;
    assert_eq!(shown_page(&app), (Some("1".into()), 500));
    assert_eq!(app.tab().exec.paging, Paging::Released);
    app.focus = Focus::Results;
    press(&mut app, 'n');
    pump(&mut app, &mut rx, 15, idle).await;
    assert_eq!(shown_page(&app), (Some("501".into()), 1000));
    press(&mut app, 'n');
    pump(&mut app, &mut rx, 15, idle).await;
    assert_eq!(shown_page(&app), (Some("1001".into()), 1234));
    assert!(matches!(&app.tab().results, Results::Rows(rs) if !rs.more));
    // A statement whose rows change from run to run is not run again: its first page is all.
    run(&mut app, &mut rx, &format!("SELECT id, RAND() FROM {t}")).await;
    assert!(app.status.as_ref().is_some_and(|n| matches!(n.msg, Msg::ResultsFirstPageOnly { .. })), "{:?}", app.status);
}

/// A filter on a `TIMESTAMP` as the table filter writes it (the instant from UTC, converted to
/// the session's zone) is run again for the next page; once the session's zone changed, it is
/// not, and the reason is said.
#[tokio::test(flavor = "multi_thread")]
async fn a_timestamp_filter_pages_again_in_the_zone_its_rows_were_read_in() {
    let Some((url, admin)) = urls("a_timestamp_filter_pages_again_in_the_zone_its_rows_were_read_in") else {
        return;
    };
    let t = format!("shop.zz_it_tzpages_{}", std::process::id());
    let _t = Made::make(
        &admin,
        &[
            "SET time_zone = '+00:00'".to_string(),
            format!("CREATE TABLE {t} (id INT PRIMARY KEY, placed_at TIMESTAMP NULL)"),
            format!(
                "INSERT INTO {t} WITH RECURSIVE d (i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM d WHERE i < 99) \
                 SELECT g.n, IF(g.n % 7 = 0, '2026-11-10 10:30:00', '2026-11-10 09:30:00') \
                 FROM (SELECT a.i + b.i * 100 + 1 AS n FROM d a, d b) g WHERE g.n <= 1234"
            ),
        ],
        &[format!("DROP TABLE IF EXISTS {t}")],
    )
    .await;
    let (mut app, mut rx) = app_on(&url, &format!("mz{}", std::process::id()), None).await;
    run(&mut app, &mut rx, "SET time_zone = '+09:00'").await;
    let sql = format!(
        "SELECT id FROM {t} WHERE placed_at = CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone) \
         ORDER BY id"
    );
    run(&mut app, &mut rx, &sql).await;
    assert_eq!(shown_page(&app), (Some("1".into()), 500));
    assert_eq!(app.tab().exec.paging, Paging::Released);
    assert!(app.can_run_again(), "{:?}", app.status);
    app.focus = Focus::Results;
    press(&mut app, 'n');
    pump(&mut app, &mut rx, 15, idle).await;
    // 1058 rows match (every id but the multiples of 7): the 501st is id 584.
    assert_eq!(shown_page(&app), (Some("584".into()), 1000));
    // The session's zone changes: the rows stay, and the next page is not run again.
    run(&mut app, &mut rx, "SET time_zone = '+00:00'").await;
    // Its Messages are shown: back to the rows.
    app.tab_mut().exec.view = datarig_tui::app::tabs::ResultView::Rows;
    app.focus = Focus::Results;
    press(&mut app, 'n');
    pump(&mut app, &mut rx, 5, idle).await;
    assert_eq!(shown_page(&app), (Some("584".into()), 1000));
    let status = app.status.as_ref().map(|n| n.render(&app.i18n).to_string()).unwrap_or_default();
    assert!(status.contains("it reads the session's time zone"), "{status}");
    // Back in the zone the rows were read in: the last page comes.
    run(&mut app, &mut rx, "SET time_zone = '+09:00'").await;
    // Its Messages are shown: back to the rows.
    app.tab_mut().exec.view = datarig_tui::app::tabs::ResultView::Rows;
    app.focus = Focus::Results;
    press(&mut app, 'n');
    pump(&mut app, &mut rx, 15, idle).await;
    assert!(matches!(&app.tab().results, Results::Rows(rs) if rs.rows.len() == 1058 && !rs.more));
}

/// `paging = "hold"`: the app reads the result to its end at once (the statement and its
/// metadata locks end with it), page by page into its store.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_result_is_read_to_its_end_at_once() {
    let Some((url, admin)) = urls("a_held_result_is_read_to_its_end_at_once") else { return };
    let (_t, t) = numbers(&admin, "tuihold", 1234).await;
    let (mut app, mut rx) = app_on(&url, &format!("mh{}", std::process::id()), Some("it-hold")).await;
    app.run(vec![format!("SELECT id FROM {t} ORDER BY id")]);
    pump(&mut app, &mut rx, 30, |a| {
        idle(a) && matches!(&a.tab().results, Results::Rows(rs) if !rs.more && rs.rows.len() == 1234)
    })
    .await;
    assert_eq!(app.tab().exec.paging, Paging::None);
    assert_eq!(shown_page(&app), (Some("1".into()), 1234));
}

/// A read-only profile: the app refuses a write before it is sent, and the server refuses one
/// the text does not show (a view whose function writes).
#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_profile_holds_in_the_app_and_on_the_server() {
    let Some((url, admin)) = urls("a_read_only_profile_holds_in_the_app_and_on_the_server") else { return };
    let p = std::process::id();
    let (t, f, v) = (format!("shop.zz_it_ro_{p}"), format!("shop.zz_it_ro_f_{p}"), format!("shop.zz_it_ro_v_{p}"));
    let _made = Made::make(
        &admin,
        &[
            format!("CREATE TABLE {t} (id INT PRIMARY KEY)"),
            format!("CREATE FUNCTION {f}() RETURNS INT DETERMINISTIC MODIFIES SQL DATA BEGIN INSERT INTO {t} VALUES (7); RETURN 1; END"),
            format!("CREATE SQL SECURITY INVOKER VIEW {v} AS SELECT {f}() AS x"),
        ],
        &[format!("DROP VIEW IF EXISTS {v}"), format!("DROP FUNCTION IF EXISTS {f}"), format!("DROP TABLE IF EXISTS {t}")],
    )
    .await;
    let (mut app, mut rx) = app_on(&url, &format!("mr{p}"), Some("it-ro")).await;
    app.run(vec![format!("INSERT INTO {t} VALUES (1)")]);
    assert!(app.tab().exec.running.is_none() && app.tab().exec.session.is_none(), "nothing was sent");
    assert!(matches!(app.status.as_ref().map(|n| &n.msg), Some(Msg::SafetyReadOnlyBlocked { .. })), "{:?}", app.status);
    run(&mut app, &mut rx, &format!("SELECT * FROM {v}")).await;
    let error = match &app.tab().results {
        Results::Error(e) => e.clone(),
        _ => panic!("the server refuses it: {:?}", app.status),
    };
    assert!(error.starts_with("ERROR 1792"), "{error}");
    let mut c = side(&admin).await;
    let n: u64 = c.query_first(format!("SELECT COUNT(*) FROM {t}")).await.unwrap().unwrap();
    c.disconnect().await.unwrap();
    assert_eq!(n, 0, "nothing was written");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_stops_a_running_statement() {
    let Some((url, _)) = urls("a_cancel_stops_a_running_statement") else { return };
    let (mut app, mut rx) = app_on(&url, &format!("mc{}", std::process::id()), None).await;
    app.run(vec!["SELECT SLEEP(30)".into()]);
    pump(&mut app, &mut rx, 15, |a| a.tab().exec.session.is_some() && a.tab().exec.running.is_some()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let t0 = Instant::now();
    app.cancel();
    pump(&mut app, &mut rx, 10, idle).await;
    assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    // MySQL answers an interrupted SLEEP with its row (1), or with the interruption.
    match &app.tab().results {
        Results::Cancelled => {}
        Results::Rows(_) => assert_eq!(shown_page(&app).0.as_deref(), Some("1")),
        _ => panic!("{:?}", app.status),
    }
}

/// What the server says beyond a statement's outcome, a statement that commits the open
/// transaction, and a changed sql mode, as the app shows them.
#[tokio::test(flavor = "multi_thread")]
async fn messages_say_what_mysql_did() {
    let Some((url, admin)) = urls("messages_say_what_mysql_did") else { return };
    let t = format!("shop.zz_it_msg_{}", std::process::id());
    let _made = Made::make(
        &admin,
        &[format!("CREATE TABLE {t} (id INT AUTO_INCREMENT PRIMARY KEY, v INT)")],
        &[format!("DROP TABLE IF EXISTS {t}_x"), format!("DROP TABLE IF EXISTS {t}")],
    )
    .await;
    let (mut app, mut rx) = app_on(&url, &format!("mm{}", std::process::id()), None).await;
    run(&mut app, &mut rx, &format!("INSERT INTO {t} (v) VALUES (1), (2)")).await;
    let n = notes(&app);
    assert!(n.iter().any(|m| m == "last insert id: 1"), "{n:?}");
    assert!(n.iter().any(|m| m.starts_with("the server says: Records: 2")), "{n:?}");
    run(&mut app, &mut rx, "SELECT 1 / 0").await;
    assert!(notes(&app).iter().any(|m| m == "1 warning: SHOW WARNINGS shows it"), "{:?}", notes(&app));
    run(&mut app, &mut rx, "BEGIN").await;
    pump(&mut app, &mut rx, 10, |a| a.tab().exec.user_tx()).await;
    run(&mut app, &mut rx, &format!("CREATE TABLE {t}_x (a INT)")).await;
    assert!(notes(&app).iter().any(|m| m.starts_with("statement 1 commits the open transaction")), "{:?}", notes(&app));
    pump(&mut app, &mut rx, 10, |a| !a.tab().exec.user_tx()).await;
    run(&mut app, &mut rx, "SET SESSION sql_mode = 'ANSI_QUOTES'").await;
    let tab = app.tab().id;
    assert!(matches!(app.tab_language(tab), Language::Sql(Dialect::MySql(m)) if m.ansi_quotes));
    assert_eq!(app.tab().editor.language(), app.tab_language(tab));
}

/// Ends the test's own sessions (`KILL` of the connections whose program name carries `tag`)
/// when the test ends, also when an assertion panics: a statement it left running goes with
/// them.
struct EndSessions {
    admin: String,
    tag: String,
}

impl Drop for EndSessions {
    fn drop(&mut self) {
        let (admin, tag) = (self.admin.clone(), self.tag.clone());
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let mut c = side(&admin).await;
                let ids: Vec<u64> = c
                    .exec(
                        "SELECT PROCESSLIST_ID FROM performance_schema.session_connect_attrs \
                         WHERE ATTR_NAME = 'program_name' AND ATTR_VALUE LIKE ?",
                        (format!("datarig-%-{tag}"),),
                    )
                    .await
                    .unwrap_or_default();
                for id in ids {
                    let _ = c.query_drop(format!("KILL {id}")).await;
                }
                let _ = c.disconnect().await;
            });
        })
        .join();
    }
}

/// Like [`pump`], with the app's clock ticking (its timers run) as the binary's loop does.
async fn pump_ticking(app: &mut App, rx: &mut UnboundedReceiver<AppEvent>, secs: u64, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !done(app) {
        assert!(Instant::now() < deadline, "timed out after {secs}s (status {:?})", app.status);
        if let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            app.on_app_event(ev);
        }
        app.on_tick(Instant::now());
    }
}

/// A cancel the server takes whose statement then rolls back for long, saying nothing (a large
/// `UPDATE` in the user's transaction): the tab waits, says the server is still stopping the
/// statement, and keeps the session and the transaction, with what it did before.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_taken_by_a_long_rollback_keeps_the_tab_and_its_transaction() {
    let Some((url, admin)) = urls("a_cancel_taken_by_a_long_rollback_keeps_the_tab") else { return };
    let tag = format!("mroll{}", std::process::id());
    let t = format!("shop.zz_it_tuiroll_{}", std::process::id());
    let mut make = vec![format!("CREATE TABLE {t} (id INT PRIMARY KEY AUTO_INCREMENT, v INT, pad CHAR(100), KEY (v))")];
    for _ in 0..3 {
        make.push(format!("INSERT INTO {t} (v, pad) SELECT 0, 'x' FROM shop.events"));
    }
    make.push(format!("INSERT INTO {t} (v, pad) SELECT v, pad FROM {t}"));
    let _t = Made::make(&admin, &make, &[format!("DROP TABLE IF EXISTS {t}")]).await;
    // Dropped first: nothing of the test's keeps running past it.
    let _end = EndSessions { admin: admin.clone(), tag: tag.clone() };
    let (mut app, mut rx) = app_on(&url, &tag, None).await;
    run(&mut app, &mut rx, "BEGIN").await;
    run(&mut app, &mut rx, &format!("INSERT INTO {t} (v, pad) VALUES (-1, 'marker')")).await;
    assert!(app.tab().exec.tx_open);
    app.run(vec![format!("UPDATE {t} SET v = v + 1, pad = 'y'")]);
    if app.overlays.run_confirm().is_some() {
        press(&mut app, 'y');
    }
    // Long enough for its rollback to outlast the tab's old 5 s and the driver's 10 s.
    let until = Instant::now() + Duration::from_secs(15);
    pump_ticking(&mut app, &mut rx, 30, |_| Instant::now() >= until).await;
    assert!(app.tab().exec.running.is_some(), "the update still runs");
    let cancelled = Instant::now();
    app.cancel();
    let stopping = |a: &App| a.tab().exec.running.is_some_and(|r| r.stopping);
    pump_ticking(&mut app, &mut rx, 240, |a| a.tab().exec.running.is_none() || stopping(a)).await;
    let said = stopping(&app);
    pump_ticking(&mut app, &mut rx, 240, |a| a.tab().exec.running.is_none()).await;
    let waited = cancelled.elapsed();
    assert!(matches!(app.tab().results, Results::Cancelled), "{:?}", app.status);
    assert!(app.tab().exec.session.is_some(), "the session stays: {:?}", app.status);
    assert!(app.tab().exec.tx_open, "and so does the transaction");
    if waited > Duration::from_secs(7) {
        assert!(said, "a long wait says the server is still stopping the statement ({waited:?})");
    }
    run(&mut app, &mut rx, &format!("SELECT v FROM {t} WHERE pad = 'marker'")).await;
    assert_eq!(shown_page(&app).0.as_deref(), Some("-1"), "{:?}", app.status);
    run(&mut app, &mut rx, "ROLLBACK").await;
}

/// Manual commit on the real server: a plain read opens no transaction; the write opens one
/// that another session does not see into until `tx.commit`; `tx.rollback` undoes the next.
#[tokio::test(flavor = "multi_thread")]
async fn manual_commit_keeps_a_write_to_itself_until_it_is_committed() {
    use datarig_tui::app::action::Action;
    let Some((url, admin)) = urls("manual_commit_keeps_a_write_to_itself_until_it_is_committed") else { return };
    let (_t, t) = numbers(&admin, "tuimanual", 3).await;
    let (mut app, mut rx) = app_on(&url, &format!("mm{}", std::process::id()), Some("it-manual")).await;
    let mut other = side(&admin).await;
    let count = format!("SELECT COUNT(*) FROM {t}");
    run(&mut app, &mut rx, &format!("SELECT * FROM {t}")).await;
    assert!(!app.tab().exec.user_tx(), "a plain read opens nothing");
    run(&mut app, &mut rx, &format!("INSERT INTO {t} VALUES (10), (11)")).await;
    pump(&mut app, &mut rx, 10, |a| a.tab().exec.user_tx()).await;
    let tx = app.tab().exec.tx.clone().expect("the open transaction");
    assert_eq!((tx.manual, tx.changes, tx.rows), (true, 1, 2));
    let seen: i64 = other.query_first(count.as_str()).await.unwrap().unwrap();
    assert_eq!(seen, 3, "not visible to another session");
    app.dispatch(Action::TxCommit);
    pump(&mut app, &mut rx, 10, |a| !a.tab_busy(a.tab().id) && !a.tab().exec.user_tx()).await;
    assert!(matches!(app.status.as_ref().map(|n| &n.msg), Some(Msg::TxCommitted { count: 1, .. })), "{:?}", app.status);
    let seen: i64 = other.query_first(count.as_str()).await.unwrap().unwrap();
    assert_eq!(seen, 5);
    run(&mut app, &mut rx, &format!("DELETE FROM {t}")).await;
    pump(&mut app, &mut rx, 10, |a| a.tab().exec.user_tx()).await;
    app.dispatch(Action::TxRollback);
    pump(&mut app, &mut rx, 10, |a| !a.tab_busy(a.tab().id) && !a.tab().exec.user_tx()).await;
    let seen: i64 = other.query_first(count.as_str()).await.unwrap().unwrap();
    assert_eq!(seen, 5, "rolled back");
    other.disconnect().await.unwrap();
}

/// `tx_idle_rollback` on the real server: once a transaction opened for manual commit sat idle
/// that long (here the wall clock jumps, as after a sleep), the app rolls it back on its own
/// session, which stays: its row lock is free for another session, the row is as it was, and the
/// next run goes on the same connection.
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_manual_transaction_is_rolled_back_and_its_session_stays() {
    let Some((url, admin)) = urls("an_idle_manual_transaction_is_rolled_back_and_its_session_stays") else { return };
    let (_t, t) = numbers(&admin, "tuirollback", 3).await;
    let (mut app, mut rx) = app_on(&url, &format!("rb{}", std::process::id()), Some("it-rollback")).await;
    run(&mut app, &mut rx, "SELECT CONNECTION_ID()").await;
    let connection = shown_page(&app).0.expect("the session's id");
    run(&mut app, &mut rx, &format!("UPDATE {t} SET id = 100 WHERE id = 1")).await;
    pump(&mut app, &mut rx, 10, |a| a.tab().exec.user_tx()).await;
    assert!(app.tab().exec.tx.as_ref().is_some_and(|x| x.manual));
    let later = std::time::SystemTime::now() + Duration::from_secs(60);
    app.set_wall_clock(Arc::new(move || later));
    app.on_tick(app.now());
    assert_eq!(app.status.as_ref().map(|n| n.msg.clone()), Some(Msg::TxIdleRollingBack { time: "10s".into() }));
    pump(&mut app, &mut rx, 10, |a| !a.tab_busy(a.tab().id) && !a.tab().exec.user_tx()).await;
    let done = Msg::TxIdleRolledBack { count: 1, time: "10s".into() };
    assert_eq!(app.status.as_ref().map(|n| n.msg.clone()), Some(done));
    assert!(app.tab().exec.session.is_some(), "the session stays");
    assert_eq!(app.tx_status(), None);
    // Another session gets the row lock and sees the row as it was (rolled back).
    let mut other = side(&admin).await;
    other.query_drop("SET SESSION innodb_lock_wait_timeout = 10").await.unwrap();
    other.query_drop("START TRANSACTION").await.unwrap();
    let id: Option<i64> = other.query_first(format!("SELECT id FROM {t} WHERE id = 1 FOR UPDATE")).await.unwrap();
    assert_eq!(id, Some(1), "rolled back, and the lock is free");
    other.query_drop("ROLLBACK").await.unwrap();
    other.disconnect().await.unwrap();
    // The next run goes on the same connection, outside any transaction.
    run(&mut app, &mut rx, &format!("SELECT COUNT(*) FROM {t} WHERE id = 100")).await;
    assert_eq!(shown_page(&app).0.as_deref(), Some("0"), "{:?}", app.status);
    run(&mut app, &mut rx, "SELECT CONNECTION_ID()").await;
    assert_eq!(shown_page(&app).0, Some(connection), "the same connection");
    assert!(!app.tab().exec.user_tx());
}

/// The explain actions on a real server: `EXPLAIN FORMAT=TREE` and `EXPLAIN ANALYZE FORMAT=TREE`
/// of the statement under the cursor come back as a plan in the Plan tab, the raw view being the
/// server's text.
#[tokio::test(flavor = "multi_thread")]
async fn the_explain_actions_show_mysqls_tree_as_a_plan() {
    let Some((url, _)) = urls("the_explain_actions_show_mysqls_tree_as_a_plan") else { return };
    let (mut app, mut rx) = app_on(&url, &format!("mx{}", std::process::id()), None).await;
    let sql = "SELECT u.id, COUNT(*) FROM users u JOIN orders o ON o.user_id = u.id GROUP BY u.id ORDER BY 2 DESC;";
    for (keys, analyzed) in [(" ex", false), (" ea", true)] {
        let mut editor = datarig_tui::widgets::editor::Editor::new(sql);
        editor.set_language(app.tab_language(app.tab().id));
        app.tab_mut().editor = editor;
        app.focus = Focus::Editor;
        for c in keys.chars() {
            press(&mut app, c);
        }
        pump(&mut app, &mut rx, 30, |a| {
            idle(a) && a.tab().exec.plan.as_ref().is_some_and(|p| p.plan.analyzed == analyzed)
        })
        .await;
        let t = app.tab();
        assert_eq!(t.exec.view, datarig_tui::app::tabs::ResultView::Plan, "{keys}: {:?}", app.status);
        let p = t.exec.plan.as_ref().unwrap();
        assert!(p.plan.nodes.len() > 2, "{keys}: {:?}", p.plan.nodes);
        assert!(p.source.starts_with("-> "), "{keys}: the server's tree: {}", p.source);
        let sent = t.exec.run.statements.last().map(|s| s.sql.clone()).unwrap_or_default();
        let want = if analyzed { "EXPLAIN ANALYZE FORMAT=TREE SELECT" } else { "EXPLAIN FORMAT=TREE SELECT" };
        assert!(sent.starts_with(want), "{keys}: {sent}");
    }
}

/// A table tab's filter MySQL refuses as a syntax error: the text it quotes (`near '…'`) is
/// found once in the statement, inside the filter, and marks it from there; the tab keeps no
/// rows of it. Reads an existing table, makes no object.
#[tokio::test(flavor = "multi_thread")]
async fn a_syntax_error_in_a_table_filter_marks_the_text_mysql_quotes() {
    use datarig_core::query::{Filter, TableName};
    use datarig_core::workspace::{self, ExplorerState, TabState, WorkspaceState};
    use datarig_tui::app::action::Action;
    use datarig_tui::app::table_query::TableQuery;
    let test = "a_syntax_error_in_a_table_filter_marks_the_text_mysql_quotes";
    let Some(url) = env_url("DATARIG_TEST_MYSQL_URL", test, "mysql://datarig:datarig@127.0.0.1:53306/shop") else {
        return;
    };
    let pw = datarig_core::profile::dsn::parse(&url).unwrap().password.unwrap_or_default();
    let dir = std::env::temp_dir().join(format!("datarig-it-mynear-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let paths = datarig_core::paths::Paths { data: Some(dir.join("data")), state: Some(dir.join("state")) };
    let p = profile(&url);
    let tab = TabState {
        id: workspace::new_id(),
        kind: workspace::TabKind::Table,
        script: None,
        profile: Some(p.id),
        cursor: (0, 0),
        top: 0,
        console: 0,
        table: Some(("shop".into(), "users".into())),
        ddl: None,
        results: None,
        database: None,
        schema: None,
        filter: None,
        order: Vec::new(),
        limit: None,
    };
    let ws =
        WorkspaceState { active: 0, explorer: ExplorerState::default(), tabs: vec![tab], unknown_tabs: Vec::new() };
    workspace::save(&dir.join("state"), &ws).unwrap();
    let store = Arc::new(MemoryStore::new());
    store.set(&p.id.account(), &pw).unwrap();
    let cfg = Config { connections: vec![p], ..Config::default() };
    let mut app = App::new(&cfg, None, Lang::En);
    app.set_secret_store(store as Arc<dyn SecretStore>);
    app.set_paths(paths);
    app.whats_new_paused = true;
    app.set_instance_tag(&format!("mynear{}", std::process::id()));
    let (tx, mut rx) = unbounded_channel();
    app.start(tx, Startup::Normal);
    assert!(app.tab().is_table());
    // `RANK` is a reserved word of MySQL 8: the server stops reading at the `=` after it.
    let filter = "name = '\u{65e5}\u{672c}' AND rank = 1";
    let mut tq = TableQuery::new(TableName { database: None, schema: "shop".into(), name: "users".into() });
    tq.draft.filter = Some(Filter { text: filter.into(), recognised: None });
    app.tab_mut().doc.table_query = Some(Box::new(tq));
    app.dispatch(Action::RunStatement);
    let failed = |a: &App| a.tab().doc.table_query.as_ref().is_some_and(|q| q.failure().is_some());
    pump(&mut app, &mut rx, 20, failed).await;
    let status = app.tab().status.as_ref().map(|n| n.render(&app.i18n).to_string()).unwrap_or_default();
    assert!(status.contains("ERROR 1064"), "{status}");
    let tq = app.tab().doc.table_query.as_ref().unwrap();
    let start = filter.find("= 1").unwrap();
    assert_eq!(tq.failure().and_then(|f| f.at.clone()), Some(start..filter.len()), "{status}");
    assert_eq!(tq.shown, None, "no rows of it are held");
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An app whose workspace holds one table tab of `shop.<table>` capped at `limit` rows, under
/// `policy` (as [`app_on`]), not run yet; `dir` holds its files.
fn limited_table_app(
    url: &str,
    table: &str,
    limit: u64,
    policy: Option<&str>,
    dir: &std::path::Path,
) -> (App, UnboundedReceiver<AppEvent>) {
    use datarig_core::policy::Policy;
    use datarig_core::workspace::{self, ExplorerState, TabState, WorkspaceState};
    let pw = datarig_core::profile::dsn::parse(url).unwrap().password.unwrap_or_default();
    let _ = std::fs::remove_dir_all(dir);
    let paths = datarig_core::paths::Paths { data: Some(dir.join("data")), state: Some(dir.join("state")) };
    let mut p = profile(url);
    p.policy = policy.map(str::to_string);
    let tab = TabState {
        id: workspace::new_id(),
        kind: workspace::TabKind::Table,
        script: None,
        profile: Some(p.id),
        cursor: (0, 0),
        top: 0,
        console: 0,
        table: Some(("shop".into(), table.into())),
        ddl: None,
        results: None,
        database: None,
        schema: None,
        filter: None,
        order: Vec::new(),
        limit: Some(limit),
    };
    let ws =
        WorkspaceState { active: 0, explorer: ExplorerState::default(), tabs: vec![tab], unknown_tabs: Vec::new() };
    workspace::save(&dir.join("state"), &ws).unwrap();
    let store = Arc::new(MemoryStore::new());
    store.set(&p.id.account(), &pw).unwrap();
    let mut cfg = Config { connections: vec![p], ..Config::default() };
    cfg.policies.insert("it-hold", Policy { paging: PagingMode::Hold, ..Policy::default() });
    let mut app = App::new(&cfg, None, Lang::En);
    app.set_secret_store(store as Arc<dyn SecretStore>);
    app.set_paths(paths);
    app.whats_new_paused = true;
    app.set_instance_tag(&format!("mylimit{}", std::process::id()));
    let (tx, rx) = unbounded_channel();
    app.start(tx, Startup::Normal);
    assert!(app.tab().is_table());
    (app, rx)
}

/// The rows the active tab's result holds, and whether it says the server has more.
fn held_rows(app: &App) -> (usize, bool) {
    match &app.tab().results {
        Results::Rows(rs) => (rs.rows.len(), rs.more),
        _ => (0, false),
    }
}

/// A table tab's limit caps its rows on MySQL's own pages (the statement sent has no `LIMIT`,
/// which would override the cap the driver sets per page): a limit below a page cuts the first
/// page, one past it stops the page run again there, and a held result read page by page stops
/// at it (the rest stopped on the server). Exactly the limit's rows each time.
#[tokio::test(flavor = "multi_thread")]
async fn a_table_tab_limit_caps_its_pages() {
    use datarig_tui::app::action::Action;
    let Some((url, admin)) = urls("a_table_tab_limit_caps_its_pages") else { return };
    let (_t, t) = numbers(&admin, "tuilimit", 2500).await;
    let table = t.trim_start_matches("shop.").to_string();
    let dir = std::env::temp_dir().join(format!("datarig-it-mylimit-{}", std::process::id()));
    for (limit, policy) in [(3, None), (503, None), (1203, Some("it-hold"))] {
        let (mut app, mut rx) = limited_table_app(&url, &table, limit, policy, &dir);
        app.dispatch(Action::RunStatement);
        pump(&mut app, &mut rx, 30, |a| idle(a) && held_rows(a).0 > 0).await;
        let size = app.page_size as u64;
        if limit > size && policy.is_none() {
            // Not held: the next page runs the statement again, past the rows it has.
            assert_eq!(held_rows(&app), (size as usize, true), "{limit}");
            app.focus = Focus::Results;
            press(&mut app, 'n');
            pump(&mut app, &mut rx, 30, |a| idle(a) && held_rows(a).0 > size as usize).await;
        }
        if policy.is_some() {
            pump(&mut app, &mut rx, 30, |a| idle(a) && !held_rows(a).1).await;
        }
        assert_eq!(held_rows(&app), (limit as usize, false), "{limit} {policy:?}");
        assert_eq!(app.tab().exec.paging, Paging::None, "{limit}");
        let sent = app.tab().exec.run.statements.iter().map(|s| s.sql.clone()).collect::<Vec<_>>();
        assert_eq!(sent, [format!("SELECT * FROM `shop`.`{table}`")], "no LIMIT in the statement");
        // Nothing past the cap: the next page is the last.
        app.focus = Focus::Results;
        press(&mut app, 'n');
        assert!(idle(&app));
        // The session is fine after its result was stopped: it runs the tab again.
        app.dispatch(Action::RunStatement);
        pump(&mut app, &mut rx, 30, |a| idle(a) && held_rows(a).0 > 0).await;
        assert!(held_rows(&app).0 as u64 <= limit);
        drop(app);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The held values of column `col` of the current tab's rows.
fn column(app: &App, col: usize) -> Vec<Option<String>> {
    let Results::Rows(rs) = &app.tab().results else { return Vec::new() };
    (0..rs.rows.len())
        .map(|r| match rs.cell(r, col) {
            datarig_tui::widgets::grid::CellRef::Here(c) => c.clone(),
            _ => None,
        })
        .collect()
}

/// Sorting a table tab by a column on MySQL (read only, the seed's `shop.products`, capped at
/// five rows): `s` on `stock` sends it going up, then down, with the primary key appended
/// (stocks tie), and the rows come in that order.
#[tokio::test(flavor = "multi_thread")]
async fn sorting_a_table_tab_by_a_column_orders_its_rows_on_the_server() {
    use datarig_tui::app::action::Action;
    use datarig_tui::app::table_query::OrderKeysState;
    let Some((url, _admin)) = urls("sorting_a_table_tab_by_a_column_orders_its_rows_on_the_server") else { return };
    let dir = std::env::temp_dir().join(format!("datarig-it-mysort-{}", std::process::id()));
    let (mut app, mut rx) = limited_table_app(&url, "products", 5, None, &dir);
    app.dispatch(Action::RunStatement);
    let keys_known =
        |a: &App| matches!(a.tab().doc.table_query.as_ref().map(|q| &q.order_keys), Some(OrderKeysState::Known { .. }));
    pump(&mut app, &mut rx, 30, |a| idle(a) && held_rows(a).0 > 0).await;
    // The first run went before the profile's metadata session was open: the next one reads
    // the table's keys with it.
    app.dispatch(Action::RunStatement);
    pump(&mut app, &mut rx, 30, |a| idle(a) && held_rows(a).0 > 0 && keys_known(a)).await;
    let stock = match &app.tab().results {
        Results::Rows(rs) => rs.columns.iter().position(|c| c.meta.name == "stock").unwrap(),
        _ => panic!("no rows"),
    };
    let sent = |app: &App| app.tab().exec.run.statements.iter().map(|s| s.sql.clone()).collect::<Vec<_>>();
    app.focus = Focus::Results;
    app.tab_mut().grid.col = stock;
    let num = |v: &Option<String>| v.as_deref().map(|s| s.parse::<i64>().expect("a number"));
    for dir in ["ASC", "DESC"] {
        let qid = app.tab().exec.query_id;
        press(&mut app, 's');
        pump(&mut app, &mut rx, 30, |a| idle(a) && a.tab().exec.query_id != qid && held_rows(a).0 > 0).await;
        assert_eq!(sent(&app), [format!("SELECT * FROM `shop`.`products` ORDER BY `stock` {dir}, `id` ASC")]);
        // The rows come in the order sent: by stock that way, ties by id going up. The seed's
        // values differ between machines, so the order is checked, not the ids.
        let rows: Vec<(Option<i64>, Option<i64>)> =
            column(&app, stock).iter().map(num).zip(column(&app, 0).iter().map(num)).collect();
        assert_eq!(rows.len(), 5);
        for pair in rows.windows(2) {
            let ((s0, id0), (s1, id1)) = (pair[0], pair[1]);
            let by_stock = if dir == "ASC" { s0.cmp(&s1) } else { s1.cmp(&s0) };
            assert!(by_stock.is_lt() || (by_stock.is_eq() && id0 < id1), "{dir}: {rows:?}");
        }
    }
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A table tab of `shop.<table>` on the MySQL profile of `url`, not run yet (state in `dir`).
fn by_value_app(url: &str, table: &str, dir: &std::path::Path) -> (App, UnboundedReceiver<AppEvent>) {
    use datarig_core::workspace::{self, ExplorerState, TabState, WorkspaceState};
    let pw = datarig_core::profile::dsn::parse(url).unwrap().password.unwrap_or_default();
    let _ = std::fs::remove_dir_all(dir);
    let paths = datarig_core::paths::Paths { data: Some(dir.join("data")), state: Some(dir.join("state")) };
    let p = profile(url);
    let tab = TabState {
        id: workspace::new_id(),
        kind: workspace::TabKind::Table,
        script: None,
        profile: Some(p.id),
        cursor: (0, 0),
        top: 0,
        console: 0,
        table: Some(("shop".into(), table.into())),
        ddl: None,
        results: None,
        database: None,
        schema: None,
        filter: None,
        order: Vec::new(),
        limit: None,
    };
    let ws =
        WorkspaceState { active: 0, explorer: ExplorerState::default(), tabs: vec![tab], unknown_tabs: Vec::new() };
    workspace::save(&dir.join("state"), &ws).unwrap();
    let store = Arc::new(MemoryStore::new());
    store.set(&p.id.account(), &pw).unwrap();
    let cfg = Config { connections: vec![p], ..Config::default() };
    let mut app = App::new(&cfg, None, Lang::En);
    app.set_secret_store(store as Arc<dyn SecretStore>);
    app.set_paths(paths);
    app.whats_new_paused = true;
    app.set_instance_tag(&format!("mybyvalue{}", std::process::id()));
    let (tx, rx) = unbounded_channel();
    app.start(tx, Startup::Normal);
    assert!(app.tab().is_table());
    (app, rx)
}

/// The values of column `col` of the rows the active tab holds, in order.
fn held_column(app: &App, col: usize) -> Vec<Option<String>> {
    let Results::Rows(rs) = &app.tab().results else { return Vec::new() };
    let mut out = Vec::new();
    rs.rows.for_each_chunk(0..rs.rows.len(), 64, |rows| out.extend(rows.iter().map(|r| r[col].clone()))).unwrap();
    out
}

/// Filter by value on the active table tab: the cursor on the cells of column `name` of the
/// rows whose `id` is in `ids` (one cell, or a range down the column), grid key `key` (`=` only
/// the value, `!` everything but it) pressed, the run waited for; then the ids of the rows it
/// read, and the filter text it ran.
async fn by_value(
    app: &mut App,
    rx: &mut UnboundedReceiver<AppEvent>,
    ids: &[&str],
    name: &str,
    key: char,
) -> (Vec<String>, String) {
    let Results::Rows(rs) = &app.tab().results else { panic!("no rows") };
    let col = rs.columns.iter().position(|c| c.meta.name == name).expect("the column");
    let held = held_column(app, 0);
    let at = |id: &str| held.iter().position(|v| v.as_deref() == Some(id)).expect("the row");
    let (first, last) = (at(ids[0]), at(ids[ids.len() - 1]));
    assert_eq!(last + 1 - first, ids.len(), "the rows are next to each other");
    let g = &mut app.tab_mut().grid;
    (g.row, g.col, g.anchor) = (last, col, (ids.len() > 1).then_some((first, col)));
    app.focus = Focus::Results;
    press(app, key);
    pump(app, rx, 30, |a| {
        idle(a)
            && a.tab()
                .doc
                .table_query
                .as_ref()
                .is_some_and(|q| q.requested.is_none() || q.shown == q.requested.as_ref().map(|(_, q)| q.clone()))
    })
    .await;
    let tq = app.tab().doc.table_query.as_ref().unwrap();
    assert!(tq.failure().is_none(), "{:?}", app.tab().status.as_ref().map(|n| n.render(&app.i18n).to_string()));
    let text = tq.shown.as_ref().and_then(|q| q.filter.as_ref()).map(|f| f.text.clone()).unwrap_or_default();
    let mut got: Vec<String> = held_column(app, 0).into_iter().flatten().collect();
    got.sort();
    (got, text)
}

/// Filter by a cell's value (`=`, `!`) on MySQL, for text under a `_ci` and a `_bin` collation, a decimal,
/// a DATETIME, bytes and NULL, one cell and several: each condition reads back the rows it
/// names. Makes `shop.zz_it_byvalue_<pid>` and drops it.
#[tokio::test(flavor = "multi_thread")]
async fn filter_by_value_reads_back_the_rows_it_names() {
    use datarig_tui::app::action::Action;
    use datarig_tui::app::table_filter::TableAction;
    let Some((url, admin)) = urls("filter_by_value_reads_back_the_rows_it_names") else { return };
    let table = format!("zz_it_byvalue_{}", std::process::id());
    let t = format!("shop.{table}");
    let _t = Made::make(
        &admin,
        &[
            format!(
                "CREATE TABLE {t} (id INT PRIMARY KEY, name VARCHAR(20) COLLATE utf8mb4_0900_ai_ci, \
                 bname VARCHAR(20) COLLATE utf8mb4_bin, n DECIMAL(10,2), at DATETIME, b VARBINARY(8))"
            ),
            format!(
                "INSERT INTO {t} VALUES (1, 'Apple', 'Apple', 1.50, '2026-11-10 09:30:00', X'01AB'), \
                 (2, 'apple pie', 'apple pie', 2, '2026-11-10 10:00:00', X'00'), \
                 (3, NULL, NULL, NULL, NULL, NULL), \
                 (4, '50%_off', '50%_off', 3.25, '2026-11-11 00:00:00', X'0102'), \
                 (5, '50ab_off2', '50ab_off2', 4, '2026-11-12 00:00:00', X'03')"
            ),
        ],
        &[format!("DROP TABLE IF EXISTS {t}")],
    )
    .await;
    let dir = std::env::temp_dir().join(format!("datarig-it-mybyvalue-{}", std::process::id()));
    let (mut app, mut rx) = by_value_app(&url, &table, &dir);
    app.dispatch(Action::RunStatement);
    pump(&mut app, &mut rx, 30, |a| idle(a) && held_rows(a).0 == 5).await;
    // The cells (by `id`), the column, the entry, the ids it reads back and the filter it runs.
    type Case<'a> = (&'a [&'a str], &'a str, char, &'a [&'a str], &'a str);
    let cases: &[Case] = &[
        (&["1"], "name", '=', &["1"], "name = 'Apple'"),
        (&["1"], "name", '!', &["2", "3", "4", "5"], "NOT (name <=> 'Apple')"),
        // `_ci`: case does not count; `_bin`: it does.
        (&["2"], "name", '=', &["2"], "name = 'apple pie'"),
        (&["2"], "bname", '=', &["2"], "bname = 'apple pie'"),
        (&["4"], "name", '=', &["4"], "name = '50%_off'"),
        (&["1"], "n", '=', &["1"], "n = 1.50"),
        (&["2"], "at", '!', &["1", "3", "4", "5"], "NOT (at <=> '2026-11-10 10:00:00')"),
        (&["1"], "at", '=', &["1"], "at = '2026-11-10 09:30:00'"),
        (&["1"], "b", '=', &["1"], "b = X'01AB'"),
        (&["4"], "b", '!', &["1", "2", "3", "5"], "NOT (b <=> X'0102')"),
        (&["3"], "name", '=', &["3"], "name IS NULL"),
        (&["3"], "name", '!', &["1", "2", "4", "5"], "name IS NOT NULL"),
        (&["1", "2", "3"], "name", '=', &["1", "2", "3"], "(name IN ('Apple', 'apple pie') OR name IS NULL)"),
        (&["1", "2", "3"], "name", '!', &["4", "5"], "(name NOT IN ('Apple', 'apple pie') AND name IS NOT NULL)"),
        (&["1", "2"], "n", '!', &["3", "4", "5"], "(n NOT IN (1.50, 2.00) OR n IS NULL)"),
    ];
    for (cells, name, key, want, text) in cases {
        let (got, ran) = by_value(&mut app, &mut rx, cells, name, *key).await;
        assert_eq!(ran, *text, "{name} {key}");
        assert_eq!(got, want.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "{text}");
        app.dispatch(Action::Table(TableAction::Clear));
        pump(&mut app, &mut rx, 30, |a| idle(a) && held_rows(a).0 == 5).await;
    }
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Filter by a value with a line break (LF, CR, CRLF) on MySQL: the condition (the value's
/// UTF-8 bytes) reads back the row it names, and the filter applied again (`f`, `Enter`)
/// runs the same statement and reads the same rows. Makes `shop.zz_it_breaks_<pid>` and drops
/// it.
#[tokio::test(flavor = "multi_thread")]
async fn filter_by_a_value_with_line_breaks_reads_the_same_rows_from_the_line() {
    use datarig_tui::app::action::Action;
    use datarig_tui::app::table_filter::TableAction;
    let Some((url, admin)) = urls("filter_by_a_value_with_line_breaks_reads_the_same_rows_from_the_line") else {
        return;
    };
    let table = format!("zz_it_breaks_{}", std::process::id());
    let t = format!("shop.{table}");
    let _t = Made::make(
        &admin,
        &[
            format!("CREATE TABLE {t} (id INT PRIMARY KEY, name VARCHAR(20) COLLATE utf8mb4_bin)"),
            format!("INSERT INTO {t} VALUES (1, 'a\\nb'), (2, 'a\\rb'), (3, 'a\\r\\nb'), (4, 'a b'), (5, 'a\\n')"),
        ],
        &[format!("DROP TABLE IF EXISTS {t}")],
    )
    .await;
    let dir = std::env::temp_dir().join(format!("datarig-it-mybreaks-{}", std::process::id()));
    let (mut app, mut rx) = by_value_app(&url, &table, &dir);
    app.dispatch(Action::RunStatement);
    pump(&mut app, &mut rx, 30, |a| idle(a) && held_rows(a).0 == 5).await;
    type Case<'a> = (&'a [&'a str], char, &'a [&'a str], &'a str);
    let cases: &[Case] = &[
        (&["1"], '=', &["1"], "name = _utf8mb4 X'610A62'"),
        (&["2"], '=', &["2"], "name = _utf8mb4 X'610D62'"),
        (&["3"], '=', &["3"], "name = _utf8mb4 X'610D0A62'"),
        (&["1"], '!', &["2", "3", "4", "5"], "NOT (name <=> _utf8mb4 X'610A62')"),
        (
            &["1", "2", "3"],
            '=',
            &["1", "2", "3"],
            "name IN (_utf8mb4 X'610A62', _utf8mb4 X'610D62', _utf8mb4 X'610D0A62')",
        ),
    ];
    for (cells, key, want, text) in cases {
        let (got, ran) = by_value(&mut app, &mut rx, cells, "name", *key).await;
        assert_eq!(ran, *text, "{key}");
        assert_eq!(got, want.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "{text}");
        let first = app.tab().exec.run.statements.iter().map(|s| s.sql.clone()).collect::<Vec<_>>();
        // The filter dialog applies it again as it is.
        let qid = app.tab().exec.query_id;
        app.focus = Focus::Results;
        press(&mut app, 'f');
        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        pump(&mut app, &mut rx, 30, |a| {
            idle(a)
                && a.tab().exec.query_id != qid
                && a.tab()
                    .doc
                    .table_query
                    .as_ref()
                    .is_some_and(|q| q.shown == q.requested.as_ref().map(|(_, q)| q.clone()))
        })
        .await;
        let tq = app.tab().doc.table_query.as_ref().unwrap();
        assert!(tq.failure().is_none(), "{text}");
        let again = tq.shown.as_ref().and_then(|q| q.filter.as_ref()).map(|f| f.text.clone()).unwrap_or_default();
        assert_eq!(again, *text, "applied again from the dialog");
        let sent = app.tab().exec.run.statements.iter().map(|s| s.sql.clone()).collect::<Vec<_>>();
        assert_eq!(sent, first, "the same statement: {text}");
        let mut rows: Vec<String> = held_column(&app, 0).into_iter().flatten().collect();
        rows.sort();
        assert_eq!(rows, got, "the same rows: {text}");
        app.dispatch(Action::Table(TableAction::Clear));
        pump(&mut app, &mut rx, 30, |a| idle(a) && held_rows(a).0 == 5).await;
    }
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}
