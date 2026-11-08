//! The monitor session (`SessionRole::Monitor`) against a real PostgreSQL
//! (`DATARIG_TEST_PG_URL`, see `integration_pg.rs`) and, where set, through PgBouncer
//! (`DATARIG_TEST_POOLER_URL`, see `pooler_pg.rs`).
//!
//! The monitor only reads. The tests make what it should see with sessions of their own: a
//! table `it_mon_<what>_<pid>` (dropped by a guard), a transaction holding a row of it and
//! another waiting for that row, and a role `zz_mon_<pid>` without `pg_read_all_stats`
//! (dropped when the test ends).

mod pg_clean;

use datarig_core::driver::{ConnectOptions, DbCommand, DbError, DbEvent, Driver, Session, SessionRole};
use datarig_core::driver::{PagingMode, TxRule};
use datarig_core::monitor::{
    Blocker, FullText, GaugeKind, Hidden, MonitorFormat, Need, Part, PgMonitor, Proof, QUERY_CHARS, SessionKey,
    SessionRow, SessionState, SessionsWanted, Snapshot, TextAsk, Timestamp, Tx, Visibility, Want,
};
use datarig_core::profile::ConnectionConfig;
use datarig_driver_postgres::PgDriver;
use std::io::Write;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

fn env_url(var: &str, require: &str, example: &str, test: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            if std::env::var(require).is_ok_and(|v| v == "1") {
                panic!("{var} must be set when {require}=1");
            }
            let _ = writeln!(std::io::stderr(), "SKIPPED {test}: set {var}={example}");
            None
        }
    }
}

fn pg_url(test: &str) -> Option<String> {
    let url = env_url(
        "DATARIG_TEST_PG_URL",
        "DATARIG_REQUIRE_PG",
        "postgres://datarig:datarig@127.0.0.1:55432/datarig",
        test,
    )?;
    pg_clean::sweep_stale(&url);
    pg_clean::hold_slot();
    Some(url)
}

fn pooler_url(test: &str) -> Option<String> {
    env_url(
        "DATARIG_TEST_POOLER_URL",
        "DATARIG_REQUIRE_POOLER",
        "postgres://datarig:datarig@127.0.0.1:56432/datarig",
        test,
    )
}

/// `url` logging in as `user` with `password`.
fn as_user(url: &str, user: &str, password: &str) -> String {
    let (scheme, rest) = url.split_once("://").expect("a URL");
    let (_, host) = rest.rsplit_once('@').expect("a URL with a user");
    format!("{scheme}://{user}:{password}@{host}")
}

const TICK: Want = Want { sessions: Some(SessionsWanted { idle: true }), server: true, limit: Duration::from_secs(5) };

struct Conn {
    session: Session,
    rx: UnboundedReceiver<DbEvent>,
    next: u64,
}

impl Conn {
    async fn open(url: &str, role: SessionRole, tag: &str) -> Conn {
        let cfg = ConnectionConfig { name: "it".into(), dsn: Some(url.to_string()), ..ConnectionConfig::test_db() };
        let (tx, rx) = unbounded_channel();
        let session = PgDriver.connect(&cfg, role, ConnectOptions::new(500, role, tag), tx);
        let mut c = Conn { session, rx, next: 0 };
        c.wait(|e| matches!(e, DbEvent::Connected), 30).await;
        c
    }

    /// A monitor session, and what its probe found.
    async fn monitor(url: &str, tag: &str) -> (Conn, PgMonitor) {
        let mut c = Conn::open(url, SessionRole::Monitor, tag).await;
        match c.wait(|e| matches!(e, DbEvent::Monitor(_)), 30).await {
            DbEvent::Monitor(Ok(MonitorFormat::Postgres(pg))) => (c, pg),
            other => panic!("probe: {other:?}"),
        }
    }

