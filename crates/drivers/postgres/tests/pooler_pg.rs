//! Tests behind a real PgBouncer in transaction mode without prepared statement support
//! (`dev/docker-compose.yml`, service `pgbouncer`, profile `pooler`): consecutive
//! transactions of one client land on different server connections, so a statement prepared in
//! one is gone in the next (`26000`), and a named statement left on a server connection is seen
//! by other clients (`42P05`).
//!
//! Connection: `DATARIG_TEST_POOLER_URL` (e.g.
//! `postgres://datarig:datarig@127.0.0.1:56432/datarig`).
//! * unset locally  -> each test prints a visible `SKIPPED` line to stderr and returns;
//! * unset with `DATARIG_REQUIRE_POOLER=1` (CI's `integration` job) -> the test fails.
//!
//! The tests only read: nothing is created behind the pooler (a temporary table dropped with
//! its transaction at most).
//!
//! The pool has three server connections, shared by the tests here as they run side by side.
//! While other clients hold some of them (an open block, a cancel, a lock wait) or run in
//! between, one client's transactions can land on the same server connection run after run,
//! so the tests that count on landing elsewhere run alone ([`alone`]); the others share
//! ([`POOL`]).

use datarig_core::driver::ddl::DdlObject;
use datarig_core::driver::{ConnectOptions, DbCommand, DbError, DbEvent, Driver, Session, SessionContext, SessionRole};
use datarig_core::driver::{PagingMode, TxOutcome, TxRule, TxStep};
use datarig_core::profile::ConnectionConfig;
use datarig_driver_postgres::PgDriver;
use std::collections::HashSet;
use std::io::Write;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

const PAGE: usize = 50;

/// Shared by the tests that do not count on landing on different server connections, taken
/// alone by those that do ([`alone`]).
static POOL: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

/// Run alone among the tests here, with each of the pool's three server connections open and
/// idle (each held by a client of its own once, then given back): consecutive transactions of
/// one client then land on different ones (round robin).
async fn alone(url: &str) -> tokio::sync::RwLockWriteGuard<'static, ()> {
    let pool = POOL.write().await;
    let mut held = Vec::new();
    let mut pids = HashSet::new();
    for _ in 0..3 {
        let mut c = Conn::open(url, SessionRole::Query, false).await;
        let ev = c.run(1, "BEGIN").await;
        assert!(!matches!(ev, DbEvent::Failed { .. }), "{ev:?}");
        let ev = c.run(2, "SELECT pg_backend_pid()").await;
        let DbEvent::Page { rows, .. } = ev else { panic!("{ev:?}") };
        pids.insert(rows[0][0].clone());
        held.push(c);
    }
    assert_eq!(pids.len(), 3, "three server connections, one held by each client");
    for c in &mut held {
        let ev = c.run(3, "COMMIT").await;
        assert!(!matches!(ev, DbEvent::Failed { .. }), "{ev:?}");
    }
    pool
}

fn pooler_url(test: &str) -> Option<String> {
    match std::env::var("DATARIG_TEST_POOLER_URL") {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            if std::env::var("DATARIG_REQUIRE_POOLER").is_ok_and(|v| v == "1") {
                panic!("DATARIG_TEST_POOLER_URL must be set when DATARIG_REQUIRE_POOLER=1");
            }
            let _ = writeln!(
                std::io::stderr(),
                "SKIPPED {test}: `docker compose --profile pooler up -d` in dev/, then set \
                 DATARIG_TEST_POOLER_URL=postgres://datarig:datarig@127.0.0.1:56432/datarig"
            );
            None
        }
    }
}

struct Conn {
    session: Session,
    rx: UnboundedReceiver<DbEvent>,
    /// `StatementCacheOff` events seen so far.
    cache_off: usize,
}

impl Conn {
    async fn open(url: &str, role: SessionRole, statement_cache: bool) -> Conn {
        let opts = ConnectOptions::new(PAGE, role, &format!("pool{}", std::process::id()));
        Conn::open_with(url, role, opts.statement_cache(statement_cache)).await
    }

    async fn open_with(url: &str, role: SessionRole, opts: ConnectOptions) -> Conn {
        let cfg = ConnectionConfig { name: "pooler".into(), dsn: Some(url.to_string()), ..ConnectionConfig::default() };
        let (tx, rx) = unbounded_channel();
        let session = PgDriver.connect(&cfg, role, opts, tx);
        let mut c = Conn { session, rx, cache_off: 0 };
        c.wait(|e| matches!(e, DbEvent::Connected), 10).await;
        c
    }

