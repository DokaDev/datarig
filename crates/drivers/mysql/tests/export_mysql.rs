//! `DbCommand::Export` against a real MySQL (`DATARIG_TEST_MYSQL_URL` and
//! `DATARIG_TEST_MYSQL_ADMIN_URL`, as `integration_mysql`): every row streams into a bounded
//! channel the reader drains, never cut by the session's paging limit; a keyset's chunks read
//! every row once in key order; a cancel (`KILL QUERY`) and a reader that stops end the statement
//! and its metadata lock; inside the user's transaction it reads there. Tables are
//! `shop.zz_it_export_*` of this process, dropped when each test ends.

use datarig_core::driver::{
    Cell, ColumnMeta, ConnectOptions, DbCommand, DbError, DbEvent, Driver, ExportBatch, ExportGuard, ExportRead,
    PagingMode, Session, SessionRole,
};
use datarig_core::export::keyset::{Keyset, table_key};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::Dialect;
use datarig_driver_mysql::MyDriver;
use mysql_async::prelude::Queryable;
use std::io::Write;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{Receiver, UnboundedReceiver, unbounded_channel};

const PAGE: usize = 500;

fn env_url(var: &str, test: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            if std::env::var("DATARIG_REQUIRE_MYSQL").is_ok_and(|v| v == "1") {
                panic!("{var} must be set when DATARIG_REQUIRE_MYSQL=1");
            }
            let _ = writeln!(std::io::stderr(), "SKIPPED {test}: set {var}");
            None
        }
    }
}

fn urls(test: &str) -> Option<(String, String)> {
    Some((env_url("DATARIG_TEST_MYSQL_URL", test)?, env_url("DATARIG_TEST_MYSQL_ADMIN_URL", test)?))
}

fn profile(url: &str) -> ConnectionConfig {
    let sslmode = std::env::var("DATARIG_TEST_MYSQL_SSLMODE").unwrap_or_else(|_| "DISABLED".into());
    ConnectionConfig {
        name: "it".into(),
        driver: "mysql".into(),
        dsn: Some(url.to_string()),
        sslmode,
        ..Default::default()
    }
}

async fn side(url: &str) -> mysql_async::Conn {
    let opts = mysql_async::Opts::from_url(url).expect("a mysql:// URL");
    let opts = mysql_async::OptsBuilder::from_opts(opts).prefer_socket(false);
    mysql_async::Conn::new(opts).await.expect("the side connection opens")
}

/// `shop.zz_it_export_<name>_<pid>`, made on a side connection, dropped with the guard.
struct Table {
    url: String,
    name: String,
}

impl Table {
    async fn make(url: &str, name: &str, ddl: &[&str]) -> Table {
        let name = format!("zz_it_export_{name}_{}", std::process::id());
        let mut c = side(url).await;
        c.query_drop(format!("DROP TABLE IF EXISTS shop.{name}")).await.unwrap();
        let t = Table { url: url.to_string(), name };
        for sql in ddl {
            c.query_drop(sql.replace("{t}", &t.q())).await.unwrap();
        }
        c.disconnect().await.unwrap();
        t
    }

    fn q(&self) -> String {
        format!("shop.{}", self.name)
    }
}

impl Drop for Table {
    fn drop(&mut self) {
        let (url, name) = (self.url.clone(), self.name.clone());
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let mut c = side(&url).await;
                let _ = c.query_drop(format!("DROP TABLE IF EXISTS shop.{name}")).await;
                let _ = c.disconnect().await;
            });
        })
        .join();
    }
}

/// A table of `n` rows (`id` 1..=n, `v` text).
async fn numbers(url: &str, name: &str, n: u32) -> Table {
    let fill = format!(
        "INSERT INTO {{t}} (id, v) WITH RECURSIVE d (i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM d WHERE i < 99) \
         SELECT g.n, CONCAT('row ', g.n) FROM (SELECT a.i + b.i * 100 + c.i * 10000 + 1 AS n FROM d a, d b, d c) g \
         WHERE g.n <= {n}"
    );
    Table::make(url, name, &["CREATE TABLE {t} (id INT PRIMARY KEY, v VARCHAR(40))", &fill]).await
}

