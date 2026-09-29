//! The explorer -> real PostgreSQL connections, driven through `App` like
//! the binary does. Needs a PostgreSQL loaded with `dev/init/*.sql`.
//!
//! Connection: `DATARIG_TEST_PG_URL` (e.g.
//! `postgres://datarig:datarig@127.0.0.1:55432/datarig`).
//! * unset locally  -> each test prints a visible `SKIPPED` line to stderr and returns;
//! * unset with `DATARIG_REQUIRE_PG=1` (CI's `integration` job) -> the test fails, so that job
//!   can never silently skip. Other CI jobs run without a database and skip like a local run.
//!
//! Tables a test creates are `public.it_*` with a drop guard, and stale ones of killed runs are
//! swept before the first test (see `pg_clean`), so a run leaves `public` empty.

#[path = "../../datarig-driver-postgres/tests/pg_clean/mod.rs"]
mod pg_clean;

use datarig_core::config::Config;
use datarig_core::i18n::Lang;
use datarig_core::profile::{ConnectionConfig, ProfileId};
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_tui::app::explorer::RowKind;
use datarig_tui::app::{App, AppEvent, Focus, Startup};
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// Profile with the fields of `url` but without its password (that comes from the store).
fn profile(url: &str) -> ConnectionConfig {
    let d = datarig_core::profile::dsn::parse(url).expect("DATARIG_TEST_PG_URL is a postgres:// URL");
    ConnectionConfig {
        name: "it-pg".into(),
        host: d.host.clone(),
        port: d.port.unwrap_or(5432),
        user: d.user.clone(),
        password: String::new(),
        database: d.database.clone(),
        sslmode: "disable".into(),
        ..ConnectionConfig::test_db()
    }
}

fn app_with(url: &str, stored_password: &str) -> (App, UnboundedReceiver<AppEvent>) {
    let p = profile(url);
    let store = Arc::new(MemoryStore::new());
    store.set(&p.id.account(), stored_password).unwrap();
    let cfg = Config { connections: vec![p], ..Config::default() };
    let mut app = App::new(&cfg, None, Lang::En);
    app.set_secret_store(store as Arc<dyn SecretStore>);
    let (tx, rx) = unbounded_channel();
    app.start(tx, Startup::Normal);
    (app, rx)
}

/// The id of the first profile.
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

fn enter(app: &mut App) {
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
}

/// The error line of profile `id`'s node.
fn node_error(app: &App, id: ProfileId) -> String {
    app.conns.get(id).and_then(|c| c.error.as_ref()).map(|m| m.render(&app.i18n).to_string()).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_via_the_explorer_opens_a_console() {
    let Some(url) = pg_url("connect_via_the_explorer_opens_a_console") else { return };
    let pw = datarig_core::profile::dsn::parse(&url).unwrap().password.unwrap_or_default();
    let (mut app, mut rx) = app_with(&url, &pw);
    let id = first(&app);
    assert_eq!(app.focus, Focus::Tree);
    assert!(app.conns.attempt().is_none(), "nothing connects before Enter");
    enter(&mut app);
    assert!(app.conns.attempt().is_some());
    pump(&mut app, &mut rx, 10, |a| a.conns.is_connected(id)).await;
    assert_eq!(app.tab().profile, Some(id), "the first connect opens its console");
    assert_eq!(app.focus, Focus::Tree, "the focus stays in the explorer");
    assert_eq!(app.last_used, Some(id));
    // The session works: the explorer gets the schemas of the test DB.
    pump(&mut app, &mut rx, 10, |a| a.conns.catalog(Some(id)).schemas.iter().any(|s| s == "shop")).await;
    pump(&mut app, &mut rx, 10, |a| a.conns.get(id).is_some_and(|c| !c.tree.schemas.is_empty())).await;
    // `x` disconnects; the tab stays.
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)));
    assert!(!app.conns.is_connected(id) && app.tab().exec.session.is_none());
    assert_eq!(app.tab().profile, Some(id));
}

/// A rejected login shows the server's error on the profile's node and opens the password
/// prompt. The wrong-password half needs password auth; a local server with `trust` auth
/// accepts any password, so an unknown role (rejected under any auth method) is checked as
/// well.
#[tokio::test(flavor = "multi_thread")]
async fn wrong_credentials_show_the_error_on_the_node_and_prompt() {
    let Some(url) = pg_url("wrong_credentials_show_the_error_on_the_node_and_prompt") else { return };
    let (mut app, mut rx) = app_with(&url, "definitely-wrong");
    let id = first(&app);
    enter(&mut app);
    pump(&mut app, &mut rx, 10, |a| a.conns.attempt().is_none()).await;
    if app.conns.is_connected(id) {
        assert!(std::env::var_os("CI").is_none(), "CI's PostgreSQL must check passwords");
        eprintln!("note: this server accepted a wrong password (trust auth); checking an unknown role only");
    } else {
        let notice = node_error(&app, id);
        assert!(notice.contains("password authentication failed"), "{notice}");
        assert!(app.overlays.prompt().is_some(), "the password prompt opens");
    }

    let mut bad = profile(&url);
    bad.user = "datarig_no_such_role".into();
    let cfg = Config { connections: vec![bad], ..Config::default() };
    let mut app = App::new(&cfg, None, Lang::En);
    app.set_secret_store(Arc::new(MemoryStore::new()) as Arc<dyn SecretStore>);
    let (tx, mut rx) = unbounded_channel();
    app.start(tx, Startup::Profile("it-pg".into()));
    assert!(app.conns.attempt().is_none(), "not before the first frame is on screen");
    app.first_frame_drawn(); // what the binary's event loop does
    assert!(app.conns.attempt().is_some(), "CLI profile connects directly");
    pump(&mut app, &mut rx, 10, |a| a.conns.attempt().is_none()).await;
    let id = first(&app);
    assert!(!app.conns.is_connected(id));
    let notice = node_error(&app, id);
    assert!(
        notice.contains("datarig_no_such_role") || notice.contains("password") || notice.contains("No saved password"),
        "{notice}"
    );
    assert!(app.overlays.prompt().is_some(), "an authentication failure opens the password prompt");
}

/// The password from an environment variable or a command: read at connect time,
/// and the session works.
#[tokio::test(flavor = "multi_thread")]
async fn connect_with_env_and_command_sources() {
    use datarig_core::secret::PasswordSource;
    let Some(url) = pg_url("connect_with_env_and_command_sources") else { return };
    let pw = datarig_core::profile::dsn::parse(&url).unwrap().password.unwrap_or_default();
    let mut sources = vec![PasswordSource::Env("DATARIG_IT_PASSWORD".into())];
    if cfg!(unix) {
        // `printf` prints it with a newline; the command source drops one trailing newline.
        sources.push(PasswordSource::Command(format!("printf '%s\\n' '{pw}'")));
    }
    for source in sources {
        let mut p = profile(&url);
        p.set_source(source.clone());
        let cfg = Config { connections: vec![p], ..Config::default() };
        let mut app = App::new(&cfg, None, Lang::En);
        app.set_secret_store(Arc::new(MemoryStore::new()) as Arc<dyn SecretStore>);
        let env_pw = pw.clone();
        app.set_env_lookup(Arc::new(move |k| (k == "DATARIG_IT_PASSWORD").then(|| env_pw.clone())));
        let (tx, mut rx) = unbounded_channel();
        app.start(tx, Startup::Normal);
        let id = first(&app);
        enter(&mut app);
        pump(&mut app, &mut rx, 15, |a| a.conns.is_connected(id) || a.conns.attempt().is_none()).await;
        assert!(app.conns.is_connected(id), "{source:?}: {}", node_error(&app, id));
        pump(&mut app, &mut rx, 10, |a| a.conns.catalog(Some(id)).schemas.iter().any(|s| s == "shop")).await;
    }
    // A wrong password from the command: the server's error, and no prompt (fixed at the source).
    if cfg!(unix) {
        let mut p = profile(&url);
        p.set_source(PasswordSource::Command("printf definitely-wrong".into()));
        let cfg = Config { connections: vec![p], ..Config::default() };
        let mut app = App::new(&cfg, None, Lang::En);
        app.set_secret_store(Arc::new(MemoryStore::new()) as Arc<dyn SecretStore>);
        let (tx, mut rx) = unbounded_channel();
        app.start(tx, Startup::Normal);
        let id = first(&app);
        enter(&mut app);
        pump(&mut app, &mut rx, 15, |a| a.conns.attempt().is_none()).await;
        if !app.conns.is_connected(id) {
            assert!(app.overlays.prompt().is_none(), "no prompt for a command source");
        }
    }
}

// ── tabs: per-tab sessions against the real server ─────────────────

/// A driver session outside the app that looks at the server.
struct Observer {
    session: datarig_core::driver::Session,
    rx: UnboundedReceiver<datarig_core::driver::DbEvent>,
    seq: u64,
}

impl Observer {
    async fn open(url: &str) -> Observer {
        use datarig_core::driver::{ConnectOptions, DbEvent, Driver, SessionRole};
        let cfg =
            ConnectionConfig { name: "observer".into(), dsn: Some(url.to_string()), ..ConnectionConfig::test_db() };
        let (tx, mut rx) = unbounded_channel();
        let opts = ConnectOptions::new(500, SessionRole::Query, "observer");
        let session = datarig_driver_postgres::PgDriver.connect(&cfg, SessionRole::Query, opts, tx);
        loop {
            match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
                Ok(Some(DbEvent::Connected)) => break,
                Ok(Some(DbEvent::ConnectFailed { error, .. })) => panic!("observer: {error:?}"),
                Ok(Some(_)) => {}
                other => panic!("observer: {other:?}"),
            }
        }
        Observer { session, rx, seq: 0 }
    }

    /// The first column of every row of `sql`.
    async fn column(&mut self, sql: &str) -> Vec<String> {
        use datarig_core::driver::{DbCommand, DbEvent};
        self.seq += 1;
        let id = self.seq;
        self.session.send(DbCommand::Execute { id, statements: vec![sql.to_string()] });
        loop {
            match tokio::time::timeout(Duration::from_secs(10), self.rx.recv()).await {
                Ok(Some(DbEvent::Page { id: i, rows, .. })) if i == id => {
                    return rows.into_iter().map(|r| r[0].clone().unwrap_or_default()).collect();
                }
                Ok(Some(DbEvent::Failed { id: i, error, .. })) if i == id => panic!("observer: {error:?}"),
                Ok(Some(_)) => {}
                other => panic!("observer: {other:?}"),
            }
        }
    }

    /// Connections of the app instance `tag`, as `role count` lines.
    async fn connections(&mut self, tag: &str) -> Vec<String> {
        self.column(&format!(
            "SELECT role || ' ' || n FROM (SELECT split_part(application_name, '-', 2) AS role, count(*) AS n \
             FROM pg_stat_activity WHERE application_name LIKE 'datarig-%-{tag}' GROUP BY 1) c ORDER BY 1"
        ))
        .await
    }

    /// Statements the query sessions of `tag` are running.
    async fn active(&mut self, tag: &str) -> Vec<String> {
        self.column(&format!(
            "SELECT query FROM pg_stat_activity WHERE application_name = 'datarig-q-{tag}' AND state = 'active'"
        ))
        .await
    }
}

/// The first cell of tab `i`'s result, once it has one.
fn cell(app: &App, i: usize) -> Option<String> {
    match &app.tabs.iter().nth(i)?.results {
        datarig_tui::app::Results::Rows(rs) => match rs.cell(0, 0) {
            datarig_tui::widgets::grid::CellRef::Here(c) => c.clone(),
            _ => None,
        },
        _ => None,
    }
}

fn idle(app: &App, i: usize) -> bool {
    app.tabs.iter().nth(i).is_some_and(|t| t.exec.running.is_none())
}