    async fn wait(&mut self, pred: impl Fn(&DbEvent) -> bool, secs: u64) -> DbEvent {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.rx.recv()).await {
                Ok(Some(DbEvent::ConnectFailed { error, .. })) => panic!("connect failed: {error:?}"),
                Ok(Some(DbEvent::StatementCacheOff)) => self.cache_off += 1,
                Ok(Some(ev)) if pred(&ev) => return ev,
                Ok(Some(_)) => {}
                Ok(None) => panic!("event channel closed"),
                Err(_) => panic!("timed out after {secs}s waiting for event"),
            }
        }
    }

    async fn run(&mut self, id: u64, sql: &str) -> DbEvent {
        self.session.send(DbCommand::Execute {
            id,
            statements: vec![sql.to_string()],
            paging: PagingMode::Hold,
            tx: TxRule::Auto,
        });
        self.wait(
            |e| matches!(e, DbEvent::Page { id: i, .. } | DbEvent::Done { id: i, .. } | DbEvent::Failed { id: i, .. } if *i == id),
            30,
        )
        .await
    }

    /// Run `sql` and read every page of its result; the rows, or the failure.
    async fn rows(&mut self, id: u64, sql: &str) -> Result<usize, String> {
        let mut n = match self.run(id, sql).await {
            DbEvent::Page { rows, more: false, .. } => return Ok(rows.len()),
            DbEvent::Page { rows, .. } => rows.len(),
            DbEvent::Done { .. } => return Ok(0),
            DbEvent::Failed { error, .. } => return Err(format!("{error:?}")),
            _ => unreachable!(),
        };
        loop {
            self.session.send(DbCommand::FetchMore { id });
            match self
                .wait(|e| matches!(e, DbEvent::Page { id: i, .. } | DbEvent::Failed { id: i, .. } if *i == id), 30)
                .await
            {
                DbEvent::Page { rows, more, .. } => {
                    n += rows.len();
                    if !more {
                        return Ok(n);
                    }
                }
                DbEvent::Failed { error, .. } => return Err(format!("{error:?}")),
                _ => unreachable!(),
            }
        }
    }
}

/// Statements a session runs again and again: new and repeated texts, results of one page and
/// of several, a table's row type (a type lookup, whose own statements must not outlive the
/// transaction either).
const TEXTS: [&str; 7] = [
    "SELECT id, email FROM shop.users ORDER BY id LIMIT 20",
    "SELECT o.id, o.status FROM shop.orders o ORDER BY o.id LIMIT 120",
    "SELECT count(*) FROM shop.products",
    "SELECT 'a'::text AS x, 42 AS y",
    "SELECT n.nspname FROM pg_catalog.pg_namespace n ORDER BY 1",
    "SELECT g FROM generate_series(1, 130) g",
    "SELECT u FROM shop.users u ORDER BY u.id LIMIT 3",
];

/// The catalog reads of a metadata session work behind the pooler: none of them may depend on
/// a statement prepared in an earlier transaction, and the short `lock_timeout` and JIT off each
/// runs with are its transaction's only: another client of the pooler never gets them.
#[tokio::test(flavor = "multi_thread")]
async fn metadata_session_reads_the_catalog_behind_a_pooler() {
    let Some(url) = pooler_url("metadata_session_reads_the_catalog_behind_a_pooler") else { return };
    let _pool = POOL.read().await;
    let mut other = Conn::open(&url, SessionRole::Query, false).await;
    let mut id = 0;
    let DbEvent::Page { rows, .. } = other.run(id, "SHOW jit").await else { panic!() };
    let jit = rows[0][0].clone();
    id += 1;
    let DbEvent::Page { rows, .. } = other.run(id, "SHOW search_path").await else { panic!() };
    let path = rows[0][0].clone();
    for _ in 0..10 {
        let mut c = Conn::open(&url, SessionRole::Meta, true).await;
        let DbEvent::Schemas(schemas) = c.wait(|e| matches!(e, DbEvent::Schemas(_)), 10).await else { panic!() };
        assert!(schemas.expect("schemas").contains(&"shop".to_string()));
        let DbEvent::Catalog(catalog) = c.wait(|e| matches!(e, DbEvent::Catalog(_)), 10).await else { panic!() };
        assert!(catalog.expect("catalog").relations.iter().any(|r| r.schema == "shop" && r.name == "users"));
        for _ in 0..30 {
            c.session.send(DbCommand::LoadObjects { schema: "shop".into() });
            let DbEvent::Objects { result, .. } = c.wait(|e| matches!(e, DbEvent::Objects { .. }), 10).await else {
                panic!()
            };
            let objects = result.expect("objects");
            assert!(objects.tables.contains(&"users".to_string()));
            assert!(objects.stats.contains_key("users"), "the estimates with them");
            c.session.send(DbCommand::LoadKeys);
            let DbEvent::Keys(keys) = c.wait(|e| matches!(e, DbEvent::Keys(_)), 10).await else { panic!() };
            keys.expect("keys");
            c.session.send(DbCommand::LoadDatabases);
            let DbEvent::Databases(dbs) = c.wait(|e| matches!(e, DbEvent::Databases(_)), 10).await else { panic!() };
            assert!(dbs.expect("databases").contains(&"datarig".to_string()));
            // A table's structure (one unnamed statement, as every read here).
            c.session.send(DbCommand::LoadStructure { schema: "shop".into(), table: "orders".into() });
            let DbEvent::Structure { result, .. } = c.wait(|e| matches!(e, DbEvent::Structure { .. }), 10).await else {
                panic!()
            };
            let orders = result.expect("structure");
            assert_eq!(orders.primary_key.map(|k| k.columns), Some(vec!["id".to_string()]));
            assert!(orders.foreign_keys.iter().any(|f| f.ref_table == "users"));
            // A table's DDL: its lookup and search path are settings of its own transaction.
            let object = DdlObject::Relation { schema: "shop".into(), name: "orders".into() };
            c.session.send(DbCommand::LoadDdl { id: 1, object });
            let DbEvent::Ddl { result, .. } = c.wait(|e| matches!(e, DbEvent::Ddl { .. }), 10).await else { panic!() };
            assert!(result.is_ok(), "{result:?}");
            id += 1;
            let DbEvent::Page { rows, .. } = other.run(id, "SHOW search_path").await else { panic!() };
            assert_eq!(rows[0][0], path, "nothing left on the server connection");
            id += 1;
            let target = "SELECT coalesce(current_setting('datarig.ddl_target', true), '')";
            let DbEvent::Page { rows, .. } = other.run(id, target).await else { panic!() };
            assert_eq!(rows[0][0].as_deref(), Some(""), "nothing left on the server connection");
            id += 1;
            let DbEvent::Page { rows, .. } = other.run(id, "SHOW lock_timeout").await else { panic!() };
            assert_eq!(rows[0][0].as_deref(), Some("0"), "nothing left on the server connection");
            id += 1;
            let DbEvent::Page { rows, .. } = other.run(id, "SHOW jit").await else { panic!() };
            assert_eq!(rows[0][0], jit, "nothing left on the server connection");
        }
    }
}