    async fn wait(&mut self, pred: impl Fn(&DbEvent) -> bool, secs: u64) -> DbEvent {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.rx.recv()).await {
                Ok(Some(DbEvent::ConnectFailed { error, .. })) => panic!("connect failed: {error:?}"),
                Ok(Some(ev)) if pred(&ev) => return ev,
                Ok(Some(_)) => {}
                Ok(None) => panic!("event channel closed"),
                Err(_) => panic!("timed out after {secs}s waiting for an event"),
            }
        }
    }

    async fn tick(&mut self, want: Want) -> Result<Snapshot, DbError> {
        self.next += 1;
        let request = self.next;
        self.session.send(DbCommand::MonitorTick { request, want });
        match self.wait(|e| matches!(e, DbEvent::MonitorSnapshot { request: r, .. } if *r == request), 30).await {
            DbEvent::MonitorSnapshot { result, .. } => result,
            _ => unreachable!(),
        }
    }

    /// The whole text of the statement `row` shows.
    async fn text(&mut self, row: &SessionRow, seen: Timestamp) -> Result<FullText, DbError> {
        let start = row.query.as_ref().map(|q| q.text.clone()).unwrap_or_default();
        self.ask(TextAsk { session: row.key, query_started: row.query_started, start, seen }).await
    }

    async fn ask(&mut self, ask: TextAsk) -> Result<FullText, DbError> {
        self.next += 1;
        let request = self.next;
        self.session.send(DbCommand::MonitorQueryText { request, ask });
        match self.wait(|e| matches!(e, DbEvent::MonitorQueryText { request: r, .. } if *r == request), 30).await {
            DbEvent::MonitorQueryText { result, .. } => result,
            _ => unreachable!(),
        }
    }

    /// Send statements without waiting for their answer (one that waits for a lock).
    fn send(&mut self, sql: &str) -> u64 {
        self.next += 1;
        let id = self.next;
        let statements = vec![sql.to_string()];
        self.session.send(DbCommand::Execute { id, statements, paging: PagingMode::Hold, tx: TxRule::Auto });
        id
    }

    async fn answer(&mut self, id: u64) -> Result<Vec<Vec<Option<String>>>, String> {
        let ev = self
            .wait(
                |e| matches!(e, DbEvent::Page { id: i, .. } | DbEvent::Done { id: i, .. } | DbEvent::Failed { id: i, .. } if *i == id),
                60,
            )
            .await;
        match ev {
            DbEvent::Page { rows, .. } => Ok(rows),
            DbEvent::Done { .. } => Ok(Vec::new()),
            DbEvent::Failed { error, .. } => Err(format!("{error:?}")),
            _ => unreachable!(),
        }
    }

    async fn run(&mut self, sql: &str) -> Vec<Vec<Option<String>>> {
        let id = self.send(sql);
        self.answer(id).await.unwrap_or_else(|e| panic!("{sql}: {e}"))
    }

    async fn one(&mut self, sql: &str) -> String {
        self.run(sql).await.first().and_then(|r| r[0].clone()).unwrap_or_default()
    }
}

/// Runs `sql` on a fresh connection when it goes out of scope; declared before the sessions
/// that use what it drops, so it runs after they are gone.
struct Cleanup {
    url: String,
    sql: String,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Err(e) = pg_clean::run_fresh(&self.url, &self.sql) {
            let _ = writeln!(std::io::stderr(), "warning: {}: {e}", self.sql);
        }
    }
}

fn session_of<'a>(snapshot: &'a Snapshot, application: &str) -> &'a SessionRow {
    let list = snapshot.sessions.as_ref().expect("sessions were asked for");
    list.rows
        .iter()
        .find(|r| r.application.as_deref() == Some(application))
        .unwrap_or_else(|| panic!("no session {application} in {:?}", list.rows))
}