/// Two tabs on one profile: a transaction in one is invisible to the other, the profile keeps
/// one metadata connection, and closing a tab mid-query stops the query on the server and rolls
/// its transaction back.
#[tokio::test(flavor = "multi_thread")]
async fn tabs_have_their_own_sessions_and_closing_one_stops_its_query() {
    use datarig_tui::app::action::Action;
    let Some(url) = pg_url("tabs_have_their_own_sessions_and_closing_one_stops_its_query") else { return };
    let pw = datarig_core::profile::dsn::parse(&url).unwrap().password.unwrap_or_default();
    let tag = format!("tabs{}", std::process::id());
    let table = format!("it_tabs_{tag}");
    // Dropped last: even a failed assertion leaves no committed table behind.
    let _guard = pg_clean::TableGuard::new(&url, &[&format!("public.{table}")]);
    let (mut app, mut rx) = app_with(&url, &pw);
    app.set_instance_tag(&tag);
    let mut obs = Observer::open(&url).await;
    let id = first(&app);
    enter(&mut app);
    pump(&mut app, &mut rx, 10, |a| a.conns.is_connected(id)).await;
    assert_eq!(obs.connections(&tag).await, ["meta 1"], "connecting opens the metadata connection only");

    // Tab 1: a transaction that creates a table.
    app.run(vec!["BEGIN".into()]);
    pump(&mut app, &mut rx, 10, |a| a.tab().exec.tx_open).await;
    app.run(vec![format!("CREATE TABLE public.{table} (x int)")]);
    pump(&mut app, &mut rx, 10, |a| idle(a, 0)).await;
    assert!(app.tab().exec.tx_open, "still open after a statement");
    // Tab 2 does not see it.
    app.dispatch(Action::NewTab);
    app.run(vec![format!("SELECT count(*) FROM pg_class WHERE relname = '{table}'")]);
    // The result's implicit transaction ends right after its last page.
    pump(&mut app, &mut rx, 10, |a| cell(a, 1).is_some() && !a.tab().exec.tx_open).await;
    assert_eq!(cell(&app, 1).as_deref(), Some("0"), "tab 2 must not see tab 1's transaction");
    assert_eq!(obs.connections(&tag).await, ["meta 1", "q 2"], "one metadata connection, one per tab");

    // Back in tab 1, a row query inside the transaction keeps it open (no commit).
    app.dispatch(Action::GotoTab(1));
    app.run(vec![format!("SELECT count(*) FROM {table}")]);
    pump(&mut app, &mut rx, 10, |a| cell(a, 0).is_some()).await;
    assert!(app.tab().exec.tx_open);
    // A long statement; closing the tab asks, then cancels it and closes the connection.
    app.run(vec!["SELECT pg_sleep(60)".into()]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while obs.active(&tag).await.is_empty() {
        assert!(Instant::now() < deadline, "the statement never started");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    app.dispatch(Action::CloseTab);
    assert!(app.overlays.confirm().is_some(), "a running query and an open transaction: asked first");
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)));
    assert_eq!(app.tabs.len(), 1);
    let t0 = Instant::now();
    while !obs.active(&tag).await.is_empty() {
        assert!(t0.elapsed() < Duration::from_secs(10), "the query still runs {:?} after closing", t0.elapsed());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // The transaction was rolled back: the table never existed.
    let t0 = Instant::now();
    loop {
        let left = obs.connections(&tag).await;
        if left == ["meta 1", "q 1"] {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "{left:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(obs.column(&format!("SELECT count(*) FROM pg_class WHERE relname = '{table}'")).await, ["0"]);
    // Quitting closes the rest.
    app.dispatch(Action::Quit);
    assert!(app.quit);
    let t0 = Instant::now();
    while !obs.connections(&tag).await.is_empty() {
        assert!(t0.elapsed() < Duration::from_secs(10), "connections left after quitting");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Idle paging against the real server, with a fake clock and a 5s policy: in
/// autocommit mode closing the idle portal ends the implicit transaction (`idle`, not
/// `idle in transaction`); inside the user's `BEGIN` only the portal closes and the block, with
/// its uncommitted row, stays; a result that fits in one page never leaves anything open.
#[tokio::test(flavor = "multi_thread")]
async fn idle_paging_portal_closes_outside_the_users_transaction_only() {
    use datarig_core::policy::Policy;
    use datarig_tui::app::Paging;
    let Some(url) = pg_url("idle_paging_portal_closes_outside_the_users_transaction_only") else { return };
    let pw = datarig_core::profile::dsn::parse(&url).unwrap().password.unwrap_or_default();
    let tag = format!("page{}", std::process::id());
    let table = format!("public.it_page_{tag}");
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    let mut p = profile(&url);
    p.policy = Some("it-short".into());
    let store = Arc::new(MemoryStore::new());
    store.set(&p.id.account(), &pw).unwrap();
    let mut cfg = Config { connections: vec![p], ..Config::default() };
    cfg.policies.insert("it-short", Policy { paging_idle_timeout: Some(Duration::from_secs(5)), ..Policy::default() });
    let mut app = App::new(&cfg, None, Lang::En);
    app.set_secret_store(store as Arc<dyn SecretStore>);
    app.set_instance_tag(&tag);
    let offset = Arc::new(std::sync::Mutex::new(Duration::ZERO));
    let base = Instant::now();
    let o = offset.clone();
    app.set_clock(Arc::new(move || base + *o.lock().unwrap()));
    let advance = |app: &mut App, d: Duration| {
        *offset.lock().unwrap() += d;
        app.on_tick(base + *offset.lock().unwrap());
    };
    let (tx, mut rx) = unbounded_channel();
    app.start(tx, Startup::Normal);
    let mut obs = Observer::open(&url).await;
    let state = format!("SELECT state FROM pg_stat_activity WHERE application_name = 'datarig-q-{tag}'");
    let id = first(&app);
    enter(&mut app);
    pump(&mut app, &mut rx, 10, |a| a.conns.is_connected(id)).await;

    // Autocommit: a big result keeps its portal in an implicit transaction.
    app.run(vec!["SELECT * FROM analytics.events".into()]);
    pump(&mut app, &mut rx, 10, |a| matches!(a.tab().exec.paging, Paging::Open { .. }) && a.tab().exec.tx_open).await;
    assert_eq!(obs.column(&state).await, ["idle in transaction"]);
    advance(&mut app, Duration::from_secs(4));
    assert!(matches!(app.tab().exec.paging, Paging::Open { .. }), "not yet");
    advance(&mut app, Duration::from_secs(1));
    assert_eq!(app.tab().exec.paging, Paging::ClosedIdle);
    pump(&mut app, &mut rx, 10, |a| !a.tab().exec.tx_open).await;
    assert_eq!(obs.column(&state).await, ["idle"], "the implicit transaction ended");

    // The user's transaction: a committed table, then BEGIN and an uncommitted row.
    pg_clean::run_fresh(&url, &format!("CREATE TABLE {table} AS SELECT generate_series(1, 1200) AS x")).unwrap();
    app.run(vec!["BEGIN".into()]);
    pump(&mut app, &mut rx, 10, |a| a.tab().exec.tx_open && idle(a, 0)).await;
    app.run(vec![format!("INSERT INTO {table} VALUES (-1)")]);
    pump(&mut app, &mut rx, 10, |a| idle(a, 0)).await;
    app.run(vec![format!("SELECT x FROM {table}")]);
    pump(&mut app, &mut rx, 10, |a| matches!(a.tab().exec.paging, Paging::Open { .. })).await;
    // A portal inside the user's transaction is never closed for being idle.
    assert!(matches!(app.tab().exec.paging, Paging::Open { in_block: true, .. }));
    advance(&mut app, Duration::from_secs(60));
    assert!(matches!(app.tab().exec.paging, Paging::Open { in_block: true, .. }), "still open");
    // Nothing closes it; no TxOpen(false) may come.
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(500) {
        if let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
            app.on_app_event(ev);
        }
    }
    assert!(app.tab().exec.tx_open, "the user's transaction stays open");
    assert_eq!(obs.column(&state).await, ["idle in transaction"]);
    assert_eq!(obs.column(&format!("SELECT count(*) FROM {table} WHERE x = -1")).await, ["0"], "still uncommitted");
    app.run(vec![format!("SELECT count(*) FROM {table} WHERE x = -1")]);
    pump(&mut app, &mut rx, 10, |a| idle(a, 0) && cell(a, 0).is_some()).await;
    assert_eq!(cell(&app, 0).as_deref(), Some("1"), "the row is still there in the user's transaction");
    assert!(app.tab().exec.tx_open);
    app.run(vec!["ROLLBACK".into()]);
    pump(&mut app, &mut rx, 10, |a| !a.tab().exec.tx_open && idle(a, 0)).await;

    // Exactly one page (the page size is 500): complete at once, nothing stays open.
    app.run(vec!["SELECT generate_series(1, 500)".into()]);
    pump(&mut app, &mut rx, 10, |a| idle(a, 0) && cell(a, 0).is_some()).await;
    let t0 = Instant::now();
    while app.tab().exec.tx_open {
        assert!(t0.elapsed() < Duration::from_secs(10), "the implicit transaction stayed open");
        if let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
            app.on_app_event(ev);
        }
    }
    assert_eq!(app.tab().exec.paging, Paging::None);
    assert_eq!(obs.column(&state).await, ["idle"]);
    app.dispatch(datarig_tui::app::action::Action::Quit);
}

/// Two profiles on the same database, both connected: Enter on a table of the second one runs
/// in the second one's tab, never in the first one's, and each profile keeps
/// one metadata connection.
#[tokio::test(flavor = "multi_thread")]
async fn two_profiles_on_one_database_route_tables_to_their_own_tab() {
    let Some(url) = pg_url("two_profiles_on_one_database_route_tables_to_their_own_tab") else { return };
    let pw = datarig_core::profile::dsn::parse(&url).unwrap().password.unwrap_or_default();
    let tag = format!("two{}", std::process::id());
    let (a, b) = (
        ConnectionConfig { name: "it-a".into(), ..profile(&url) },
        ConnectionConfig { name: "it-b".into(), ..profile(&url) },
    );
    let (ida, idb) = (a.id, b.id);
    let store = Arc::new(MemoryStore::new());
    store.set(&ida.account(), &pw).unwrap();
    store.set(&idb.account(), &pw).unwrap();
    let cfg = Config { connections: vec![a, b], ..Config::default() };
    let mut app = App::new(&cfg, None, Lang::En);
    app.set_secret_store(store as Arc<dyn SecretStore>);
    app.set_instance_tag(&tag);
    let (tx, mut rx) = unbounded_channel();
    app.start(tx, Startup::Normal);
    let mut obs = Observer::open(&url).await;
    let key = |app: &mut App, c: KeyCode| app.handle_event(Event::Key(KeyEvent::new(c, KeyModifiers::NONE)));
    // Connect both from the explorer: it-a (the cursor starts there), then it-b.
    enter(&mut app);
    pump(&mut app, &mut rx, 10, |a| a.conns.is_connected(ida)).await;
    let select = |app: &mut App, kind: RowKind| {
        let rows = app.explorer_rows();
        let i = rows.iter().position(|r| r.kind == kind).expect("row");
        app.explorer.select(&rows, i);
    };
    select(&mut app, RowKind::Profile(idb));
    enter(&mut app);
    pump(&mut app, &mut rx, 10, |a| a.conns.is_connected(idb)).await;
    assert_eq!(app.tabs.len(), 2, "each profile got its console");
    let tab_of = |app: &App, id: ProfileId| app.tabs.iter().position(|t| t.profile == Some(id)).unwrap();
    // it-a's tab is active; open it-b's shop.users from the explorer.
    let (ta, tb) = (tab_of(&app, ida), tab_of(&app, idb));
    app.tabs.activate(ta);
    pump(&mut app, &mut rx, 10, |a| a.conns.get(idb).is_some_and(|c| c.tree.schemas.iter().any(|s| s.name == "shop")))
        .await;
    let shop = app.conns.get(idb).unwrap().tree.schemas.iter().position(|s| s.name == "shop").unwrap();
    select(&mut app, RowKind::Node(idb, datarig_tui::widgets::tree::Node::Schema(shop)));
    key(&mut app, KeyCode::Char('l'));
    let users = |app: &App| {
        app.explorer_rows().into_iter().find(|r| match r.kind {
            RowKind::Node(id, n) => {
                id == idb && app.conns.get(id).is_some_and(|c| c.tree.label(n, &app.i18n).0 == "users")
            }
            _ => false,
        })
    };
    pump(&mut app, &mut rx, 10, |a| users(a).is_some()).await;
    let row = users(&app).unwrap();
    select(&mut app, row.kind);
    enter(&mut app);
    assert_eq!(app.tabs.active_index(), tb, "it-b's tab becomes active");
    assert_eq!(app.focus, Focus::Tree, "the focus stays in the explorer");
    pump(&mut app, &mut rx, 10, |a| cell(a, tb).is_some()).await;
    assert!(cell(&app, ta).is_none(), "nothing ran in it-a's tab");
    assert!(app.tabs.iter().nth(ta).unwrap().exec.session.is_none(), "it-a's tab never opened a session");
    assert_eq!(obs.connections(&tag).await, ["meta 2", "q 1"], "one metadata connection per profile");
    app.dispatch(datarig_tui::app::action::Action::Quit);
}

/// Restart with a restored tab: nothing connects until the tab runs (or its
/// editor gets the focus); then its profile connects and the statement runs there.
#[tokio::test(flavor = "multi_thread")]
async fn a_restored_tab_connects_only_when_it_runs() {
    use datarig_tui::app::action::Action;
    let Some(url) = pg_url("a_restored_tab_connects_only_when_it_runs") else { return };
    let pw = datarig_core::profile::dsn::parse(&url).unwrap().password.unwrap_or_default();
    let tag = format!("restore{}", std::process::id());
    let dir = std::env::temp_dir().join(format!("datarig-it-restore-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let paths = datarig_core::paths::Paths { data: Some(dir.join("data")), state: Some(dir.join("state")) };
    let p = profile(&url);
    let store = Arc::new(MemoryStore::new());
    store.set(&p.id.account(), &pw).unwrap();
    let cfg = Config { connections: vec![p], ..Config::default() };
    let start = || {
        let mut app = App::new(&cfg, None, Lang::En);
        app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
        app.set_paths(paths.clone());
        app.set_instance_tag(&tag);
        let (tx, rx) = unbounded_channel();
        app.start(tx, Startup::Normal);
        (app, rx)
    };
    let mut obs = Observer::open(&url).await;

    // First run: connect, write a statement in the console, quit.
    let (mut app, mut rx) = start();
    let id = first(&app);
    enter(&mut app);
    pump(&mut app, &mut rx, 10, |a| a.conns.is_connected(id)).await;
    app.tab_mut().editor = datarig_tui::widgets::editor::Editor::new("SELECT 40 + 2 AS answer");
    app.dispatch(Action::Quit);
    assert!(app.quit);
    drop(app);
    let t0 = Instant::now();
    while !obs.connections(&tag).await.is_empty() {
        assert!(t0.elapsed() < Duration::from_secs(5), "the first run's connections did not close");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Second run: the tab is back on its profile, and nothing is connected.
    let (mut app, mut rx) = start();
    assert_eq!(app.tab().profile, Some(id));
    assert_eq!(app.tab().editor.text(), "SELECT 40 + 2 AS answer");
    assert_eq!(app.focus, Focus::Tree);
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(ev) = rx.try_recv() {
        app.on_app_event(ev);
    }
    assert!(obs.connections(&tag).await.is_empty(), "no datarig connection before the tab runs");
    assert!(app.conns.attempt().is_none() && app.overlays.prompt().is_none());
    // Run: the profile connects, then the statement runs in that tab.
    app.execute_current();
    pump(&mut app, &mut rx, 10, |a| cell(a, 0).is_some()).await;
    assert_eq!(cell(&app, 0).as_deref(), Some("42"));
    assert_eq!(obs.connections(&tag).await, ["meta 1", "q 1"]);
    app.dispatch(Action::Quit);
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `Ctrl+E` on a restored table tab of a table with more rows
/// than a page pages in the driver's own transaction, which is not the user's: no user block,
/// the tab connected (`●`), nothing to ask on quit.
#[tokio::test(flavor = "multi_thread")]
async fn a_restored_table_tab_pages_outside_any_user_transaction() {
    use datarig_core::workspace::{self, ExplorerState, TabState, WorkspaceState};
    use datarig_tui::app::action::Action;
    use datarig_tui::app::paging::Paging;
    use datarig_tui::widgets::tabbar::{self, State};
    let Some(url) = pg_url("a_restored_table_tab_pages_outside_any_user_transaction") else { return };
    let pw = datarig_core::profile::dsn::parse(&url).unwrap().password.unwrap_or_default();
    let tag = format!("tabletab{}", std::process::id());
    let dir = std::env::temp_dir().join(format!("datarig-it-tabletab-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let paths = datarig_core::paths::Paths { data: Some(dir.join("data")), state: Some(dir.join("state")) };
    let p = profile(&url);
    let id = p.id;
    let tab = TabState {
        id: workspace::new_id(),
        kind: workspace::TabKind::Table,
        script: None,
        profile: Some(id),
        cursor: (0, 0),
        top: 0,
        console: 0,
        table: Some(("analytics".into(), "events".into())),
        results: None,
        database: None,
        schema: None,
    };
    let ws =
        WorkspaceState { active: 0, explorer: ExplorerState::default(), tabs: vec![tab], unknown_tabs: Vec::new() };
    workspace::save(&dir.join("state"), &ws).unwrap();
    let store = Arc::new(MemoryStore::new());
    store.set(&p.id.account(), &pw).unwrap();
    let cfg = Config { connections: vec![p], ..Config::default() };
    let mut app = App::new(&cfg, None, Lang::En);
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.set_paths(paths);
    app.set_instance_tag(&tag);
    let (tx, mut rx) = unbounded_channel();
    app.start(tx, Startup::Normal);
    let mut obs = Observer::open(&url).await;
    assert!(app.tab().is_table());
    assert!(obs.connections(&tag).await.is_empty(), "restored: nothing sent");
    app.execute_current();
    pump(&mut app, &mut rx, 10, |a| matches!(a.tab().exec.paging, Paging::Open { .. }) && a.tab().exec.tx_open).await;
    let state = format!("SELECT state FROM pg_stat_activity WHERE application_name = 'datarig-q-{tag}'");
    assert_eq!(obs.column(&state).await, ["idle in transaction"], "the portal's own transaction");
    let e = &app.tab().exec;
    assert!(!e.in_block && !e.user_tx() && !e.tx_at_risk(), "not the user's transaction");
    assert!(matches!(e.paging, Paging::Open { in_block: false, .. }));
    assert_eq!(tabbar::state(&app, app.tab()), State::Connected);
    app.dispatch(Action::Quit);
    assert!(app.quit && app.overlays.confirm().is_none(), "quitting asks nothing");
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

fn pg_url(test: &str) -> Option<String> {
    match std::env::var("DATARIG_TEST_PG_URL") {
        Ok(u) if !u.is_empty() => {
            pg_clean::sweep_stale(&u);
            pg_clean::hold_slot();
            Some(u)
        }
        _ => {
            if std::env::var("DATARIG_REQUIRE_PG").is_ok_and(|v| v == "1") {
                panic!("DATARIG_TEST_PG_URL must be set when DATARIG_REQUIRE_PG=1");
            }
            // Written to the real stderr handle so it is visible even with output capture.
            let _ = writeln!(
                std::io::stderr(),
                "SKIPPED {test}: set DATARIG_TEST_PG_URL=postgres://datarig:datarig@127.0.0.1:55432/datarig"
            );
            None
        }
    }
}

/// A system clipboard that keeps what it was given.
#[derive(Clone, Default)]
struct KeptClipboard(Arc<std::sync::Mutex<Vec<String>>>);

impl datarig_tui::clipboard::SystemClipboard for KeptClipboard {
    fn set_text(&mut self, text: &str) -> Result<(), String> {
        self.0.lock().unwrap().push(text.to_string());
        Ok(())
    }
}

/// Against the real server: the key marks of a JOIN across shop.orders, users
/// and order_items come from the RowDescription and the profile's key cache, and a copy as SQL
/// INSERT of real rows (CJK, emoji, tabs, newlines, NULL, jsonb, timestamps) goes back into a
/// scratch table with exactly the same values.
#[tokio::test(flavor = "multi_thread")]
async fn key_marks_of_a_join_and_sql_insert_copies_that_run_again() {
    use datarig_tui::app::Results;
    use datarig_tui::app::action::Action;
    use datarig_tui::app::copy::{CopyFormat, CopyScope};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let Some(url) = pg_url("key_marks_of_a_join_and_sql_insert_copies_that_run_again") else { return };
    let pw = datarig_core::profile::dsn::parse(&url).unwrap().password.unwrap_or_default();
    let scratch = format!("public.zz_copy_{}", std::process::id());
    // Dropped last, also when an assertion fails.
    let _guard = pg_clean::TableGuard::new(&url, &[&scratch]);
    let (mut app, mut rx) = app_with(&url, &pw);
    let clip = KeptClipboard::default();
    let c = clip.clone();
    app.set_clipboard(Arc::new(move || Ok(Box::new(c.clone()) as Box<dyn datarig_tui::clipboard::SystemClipboard>)));
    app.set_env_lookup(Arc::new(|_| None));
    let id = first(&app);
    enter(&mut app);
    pump(&mut app, &mut rx, 10, |a| a.conns.get(id).is_some_and(|c| c.keys.catalog().is_some())).await;

    // The JOIN: every source column marked, the expression not.
    app.run(vec![
        "SELECT o.id, o.user_id, u.email, u.name, i.order_id, i.line_no, i.product_id, o.total_amount * 2 AS doubled \
         FROM shop.orders o JOIN shop.users u ON u.id = o.user_id JOIN shop.order_items i ON i.order_id = o.id \
         ORDER BY o.id, i.line_no LIMIT 5"
            .into(),
    ]);
    pump(&mut app, &mut rx, 10, |a| matches!(a.tab().results, Results::Rows(_)) && idle(a, 0)).await;
    app.detail.visible = false;
    let mut t = Terminal::new(TestBackend::new(200, 40)).unwrap();
    t.draw(|f| datarig_tui::screens::draw(f, &mut app)).unwrap();
    let y = app.layout.results.y + 1;
    let header: String = (0..200).map(|x| t.backend().buffer()[(x, y)].symbol().to_string()).collect();
    for want in
        ["PK id", "FK user_id", "UQ email", "│ name", "PK FK order_id", "PK line_no", "FK product_id", "│ doubled"]
    {
        assert!(header.contains(want), "{want} in {header}");
    }

    // A copy of real rows as SQL INSERT, run again into a scratch copy of the table.
    app.run(vec!["SELECT * FROM shop.users WHERE id <= 8 ORDER BY id".into()]);
    pump(&mut app, &mut rx, 10, |a| matches!(&a.tab().results, Results::Rows(rs) if rs.rows.len() == 8)).await;
    app.dispatch(Action::Copy(CopyScope::Fetched, CopyFormat::SqlInsert));
    let sql = clip.0.lock().unwrap().last().cloned().expect("copied");
    assert!(sql.starts_with("INSERT INTO \"shop\".\"users\" (\"id\", \"email\", "), "{sql}");
    // One statement per row (values may span lines: the statement splitter knows literals).
    let stmts = datarig_core::sql::split::split(&sql);
    assert_eq!(stmts.len(), 8, "{sql}");
    pg_clean::run_fresh(&url, &format!("CREATE TABLE {scratch} (LIKE shop.users)")).unwrap();
    for st in &stmts {
        let insert = st.body(&sql).replacen("\"shop\".\"users\"", &scratch, 1);
        pg_clean::run_fresh(&url, &insert).unwrap_or_else(|e| panic!("{e}: {insert}"));
    }
    let same = format!(
        "DO $$ BEGIN IF EXISTS (SELECT * FROM shop.users WHERE id <= 8 EXCEPT ALL SELECT * FROM {scratch}) \
         OR EXISTS (SELECT * FROM {scratch} EXCEPT ALL SELECT * FROM shop.users WHERE id <= 8) \
         THEN RAISE EXCEPTION 'the copied rows differ'; END IF; END $$"
    );
    pg_clean::run_fresh(&url, &same).expect("the INSERTs give back the same rows");
    app.dispatch(Action::Quit);
}

/// Connect a new app to `url`, wait for its key cache, run `select` and copy the result as
/// SQL INSERT; the copied text.
async fn copy_as_sql(url: &str, select: &str) -> String {
    copy_as(url, select, datarig_tui::app::copy::CopyFormat::SqlInsert).await
}

/// [`copy_as_sql`] in `format`: every fetched row.
async fn copy_as(url: &str, select: &str, format: datarig_tui::app::copy::CopyFormat) -> String {
    use datarig_tui::app::Results;
    use datarig_tui::app::action::Action;
    use datarig_tui::app::copy::CopyScope;
    let pw = datarig_core::profile::dsn::parse(url).unwrap().password.unwrap_or_default();
    let (mut app, mut rx) = app_with(url, &pw);
    let clip = KeptClipboard::default();
    let c = clip.clone();
    app.set_clipboard(Arc::new(move || Ok(Box::new(c.clone()) as Box<dyn datarig_tui::clipboard::SystemClipboard>)));
    app.set_env_lookup(Arc::new(|_| None));
    let id = first(&app);
    enter(&mut app);
    pump(&mut app, &mut rx, 10, |a| a.conns.get(id).is_some_and(|c| c.keys.catalog().is_some())).await;
    app.run(vec![select.into()]);
    pump(&mut app, &mut rx, 10, |a| matches!(a.tab().results, Results::Rows(_)) && idle(a, 0)).await;
    app.dispatch(Action::Copy(CopyScope::Fetched, format));
    let sql = clip.0.lock().unwrap().last().cloned();
    let notices = format!("{:?} {:?}", app.notices, app.transient);
    app.dispatch(Action::Quit);
    sql.unwrap_or_else(|| panic!("nothing copied: {notices}"))
}

/// Run each statement of `sql` (INSERTs into `from`) against `into` instead.
fn insert_into(url: &str, sql: &str, from: &str, into: &str) {
    let stmts = datarig_core::sql::split::split(sql);
    assert!(!stmts.is_empty(), "{sql}");
    for st in &stmts {
        let insert = st.body(sql).replacen(from, into, 1);
        pg_clean::run_fresh(url, &insert).unwrap_or_else(|e| panic!("{e}: {insert}"));
    }
}

/// Fails unless tables `a` and `b` hold the same rows, compared as text (so types without an
/// equality operator compare too), with EXCEPT ALL both ways. (The row aliases must not be
/// column names: a column shadows a row alias of the same name.)
fn same_rows(url: &str, a: &str, b: &str) {
    let same = format!(
        "DO $$ BEGIN IF EXISTS (SELECT zz_row_a::text FROM {a} zz_row_a EXCEPT ALL SELECT zz_row_b::text FROM {b} zz_row_b) \
         OR EXISTS (SELECT zz_row_b::text FROM {b} zz_row_b EXCEPT ALL SELECT zz_row_a::text FROM {a} zz_row_a) \
         THEN RAISE EXCEPTION 'the copied rows differ'; END IF; END $$"
    );
    pg_clean::run_fresh(url, &same).unwrap_or_else(|e| panic!("{a} and {b} differ: {e}"));
}

/// A copy as SQL INSERT leaves out `GENERATED ALWAYS AS (…) STORED` columns (the server
/// computes them again) and keeps `GENERATED ALWAYS AS IDENTITY` values with OVERRIDING SYSTEM
/// VALUE, so the INSERTs run and give back the same rows.
#[tokio::test(flavor = "multi_thread")]
async fn sql_insert_of_generated_and_identity_columns_runs_again() {
    let Some(url) = pg_url("sql_insert_of_generated_and_identity_columns_runs_again") else { return };
    let pid = std::process::id();
    let (src, back) = (format!("public.zz_gen_{pid}"), format!("public.zz_gen_back_{pid}"));
    let _guard = pg_clean::TableGuard::new(&url, &[&src, &back]);
    pg_clean::run_fresh(
        &url,
        &format!(
            "CREATE TABLE {src} (id int GENERATED ALWAYS AS IDENTITY PRIMARY KEY, n int, \
             twice int GENERATED ALWAYS AS (n * 2) STORED, d int GENERATED BY DEFAULT AS IDENTITY)"
        ),
    )
    .unwrap();
    pg_clean::run_fresh(&url, &format!("INSERT INTO {src} (n) VALUES (1), (NULL), (30)")).unwrap();
    pg_clean::run_fresh(&url, &format!("CREATE TABLE {back} (LIKE {src} INCLUDING GENERATED INCLUDING IDENTITY)"))
        .unwrap();
    let sql = copy_as_sql(&url, &format!("SELECT * FROM {src} ORDER BY id")).await;
    let table = format!("\"public\".\"zz_gen_{pid}\"");
    assert!(
        sql.starts_with(&format!(
            "INSERT INTO {table} (\"id\", \"n\", \"d\") OVERRIDING SYSTEM VALUE VALUES (1, 1, 1);"
        )),
        "{sql}"
    );
    insert_into(&url, &sql, &table, &back);
    same_rows(&url, &src, &back);
}

/// A scratch schema of its own, dropped with everything in it when the guard goes (also when
/// an assertion fails).
struct SchemaGuard {
    url: String,
    schema: String,
}

impl Drop for SchemaGuard {
    fn drop(&mut self) {
        let sql = format!("DROP SCHEMA IF EXISTS {} CASCADE", self.schema);
        if let Err(e) = pg_clean::run_fresh(&self.url, &sql) {
            let _ = writeln!(std::io::stderr(), "warning: {sql}: {e}");
        }
    }
}

/// Types the driver used to decode wrong (bit strings, ranges, multiranges, geometry, text
/// search, network, money, `"char"`, infinity and BC dates, enums, composites, …): a copy as SQL
/// INSERT of one column of each runs again into a copy of the table and gives back the same
/// rows (compared as text with EXCEPT ALL both ways).
#[tokio::test(flavor = "multi_thread")]
async fn sql_insert_of_every_type_round_trips() {
    let Some(url) = pg_url("sql_insert_of_every_type_round_trips") else { return };
    let schema = format!("zz_types_{}", std::process::id());
    let _guard = SchemaGuard { url: url.clone(), schema: schema.clone() };
    let setup = format!(
        "CREATE SCHEMA {schema}; \
         CREATE TYPE {schema}.mood AS ENUM ('sad', 'ok', 'happy'); \
         CREATE TYPE {schema}.pair AS (a int, b text); \
         CREATE TABLE {schema}.t (n int PRIMARY KEY, bit4 bit(4), vb varbit, i4r int4range, i8r int8range, \
           nr numrange, tsr tsrange, tstzr tstzrange, dr daterange, i4mr int4multirange, dmr datemultirange, \
           pt point, ln line, ls lseg, bx box, ci circle, pg polygon, pa path, tv tsvector, tq tsquery, \
           ip inet, cd cidr, mac macaddr, mac8 macaddr8, mo money, ch \"char\", d date, ts timestamp, \
           tstz timestamptz, iv interval, f4 real, f8 double precision, nu numeric, lsn pg_lsn, jp jsonpath, \
           rc regclass, x xml, u uuid, ba bytea, mood {schema}.mood, pr {schema}.pair, bits bit(2)[], \
           ranges int4range[], pts point[], grid int[][], words text[][], based int[], cube int8[][][]); \
         INSERT INTO {schema}.t VALUES \
          (1, B'1010', B'1', '[1,5)', '(,10]', '[1.5,2.25]', '[2024-01-01 10:00,2024-01-02)', \
           '[2024-01-01 10:00+09,)', '[2024-01-01,2024-02-01)', '{{[1,3),[5,7)}}', '{{[2024-01-01,2024-01-05)}}', \
           '(1.5,-2)', '{{1,-1,0}}', '[(0,0),(1,1)]', '(2,2),(0,0)', '<(1,1),0.5>', '((0,0),(1,0),(1,1))', \
           '[(0,0),(1,1),(2,0)]', 'a fat cat:2A sat & rat', 'fat & !(rat | cat)', '192.168.0.1/24', '10.0.0.0/8', \
           '08:00:2b:01:02:03', '08:00:2b:01:02:03:04:05', 1234.56, 'x', '2024-02-29', '2024-02-29 23:59:59.999999', \
           '2024-02-29 23:59:59.5+05:30', '1 year -2 mons 3 days -04:05:06.5', 0.1, 1.7976931348623157e308, \
           1234567890123456789012345678901234567890.123456789, '16/B374D848', '$.a[*] ? (@ > 1)', 'pg_class', \
           '<a b=\"1\">x &lt;y&gt;</a>', 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11', '\\x00ff10', 'happy', \
           ROW(1, 'a \"q\", b'), '{{10,01}}', '{{\"[1,2)\",empty}}', ARRAY['(1,2)'::point, NULL], '{{{{1,2}},{{3,NULL}}}}', \
           '{{{{\"a b\",\"q\\\"uote\"}},{{\"back\\\\slash\",NULL}},{{\"NULL\",\"\"}}}}', '[0:1]={{7,8}}', \
           '{{{{{{1}},{{2}}}},{{{{3}},{{4}}}}}}'), \
          (2, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, \
           NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, \
           NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL), \
          (3, B'0000', B'', 'empty', '[-9223372036854775808,0)', '(,)', '(-infinity,infinity)', 'empty', \
           '[0044-03-15 BC,0001-01-01)', '{{}}', '{{}}', '(0,0)', '{{0,1,0}}', '[(0,0),(0,0)]', '(0,0),(0,0)', \
           '<(0,0),0>', '((0,0))', '((0,0),(1,1))', '', '!a', '::1', '::/0', '00:00:00:00:00:00', \
           'ff:ff:ff:ff:ff:ff:ff:ff', -0.01, ' ', '0044-03-15 BC', '-infinity', 'infinity', '-infinity', \
           'NaN', '-Infinity', 'NaN', '0/0', '$', '-', '<empty/>', '00000000-0000-0000-0000-000000000000', '\\x', \
           'sad', ROW(NULL, ''), '{{}}', '{{}}', '{{}}', '{{}}', '{{}}', '[-1:-1]={{0}}', '{{}}'), \
          (4, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, \
           NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, '\\\\', '0001-01-01 BC', '0001-01-01 00:00:00 BC', \
           '0001-01-01 00:00:00+00 BC', 'infinity', 'Infinity', '-0', 'Infinity', NULL, NULL, NULL, NULL, NULL, \
           NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL)"
    );
    for st in setup.split("; ") {
        pg_clean::run_fresh(&url, st).unwrap_or_else(|e| panic!("{e}: {st}"));
    }
    let back = format!("{schema}.back");
    pg_clean::run_fresh(&url, &format!("CREATE TABLE {back} (LIKE {schema}.t)")).unwrap();
    let sql = copy_as_sql(&url, &format!("SELECT * FROM {schema}.t ORDER BY n")).await;
    assert!(!sql.contains('\0'), "no raw binary: {sql}");
    insert_into(&url, &sql, &format!("\"{schema}\".\"t\""), &back);
    same_rows(&url, &format!("{schema}.t"), &back);
}

/// A table created in the app gets into the key cache without a manual refresh, so its rows
/// copy as SQL INSERT for that table (not the `<table>` placeholder).
#[tokio::test(flavor = "multi_thread")]
async fn a_table_created_in_the_app_is_known_to_sql_insert_copies() {
    use datarig_tui::app::Results;
    use datarig_tui::app::action::Action;
    use datarig_tui::app::copy::{CopyFormat, CopyScope};
    let Some(url) = pg_url("a_table_created_in_the_app_is_known_to_sql_insert_copies") else { return };
    let schema = format!("zz_ddl_{}", std::process::id());
    let _guard = SchemaGuard { url: url.clone(), schema: schema.clone() };
    let pw = datarig_core::profile::dsn::parse(&url).unwrap().password.unwrap_or_default();
    let (mut app, mut rx) = app_with(&url, &pw);
    let clip = KeptClipboard::default();
    let c = clip.clone();
    app.set_clipboard(Arc::new(move || Ok(Box::new(c.clone()) as Box<dyn datarig_tui::clipboard::SystemClipboard>)));
    app.set_env_lookup(Arc::new(|_| None));
    let id = first(&app);
    enter(&mut app);
    pump(&mut app, &mut rx, 10, |a| a.conns.get(id).is_some_and(|c| c.keys.catalog().is_some())).await;
    for sql in [
        format!("CREATE SCHEMA {schema}"),
        format!("CREATE TABLE {schema}.t (id int PRIMARY KEY, v text)"),
        format!("INSERT INTO {schema}.t VALUES (1, 'a')"),
    ] {
        app.run(vec![sql]);
        pump(&mut app, &mut rx, 10, |a| idle(a, 0)).await;
    }
    let known = |a: &App| {
        a.conns.get(id).and_then(|c| c.keys.catalog()).is_some_and(|k| k.marks_by_name(&schema, "t", "id").pk)
    };
    pump(&mut app, &mut rx, 10, known).await;
    app.run(vec![format!("SELECT * FROM {schema}.t")]);
    pump(&mut app, &mut rx, 10, |a| matches!(a.tab().results, Results::Rows(_)) && idle(a, 0)).await;
    app.dispatch(Action::Copy(CopyScope::Fetched, CopyFormat::SqlInsert));
    let sql = clip.0.lock().unwrap().last().cloned().expect("copied");
    assert_eq!(sql, format!("INSERT INTO \"{schema}\".\"t\" (\"id\", \"v\") VALUES (1, 'a');"));
    app.dispatch(Action::Quit);
}

/// Type `:text` Enter in the app's command line.
fn command(app: &mut App, text: &str) {
    for c in std::iter::once(':').chain(text.chars()) {
        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
    }
    enter(app);
}

/// SQL INSERT copies against a real server: the RowDescription names the CTE's table for every
/// column of a CTE self-join, so only the allowlist keeps it from becoming INSERTs of rows that
/// never existed; views, set operations, subqueries and grouping are refused too; a plain
/// single-table SELECT (also `SELECT status FROM status`) is copied; `:copy insert` copies into
/// the table the user names and warns when the rows are not that table's own.
#[tokio::test(flavor = "multi_thread")]
async fn sql_insert_copies_only_one_tables_rows() {
    use datarig_tui::app::Results;
    let Some(url) = pg_url("sql_insert_copies_only_one_tables_rows") else { return };
    let schema = format!("zz_allow_{}", std::process::id());
    let _guard = SchemaGuard { url: url.clone(), schema: schema.clone() };
    for st in [
        format!("CREATE SCHEMA {schema}"),
        format!("CREATE TABLE {schema}.tree (id int PRIMARY KEY, parent int, name text)"),
        format!("INSERT INTO {schema}.tree VALUES (1, NULL, 'root'), (2, 1, 'child'), (3, 2, 'grand')"),
        format!("CREATE TABLE {schema}.status (status text)"),
        format!("INSERT INTO {schema}.status VALUES ('open')"),
        format!("CREATE VIEW {schema}.leaves AS SELECT id, name FROM {schema}.tree WHERE id > 1"),
        format!("CREATE TABLE {schema}.back (LIKE {schema}.tree)"),
    ] {
        pg_clean::run_fresh(&url, &st).unwrap_or_else(|e| panic!("{e}: {st}"));
    }
    let pw = datarig_core::profile::dsn::parse(&url).unwrap().password.unwrap_or_default();
    let (mut app, mut rx) = app_with(&url, &pw);
    let clip = KeptClipboard::default();
    let c = clip.clone();
    app.set_clipboard(Arc::new(move || Ok(Box::new(c.clone()) as Box<dyn datarig_tui::clipboard::SystemClipboard>)));
    app.set_env_lookup(Arc::new(|_| None));
    let id = first(&app);
    enter(&mut app);
    pump(&mut app, &mut rx, 10, |a| a.conns.get(id).is_some_and(|c| c.keys.catalog().is_some())).await;
    let copied = |clip: &KeptClipboard| clip.0.lock().unwrap().len();
    // Run `select`, then `copy`; what was copied (if anything) and the notice.
    async fn copy(
        app: &mut App,
        rx: &mut UnboundedReceiver<AppEvent>,
        clip: &KeptClipboard,
        select: &str,
        cmd: &str,
    ) -> (Option<String>, String) {
        let before = clip.0.lock().unwrap().len();
        app.run(vec![select.to_string()]);
        pump(app, rx, 10, |a| matches!(a.tab().results, Results::Rows(_)) && idle(a, 0)).await;
        app.focus = Focus::Results;
        command(app, cmd);
        let texts = clip.0.lock().unwrap();
        let text = (texts.len() > before).then(|| texts.last().cloned().unwrap_or_default());
        let notice = app.transient.as_ref().map(|(n, _)| n.render(&app.i18n).to_string()).unwrap_or_default();
        (text, notice)
    }
    let q = |t: &str| format!("\"{schema}\".\"{t}\"");

    // The repro: before the allowlist this became INSERTs of (2, 'root') and (3, 'child').
    let cte = format!(
        "WITH x AS (SELECT * FROM {schema}.tree) SELECT a.id, b.name FROM x a JOIN x b ON a.parent = b.id ORDER BY a.id"
    );
    let (text, notice) = copy(&mut app, &mut rx, &clip, &cte, "copy sql").await;
    assert_eq!(text, None, "{notice}");
    assert!(notice.starts_with("Not copied as INSERT: it uses WITH"), "{notice}");
    assert!(notice.ends_with(":copy insert <schema.table>"), "{notice}");
    for (select, want) in [
        (format!("SELECT id, name FROM {schema}.leaves"), "it reads a view, not a table"),
        (
            format!("SELECT id, name FROM {schema}.tree UNION ALL SELECT id, name FROM {schema}.tree"),
            "it combines results",
        ),
        (format!("SELECT id, name FROM (SELECT * FROM {schema}.tree) t"), "it has a subquery"),
        (format!("SELECT parent FROM {schema}.tree GROUP BY parent"), "it groups rows"),
        (format!("SELECT id, count(*) OVER () FROM {schema}.tree"), "it uses a window function"),
        (format!("SELECT t.id, g FROM {schema}.tree t, LATERAL generate_series(1, 2) g"), "FROM names no plain table"),
        ("VALUES (1, 'x')".to_string(), "the statement is not a single SELECT"),
        (
            format!("SELECT a.id, b.name FROM {schema}.tree a JOIN {schema}.tree b ON a.parent = b.id"),
            "more than one table",
        ),
    ] {
        let (text, notice) = copy(&mut app, &mut rx, &clip, &select, "copy sql").await;
        assert_eq!(text, None, "{select}: {notice}");
        assert!(notice.contains(want), "{select}: {notice}");
    }
    // Allowed: one plain table, with WHERE, ORDER BY and LIMIT; a table named like its column.
    let simple = format!("SELECT id, name FROM {schema}.tree t WHERE t.id > 1 ORDER BY id DESC LIMIT 5");
    let (text, notice) = copy(&mut app, &mut rx, &clip, &simple, "copy sql").await;
    assert_eq!(
        text.as_deref(),
        Some(format!("INSERT INTO {} (\"id\", \"name\") VALUES (3, 'grand');\nINSERT INTO {} (\"id\", \"name\") VALUES (2, 'child');", q("tree"), q("tree")).as_str()),
        "{notice}"
    );
    app.run(vec![format!("SET search_path = {schema}")]);
    pump(&mut app, &mut rx, 10, |a| idle(a, 0)).await;
    let (text, notice) = copy(&mut app, &mut rx, &clip, "SELECT status FROM status", "copy sql").await;
    assert_eq!(text, Some(format!("INSERT INTO {} (\"status\") VALUES ('open');", q("status"))), "{notice}");

    // `:copy insert`: the CTE's rows into a table the user names, with a warning.
    let (text, notice) = copy(&mut app, &mut rx, &clip, &cte, &format!("copy insert {schema}.back")).await;
    let text = text.unwrap_or_else(|| panic!("copied: {notice}"));
    assert_eq!(notice, format!("The rows are copied as-is; they may not exist in {}", q("back")));
    insert_into(&url, &text, &q("back"), &format!("{schema}.back"));
    // The rows went in as they were shown: they are not rows of tree.
    pg_clean::run_fresh(
        &url,
        &format!(
            "DO $$ BEGIN IF (SELECT count(*) FROM {schema}.back) <> 2 \
             OR EXISTS (SELECT id, name FROM {schema}.back INTERSECT SELECT id, name FROM {schema}.tree) \
             THEN RAISE EXCEPTION 'unexpected rows'; END IF; END $$"
        ),
    )
    .expect("two rows that are not rows of tree");
    // Its errors: nothing copied.
    let n = copied(&clip);
    for (cmd, want) in [
        (format!("copy insert {schema}.nope"), "no table"),
        (format!("copy insert {schema}.status"), "has no column “id”"),
    ] {
        let (text, notice) = copy(&mut app, &mut rx, &clip, &cte, &cmd).await;
        assert_eq!(text, None, "{cmd}: {notice}");
        assert!(notice.contains(want), "{cmd}: {notice}");
    }
    assert_eq!(copied(&clip), n);
    app.dispatch(datarig_tui::app::action::Action::Quit);
}

/// A result paged past the window of rows kept in memory: the rows spill to
/// a private file, every row copies back from it exactly, and the file goes away with the app.
#[tokio::test(flavor = "multi_thread")]
async fn a_result_paged_past_the_window_spills_and_copies_every_row() {
    use datarig_tui::app::Results;
    let Some(url) = pg_url("a_result_paged_past_the_window_spills_and_copies_every_row") else { return };
    let pw = datarig_core::profile::dsn::parse(&url).unwrap().password.unwrap_or_default();
    let state = std::env::temp_dir().join(format!("datarig-it-spill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    let p = profile(&url);
    let store = Arc::new(MemoryStore::new());
    store.set(&p.id.account(), &pw).unwrap();
    let cfg = Config { connections: vec![p], result_window_rows: 1_000, ..Config::default() };
    let mut app = App::new(&cfg, None, Lang::En);
    app.set_secret_store(store as Arc<dyn SecretStore>);
    app.set_paths(datarig_core::paths::Paths { data: None, state: Some(state.clone()) });
    let clip = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let c = clip.clone();
    app.set_clipboard(Arc::new(move || {
        Ok(Box::new(Recorder(c.clone())) as Box<dyn datarig_tui::clipboard::SystemClipboard>)
    }));
    app.set_env_lookup(Arc::new(|_: &str| None));
    let (tx, mut rx) = unbounded_channel();
    app.start(tx, Startup::Normal);
    let id = first(&app);
    enter(&mut app);
    pump(&mut app, &mut rx, 10, |a| a.conns.is_connected(id)).await;
    app.run(vec!["SELECT id, event_type, payload FROM analytics.events WHERE id <= 4321 ORDER BY id".into()]);
    pump(&mut app, &mut rx, 10, |a| matches!(a.tab().results, Results::Rows(_))).await;
    app.focus = datarig_tui::app::Focus::Results;
    let len = |a: &App| match &a.tab().results {
        Results::Rows(rs) => (rs.rows.len(), rs.more),
        _ => (0, false),
    };
    // The next page, page after page (pages are explicit).
    while len(&app).1 {
        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)));
        pump(&mut app, &mut rx, 10, |a| a.tab().exec.running.is_none()).await;
    }
    assert_eq!(len(&app).0, 4_321);
    let Results::Rows(rs) = &app.tab().results else { panic!() };
    assert!(rs.rows.resident().len() <= 1_000, "{:?}", rs.rows.resident());
    let file = rs.rows.spill_path().expect("spilled").to_path_buf();
    assert!(file.starts_with(state.join("spill")));
    // Copy every row as TSV (id column only is checked in full; every line has three cells).
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE)));
    for ch in "copy tsv_header".chars() {
        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)));
    }
    enter(&mut app);
    let text = clip.lock().unwrap().last().cloned().expect("copied");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4_322, "header and every row");
    for (i, l) in lines.iter().skip(1).enumerate() {
        assert_eq!(l.split('\t').next(), Some((i + 1).to_string().as_str()), "line {i}: {l}");
    }
    drop(app);
    assert!(!file.exists(), "the spill file goes with the app");
    let _ = std::fs::remove_dir_all(&state);
}

/// A system clipboard that records what it is given.
struct Recorder(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

impl datarig_tui::clipboard::SystemClipboard for Recorder {
    fn set_text(&mut self, text: &str) -> Result<(), String> {
        self.0.lock().unwrap().push(text.to_string());
        Ok(())
    }
}

/// A selection of several statements runs them one after the other in the
/// tab's session. The first failure stops the run and is named; the statements after it never
/// run. A cancel while one runs stops it and the rest. All well, the last one's rows show.
#[tokio::test(flavor = "multi_thread")]
async fn several_statements_run_in_order_and_stop_at_a_failure_or_a_cancel() {
    use datarig_tui::app::Results;
    use datarig_tui::app::runlog::StatementOutcome;
    let Some(url) = pg_url("several_statements_run_in_order_and_stop_at_a_failure_or_a_cancel") else { return };
    let pw = datarig_core::profile::dsn::parse(&url).unwrap().password.unwrap_or_default();
    let table = format!("public.zz_runs_{}", std::process::id());
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    let (mut app, mut rx) = app_with(&url, &pw);
    let id = first(&app);
    enter(&mut app);
    pump(&mut app, &mut rx, 10, |a| a.conns.is_connected(id)).await;
    let run = |app: &mut App, sql: Vec<String>| app.run(sql);
    run(&mut app, vec![format!("CREATE TABLE {table} (a int4)")]);
    pump(&mut app, &mut rx, 10, |a| idle(a, 0)).await;

    let insert = |v: i32| format!("INSERT INTO {table} VALUES ({v})");
    run(&mut app, vec![insert(1), "SELECT 1/0".into(), insert(2)]);
    pump(&mut app, &mut rx, 10, |a| idle(a, 0)).await;
    let outcomes: Vec<_> = app.tab().exec.run.statements.iter().map(|s| s.outcome.clone()).collect();
    assert!(
        matches!(&outcomes[..], [StatementOutcome::Affected(1), StatementOutcome::Failed(e), StatementOutcome::NotRun] if e.contains("division by zero")),
        "{outcomes:?}"
    );
    let status = app.status.as_ref().map(|n| n.render(&app.i18n).to_string()).unwrap_or_default();
    assert!(status.starts_with("Statement 2 of 3 failed (SELECT 1/0): ERROR: division by zero"), "{status}");

    // Cancelled while the second statement sleeps: it stops, the third never runs.
    run(&mut app, vec![insert(3), "SELECT pg_sleep(30)".into(), insert(4)]);
    pump(&mut app, &mut rx, 10, |a| a.tab().exec.run.running() == Some(1)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    app.cancel();
    pump(&mut app, &mut rx, 10, |a| idle(a, 0)).await;
    let outcomes: Vec<_> = app.tab().exec.run.statements.iter().map(|s| s.outcome.clone()).collect();
    assert_eq!(outcomes, [StatementOutcome::Affected(1), StatementOutcome::Cancelled, StatementOutcome::NotRun]);

    // All well: the last statement's rows are the result.
    run(&mut app, vec![insert(5), format!("SELECT a FROM {table} ORDER BY a")]);
    pump(&mut app, &mut rx, 10, |a| idle(a, 0)).await;
    let Results::Rows(rs) = &app.tab().results else { panic!("rows") };
    let values: Vec<String> = (0..rs.rows.len())
        .filter_map(|r| match rs.cell(r, 0) {
            datarig_tui::widgets::grid::CellRef::Here(c) => c.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(values, ["1", "3", "5"], "2 and 4 never ran");
}

/// Copies as SQL INSERT and UPDATE of real rows (a composite primary key,
/// CJK, emoji, quotes, newlines, NULL, jsonb) run again against scratch tables and give back
/// the same rows (EXCEPT ALL both ways), and an IN list used in a real `WHERE … IN` finds the
/// same rows.
#[tokio::test(flavor = "multi_thread")]
async fn insert_update_and_in_copies_run_again() {
    use datarig_tui::app::copy::CopyFormat;
    let Some(url) = pg_url("insert_update_and_in_copies_run_again") else { return };
    let pid = std::process::id();
    let (src, ins, upd, found, want) = (
        format!("public.zz_rt_{pid}"),
        format!("public.zz_rt_ins_{pid}"),
        format!("public.zz_rt_upd_{pid}"),
        format!("public.zz_rt_in_{pid}"),
        format!("public.zz_rt_want_{pid}"),
    );
    let _guard = pg_clean::TableGuard::new(&url, &[&src, &ins, &upd, &found, &want]);
    pg_clean::run_fresh(
        &url,
        &format!("CREATE TABLE {src} (id int, k text, name text, note text, j jsonb, n numeric, PRIMARY KEY (id, k))"),
    )
    .unwrap();
    pg_clean::run_fresh(
        &url,
        &format!(
            "INSERT INTO {src} VALUES \
             (1, 'a', '陳大文 🐘', 'it''s \"quoted\"', '{{\"x\": [1, 2]}}', 12345678901234567890.25), \
             (1, 'b', 'O''Reilly', E'two\\nlines\\ttab', NULL, -0.5), \
             (2, '漢', '', NULL, '[]', NULL), \
             (3, 'z', 'NULL', '<b>&amp;</b>', 'null', 0)"
        ),
    )
    .unwrap();
    let table = format!("\"public\".\"zz_rt_{pid}\"");
    let select = format!("SELECT * FROM {src} ORDER BY id, k");

    // INSERT into an empty copy of the table.
    pg_clean::run_fresh(&url, &format!("CREATE TABLE {ins} (LIKE {src} INCLUDING ALL)")).unwrap();
    let sql = copy_as(&url, &select, CopyFormat::SqlInsert).await;
    insert_into(&url, &sql, &table, &ins);
    same_rows(&url, &src, &ins);

    // UPDATE a copy whose other columns are all different: the keys find the rows, the rest
    // is set back.
    pg_clean::run_fresh(&url, &format!("CREATE TABLE {upd} (LIKE {src} INCLUDING ALL)")).unwrap();
    pg_clean::run_fresh(&url, &format!("INSERT INTO {upd} SELECT id, k, 'x', 'y', '{{}}', 7 FROM {src}")).unwrap();
    let sql = copy_as(&url, &select, CopyFormat::SqlUpdate).await;
    assert!(sql.starts_with(&format!("UPDATE {table} SET \"name\" = '陳大文 🐘', ")), "{sql}");
    assert!(sql.contains("WHERE \"id\" = 1 AND \"k\" = 'a';") && !sql.contains("IS NULL"), "{sql}");
    insert_into(&url, &sql, &table, &upd);
    same_rows(&url, &src, &upd);

    // IN lists of a text and of a number column, in a real WHERE.
    for (column, format) in [("name", CopyFormat::SqlIn), ("n", CopyFormat::SqlIn)] {
        let list = copy_as(&url, &format!("SELECT {column} FROM {src} ORDER BY id, k"), format).await;
        for t in [&found, &want] {
            pg_clean::run_fresh(&url, &format!("DROP TABLE IF EXISTS {t}")).unwrap();
        }
        let query = format!("CREATE TABLE {found} AS SELECT * FROM {src} WHERE {column} IN {list}");
        pg_clean::run_fresh(&url, &query).unwrap_or_else(|e| panic!("{e}: {query}"));
        let all = format!("CREATE TABLE {want} AS SELECT * FROM {src} WHERE {column} IS NOT NULL");
        pg_clean::run_fresh(&url, &all).unwrap();
        same_rows(&url, &found, &want);
    }
}

// ── safety ────────────────────────────────────────────────

/// A connected app on the test database whose profile has policy `policy` (from `cfg`).
async fn safety_app(url: &str, tag: &str, policy: Option<&str>) -> (App, UnboundedReceiver<AppEvent>) {
    use datarig_core::policy::Policy;
    let pw = datarig_core::profile::dsn::parse(url).unwrap().password.unwrap_or_default();
    let mut p = profile(url);
    p.policy = policy.map(str::to_string);
    let store = Arc::new(MemoryStore::new());
    store.set(&p.id.account(), &pw).unwrap();
    let mut cfg = Config { connections: vec![p], ..Config::default() };
    cfg.policies.insert("it-ro", Policy { read_only: true, ..Policy::default() });
    let mut app = App::new(&cfg, None, Lang::En);
    app.set_secret_store(store as Arc<dyn SecretStore>);
    app.set_instance_tag(tag);
    let (tx, mut rx) = unbounded_channel();
    app.start(tx, Startup::Normal);
    let id = first(&app);
    enter(&mut app);
    pump(&mut app, &mut rx, 10, |a| a.conns.is_connected(id)).await;
    (app, rx)
}

/// Run `sql` in the active tab and wait until it ended; `answer` answers a run confirmation.
async fn run_and_wait(app: &mut App, rx: &mut UnboundedReceiver<AppEvent>, sql: &str, answer: Option<char>) {
    app.run(vec![sql.to_string()]);
    if let Some(key) = answer {
        assert!(app.overlays.run_confirm().is_some(), "{sql} asks first");
        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)));
    }
    pump(app, rx, 10, |a| idle(a, 0)).await;
}

/// EXPLAIN ANALYZE of a DELETE of every row asks first (it does run), then keeps nothing:
/// outside a transaction, and inside the user's transaction, whose own uncommitted change
/// stays.
#[tokio::test(flavor = "multi_thread")]
async fn explain_analyze_delete_asks_and_leaves_the_rows() {
    use datarig_core::i18n::Label;
    let Some(url) = pg_url("explain_analyze_delete_asks_and_leaves_the_rows") else { return };
    let tag = format!("safe{}", std::process::id());
    let table = format!("public.it_safe_{tag}");
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    pg_clean::run_fresh(&url, &format!("CREATE TABLE {table} AS SELECT generate_series(1, 3) AS x")).unwrap();
    let mut obs = Observer::open(&url).await;
    let count = format!("SELECT count(*) FROM {table}");
    let (mut app, mut rx) = safety_app(&url, &tag, None).await;
    let explain = format!("EXPLAIN ANALYZE DELETE FROM {table}");
    // Cancelled at the question: nothing ran.
    app.run(vec![explain.clone()]);
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    assert!(app.tab().exec.running.is_none() && app.overlays.run_confirm().is_none());
    // Confirmed: it runs, the plan comes, the rows stay.
    run_and_wait(&mut app, &mut rx, &explain, Some('y')).await;
    assert!(matches!(app.tab().results, datarig_tui::app::Results::Rows(_)), "the plan");
    assert_eq!(app.status.as_ref().map(|n| n.msg.clone()), Some(Label::SafetyExplainRolledBack.into()));
    assert_eq!(obs.column(&count).await, ["3"]);
    pump(&mut app, &mut rx, 10, |a| !a.tab().exec.tx_open).await;
    // Inside the user's transaction: its insert stays, the delete does not, the block goes on.
    run_and_wait(&mut app, &mut rx, "BEGIN", None).await;
    run_and_wait(&mut app, &mut rx, &format!("INSERT INTO {table} VALUES (100)"), None).await;
    run_and_wait(&mut app, &mut rx, &explain, Some('y')).await;
    run_and_wait(&mut app, &mut rx, &count, None).await;
    assert_eq!(cell(&app, 0).as_deref(), Some("4"), "the insert stays, the delete does not");
    assert!(app.tab().exec.tx_open, "the user's transaction is still open");
    assert_eq!(obs.column(&count).await, ["3"], "uncommitted");
    run_and_wait(&mut app, &mut rx, "COMMIT", None).await;
    assert_eq!(obs.column(&count).await, ["4"], "the user's own change is committed");
}

/// A read-only policy: the app refuses a DELETE and sends nothing; a writing function called
/// from a SELECT reaches the server, which refuses it; nothing changes.
#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_policy_holds_in_the_app_and_on_the_server() {
    let Some(url) = pg_url("a_read_only_policy_holds_in_the_app_and_on_the_server") else { return };
    let tag = format!("ro{}", std::process::id());
    let table = format!("public.it_ro_{tag}");
    let func = format!("public.it_ro_write_{tag}");
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    struct DropFunction(String, String);
    impl Drop for DropFunction {
        fn drop(&mut self) {
            let _ = pg_clean::run_fresh(&self.0, &format!("DROP FUNCTION IF EXISTS {}()", self.1));
        }
    }
    let _func = DropFunction(url.clone(), func.clone());
    pg_clean::run_fresh(&url, &format!("CREATE TABLE {table} AS SELECT generate_series(1, 3) AS x")).unwrap();
    let body = format!("INSERT INTO {table} VALUES (99) RETURNING x");
    pg_clean::run_fresh(&url, &format!("CREATE FUNCTION {func}() RETURNS int LANGUAGE sql VOLATILE AS $${body}$$"))
        .unwrap();
    let mut obs = Observer::open(&url).await;
    let count = format!("SELECT count(*) FROM {table}");
    let (mut app, mut rx) = safety_app(&url, &tag, Some("it-ro")).await;
    app.run(vec![format!("DELETE FROM {table} WHERE x = 1")]);
    assert!(app.tab().exec.running.is_none() && app.tab().exec.session.is_none(), "nothing was sent");
    assert!(matches!(app.status.as_ref().map(|n| &n.msg), Some(datarig_core::i18n::Msg::SafetyReadOnlyBlocked { .. })));
    run_and_wait(&mut app, &mut rx, &format!("SELECT {func}()"), None).await;
    let error = match &app.tab().results {
        datarig_tui::app::Results::Error(e) => e.clone(),
        other => panic!("the server refuses it: {:?}", std::mem::discriminant(other)),
    };
    assert!(error.contains("read-only transaction"), "{error}");
    run_and_wait(&mut app, &mut rx, "SHOW default_transaction_read_only", None).await;
    assert_eq!(cell(&app, 0).as_deref(), Some("on"));
    assert_eq!(obs.column(&count).await, ["3"], "nothing changed");
}

/// Adversarial probes on a real server: `EXPLAIN ("analyze") DELETE` asks and keeps the rows,
/// and a read-only policy holds after a function turned the session's read-only default off:
/// the app refuses the plain `set_config()`, and every transaction stays read-only.
#[tokio::test(flavor = "multi_thread")]
async fn adversarial_probes_on_a_real_server() {
    let Some(url) = pg_url("adversarial_probes_on_a_real_server") else { return };
    let tag = format!("vp{}", std::process::id());
    let table = format!("public.it_vp_{tag}");
    let write = format!("public.it_vp_write_{tag}");
    let off = format!("public.it_vp_off_{tag}");
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    struct DropFunction(String, String);
    impl Drop for DropFunction {
        fn drop(&mut self) {
            let _ = pg_clean::run_fresh(&self.0, &format!("DROP FUNCTION IF EXISTS {}()", self.1));
        }
    }
    let _fns = [&write, &off].map(|f| DropFunction(url.clone(), f.clone()));
    pg_clean::run_fresh(&url, &format!("CREATE TABLE {table} AS SELECT generate_series(1, 3) AS x")).unwrap();
    let body = format!("INSERT INTO {table} VALUES (99) RETURNING x");
    pg_clean::run_fresh(&url, &format!("CREATE FUNCTION {write}() RETURNS int LANGUAGE sql VOLATILE AS $${body}$$"))
        .unwrap();
    let body = "SELECT set_config('default_transaction_read_only', 'off', false)";
    pg_clean::run_fresh(&url, &format!("CREATE FUNCTION {off}() RETURNS text LANGUAGE sql VOLATILE AS $${body}$$"))
        .unwrap();
    let mut obs = Observer::open(&url).await;
    let count = format!("SELECT count(*) FROM {table}");
    // (1) EXPLAIN with a quoted analyze option runs the DELETE: it asks, and the driver rolls it
    // back.
    let (mut app, mut rx) = safety_app(&url, &tag, None).await;
    run_and_wait(&mut app, &mut rx, &format!("EXPLAIN (\"analyze\") DELETE FROM {table}"), Some('y')).await;
    assert!(matches!(app.tab().results, datarig_tui::app::Results::Rows(_)), "the plan");
    assert_eq!(obs.column(&count).await, ["3"]);
    drop(app);
    // The escape under a read-only policy.
    let (mut app, mut rx) = safety_app(&url, &format!("{tag}ro"), Some("it-ro")).await;
    run_and_wait(&mut app, &mut rx, "SELECT 1", None).await;
    app.run(vec!["SELECT set_config('default_transaction_read_only','off',false)".into()]);
    assert!(app.tab().exec.running.is_none(), "refused before it is sent");
    assert!(matches!(app.status.as_ref().map(|n| &n.msg), Some(datarig_core::i18n::Msg::SafetyReadOnlyBlocked { .. })));
    // What the app cannot see: a function turns the default off.
    run_and_wait(&mut app, &mut rx, &format!("SELECT {off}()"), None).await;
    run_and_wait(&mut app, &mut rx, "SHOW default_transaction_read_only", None).await;
    assert_eq!(cell(&app, 0).as_deref(), Some("off"));
    for sql in [format!("SELECT {write}()"), "BEGIN".into(), format!("SELECT {write}()"), "ROLLBACK".into()] {
        run_and_wait(&mut app, &mut rx, &sql, None).await;
        if sql.starts_with("SELECT") {
            // The failure of the last run is in its Messages (the rows of the
            // run before stay in their result tab).
            let (_, e) = app.tab().exec.run.failed().unwrap_or_else(|| panic!("{sql}: refused"));
            assert!(e.contains("read-only transaction"), "{sql}: {e}");
            assert_eq!(app.tab().exec.view, datarig_tui::app::tabs::ResultView::Messages);
        }
    }
    assert_eq!(obs.column(&count).await, ["3"], "nothing changed");
}

/// Sent is not succeeded: the tab's prepared names follow what the server did.
/// ⓐ A `PREPARE` of a name that exists fails on the server: the name keeps its DELETE, so its
/// `EXECUTE` asks. ⓑ A run stopped by a failure before its `PREPARE`: the `PREPARE` never ran,
/// the `EXECUTE` still asks. Nothing is sent before the answer; the rows stay.
#[tokio::test(flavor = "multi_thread")]
async fn prepared_names_follow_what_the_server_did() {
    let Some(url) = pg_url("prepared_names_follow_what_the_server_did") else { return };
    let tag = format!("px{}", std::process::id());
    let table = format!("public.it_px_{tag}");
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    pg_clean::run_fresh(&url, &format!("CREATE TABLE {table} AS SELECT generate_series(1, 5) AS x")).unwrap();
    let mut obs = Observer::open(&url).await;
    let count = format!("SELECT count(*) FROM {table}");
    let (mut app, mut rx) = safety_app(&url, &tag, None).await;
    // Asks, and nothing runs when the answer is no.
    async fn asks(app: &mut App, sql: &str) {
        app.run(vec![sql.to_string()]);
        assert!(app.overlays.run_confirm().is_some(), "{sql} asks first");
        assert!(app.tab().exec.running.is_none(), "nothing is sent before the answer");
        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)));
        assert!(app.tab().exec.running.is_none() && app.overlays.run_confirm().is_none());
    }
    run_and_wait(&mut app, &mut rx, &format!("PREPARE pz AS DELETE FROM {table}"), None).await;
    // ⓐ
    run_and_wait(&mut app, &mut rx, "PREPARE pz AS SELECT 1", None).await;
    assert!(matches!(&app.tab().results, datarig_tui::app::Results::Error(e) if e.contains("already exists")));
    asks(&mut app, "EXECUTE pz").await;
    assert_eq!(obs.column(&count).await, ["5"]);
    // ⓑ
    run_and_wait(&mut app, &mut rx, "DEALLOCATE ALL", None).await;
    run_and_wait(&mut app, &mut rx, &format!("PREPARE pz AS DELETE FROM {table}"), None).await;
    app.run(vec!["SELECT 1/0".into(), "PREPARE pz AS SELECT 1".into()]);
    pump(&mut app, &mut rx, 10, |a| idle(a, 0)).await;
    assert!(matches!(&app.tab().results, datarig_tui::app::Results::Error(e) if e.contains("division by zero")));
    asks(&mut app, "EXECUTE pz").await;
    assert_eq!(obs.column(&count).await, ["5"]);
    // What the server confirmed counts: a prepared read runs at once, a failed DEALLOCATE
    // leaves the name unknown (asks), a successful one forgets it.
    run_and_wait(&mut app, &mut rx, "PREPARE ok AS SELECT 7", None).await;
    run_and_wait(&mut app, &mut rx, "EXECUTE ok", None).await;
    assert_eq!(cell(&app, 0).as_deref(), Some("7"));
    run_and_wait(&mut app, &mut rx, "DEALLOCATE pz", None).await;
    asks(&mut app, "EXECUTE pz").await;
    assert_eq!(obs.column(&count).await, ["5"], "nothing changed");
}