/// With the statement cache off no statement outlives its transaction: runs never fail with
/// `26000` or `42P05`, and nothing turns the cache off (it is off).
#[tokio::test(flavor = "multi_thread")]
async fn statement_cache_off_runs_behind_a_pooler() {
    let Some(url) = pooler_url("statement_cache_off_runs_behind_a_pooler") else { return };
    let _pool = alone(&url).await;
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    let mut id = 0;
    for round in 0..20 {
        for sql in TEXTS {
            id += 1;
            let rows = c.rows(id, sql).await.unwrap_or_else(|e| panic!("round {round}: {sql}: {e}"));
            assert!(rows > 0, "{sql}");
        }
    }
    // Inside the user's block too, and through a statement without rows of the prepared path.
    for (i, sql) in ["BEGIN", TEXTS[0], TEXTS[5], TEXTS[0], "COMMIT"].into_iter().enumerate() {
        id += 1;
        let ev = c.run(id, sql).await;
        assert!(!matches!(ev, DbEvent::Failed { .. }), "step {i} {sql}: {ev:?}");
    }
    assert_eq!(c.cache_off, 0, "the cache was off from the start");
}

/// With the cache on (the default) behind such a pooler, prepared statements vanish: the
/// session turns its cache off once, says so once, and every run still gets its rows.
#[tokio::test(flavor = "multi_thread")]
async fn statement_cache_turns_itself_off_behind_a_pooler() {
    let Some(url) = pooler_url("statement_cache_turns_itself_off_behind_a_pooler") else { return };
    let _pool = alone(&url).await;
    let mut c = Conn::open(&url, SessionRole::Query, true).await;
    let mut id = 0;
    for round in 0..20 {
        for sql in TEXTS {
            id += 1;
            let rows = c.rows(id, sql).await.unwrap_or_else(|e| panic!("round {round}: {sql}: {e}"));
            assert!(rows > 0, "{sql}");
        }
    }
    assert_eq!(c.cache_off, 1, "turned off once, and said so once");
}

/// What a connect asks the server (where a session in a schema of its own works) and the
/// test connection's version read work behind the pooler, every time.
#[tokio::test(flavor = "multi_thread")]
async fn connect_checks_and_ping_work_behind_a_pooler() {
    let Some(url) = pooler_url("connect_checks_and_ping_work_behind_a_pooler") else { return };
    let _pool = POOL.read().await;
    let cfg = ConnectionConfig { name: "pooler".into(), dsn: Some(url.clone()), ..ConnectionConfig::default() };
    for _ in 0..20 {
        let info = PgDriver.ping(&cfg, Duration::from_secs(10), None).await.expect("ping");
        assert!(!info.server_version.is_empty());
        let context = SessionContext { database: None, schema: Some("shop".into()) };
        let (tx, rx) = unbounded_channel();
        let opts = ConnectOptions::new(PAGE, SessionRole::Query, "pool").context(context);
        let session = PgDriver.connect(&cfg, SessionRole::Query, opts, tx);
        let mut c = Conn { session, rx, cache_off: 0 };
        let DbEvent::Context { schemas, .. } = c.wait(|e| matches!(e, DbEvent::Context { .. }), 10).await else {
            panic!()
        };
        assert_eq!(schemas.first().map(String::as_str), Some("shop"));
    }
}

/// A read-only session in a schema of its own behind the pooler (which drops both startup
/// options, so each transaction starts with `BEGIN READ ONLY` and `SET LOCAL search_path`),
/// with the cache off and on: names resolve through the schema, and nothing fails.
#[tokio::test(flavor = "multi_thread")]
async fn read_only_session_in_a_schema_behind_a_pooler() {
    let Some(url) = pooler_url("read_only_session_in_a_schema_behind_a_pooler") else { return };
    let _pool = POOL.read().await;
    for cache in [false, true] {
        let context = SessionContext { database: None, schema: Some("shop".into()) };
        let opts = ConnectOptions::new(PAGE, SessionRole::Query, "pool")
            .read_only(true)
            .context(context)
            .statement_cache(cache);
        let mut c = Conn::open_with(&url, SessionRole::Query, opts).await;
        let mut id = 0;
        for round in 0..15 {
            for sql in ["SELECT id, email FROM users ORDER BY id LIMIT 60", "SELECT count(*) FROM orders", TEXTS[6]] {
                id += 1;
                let rows = c.rows(id, sql).await.unwrap_or_else(|e| panic!("cache {cache}, round {round}: {sql}: {e}"));
                assert!(rows > 0, "{sql}");
            }
        }
        for (i, sql) in ["BEGIN", "SELECT id FROM users LIMIT 3", TEXTS[6], "COMMIT"].into_iter().enumerate() {
            id += 1;
            let ev = c.run(id, sql).await;
            assert!(!matches!(ev, DbEvent::Failed { .. }), "cache {cache}, step {i} {sql}: {ev:?}");
        }
        assert!(c.cache_off <= usize::from(cache), "said at most once, and only when it was on");
    }
}