struct Conn {
    session: Session,
    rx: UnboundedReceiver<DbEvent>,
    id: u64,
}

impl Conn {
    async fn open(url: &str, role: SessionRole, test: &str) -> Conn {
        let (tx, rx) = unbounded_channel();
        let tag = format!("it{}-export-{test}", std::process::id());
        let session = MyDriver.connect(&profile(url), role, ConnectOptions::new(PAGE, role, &tag), tx);
        let mut c = Conn { session, rx, id: 0 };
        c.wait(|e| matches!(e, DbEvent::Connected), 15).await;
        c
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
                Err(_) => panic!("timed out after {secs}s waiting for event"),
            }
        }
    }

    async fn run(&mut self, sql: &str) -> DbEvent {
        self.id += 1;
        let id = self.id;
        self.session.send(DbCommand::Execute { id, statements: vec![sql.to_string()], paging: PagingMode::NoHold });
        self.wait(
            |e| matches!(e, DbEvent::Page { id: i, columns: Some(_), .. } | DbEvent::Done { id: i, .. } | DbEvent::Failed { id: i, .. } if *i == id),
            60,
        )
        .await
    }

    async fn ok(&mut self, sql: &str) {
        let ev = self.run(sql).await;
        assert!(!matches!(ev, DbEvent::Failed { .. }), "{sql}: {ev:?}");
    }

    async fn rows(&mut self, sql: &str) -> Vec<Vec<Cell>> {
        match self.run(sql).await {
            DbEvent::Page { rows, more: false, .. } => rows,
            other => panic!("{sql}: {other:?}"),
        }
    }

    fn export(&mut self, sql: &str, guard: ExportGuard, cap: usize, batch: usize) -> (u64, Receiver<ExportBatch>) {
        self.id += 1;
        let (sink, rx) = tokio::sync::mpsc::channel(cap);
        self.session.send(DbCommand::Export { id: self.id, sql: sql.to_string(), guard, sink, batch });
        (self.id, rx)
    }

    async fn exported(&mut self, id: u64) -> (Result<ExportRead, DbError>, bool) {
        match self.wait(|e| matches!(e, DbEvent::Exported { id: i, .. } if *i == id), 60).await {
            DbEvent::Exported { result, cancelled, .. } => (result, cancelled),
            _ => unreachable!(),
        }
    }

    fn answered(&mut self, id: u64) -> bool {
        while let Ok(ev) = self.rx.try_recv() {
            if matches!(ev, DbEvent::Exported { id: i, .. } if i == id) {
                return true;
            }
        }
        false
    }
}

async fn drain(rx: &mut Receiver<ExportBatch>) -> (Vec<ColumnMeta>, Vec<Vec<Cell>>, usize) {
    let (mut cols, mut rows, mut largest) = (Vec::new(), Vec::new(), 0);
    while let Some(b) = rx.recv().await {
        match b {
            ExportBatch::Columns(c) => cols = c,
            ExportBatch::Rows(r) => {
                largest = largest.max(r.len());
                rows.extend(r);
            }
        }
    }
    (cols, rows, largest)
}