// ── explicit pages, counts and re-runs on a real server ───────────────────

fn press(app: &mut App, c: char) {
    let m = if c.is_ascii_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
    app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(c), m)));
}

/// The first cell of the first row the grid shows, and the rows fetched.
fn shown_page(app: &App) -> (Option<String>, usize) {
    let t = app.tab();
    let datarig_tui::app::Results::Rows(rs) = &t.results else { return (None, 0) };
    let start = t.grid.window(rs.rows.len()).start;
    let first = match rs.cell(start, 0) {
        datarig_tui::widgets::grid::CellRef::Here(c) => c.clone(),
        _ => None,
    };
    (first, rs.rows.len())
}

/// A page fetched with `n`, a count asked for with `#`, a SELECT run again past a portal that
/// another statement closed, and a portal inside the user's transaction that stays open.
#[tokio::test(flavor = "multi_thread")]
async fn pages_counts_and_re_runs_on_a_real_server() {
    use datarig_tui::app::{Focus, Paging};
    let Some(url) = pg_url("pages_counts_and_re_runs_on_a_real_server") else { return };
    let (mut app, mut rx) = safety_app(&url, &format!("pg{}", std::process::id()), None).await;
    let sql = "SELECT g FROM generate_series(1, 1234) g ORDER BY g";
    run_and_wait(&mut app, &mut rx, sql, None).await;
    assert_eq!(shown_page(&app), (Some("1".into()), 500));
    app.focus = Focus::Results;
    press(&mut app, 'n');
    pump(&mut app, &mut rx, 10, |a| idle(a, 0)).await;
    assert_eq!(shown_page(&app), (Some("501".into()), 1000));
    press(&mut app, '#');
    pump(&mut app, &mut rx, 10, |a| idle(a, 0)).await;
    assert!(matches!(&app.tab().results, datarig_tui::app::Results::Rows(rs) if rs.counted == Some(1234)));
    assert!(matches!(app.tab().exec.paging, Paging::Open { in_block: false, .. }), "the count kept the portal");
    // Another statement ends the portal; its rows stay, and past them the SELECT runs again.
    run_and_wait(&mut app, &mut rx, "SET work_mem = '8MB'", None).await;
    assert_eq!(app.tab().exec.paging, Paging::Replaced);
    app.focus = Focus::Results;
    press(&mut app, 'L');
    assert_eq!(shown_page(&app), (Some("501".into()), 1000));
    press(&mut app, 'n');
    pump(&mut app, &mut rx, 10, |a| idle(a, 0)).await;
    assert_eq!(shown_page(&app), (Some("1001".into()), 1234));
    assert!(matches!(&app.tab().results, datarig_tui::app::Results::Rows(rs) if !rs.more));
    // Inside the user's transaction the portal is never closed for being idle.
    run_and_wait(&mut app, &mut rx, "BEGIN", None).await;
    run_and_wait(&mut app, &mut rx, sql, None).await;
    assert!(matches!(app.tab().exec.paging, Paging::Open { in_block: true, .. }));
    assert_eq!(app.paging_left(), None);
    run_and_wait(&mut app, &mut rx, "ROLLBACK", None).await;
    // The block's end follows the ROLLBACK's answer.
    pump(&mut app, &mut rx, 10, |a| !a.tab().exec.tx_open).await;
    assert!(!app.tab().exec.in_block);
}