/// Results not held (the default) behind the pooler, with the cache off and on, on a read-only
/// session in a schema of its own (the pooler drops both startup options): each first page and
/// each statement run again for the next page is one transaction that starts with `BEGIN READ
/// ONLY` and the search path and ends with its page, so every row reads read-only in the
/// schema, the pages follow each other, and other clients of the pooler get none of it.
#[tokio::test(flavor = "multi_thread")]
async fn results_not_held_page_behind_a_pooler() {
    let Some(url) = pooler_url("results_not_held_page_behind_a_pooler") else { return };
    let _pool = POOL.read().await;
    let sql = "SELECT id, current_setting('transaction_read_only') AS ro, current_setting('search_path') AS path \
               FROM users ORDER BY id";
    for cache in [false, true] {
        let context = SessionContext { database: None, schema: Some("shop".into()) };
        let opts = ConnectOptions::new(PAGE, SessionRole::Query, "pool")
            .read_only(true)
            .context(context)
            .statement_cache(cache);
        let mut c = Conn::open_with(&url, SessionRole::Query, opts).await;
        let mut id = 0;
        for round in 0..10 {
            id += 1;
            c.session.send(DbCommand::Execute {
                id,
                statements: vec![sql.into()],
                paging: PagingMode::NoHold,
                tx: TxRule::Auto,
            });
            let first = c
                .wait(|e| matches!(e, DbEvent::Released { .. } | DbEvent::Page { .. } | DbEvent::Failed { .. }), 30)
                .await;
            assert!(matches!(first, DbEvent::Released { .. }), "cache {cache}, round {round}: {first:?}");
            let DbEvent::Page { rows: page1, more: true, .. } = c.wait(|e| matches!(e, DbEvent::Page { .. }), 30).await
            else {
                panic!("cache {cache}, round {round}: a page with more")
            };
            id += 1;
            c.session.send(DbCommand::Resume { id, sql: sql.into(), skip: PAGE as u64, paging: PagingMode::NoHold });
            let page2 = match c
                .wait(|e| matches!(e, DbEvent::Page { id: i, .. } | DbEvent::Failed { id: i, .. } if *i == id), 30)
                .await
            {
                DbEvent::Page { rows, .. } => rows,
                ev => panic!("cache {cache}, round {round}: {ev:?}"),
            };
            assert!(!page2.is_empty(), "cache {cache}, round {round}");
            let ids = |rows: &[Vec<Option<String>>]| -> Vec<i64> {
                rows.iter().map(|r| r[0].as_deref().unwrap_or_default().parse().unwrap()).collect()
            };
            assert!(ids(&page1).last() < ids(&page2).first(), "cache {cache}: the next page follows");
            for row in page1.iter().chain(&page2) {
                assert_eq!(row[1].as_deref(), Some("on"), "cache {cache}, round {round}: read-only");
                assert!(row[2].as_deref().is_some_and(|p| p.contains("shop")), "cache {cache}: {row:?}");
            }
        }
        assert!(c.cache_off <= usize::from(cache), "said at most once, and only when it was on");
    }
    // Nothing of it stays on the pooler's server connections.
    let mut other = Conn::open(&url, SessionRole::Query, false).await;
    for id in 0..12 {
        let DbEvent::Page { rows, .. } = other
            .run(
                id,
                "SELECT current_setting('transaction_read_only'), \
                                                       current_setting('search_path')",
            )
            .await
        else {
            panic!()
        };
        assert_eq!(rows[0][0].as_deref(), Some("off"), "{rows:?}");
        assert!(!rows[0][1].as_deref().unwrap_or_default().contains("shop"), "{rows:?}");
    }
}

/// `COPY … TO STDOUT` behind the pooler, with the statement cache on or off, held or not, in a
/// read-only session or not: refused before anything is sent, and the session goes on.
#[tokio::test(flavor = "multi_thread")]
async fn copy_to_the_client_is_refused_behind_a_pooler() {
    let Some(url) = pooler_url("copy_to_the_client_is_refused_behind_a_pooler") else { return };
    let _pool = POOL.read().await;
    let copy = "COPY (SELECT g FROM generate_series(1, 10) g) TO STDOUT";
    for (cache, read_only) in [(false, false), (true, false), (false, true)] {
        let opts = ConnectOptions::new(PAGE, SessionRole::Query, "pool").read_only(read_only).statement_cache(cache);
        let mut c = Conn::open_with(&url, SessionRole::Query, opts).await;
        for (id, paging) in [(1, PagingMode::Hold), (2, PagingMode::NoHold)] {
            c.session.send(DbCommand::Execute { id, statements: vec![copy.into()], paging, tx: TxRule::Auto });
            let ev = c
                .wait(|e| matches!(e, DbEvent::Page { id: i, .. } | DbEvent::Done { id: i, .. } | DbEvent::Failed { id: i, .. } if *i == id), 30)
                .await;
            assert!(
                matches!(ev, DbEvent::Failed { error: DbError::CopyStdio, cancelled: false, .. }),
                "cache {cache}, read-only {read_only}, {paging:?}: {ev:?}"
            );
            assert_eq!(c.rows(id + 10, "SELECT g FROM generate_series(1, 130) g").await, Ok(130));
        }
    }
}

