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
use datarig_core::i18n::{Lang, Msg};
use datarig_core::profile::{ConnectionConfig, ProfileId};
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_core::sql::dialect::{Dialect, Language};
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
/// held results), connected.
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