/// Results read inside the user's transaction on a real server: labelled while it is open, and
/// "rolled back" once a ROLLBACK ended it (the rows stay: the ROLLBACK returned none).
#[tokio::test(flavor = "multi_thread")]
async fn results_say_they_were_read_in_a_transaction_on_a_real_server() {
    use datarig_tui::widgets::grid::TxMark;
    let Some(url) = pg_url("results_say_they_were_read_in_a_transaction_on_a_real_server") else { return };
    let (mut app, mut rx) = safety_app(&url, &format!("tx{}", std::process::id()), None).await;
    let mark = |a: &App| match &a.tab().results {
        datarig_tui::app::Results::Rows(rs) => rs.tx,
        _ => None,
    };
    run_and_wait(&mut app, &mut rx, "SELECT 1", None).await;
    assert_eq!(mark(&app), None);
    run_and_wait(&mut app, &mut rx, "BEGIN", None).await;
    pump(&mut app, &mut rx, 10, |a| a.tab().exec.in_block).await;
    run_and_wait(&mut app, &mut rx, "SELECT 2", None).await;
    assert_eq!(mark(&app), Some(TxMark::InTx(1)));
    run_and_wait(&mut app, &mut rx, "ROLLBACK", None).await;
    pump(&mut app, &mut rx, 10, |a| !a.tab().exec.tx_open).await;
    assert_eq!(mark(&app), Some(TxMark::RolledBack));
    assert_eq!(cell(&app, 0).as_deref(), Some("2"), "the rows stay");
}