/// The first column of the first row `sql` returns, run on the server directly (not through
/// the pooler) on a thread and runtime of its own: callable from `Drop` while a test unwinds.
fn direct(url: &str, sql: &str) -> Result<String, String> {
    let (url, sql) = (url.to_string(), sql.to_string());
    let worker = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
        rt.block_on(async move {
            let (client, conn) =
                tokio_postgres::connect(&url, tokio_postgres::NoTls).await.map_err(|e| e.to_string())?;
            tokio::spawn(conn);
            let rows = client.simple_query(&sql).await.map_err(|e| e.to_string())?;
            Ok(rows
                .iter()
                .find_map(|m| match m {
                    tokio_postgres::SimpleQueryMessage::Row(r) => Some(r.get(0).unwrap_or_default().to_string()),
                    _ => None,
                })
                .unwrap_or_default())
        })
    });
    worker.join().map_err(|_| "the thread panicked".to_string())?
}

/// Ends every backend that runs a statement with `tag` in its text when it goes out of scope
/// (also when an assertion fails), so a COPY a test started never keeps running on the server.
struct TerminateGuard {
    url: String,
    tag: String,
}

impl Drop for TerminateGuard {
    fn drop(&mut self) {
        let sql = format!(
            "SELECT count(pg_terminate_backend(pid)) FROM pg_stat_activity \
             WHERE query LIKE '%{}%' AND pid <> pg_backend_pid()",
            self.tag
        );
        if let Err(e) = direct(&self.url, &sql) {
            let _ = writeln!(std::io::stderr(), "warning: {sql}: {e}");
        }
    }
}