/// The metadata locks other sessions hold on `table` (by name, in `shop`).
async fn metadata_locks(admin: &str, table: &str) -> u64 {
    let mut a = side(admin).await;
    let n: Option<u64> = a
        .exec_first(
            "SELECT COUNT(*) FROM performance_schema.metadata_locks WHERE OBJECT_SCHEMA = 'shop' AND OBJECT_NAME = ? \
             AND OWNER_THREAD_ID <> PS_CURRENT_THREAD_ID()",
            (table,),
        )
        .await
        .unwrap();
    a.disconnect().await.unwrap();
    n.unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread")]
async fn every_row_streams_past_the_paging_limit_and_waits_for_its_reader() {
    let Some((url, admin)) = urls("every_row_streams_past_the_paging_limit_and_waits_for_its_reader") else { return };
    let t = numbers(&admin, "stream", 30_000).await;
    let mut c = Conn::open(&url, SessionRole::Query, "stream").await;
    // A run not held sets the session's limit to a page and one row.
    let DbEvent::Page { more: true, .. } = c.run(&format!("SELECT * FROM {}", t.q())).await else { panic!("a page") };
    let (id, mut rx) = c.export(&format!("SELECT * FROM {}", t.q()), ExportGuard::Repeatable, 1, 1_000);
    let Some(ExportBatch::Columns(cols)) = rx.recv().await else { panic!("columns first") };
    assert_eq!(cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id", "v"]);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(!c.answered(id), "the session waits for the reader");
    let (_, rows, largest) = drain(&mut rx).await;
    assert_eq!(rows.len(), 30_000, "never cut at a page");
    assert!(largest <= 1_000);
    let read = c.exported(id).await.0.unwrap();
    assert!(read.complete && read.rows == 30_000);
    assert_eq!(metadata_locks(&admin, &t.name).await, 0, "the statement ended");
    // The next run pages again.
    let DbEvent::Page { rows, more: true, .. } = c.run(&format!("SELECT * FROM {}", t.q())).await else {
        panic!("a page")
    };
    assert_eq!(rows.len(), PAGE);
}

#[tokio::test(flavor = "multi_thread")]
async fn keyset_chunks_read_every_row_once_in_key_order() {
    let Some((url, admin)) = urls("keyset_chunks_read_every_row_once_in_key_order") else { return };
    // Text keys that sort with the column's collation, quotes, backslashes, Hangul.
    let keys = ["'a'", "'B'", "'c'", "'O''Neil'", "'back\\\\slash'", "_utf8mb4 X'ED959CEAB880'", "'z z'", "''"];
    let values: Vec<String> =
        keys.iter().flat_map(|k| (1..=5).map(move |n| format!("({k}, {n}, CONCAT({k}, {n}))"))).collect();
    let fill = format!("INSERT INTO {{t}} (k, n, v) VALUES {}", values.join(", "));
    let t = Table::make(
        &admin,
        "keyset",
        &[
            "CREATE TABLE {t} (k VARCHAR(20) NOT NULL, n INT NOT NULL, v VARCHAR(40), PRIMARY KEY (k, n)) \
             DEFAULT CHARSET utf8mb4 COLLATE utf8mb4_0900_ai_ci",
            &fill,
        ],
    )
    .await;
    let mut c = Conn::open(&url, SessionRole::Query, "keyset").await;
    let mut meta = Conn::open(&url, SessionRole::Meta, "keyset_meta").await;
    meta.session.send(DbCommand::LoadStructure { schema: "shop".into(), table: t.name.clone() });
    let DbEvent::Structure { result: Ok(s), .. } = meta.wait(|e| matches!(e, DbEvent::Structure { .. }), 20).await
    else {
        panic!("no structure")
    };
    let key = table_key(&s).unwrap();
    assert_eq!(key, ["k", "n"]);
    let DbEvent::Page { columns: Some(cols), .. } = c.run(&format!("SELECT * FROM {} LIMIT 1", t.q())).await else {
        panic!("no columns")
    };
    let names: Vec<String> = s.columns.iter().map(|c| c.name.clone()).collect();
    let d = Dialect::MySql(Default::default());
    let mut k = Keyset::new(d, "shop", &t.name, &key, &cols, &names, true).unwrap();
    k.chunk = 6;
    let (mut all, mut sql, mut chunks) = (Vec::new(), Some(k.first()), 0);
    while let Some(q) = sql.take() {
        chunks += 1;
        let (id, mut rx) = c.export(&q, ExportGuard::Keyset, 2, 4);
        let (_, rows, _) = drain(&mut rx).await;
        let read = c.exported(id).await.0.unwrap();
        all.extend(rows);
        if read.rows == k.chunk {
            sql = k.after(&read.last.unwrap());
        }
    }
    assert_eq!(chunks, 40 / 6 + 1);
    let direct = c.rows(&format!("SELECT * FROM {} ORDER BY k, n", t.q())).await;
    assert_eq!(all.len(), 40);
    assert_eq!(all, direct, "the chunks are the table in key order, every row once");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_kills_the_export_and_the_session_goes_on() {
    let Some((url, admin)) = urls("a_cancel_kills_the_export_and_the_session_goes_on") else { return };
    let t = numbers(&admin, "cancel", 200_000).await;
    let mut c = Conn::open(&url, SessionRole::Query, "cancel").await;
    // A cross join of the table with itself: far more rows than it takes to cancel.
    let sql = format!("SELECT a.id, b.v FROM {} a, {} b", t.q(), t.q());
    let (id, mut rx) = c.export(&sql, ExportGuard::Confirmed, 1, 1_000);
    assert!(matches!(rx.recv().await, Some(ExportBatch::Columns(_))));
    assert!(matches!(rx.recv().await, Some(ExportBatch::Rows(_))));
    c.session.cancel();
    drain(&mut rx).await;
    let (result, cancelled) = c.exported(id).await;
    assert!(cancelled && result == Err(DbError::Cancelled), "{result:?}");
    assert_eq!(c.rows("SELECT 1").await, [[Some("1".into())]]);
    assert_eq!(metadata_locks(&admin, &t.name).await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reader_that_stops_stops_the_statement() {
    let Some((url, admin)) = urls("a_reader_that_stops_stops_the_statement") else { return };
    let t = numbers(&admin, "stop", 100_000).await;
    let mut c = Conn::open(&url, SessionRole::Query, "stop").await;
    let (id, mut rx) = c.export(&format!("SELECT * FROM {}", t.q()), ExportGuard::Repeatable, 1, 1_000);
    assert!(matches!(rx.recv().await, Some(ExportBatch::Columns(_))));
    assert!(matches!(rx.recv().await, Some(ExportBatch::Rows(_))));
    drop(rx);
    let read = c.exported(id).await.0.unwrap();
    assert!(!read.complete);
    assert_eq!(metadata_locks(&admin, &t.name).await, 0, "no lock left");
    assert_eq!(c.rows("SELECT 2").await, [[Some("2".into())]]);
}

#[tokio::test(flavor = "multi_thread")]
async fn inside_the_users_transaction_it_reads_there() {
    let Some((url, admin)) = urls("inside_the_users_transaction_it_reads_there") else { return };
    let t = numbers(&admin, "tx", 10).await;
    let mut c = Conn::open(&url, SessionRole::Query, "tx").await;
    c.ok("BEGIN").await;
    c.ok(&format!("INSERT INTO {} VALUES (11, 'new')", t.q())).await;
    let (id, mut rx) = c.export(&format!("SELECT * FROM {}", t.q()), ExportGuard::Repeatable, 2, 3);
    assert_eq!(drain(&mut rx).await.1.len(), 11, "the transaction's own row");
    assert!(c.exported(id).await.0.unwrap().complete);
    // A failed export leaves the transaction open.
    let (id, mut rx) = c.export(&format!("SELECT * FROM {}_missing", t.q()), ExportGuard::Confirmed, 2, 3);
    drain(&mut rx).await;
    assert!(matches!(c.exported(id).await.0, Err(DbError::Server(_))));
    assert_eq!(c.rows(&format!("SELECT COUNT(*) FROM {}", t.q())).await, [[Some("11".into())]]);
    c.ok("ROLLBACK").await;
    assert_eq!(c.rows(&format!("SELECT COUNT(*) FROM {}", t.q())).await, [[Some("10".into())]]);
    // A write is never run for an export.
    let (id, mut rx) = c.export(&format!("DELETE FROM {}", t.q()), ExportGuard::Confirmed, 2, 3);
    drain(&mut rx).await;
    assert_eq!(c.exported(id).await.0, Err(DbError::NotSupported));
    assert_eq!(c.rows(&format!("SELECT COUNT(*) FROM {}", t.q())).await, [[Some("10".into())]]);
}

/// Outside the user's transaction an export reads in a read-only one: a stored function that
/// writes fails, and nothing is written.
#[tokio::test(flavor = "multi_thread")]
async fn an_export_never_writes_through_a_function() {
    let Some((url, admin)) = urls("an_export_never_writes_through_a_function") else { return };
    let t = numbers(&admin, "fnw", 3).await;
    let f = format!("shop.zz_it_export_fn_{}", std::process::id());
    let _drop = FunctionGuard(admin.clone(), f.clone());
    let mut a = side(&admin).await;
    a.query_drop(format!(
        "CREATE FUNCTION {f}(x INT) RETURNS INT DETERMINISTIC MODIFIES SQL DATA \
         BEGIN INSERT INTO {} VALUES (x + 1000, 'w'); RETURN x; END",
        t.q()
    ))
    .await
    .unwrap();
    a.disconnect().await.unwrap();
    let mut c = Conn::open(&url, SessionRole::Query, "fnw").await;
    let (id, mut rx) = c.export(&format!("SELECT {f}(id) FROM {}", t.q()), ExportGuard::Confirmed, 2, 10);
    drain(&mut rx).await;
    let (result, _) = c.exported(id).await;
    assert!(
        matches!(&result, Err(DbError::Server(m)) if m.contains("READ ONLY") || m.contains("read only")),
        "{result:?}"
    );
    assert_eq!(c.rows(&format!("SELECT COUNT(*) FROM {}", t.q())).await, [[Some("3".into())]]);
    // The session is out of the export's transaction: a plain write goes through.
    c.ok(&format!("INSERT INTO {} VALUES (4, 'ok')", t.q())).await;
}

/// Wide rows come in batches bounded by bytes.
#[tokio::test(flavor = "multi_thread")]
async fn wide_rows_come_in_batches_bounded_by_bytes() {
    let Some((url, admin)) = urls("wide_rows_come_in_batches_bounded_by_bytes") else { return };
    let t = Table::make(&admin, "wide", &["CREATE TABLE {t} (id INT PRIMARY KEY, v LONGTEXT)"]).await;
    let mut a = side(&admin).await;
    for i in 0..60 {
        a.query_drop(format!("INSERT INTO {} VALUES ({i}, REPEAT('x', 1000000))", t.q())).await.unwrap();
    }
    a.disconnect().await.unwrap();
    let mut c = Conn::open(&url, SessionRole::Query, "wide").await;
    let (id, mut rx) = c.export(&format!("SELECT * FROM {}", t.q()), ExportGuard::Repeatable, 1, 2_000);
    let (mut rows, mut largest) = (0, 0usize);
    while let Some(b) = rx.recv().await {
        if let ExportBatch::Rows(r) = b {
            rows += r.len();
            largest =
                largest.max(r.iter().map(|r| r.iter().map(|v| v.as_ref().map_or(0, String::len)).sum::<usize>()).sum());
        }
    }
    assert_eq!(rows, 60);
    assert!(largest <= 2 * datarig_core::driver::EXPORT_BATCH_BYTES, "{largest}");
    assert!(c.exported(id).await.0.unwrap().complete);
}

/// Drops a function from the side when the test ends.
struct FunctionGuard(String, String);

impl Drop for FunctionGuard {
    fn drop(&mut self) {
        let (url, f) = (self.0.clone(), self.1.clone());
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let mut c = side(&url).await;
                let _ = c.query_drop(format!("DROP FUNCTION IF EXISTS {f}")).await;
                let _ = c.disconnect().await;
            });
        })
        .join();
    }
}