/// Past a closed portal inside the user's transaction, a SELECT run again for the user that
/// fails (division by zero on the rows it skips) runs under a savepoint: the user's
/// transaction is not aborted and keeps its insert.
#[tokio::test(flavor = "multi_thread")]
async fn a_select_run_again_in_the_users_transaction_never_aborts_it() {
    use datarig_tui::app::{Focus, Paging};
    let Some(url) = pg_url("a_select_run_again_in_the_users_transaction_never_aborts_it") else { return };
    let (mut app, mut rx) = safety_app(&url, &format!("rs{}", std::process::id()), None).await;
    run_and_wait(&mut app, &mut rx, "CREATE TEMP TABLE zz_work (x int)", None).await;
    run_and_wait(&mut app, &mut rx, "SELECT 1 / (g - 1000) AS q FROM generate_series(1, 2000) g", None).await;
    assert_eq!(shown_page(&app).1, 500);
    run_and_wait(&mut app, &mut rx, "BEGIN", None).await;
    run_and_wait(&mut app, &mut rx, "INSERT INTO zz_work VALUES (1)", None).await;
    assert_eq!(app.tab().exec.paging, Paging::Replaced);
    app.focus = Focus::Results;
    press(&mut app, 'L'); // from Messages back to the kept rows
    assert_eq!(shown_page(&app), (Some("0".into()), 500));
    press(&mut app, 'n');
    assert!(app.tab().exec.running.is_some(), "the SELECT runs again");
    pump(&mut app, &mut rx, 10, |a| idle(a, 0)).await;
    let status = app.status.as_ref().map(|n| n.render(&app.i18n).to_string()).unwrap_or_default();
    assert!(status.contains("division by zero"), "{status}");
    assert!(!app.tab().exec.tx_aborted, "the user's transaction is not aborted");
    run_and_wait(&mut app, &mut rx, "SELECT count(*) FROM zz_work", None).await;
    assert_eq!(cell(&app, 0).as_deref(), Some("1"), "the insert is kept");
    run_and_wait(&mut app, &mut rx, "ROLLBACK", None).await;
}