/// Behind the pooler, a COPY with the client hidden from a lexer that keeps
/// `standard_conforming_strings` on (set off for the user's block only) is refused before it is
/// sent, and the block goes on; the server runs nothing of it. Before the session read the
/// text both ways it reached the server, and only the client library's check ended it.
#[tokio::test(flavor = "multi_thread")]
async fn a_copy_hidden_by_backslash_strings_is_refused_behind_a_pooler() {
    let Some(url) = pooler_url("a_copy_hidden_by_backslash_strings_is_refused_behind_a_pooler") else { return };
    let _pool = POOL.read().await;
    let Ok(pg) = std::env::var("DATARIG_TEST_PG_URL") else {
        let _ = writeln!(std::io::stderr(), "SKIPPED: DATARIG_TEST_PG_URL watches the server behind the pooler");
        return;
    };
    let tag = format!("zzpoolcopy{}", std::process::id());
    let _guard = TerminateGuard { url: pg.clone(), tag: tag.clone() };
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    for (id, sql) in [(1, "BEGIN"), (2, "SET LOCAL standard_conforming_strings = off")] {
        let ev = c.run(id, sql).await;
        assert!(matches!(ev, DbEvent::Done { .. }), "{sql}: {ev:?}");
    }
    let copy = format!(r"COPY (SELECT g, 'a\'' FROM generate_series(1, 200000000) g) TO STDOUT -- ' {tag}");
    c.session.send(DbCommand::Execute { id: 3, statements: vec![copy], paging: PagingMode::Hold, tx: TxRule::Auto });
    let mut seen = Vec::new();
    let mut deadline = Instant::now() + Duration::from_secs(10);
    while let Ok(Some(ev)) = tokio::time::timeout_at(deadline.into(), c.rx.recv()).await {
        let answer =
            matches!(&ev, DbEvent::Page { id: 3, .. } | DbEvent::Done { id: 3, .. } | DbEvent::Failed { id: 3, .. });
        if answer || matches!(ev, DbEvent::Lost { .. }) {
            if seen.is_empty() {
                deadline = Instant::now() + Duration::from_millis(300);
            }
            seen.push(ev);
        }
    }
    assert!(
        matches!(&seen[..], [DbEvent::Failed { id: 3, error: DbError::CopyStdio, cancelled: false }]),
        "refused, and the session goes on: {seen:?}"
    );
    let DbEvent::Page { rows, .. } = c.run(4, r"SELECT 'b\''").await else { panic!("the block goes on") };
    assert_eq!(rows[0][0].as_deref(), Some("b'"), "still off in the block");
    assert!(matches!(c.run(5, "ROLLBACK").await, DbEvent::Done { .. }));
    let active = format!(
        "SELECT count(*) FROM pg_stat_activity WHERE query LIKE '%{tag}%' AND state = 'active' \
         AND pid <> pg_backend_pid()"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while direct(&pg, &active).as_deref() != Ok("0") {
        assert!(Instant::now() < deadline, "the server still runs it");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Behind the pooler, inside the user's block: a table named by a reserved word after its schema
/// (`pg_temp.to`) is the COPY's table, not where its data goes. The COPY is refused before it is
/// sent and the block goes on.
#[tokio::test(flavor = "multi_thread")]
async fn a_copy_of_a_table_named_to_is_refused_behind_a_pooler() {
    let Some(url) = pooler_url("a_copy_of_a_table_named_to_is_refused_behind_a_pooler") else { return };
    let _pool = POOL.read().await;
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    for (id, sql) in [(1, "BEGIN"), (2, r#"CREATE TEMP TABLE "to" (a int) ON COMMIT DROP"#)] {
        let ev = c.run(id, sql).await;
        assert!(matches!(ev, DbEvent::Done { .. }), "{sql}: {ev:?}");
    }
    c.session.send(DbCommand::Execute {
        id: 3,
        statements: vec!["COPY pg_temp.to TO STDOUT".into()],
        paging: PagingMode::Hold,
        tx: TxRule::Auto,
    });
    let mut seen = Vec::new();
    let mut deadline = Instant::now() + Duration::from_secs(10);
    while let Ok(Some(ev)) = tokio::time::timeout_at(deadline.into(), c.rx.recv()).await {
        let answer =
            matches!(&ev, DbEvent::Page { id: 3, .. } | DbEvent::Done { id: 3, .. } | DbEvent::Failed { id: 3, .. });
        if answer || matches!(ev, DbEvent::Lost { .. }) {
            if seen.is_empty() {
                deadline = Instant::now() + Duration::from_millis(300);
            }
            seen.push(ev);
        }
    }
    assert!(
        matches!(&seen[..], [DbEvent::Failed { id: 3, error: DbError::CopyStdio, cancelled: false }]),
        "refused, and the block goes on: {seen:?}"
    );
    let DbEvent::Page { rows, .. } = c.run(4, "SELECT count(*) FROM pg_temp.to").await else {
        panic!("the block goes on")
    };
    assert_eq!(rows[0][0].as_deref(), Some("0"));
    assert!(matches!(c.run(5, "ROLLBACK").await, DbEvent::Done { .. }));
}

/// A cancel goes through the pooler (PgBouncer forwards it to the server connection the
/// statement runs on): the statement ends cancelled and the session works on. A cancel that
/// reached the pooler is never taken for one that could not be sent.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_goes_through_the_pooler_and_the_session_works_on() {
    let Some(url) = pooler_url("a_cancel_goes_through_the_pooler_and_the_session_works_on") else { return };
    let _pool = POOL.read().await;
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    c.session.send(DbCommand::Execute {
        id: 1,
        statements: vec!["SELECT pg_sleep(30)".into()],
        paging: PagingMode::Hold,
        tx: TxRule::Auto,
    });
    // A cancel that arrives before the statement runs is lost; this one is well after.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let t0 = Instant::now();
    c.session.cancel();
    let mut seen = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(10), c.rx.recv()).await {
            Ok(Some(ev @ DbEvent::Failed { id: 1, .. })) => {
                assert!(matches!(ev, DbEvent::Failed { cancelled: true, .. }), "{ev:?}");
                break;
            }
            Ok(Some(ev)) => seen.push(ev),
            other => panic!("{other:?} after {seen:?}"),
        }
    }
    assert!(t0.elapsed() < Duration::from_secs(10), "cancel took {:?}", t0.elapsed());
    assert!(!seen.iter().any(|e| matches!(e, DbEvent::Lost { .. })), "{seen:?}");
    let DbEvent::Page { rows, .. } = c.run(2, "SELECT 42").await else { panic!("the session works on") };
    assert_eq!(rows[0][0].as_deref(), Some("42"));
    // Nothing else came: no `Lost` after the answer either.
    assert!(c.rx.try_recv().is_err());
}

impl Conn {
    /// Every event `cmd` brings, up to the state that follows its answer (a fetch of no result
    /// answers at once, without asking the server, after them).
    async fn events_of(&mut self, cmd: DbCommand) -> Vec<DbEvent> {
        const MARK: u64 = u64::MAX;
        self.session.send(cmd);
        self.session.send(DbCommand::FetchMore { id: MARK });
        let mut out = Vec::new();
        loop {
            match self.wait(|_| true, 30).await {
                DbEvent::Page { id: MARK, .. } => return out,
                ev => out.push(ev),
            }
        }
    }
}

/// Manual commit behind the pooler, with the statement cache off and on: from the first write
/// (a temporary table dropped when the transaction ends, so nothing is left behind) to the
/// commit, the transaction stays on one server connection, and row-returning statements run in
/// it; its idle limit (`SET LOCAL`, with the `BEGIN`) is that transaction's only: the pooled
/// connections show the server's defaults afterwards, as they do `lock_timeout`.
#[tokio::test(flavor = "multi_thread")]
async fn a_manual_transaction_stays_on_one_server_connection_behind_a_pooler() {
    let Some(url) = pooler_url("a_manual_transaction_stays_on_one_server_connection_behind_a_pooler") else {
        return;
    };
    let _pool = POOL.read().await;
    let mut other = Conn::open(&url, SessionRole::Query, false).await;
    let show = "SHOW idle_in_transaction_session_timeout";
    let DbEvent::Page { rows, .. } = other.run(1, show).await else { panic!("{show}") };
    let idle = rows[0][0].clone();
    let mut id = 1;
    for cache in [false, true] {
        let mut q = Conn::open(&url, SessionRole::Query, cache).await;
        let rule = |opens: &[bool]| TxRule::Manual {
            steps: opens.iter().map(|&o| if o { TxStep::Open } else { TxStep::AsIs }).collect(),
            idle_timeout: Some(Duration::from_secs(90)),
        };
        let run = |id, sqls: &[&str], tx| DbCommand::Execute {
            id,
            statements: sqls.iter().map(|s| s.to_string()).collect(),
            paging: PagingMode::Hold,
            tx,
        };
        let first = [
            "CREATE TEMP TABLE zz_manual (x int) ON COMMIT DROP",
            "INSERT INTO zz_manual VALUES (1) RETURNING x",
            "SELECT pg_backend_pid()",
        ];
        let events = q.events_of(run(1, &first, rule(&[true, true, false]))).await;
        assert!(events.iter().any(|e| matches!(e, DbEvent::ManualBegun { id: 1, index: 0 })), "{events:?}");
        let Some(DbEvent::Page { rows, .. }) = events.iter().find(|e| matches!(e, DbEvent::Page { id: 1, .. })) else {
            panic!("cache {cache}: {events:?}")
        };
        let pid = rows[0][0].clone();
        for i in 2..8 {
            let sqls =
                ["INSERT INTO zz_manual SELECT max(x) + 1 FROM zz_manual RETURNING x", "SELECT pg_backend_pid()"];
            let events = q.events_of(run(i, &sqls, rule(&[true, false]))).await;
            let Some(DbEvent::Page { rows, .. }) =
                events.iter().find(|e| matches!(e, DbEvent::Page { id, .. } if *id == i))
            else {
                panic!("cache {cache}: {events:?}")
            };
            assert_eq!(rows[0][0], pid, "cache {cache}: one server connection for the transaction");
        }
        let DbEvent::Page { rows, .. } = q.run(8, "SELECT count(*) FROM zz_manual").await else { panic!() };
        assert_eq!(rows[0][0].as_deref(), Some("7"), "cache {cache}");
        let DbEvent::Page { rows, .. } = q.run(9, show).await else { panic!() };
        assert_eq!(rows[0][0].as_deref(), Some("90s"), "cache {cache}: the transaction's limit");
        let events = q.events_of(DbCommand::EndTx { id: 10, commit: true }).await;
        assert!(
            events.iter().any(|e| matches!(e, DbEvent::TxEnded { id: 10, outcome: TxOutcome::Committed })),
            "cache {cache}: {events:?}"
        );
        assert!(events.iter().any(|e| matches!(e, DbEvent::Block(false))), "{events:?}");
        for _ in 0..8 {
            for c in [&mut q, &mut other] {
                id += 1;
                let DbEvent::Page { rows, .. } = c.run(id, show).await else { panic!() };
                assert_eq!(rows[0][0], idle, "cache {cache}: nothing left on the server connection");
                id += 1;
                let DbEvent::Page { rows, .. } = c.run(id, "SHOW lock_timeout").await else { panic!() };
                assert_eq!(rows[0][0].as_deref(), Some("0"), "cache {cache}: nothing left on the server connection");
            }
        }
    }
}

/// The lock wait limit behind the pooler goes in each transaction (`SET LOCAL`, nothing for the
/// session): a read sees it, a wait for a transaction-level advisory lock another client holds is
/// cut short (`LockTimeout`), and the pooled server connections keep the server's `lock_timeout`
/// afterwards. The server's idle limit is not set behind a pooler (it would end server
/// connections other clients share): the session outlives it.
#[tokio::test(flavor = "multi_thread")]
async fn behind_a_pooler_the_lock_wait_limit_is_per_transaction_and_no_idle_limit_is_set() {
    let Some(url) = pooler_url("behind_a_pooler_the_lock_wait_limit_is_per_transaction_and_no_idle_limit_is_set")
    else {
        return;
    };
    let _pool = POOL.read().await;
    let mut other = Conn::open(&url, SessionRole::Query, false).await;
    let DbEvent::Page { rows, .. } = other.run(1, "SHOW lock_timeout").await else { panic!("lock_timeout") };
    let server = rows[0][0].clone();
    let tag = format!("poolw{}", std::process::id());
    for cache in [false, true] {
        let opts = ConnectOptions::new(PAGE, SessionRole::Query, &tag)
            .statement_cache(cache)
            .lock_wait(Some(Duration::from_secs(1)))
            .server_idle(Some(Duration::from_secs(1)));
        let mut q = Conn::open_with(&url, SessionRole::Query, opts).await;
        let DbEvent::Page { rows, .. } = q.run(1, "SELECT pg_catalog.current_setting('lock_timeout')").await else {
            panic!("cache {cache}")
        };
        assert_eq!(rows[0][0].as_deref(), Some("1s"), "cache {cache}: its transaction's");
        // Another client holds a transaction-level advisory lock (gone with its transaction).
        let key = 0x7a7a_0000_i64 + i64::from(std::process::id());
        let hold = DbCommand::Execute {
            id: 2,
            statements: vec![format!("SELECT pg_catalog.pg_advisory_xact_lock({key})")],
            paging: PagingMode::Hold,
            tx: TxRule::Manual { steps: vec![TxStep::Open], idle_timeout: None },
        };
        let events = other.events_of(hold).await;
        assert!(events.iter().any(|e| matches!(e, DbEvent::ManualBegun { id: 2, .. })), "{events:?}");
        let start = Instant::now();
        let waited = q.run(3, &format!("SELECT pg_catalog.pg_advisory_xact_lock({key})")).await;
        assert!(matches!(waited, DbEvent::Failed { error: DbError::LockTimeout(_), .. }), "cache {cache}: {waited:?}");
        assert!(start.elapsed() < Duration::from_secs(10), "{:?}", start.elapsed());
        let events = other.events_of(DbCommand::EndTx { id: 4, commit: false }).await;
        assert!(events.iter().any(|e| matches!(e, DbEvent::TxEnded { id: 4, .. })), "{events:?}");
        // Past the idle limit it was given, the session still works.
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(matches!(q.run(5, "SELECT 1").await, DbEvent::Page { .. }), "cache {cache}");
        // Another client goes round the pooled server connections (`q`'s own statements may run
        // in a transaction of its own that sets the limit, with the cache off).
        let mut id = 10;
        for _ in 0..8 {
            id += 1;
            let DbEvent::Page { rows, .. } = other.run(id, "SHOW lock_timeout").await else { panic!() };
            assert_eq!(rows[0][0], server, "cache {cache}: nothing left on the server connection");
            id += 1;
            let DbEvent::Page { rows, .. } = other.run(id, "SHOW idle_session_timeout").await else { panic!() };
            assert_eq!(rows[0][0].as_deref(), Some("0"), "cache {cache}: never set behind the pooler");
        }
    }
}

/// A query session with start-up statements behind the pooler: each goes first in every
/// transaction as `SET LOCAL` (in order: a later one of the same setting wins), one with no such
/// form (`SET ROLE`) is not sent and said so (`SessionPerTransaction`, before `Connected`), and
/// the session still connects; another client going round the pooled server connections finds
/// the server's settings on each (nothing stays). A value the server refuses fails the connect
/// there too.
#[tokio::test(flavor = "multi_thread")]
async fn behind_a_pooler_start_up_statements_are_per_transaction() {
    use datarig_core::driver::SessionStatement;
    use datarig_core::profile::session::{Defaults, plan};
    let Some(url) = pooler_url("behind_a_pooler_start_up_statements_are_per_transaction") else { return };
    let mut other = Conn::open(&url, SessionRole::Query, false).await;
    let DbEvent::Page { rows, .. } = other.run(1, "SHOW work_mem").await else { panic!("work_mem") };
    let work_mem = rows[0][0].clone();
    let DbEvent::Page { rows, .. } = other.run(2, "SHOW statement_timeout").await else { panic!("timeout") };
    let timeout = rows[0][0].clone();
    assert_ne!(work_mem.as_deref(), Some("9MB"));
    let profile = ConnectionConfig {
        session_sql: vec![
            "SET work_mem = '7MB'".into(),
            "SET ROLE datarig".into(),
            "SET statement_timeout = '45s'".into(),
            "SET work_mem TO '9MB'".into(),
        ],
        ..ConnectionConfig::default()
    };
    let sql = plan(&profile, &Default::default(), Defaults::default(), &[]).unwrap().statements();
    assert_eq!(sql[1].per_transaction, None, "an identity has no form for one transaction");
    let tag = format!("pools{}", std::process::id());
    for cache in [false, true] {
        let opts = ConnectOptions::new(PAGE, SessionRole::Query, &tag).statement_cache(cache).session_sql(sql.clone());
        let cfg = ConnectionConfig { name: "pooler".into(), dsn: Some(url.clone()), ..ConnectionConfig::default() };
        let (tx, mut rx) = unbounded_channel();
        let session = PgDriver.connect(&cfg, SessionRole::Query, opts, tx);
        // Said before `Connected`, with the one not sent.
        let mut said = Vec::new();
        loop {
            match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("an answer") {
                Some(DbEvent::Connected) => break,
                Some(DbEvent::ConnectFailed { error, .. }) => panic!("{error:?}"),
                Some(DbEvent::SessionPerTransaction { refused }) => said.push(refused),
                Some(_) => {}
                None => panic!("closed"),
            }
        }
        assert_eq!(said, [vec![1]], "cache {cache}");
        let mut q = Conn { session, rx, cache_off: 0 };
        let mut id = 1;
        for (setting, want) in [("work_mem", "9MB"), ("statement_timeout", "45s")] {
            id += 1;
            let DbEvent::Page { rows, .. } = q.run(id, &format!("SHOW {setting}")).await else { panic!("{setting}") };
            assert_eq!(rows[0][0].as_deref(), Some(want), "cache {cache}: {setting} in its transaction");
            id += 1;
            let read = format!("SELECT pg_catalog.current_setting('{setting}') FROM generate_series(1, 2)");
            let DbEvent::Page { rows, .. } = q.run(id, &read).await else { panic!("{setting}") };
            assert_eq!(rows[0][0].as_deref(), Some(want), "cache {cache}: a query sees it");
        }
        // Not sent: the session's user stays the login's.
        let DbEvent::Page { rows, .. } = q.run(20, "SELECT current_user::text").await else { panic!("user") };
        assert_eq!(rows[0][0].as_deref(), Some("datarig"));
        let mut id = 30;
        for _ in 0..8 {
            id += 1;
            let DbEvent::Page { rows, .. } = other.run(id, "SHOW work_mem").await else { panic!() };
            assert_eq!(rows[0][0], work_mem, "cache {cache}: nothing left on the server connection");
            id += 1;
            let DbEvent::Page { rows, .. } = other.run(id, "SHOW statement_timeout").await else { panic!() };
            assert_eq!(rows[0][0], timeout, "cache {cache}: nothing left on the server connection");
        }
    }
    // The server checks each `SET LOCAL` once at connect: a value it refuses fails it.
    let bad = vec![SessionStatement {
        sql: "SET work_mem = 'lots'".into(),
        per_transaction: Some("SET LOCAL work_mem = 'lots'".into()),
    }];
    let cfg = ConnectionConfig { name: "pooler".into(), dsn: Some(url.clone()), ..ConnectionConfig::default() };
    let (tx, mut rx) = unbounded_channel();
    let _s = PgDriver.connect(
        &cfg,
        SessionRole::Query,
        ConnectOptions::new(PAGE, SessionRole::Query, &tag).session_sql(bad),
        tx,
    );
    loop {
        match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("an answer") {
            Some(DbEvent::ConnectFailed { error: DbError::SessionSql { index: 0, .. }, .. }) => break,
            Some(DbEvent::Connected) => panic!("connected with a refused value"),
            Some(DbEvent::ConnectFailed { error, .. }) => panic!("{error:?}"),
            Some(_) => {}
            None => panic!("closed"),
        }
    }
    // And still nothing on the pooled connections.
    for id in 60..64 {
        let DbEvent::Page { rows, .. } = other.run(id, "SHOW work_mem").await else { panic!() };
        assert_eq!(rows[0][0], work_mem);
    }
}