/// A transaction holding a row and a statement waiting for it show as such, the waiter naming
/// the holder; the monitor's own session is marked; the server and counters are read; every
/// tick is one committed transaction of the monitor's, counted for the rates.
#[tokio::test(flavor = "multi_thread")]
async fn sessions_show_who_waits_for_whom() {
    let Some(url) = pg_url("sessions_show_who_waits_for_whom") else { return };
    let pid = std::process::id();
    let table = format!("it_mon_wait_{pid}");
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    let tag = format!("mon{pid}");
    let mut holder = Conn::open(&url, SessionRole::Query, &format!("{tag}h")).await;
    holder.run(&format!("CREATE TABLE {table} (id int PRIMARY KEY, v int)")).await;
    holder.run(&format!("INSERT INTO {table} VALUES (1, 0)")).await;
    holder.run("BEGIN").await;
    holder.run(&format!("UPDATE {table} SET v = v + 1 WHERE id = 1")).await;
    let mut waiter = Conn::open(&url, SessionRole::Query, &format!("{tag}w")).await;
    let waiting = waiter.send(&format!("UPDATE {table} SET v = v + 10 WHERE id = 1"));

    let (mut monitor, pg) = Conn::monitor(&url, &tag).await;
    assert!(pg.version >= 120000);
    // The waiter is seen waiting once the server has it queued.
    let deadline = Instant::now() + Duration::from_secs(10);
    let snapshot = loop {
        let s = monitor.tick(TICK).await.expect("tick");
        if session_of(&s, &format!("datarig-q-{tag}w")).waits_for_lock() || Instant::now() > deadline {
            break s;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    let h = session_of(&snapshot, &format!("datarig-q-{tag}h"));
    assert_eq!(h.state, SessionState::IdleInTx);
    assert!(matches!(h.tx, Tx::Since(_)));
    let w = session_of(&snapshot, &format!("datarig-q-{tag}w"));
    assert!(w.waits_for_lock(), "{w:?}");
    assert_eq!(w.blocked_by, Some(vec![Blocker::Session(h.key.id)]));
    assert!(w.query.as_ref().is_some_and(|q| q.text.contains("SET v = v + 10")));
    let own = session_of(&snapshot, &format!("datarig-mon-{tag}"));
    assert!(own.monitor && own.state == SessionState::Active);
    let gauge = |k| snapshot.gauges.iter().find(|g| g.kind == k).map(|g| g.value);
    assert!(gauge(GaugeKind::WaitingForLock).is_some_and(|n| n >= 1));
    assert!(gauge(GaugeKind::IdleInTx).is_some_and(|n| n >= 1));
    let server = snapshot.server.as_ref().expect("server info was asked for");
    assert!(server.version.starts_with("PostgreSQL"), "{}", server.version);
    assert!(snapshot.hidden.is_empty(), "the dev role sees everything");

    // Counters only; one more of our transactions counted.
    let counted = snapshot.sample.own.transactions;
    let only = monitor.tick(Want { sessions: None, server: false, ..TICK }).await.expect("tick");
    assert_eq!((only.sessions.is_none(), only.server.is_none()), (true, true));
    assert_eq!(only.sample.own.transactions, counted + 1);
    assert!(only.sample.time > snapshot.sample.time);

    holder.run("ROLLBACK").await;
    waiter.answer(waiting).await.expect("the waiter goes on once the holder is gone");
}

/// A statement longer than the list holds is read whole, by its session and its start: only
/// while that session runs that statement; afterwards the read says it runs another one, and
/// once the session has ended, that it is gone. Each read is one of the monitor's own
/// transactions.
#[tokio::test(flavor = "multi_thread")]
async fn a_statements_whole_text_is_read_only_while_it_runs() {
    let Some(url) = pg_url("a_statements_whole_text_is_read_only_while_it_runs") else { return };
    let pid = std::process::id();
    let tag = format!("mont{pid}");
    let mut runner = Conn::open(&url, SessionRole::Query, &format!("{tag}r")).await;
    let pad = "x".repeat(QUERY_CHARS);
    let sql = format!("SELECT pg_sleep(2), '{pad}' AS pad /* the end {pid} */");
    let running = runner.send(&sql);
    let (mut monitor, _) = Conn::monitor(&url, &tag).await;
    let app = format!("datarig-q-{tag}r");
    let deadline = Instant::now() + Duration::from_secs(10);
    let (row, seen) = loop {
        let s = monitor.tick(TICK).await.expect("tick");
        let row = session_of(&s, &app).clone();
        if row.query.as_ref().is_some_and(|q| q.text.starts_with("SELECT pg_sleep(2)")) || Instant::now() > deadline {
            break (row, s.sessions.as_ref().expect("sessions were asked for").time);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let shown = row.query.clone().expect("the statement is listed");
    assert!(shown.shortened && shown.text.chars().count() == QUERY_CHARS, "{shown:?}");
    let before = monitor.tick(Want { sessions: None, server: false, ..TICK }).await.expect("tick").sample.own;
    match monitor.text(&row, seen).await.expect("read") {
        FullText::Text { text, proof } => {
            assert_eq!(
                (text.text.as_str(), text.shortened, text.maybe_cut_by_server, proof),
                (sql.as_str(), false, false, Proof::Same)
            );
        }
        other => panic!("{other:?}"),
    }
    let after = monitor.tick(Want { sessions: None, server: false, ..TICK }).await.expect("tick").sample.own;
    assert_eq!(after.transactions, before.transactions + 2, "the text read and the tick, nothing else");

    runner.answer(running).await.expect("the statement ends");
    runner.run("SELECT 1").await;
    assert_eq!(monitor.text(&row, seen).await, Ok(FullText::Moved), "another statement now");
    drop(runner);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match monitor.text(&row, seen).await.expect("read") {
            FullText::Gone => break,
            other => assert!(Instant::now() < deadline, "still {other:?}"),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A tick never waits for a lock: a table held in `ACCESS EXCLUSIVE` mode by another session
/// does not slow it down. A statement over its limit fails the tick alone; the next one reads.
#[tokio::test(flavor = "multi_thread")]
async fn a_tick_never_waits_and_a_slow_one_fails_alone() {
    let Some(url) = pg_url("a_tick_never_waits_and_a_slow_one_fails_alone") else { return };
    let pid = std::process::id();
    let table = format!("it_mon_lock_{pid}");
    let _guard = pg_clean::TableGuard::new(&url, &[&table]);
    let tag = format!("monl{pid}");
    let mut holder = Conn::open(&url, SessionRole::Query, &format!("{tag}h")).await;
    holder.run(&format!("CREATE TABLE {table} (id int)")).await;
    holder.run("BEGIN").await;
    holder.run(&format!("LOCK TABLE {table} IN ACCESS EXCLUSIVE MODE")).await;
    let (mut monitor, _) = Conn::monitor(&url, &tag).await;
    let t0 = Instant::now();
    monitor.tick(TICK).await.expect("tick");
    assert!(t0.elapsed() < Duration::from_secs(2), "took {:?}", t0.elapsed());
    holder.run("ROLLBACK").await;

    let slow = monitor.tick(Want { limit: Duration::from_millis(1), ..TICK }).await;
    // On a fast server a statement may still finish in a millisecond; when it did not, the
    // server's own limit stopped it.
    if let Err(e) = &slow {
        assert!(format!("{e:?}").contains("statement timeout"), "{e:?}");
    }
    monitor.tick(TICK).await.expect("the next tick reads: nothing was left open");
}

/// A role without `pg_read_all_stats` sees other roles' sessions hidden, and the monitor says
/// which grant would show them; its own session it sees in full. The role's own defaults do not
/// reach the monitor's reads: a schema of its own ahead of the catalogs whose function shadows
/// a built-in the statements call, and `SERIALIZABLE DEFERRABLE` transactions.
#[tokio::test(flavor = "multi_thread")]
async fn hidden_sessions_say_which_grant_shows_them() {
    let Some(url) = pg_url("hidden_sessions_say_which_grant_shows_them") else { return };
    let pid = std::process::id();
    let role = format!("zz_mon_{pid}");
    let schema = format!("zz_mon_s_{pid}");
    let password = format!("zz-{pid}");
    let mut admin = Conn::open(&url, SessionRole::Query, &format!("mona{pid}")).await;
    // Dropped in the reverse order: the schema, then the role.
    let _drop_role = Cleanup { url: url.clone(), sql: format!("DROP ROLE IF EXISTS {role}") };
    let _drop_schema = Cleanup { url: url.clone(), sql: format!("DROP SCHEMA IF EXISTS {schema} CASCADE") };
    admin.run(&format!("CREATE ROLE {role} LOGIN PASSWORD '{password}'")).await;
    admin.run(&format!("CREATE SCHEMA {schema}")).await;
    admin
        .run(&format!(
            "CREATE FUNCTION {schema}.current_setting(text) RETURNS text LANGUAGE sql AS $$ SELECT 'shadowed' $$"
        ))
        .await;
    admin.run(&format!("GRANT USAGE ON SCHEMA {schema} TO {role}")).await;
    admin.run(&format!("ALTER ROLE {role} SET search_path = {schema}, pg_catalog")).await;
    admin.run(&format!("ALTER ROLE {role} SET default_transaction_isolation = 'serializable'")).await;
    admin.run(&format!("ALTER ROLE {role} SET default_transaction_deferrable = on")).await;
    let tag = format!("monu{pid}");
    let (mut monitor, pg) = Conn::monitor(&as_user(&url, &role, &password), &tag).await;
    assert!(!pg.superuser && !pg.read_all_stats && !pg.signal_backend, "{pg:?}");
    let snapshot = monitor.tick(TICK).await.expect("tick");
    assert_eq!(snapshot.hidden, [Hidden { part: Part::OtherSessionDetails, need: Need::Grant("pg_read_all_stats") }]);
    let other = session_of(&snapshot, &format!("datarig-q-mona{pid}"));
    assert_eq!((other.visible, &other.state, other.query.as_ref()), (Visibility::Hidden, &SessionState::Unknown, None));
    let own = session_of(&snapshot, &format!("datarig-mon-{tag}"));
    assert_eq!((own.visible, own.user.as_deref()), (Visibility::Full, Some(role.as_str())));
    drop(monitor);
}

/// The monitor session runs nothing but its own reads: a statement sent to it is refused.
#[tokio::test(flavor = "multi_thread")]
async fn the_monitor_session_runs_no_statement() {
    let Some(url) = pg_url("the_monitor_session_runs_no_statement") else { return };
    let (mut monitor, _) = Conn::monitor(&url, &format!("monx{}", std::process::id())).await;
    let id = monitor.send("SELECT 1");
    assert_eq!(monitor.answer(id).await, Err(format!("{:?}", DbError::NotSupported)));
    monitor.tick(TICK).await.expect("still reads");
}

/// Through a pooler in transaction mode: every tick works (each is one transaction, so the
/// server connection may change between them), the monitor marks its own server connection in
/// each, and nothing it set stays on the pooled connections.
#[tokio::test(flavor = "multi_thread")]
async fn the_monitor_reads_through_a_pooler() {
    let Some(url) = pooler_url("the_monitor_reads_through_a_pooler") else { return };
    let tag = format!("monp{}", std::process::id());
    let (mut monitor, _) = Conn::monitor(&url, &tag).await;
    for _ in 0..4 {
        let s = monitor.tick(TICK).await.expect("tick");
        let own: Vec<_> = s.sessions.as_ref().unwrap().rows.iter().filter(|r| r.monitor).collect();
        assert_eq!(own.len(), 1, "exactly the reading server connection is ours");
    }
    let mut client = Conn::open(&url, SessionRole::Query, &format!("{tag}c")).await;
    for _ in 0..4 {
        assert_eq!(client.one("SHOW lock_timeout").await, "0");
        assert_eq!(client.one("SHOW statement_timeout").await, "0");
        assert_eq!(client.one("SHOW transaction_read_only").await, "off");
    }
    drop(monitor);
}

/// The other sessions answer a tick sent to them, so nothing waits for it.
#[tokio::test(flavor = "multi_thread")]
async fn other_sessions_answer_a_tick_not_supported() {
    let Some(url) = pg_url("other_sessions_answer_a_tick_not_supported") else { return };
    let pid = std::process::id();
    let mut query = Conn::open(&url, SessionRole::Query, &format!("monq{pid}")).await;
    assert_eq!(query.tick(TICK).await, Err(DbError::NotSupported));
    assert_eq!(query.one("SELECT 1").await, "1", "and goes on");
    let ask = TextAsk {
        session: SessionKey { id: 1, started: None },
        query_started: None,
        start: String::new(),
        seen: Timestamp(0),
    };
    assert_eq!(query.ask(ask.clone()).await, Err(DbError::NotSupported));
    let mut meta = Conn::open(&url, SessionRole::Meta, &format!("monm{pid}")).await;
    assert_eq!(meta.tick(TICK).await, Err(DbError::NotSupported));
    assert_eq!(meta.ask(ask).await, Err(DbError::NotSupported));
}