/// What only the server can tell for the allowlist is asked before a count or a re-run: a
/// view's query and a function of the user's that overloads a built-in's name (`abs('x')`) are
/// code the text does not show, so the rows are neither counted nor run again, with the reason;
/// a plain table is.
#[tokio::test(flavor = "multi_thread")]
async fn a_view_or_an_overload_is_never_run_again_on_a_real_server() {
    use datarig_tui::app::{Focus, Paging, Results};
    let Some(url) = pg_url("a_view_or_an_overload_is_never_run_again_on_a_real_server") else { return };
    let schema = format!("zz_rv_{}", std::process::id());
    let _guard = SchemaGuard { url: url.clone(), schema: schema.clone() };
    for sql in [
        format!("CREATE SCHEMA {schema}"),
        format!("CREATE TABLE {schema}.t AS SELECT g AS id FROM generate_series(1, 1200) g"),
        format!("CREATE VIEW {schema}.v AS SELECT * FROM {schema}.t"),
        format!("CREATE FUNCTION {schema}.abs(text) RETURNS text LANGUAGE sql AS 'SELECT $1'"),
    ] {
        pg_clean::run_fresh(&url, &sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    let (mut app, mut rx) = safety_app(&url, &format!("rv{}", std::process::id()), None).await;
    let status = |a: &App| a.tab().status.as_ref().map(|n| n.render(&a.i18n).to_string()).unwrap_or_default();
    let counted = |a: &App| match &a.tab().results {
        Results::Rows(rs) => rs.counted,
        _ => None,
    };
    run_and_wait(&mut app, &mut rx, &format!("SET search_path = {schema}, public"), None).await;
    for (sql, why) in [
        ("SELECT * FROM v ORDER BY id", "it reads v, which is not a table"),
        ("SELECT abs('x') AS a FROM generate_series(1, 1200)", "is also named abs"),
    ] {
        run_and_wait(&mut app, &mut rx, sql, None).await;
        assert!(matches!(app.tab().exec.paging, Paging::Open { .. }), "{sql}");
        app.focus = Focus::Results;
        press(&mut app, '#');
        pump(&mut app, &mut rx, 10, |a| idle(a, 0)).await;
        assert_eq!(counted(&app), None, "{sql}: not counted");
        let s = status(&app);
        assert!(s.contains("Rows are not counted for you") && s.contains(why), "{sql}: {s}");
        // Past a closed portal: not run again, nothing added.
        run_and_wait(&mut app, &mut rx, "SET work_mem = '8MB'", None).await;
        app.focus = Focus::Results;
        press(&mut app, 'L');
        assert_eq!(shown_page(&app).1, 500, "{sql}");
        press(&mut app, 'n');
        pump(&mut app, &mut rx, 10, |a| idle(a, 0)).await;
        assert_eq!(shown_page(&app).1, 500, "{sql}: nothing added");
        let s = status(&app);
        assert!(s.contains("not run again for you") && s.contains(why), "{sql}: {s}");
    }
    // A plain table is counted.
    run_and_wait(&mut app, &mut rx, "SELECT * FROM t", None).await;
    app.focus = Focus::Results;
    press(&mut app, '#');
    pump(&mut app, &mut rx, 10, |a| idle(a, 0)).await;
    assert_eq!(counted(&app), Some(1200));
}

/// Runs a drop statement (`url`, `sql`) at the end of a test, also when it fails.
struct DropGuard(String, String);

impl Drop for DropGuard {
    fn drop(&mut self) {
        let sql = &self.1;
        if let Err(e) = pg_clean::run_fresh(&self.0, sql) {
            let _ = writeln!(std::io::stderr(), "warning: {sql}: {e}");
        }
    }
}

/// On a real server: `Space c d` lists the server's databases and a database's
/// schemas; a console in a schema runs an unqualified `SELECT` of that schema's table; a console
/// in another database runs there; the profile's metadata session never moves.
#[tokio::test(flavor = "multi_thread")]
async fn a_console_works_in_the_database_and_schema_picked() {
    use datarig_tui::app::quick::QuickRow;
    let Some(url) = pg_url("a_console_works_in_the_database_and_schema_picked") else { return };
    let pw = datarig_core::profile::dsn::parse(&url).unwrap().password.unwrap_or_default();
    let pid = std::process::id();
    let (schema, db) = (format!("zz_pick_{pid}"), format!("zz_pickdb_{pid}"));
    // Dropped in reverse order: the database (its sessions closed first), then the schema.
    let _schema_guard = DropGuard(url.clone(), format!("DROP SCHEMA IF EXISTS {schema} CASCADE"));
    let _db_guard = DropGuard(url.clone(), format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"));
    for sql in [
        format!("CREATE SCHEMA {schema}"),
        format!("CREATE TABLE {schema}.zz_t (x int)"),
        format!("INSERT INTO {schema}.zz_t VALUES (7)"),
        format!("CREATE DATABASE {db}"),
    ] {
        pg_clean::run_fresh(&url, &sql).unwrap_or_else(|e| panic!("{e}: {sql}"));
    }
    let (mut app, mut rx) = app_with(&url, &pw);
    let id = first(&app);
    enter(&mut app);
    pump(&mut app, &mut rx, 10, |a| a.conns.is_connected(id)).await;
    pump(&mut app, &mut rx, 10, |a| a.conns.get(id).is_some_and(|c| c.tree.schemas.iter().any(|s| s.name == schema)))
        .await;
    let key = |app: &mut App, c: KeyCode| app.handle_event(Event::Key(KeyEvent::new(c, KeyModifiers::NONE)));
    let pick = |app: &mut App, row: &QuickRow| {
        let q = app.overlays.quick_mut().expect("the picker");
        q.selected = q.items.iter().position(|r| r == row).unwrap_or_else(|| panic!("{row:?} in {:?}", q.items));
    };
    for c in [' ', 'c', 'd'] {
        key(&mut app, KeyCode::Char(c));
    }
    let row = |a: &App, r: &QuickRow| a.overlays.quick().is_some_and(|q| q.items.contains(r));
    let theirs = QuickRow::Database(id, db.clone());
    pump(&mut app, &mut rx, 10, |a| row(a, &theirs)).await;
    let here = QuickRow::Schema(id, "datarig".into(), schema.clone());
    assert!(row(&app, &here), "the tab's database is open");
    pick(&mut app, &here);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.tab().context.schema.as_deref(), Some(schema.as_str()));
    app.run(vec!["SELECT x FROM zz_t".into()]);
    pump(&mut app, &mut rx, 10, |a| cell(a, 0).is_some()).await;
    assert_eq!(cell(&app, 0).as_deref(), Some("7"), "the unqualified name is the schema's");
    let (database, schemas) = app.tab().exec.context.clone().expect("the session said where it works");
    // `public` follows the chosen schema.
    assert_eq!((database.as_str(), schemas), ("datarig", vec![schema.clone(), "public".to_string()]));
    // Another database: its schemas from a session of its own, then a console there.
    for c in [' ', 'c', 'd'] {
        key(&mut app, KeyCode::Char(c));
    }
    pump(&mut app, &mut rx, 10, |a| row(a, &theirs)).await;
    pick(&mut app, &theirs);
    key(&mut app, KeyCode::Right);
    let public = QuickRow::Schema(id, db.clone(), "public".into());
    pump(&mut app, &mut rx, 10, |a| row(a, &public)).await;
    pick(&mut app, &theirs);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.tab().context.database.as_deref(), Some(db.as_str()));
    app.run(vec!["SELECT current_database()::text".into()]);
    pump(&mut app, &mut rx, 10, |a| cell(a, 0).is_some_and(|c| c == db)).await;
    // The explorer's metadata session still works in the profile's own database.
    assert!(app.conns.get(id).is_some_and(|c| c.tree.schemas.iter().any(|s| s.name == schema)));
    app.dispatch(datarig_tui::app::action::Action::Quit);
}
