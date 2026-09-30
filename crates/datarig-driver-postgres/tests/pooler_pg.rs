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
//! The tests only read: nothing is created behind the pooler.

use datarig_core::driver::{ConnectOptions, DbCommand, DbEvent, Driver, Session, SessionContext, SessionRole};
use datarig_core::profile::ConnectionConfig;
use datarig_driver_postgres::PgDriver;
use std::io::Write;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

const PAGE: usize = 50;

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
        self.session.send(DbCommand::Execute { id, statements: vec![sql.to_string()] });
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
    let mut other = Conn::open(&url, SessionRole::Query, false).await;
    let mut id = 0;
    let DbEvent::Page { rows, .. } = other.run(id, "SHOW jit").await else { panic!() };
    let jit = rows[0][0].clone();
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
