//! `DbCommand::Export` against a real PostgreSQL (`DATARIG_TEST_PG_URL`, as `integration_pg`):
//! every row streams into a bounded channel the reader drains (the session waits for it), a
//! keyset's chunks read every row of a table once in key order, a cancel and a reader that
//! stops end the statement and leave nothing open, inside the user's block it reads there under
//! a savepoint, and the allowlist's server side is asked first. Tables are `public.zz_export_*`
//! of this process, dropped when each test ends.

#[allow(dead_code)]
mod pg_clean;

use datarig_core::driver::PagingMode;
use datarig_core::driver::{
    Cell, ColumnMeta, ConnectOptions, DbCommand, DbError, DbEvent, Driver, ExportBatch, ExportGuard, ExportRead,
    Session, SessionRole,
};
use datarig_core::export::keyset::{Keyset, table_key};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::Dialect;
use datarig_driver_postgres::PgDriver;
use std::io::Write;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{Receiver, UnboundedReceiver, unbounded_channel};

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
            let _ = writeln!(std::io::stderr(), "SKIPPED {test}: set DATARIG_TEST_PG_URL");
            None
        }
    }
}

/// Drops a table (or view) on a fresh connection when it goes out of scope.
struct Guard(String, String);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = pg_clean::run_fresh(&self.0, &self.1);
    }
}

struct Conn {
    session: Session,
    rx: UnboundedReceiver<DbEvent>,
    id: u64,
}

impl Conn {
    async fn open(url: &str, role: SessionRole, read_only: bool) -> Conn {
        let cfg = ConnectionConfig { name: "it".into(), dsn: Some(url.to_string()), ..ConnectionConfig::test_db() };
        let (tx, rx) = unbounded_channel();
        let opts = ConnectOptions::new(500, role, &format!("it{}", std::process::id())).read_only(read_only);
        let session = PgDriver.connect(&cfg, role, opts, tx);
        let mut c = Conn { session, rx, id: 0 };
        c.wait(|e| matches!(e, DbEvent::Connected), 10).await;
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

    /// Run `sql` (held), its answer.
    async fn run(&mut self, sql: &str) -> DbEvent {
        self.id += 1;
        let id = self.id;
        self.session.send(DbCommand::Execute { id, statements: vec![sql.to_string()], paging: PagingMode::Hold });
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

    /// Every row of a statement run whole (one page).
    async fn rows(&mut self, sql: &str) -> Vec<Vec<Cell>> {
        match self.run(sql).await {
            DbEvent::Page { rows, more: false, .. } => rows,
            other => panic!("{sql}: {other:?}"),
        }
    }

    /// Start export `sql` into a channel of `cap` batches of `batch` rows.
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

    /// Whether export `id` has answered already (without waiting).
    fn answered(&mut self, id: u64) -> bool {
        while let Ok(ev) = self.rx.try_recv() {
            if matches!(ev, DbEvent::Exported { id: i, .. } if i == id) {
                return true;
            }
        }
        false
    }
}

/// The columns and every row from `rx` until it closes, the largest batch seen.
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

fn table(test: &str) -> String {
    format!("public.zz_export_{test}_{}", std::process::id())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_export_streams_every_row_and_waits_for_its_reader() {
    let Some(url) = pg_url("an_export_streams_every_row_and_waits_for_its_reader") else { return };
    let t = table("stream");
    let _g = Guard(url.clone(), format!("DROP TABLE IF EXISTS {t}"));
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    c.ok(&format!("CREATE TABLE {t} (id int PRIMARY KEY, v text)")).await;
    c.ok(&format!("INSERT INTO {t} SELECT g, chr(54620) || ' row ' || g FROM generate_series(1, 25000) g")).await;
    let (id, mut rx) = c.export(&format!("SELECT * FROM {t}"), ExportGuard::Repeatable, 1, 1_000);
    let Some(ExportBatch::Columns(cols)) = rx.recv().await else { panic!("columns first") };
    assert_eq!(cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id", "v"]);
    // Nobody reads: the session waits with at most a batch in the channel.
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(!c.answered(id), "the session waits for the reader");
    let (_, rows, largest) = drain(&mut rx).await;
    assert_eq!(rows.len(), 25_000);
    assert!(largest <= 1_000, "{largest}");
    let mut ids: Vec<i64> = rows.iter().map(|r| r[0].as_deref().unwrap().parse().unwrap()).collect();
    ids.sort_unstable();
    assert!(ids.iter().enumerate().all(|(i, id)| *id == i as i64 + 1), "every row once");
    assert!(rows.iter().all(|r| r[1].as_deref().is_some_and(|v| v.starts_with("\u{d55c} row "))));
    let (result, cancelled) = c.exported(id).await;
    let read = result.unwrap();
    assert!(!cancelled && read.complete && read.rows == 25_000 && read.last.is_some());
    // Its own read-only transaction ended with it: another session alters the table at once.
    let mut side = Conn::open(&url, SessionRole::Query, false).await;
    side.ok("SET lock_timeout = '2s'").await;
    side.ok(&format!("ALTER TABLE {t} ADD COLUMN w int")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn keyset_chunks_read_every_row_once_in_key_order() {
    let Some(url) = pg_url("keyset_chunks_read_every_row_once_in_key_order") else { return };
    let t = table("keyset");
    let _g = Guard(url.clone(), format!("DROP TABLE IF EXISTS {t}"));
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    c.ok(&format!("CREATE TABLE {t} (k text NOT NULL, n int NOT NULL, v text, PRIMARY KEY (k, n))")).await;
    // Text keys that sort with the column's collation, quotes, backslashes, Hangul.
    c.ok(&format!(
        "INSERT INTO {t} SELECT k, n, k || n FROM unnest(ARRAY['a', 'B', 'b', 'O''Neil', 'back\\slash', \
         chr(54620) || chr(44544), 'z z', '']) k, generate_series(1, 5) n"
    ))
    .await;
    let mut meta = Conn::open(&url, SessionRole::Meta, false).await;
    let name = t.trim_start_matches("public.").to_string();
    meta.session.send(DbCommand::LoadStructure { schema: "public".into(), table: name.clone() });
    let DbEvent::Structure { result: Ok(s), .. } = meta.wait(|e| matches!(e, DbEvent::Structure { .. }), 20).await
    else {
        panic!("no structure")
    };
    let key = table_key(&s).unwrap();
    assert_eq!(key, ["k", "n"]);
    let DbEvent::Page { columns: Some(cols), .. } = c.run(&format!("SELECT * FROM {t} LIMIT 1")).await else {
        panic!("no columns")
    };
    let names: Vec<String> = s.columns.iter().map(|c| c.name.clone()).collect();
    let mut k = Keyset::new(Dialect::Postgres, "public", &name, &key, &cols, &names, true).unwrap();
    k.chunk = 7;
    let mut all = Vec::new();
    let mut sql = Some(k.first());
    let mut chunks = 0;
    while let Some(q) = sql.take() {
        chunks += 1;
        let (id, mut rx) = c.export(&q, ExportGuard::Keyset, 2, 3);
        let (_, rows, _) = drain(&mut rx).await;
        let read = c.exported(id).await.0.unwrap();
        assert_eq!(read.rows as usize, rows.len());
        all.extend(rows);
        if read.rows == k.chunk {
            sql = k.after(&read.last.unwrap());
            assert!(sql.is_some());
        }
    }
    assert_eq!(chunks, 40 / 7 + 1);
    let direct = c.rows(&format!("SELECT * FROM {t} ORDER BY k, n")).await;
    assert_eq!(all, direct, "the chunks are the table in key order, every row once");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_stops_the_export_and_the_session_goes_on() {
    let Some(url) = pg_url("a_cancel_stops_the_export_and_the_session_goes_on") else { return };
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    let sql = "SELECT g, md5(g::text) FROM generate_series(1, 20000000) g";
    let (id, mut rx) = c.export(sql, ExportGuard::Confirmed, 1, 1_000);
    assert!(matches!(rx.recv().await, Some(ExportBatch::Columns(_))));
    assert!(matches!(rx.recv().await, Some(ExportBatch::Rows(_))));
    c.session.cancel();
    drain(&mut rx).await;
    let (result, cancelled) = c.exported(id).await;
    assert!(cancelled, "{result:?}");
    assert_eq!(result, Err(DbError::Cancelled));
    assert_eq!(c.rows("SELECT 1").await, [[Some("1".into())]]);
}

#[tokio::test(flavor = "multi_thread")]
async fn inside_the_users_block_it_reads_there_and_a_failure_keeps_the_block() {
    let Some(url) = pg_url("inside_the_users_block_it_reads_there_and_a_failure_keeps_the_block") else { return };
    let t = table("block");
    let _g = Guard(url.clone(), format!("DROP TABLE IF EXISTS {t}"));
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    c.ok(&format!("CREATE TABLE {t} (id int PRIMARY KEY)")).await;
    c.ok(&format!("INSERT INTO {t} SELECT generate_series(1, 10)")).await;
    c.ok("BEGIN").await;
    c.ok(&format!("INSERT INTO {t} VALUES (11)")).await;
    let (id, mut rx) = c.export(&format!("SELECT * FROM {t}"), ExportGuard::Repeatable, 2, 100);
    let (_, rows, _) = drain(&mut rx).await;
    assert_eq!(rows.len(), 11, "the block's own row is read");
    assert!(c.exported(id).await.0.unwrap().complete);
    // A failure under the savepoint: the block is not aborted.
    let (id, mut rx) = c.export(&format!("SELECT 1 / (id - 5) FROM {t}"), ExportGuard::Confirmed, 2, 1);
    drain(&mut rx).await;
    let (result, _) = c.exported(id).await;
    assert!(matches!(&result, Err(DbError::Server(m)) if m.contains("division by zero")), "{result:?}");
    assert_eq!(c.rows(&format!("SELECT count(*) FROM {t}")).await, [[Some("11".into())]]);
    c.ok("ROLLBACK").await;
    assert_eq!(c.rows(&format!("SELECT count(*) FROM {t}")).await, [[Some("10".into())]]);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_allowlists_server_side_refuses_a_view() {
    let Some(url) = pg_url("the_allowlists_server_side_refuses_a_view") else { return };
    let v = table("view");
    let _g = Guard(url.clone(), format!("DROP VIEW IF EXISTS {v}"));
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    c.ok(&format!("CREATE VIEW {v} AS SELECT 1 AS a")).await;
    let (id, mut rx) = c.export(&format!("SELECT * FROM {v}"), ExportGuard::Repeatable, 1, 10);
    let (cols, rows, _) = drain(&mut rx).await;
    assert!(cols.is_empty() && rows.is_empty(), "nothing ran");
    let (result, _) = c.exported(id).await;
    assert!(matches!(result, Err(DbError::NotRepeatable(_))), "{result:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reader_that_stops_stops_the_statement_and_leaves_no_lock() {
    let Some(url) = pg_url("a_reader_that_stops_stops_the_statement_and_leaves_no_lock") else { return };
    let t = table("stop");
    let _g = Guard(url.clone(), format!("DROP TABLE IF EXISTS {t}"));
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    c.ok(&format!("CREATE TABLE {t} (id int PRIMARY KEY)")).await;
    c.ok(&format!("INSERT INTO {t} SELECT generate_series(1, 50000)")).await;
    let (id, mut rx) = c.export(&format!("SELECT * FROM {t}"), ExportGuard::Repeatable, 1, 1_000);
    assert!(matches!(rx.recv().await, Some(ExportBatch::Columns(_))));
    assert!(matches!(rx.recv().await, Some(ExportBatch::Rows(_))));
    drop(rx);
    let read = c.exported(id).await.0.unwrap();
    assert!(!read.complete, "it says it did not read to the end");
    let mut side = Conn::open(&url, SessionRole::Query, false).await;
    let name = t.trim_start_matches("public.");
    let locks = side
        .rows(&format!(
            "SELECT count(*) FROM pg_catalog.pg_locks l JOIN pg_catalog.pg_class c ON c.oid = l.relation \
             WHERE c.relname = '{name}' AND l.pid <> pg_catalog.pg_backend_pid()"
        ))
        .await;
    assert_eq!(locks, [[Some("0".into())]], "no lock left on the table");
    assert_eq!(c.rows("SELECT 2").await, [[Some("2".into())]]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_session_exports_and_runs_no_write() {
    let Some(url) = pg_url("a_read_only_session_exports_and_runs_no_write") else { return };
    let t = table("ro");
    let _g = Guard(url.clone(), format!("DROP TABLE IF EXISTS {t}"));
    let mut w = Conn::open(&url, SessionRole::Query, false).await;
    w.ok(&format!("CREATE TABLE {t} (id int PRIMARY KEY)")).await;
    w.ok(&format!("INSERT INTO {t} VALUES (1), (2)")).await;
    let mut c = Conn::open(&url, SessionRole::Query, true).await;
    let (id, mut rx) = c.export(&format!("SELECT * FROM {t}"), ExportGuard::Repeatable, 1, 10);
    assert_eq!(drain(&mut rx).await.1.len(), 2);
    assert!(c.exported(id).await.0.unwrap().complete);
    let (id, mut rx) = c.export(&format!("INSERT INTO {t} VALUES (3) RETURNING id"), ExportGuard::Confirmed, 1, 10);
    drain(&mut rx).await;
    assert_eq!(c.exported(id).await.0, Err(DbError::NotSupported), "an export never writes");
    // Even a function that writes is refused by the read-only transaction it reads in.
    let _s = Guard(url.clone(), format!("DROP SEQUENCE IF EXISTS {t}_seq"));
    w.ok(&format!("CREATE SEQUENCE {t}_seq")).await;
    let mut w2 = Conn::open(&url, SessionRole::Query, false).await;
    let (id, mut rx) = w2.export(&format!("SELECT nextval('{}_seq')", t), ExportGuard::Confirmed, 1, 10);
    drain(&mut rx).await;
    let (result, _) = w2.exported(id).await;
    assert!(matches!(&result, Err(DbError::Server(m)) if m.contains("read-only")), "{result:?}");
    assert_eq!(w.rows(&format!("SELECT count(*) FROM {t}")).await, [[Some("2".into())]]);
}

/// Behind a pooler in transaction mode (`DATARIG_TEST_POOLER_URL`): the export's statement, its
/// pages and its COMMIT stay in one transaction, so on one server connection.
#[tokio::test(flavor = "multi_thread")]
async fn an_export_through_a_pooler_streams_every_row() {
    let pooler = match std::env::var("DATARIG_TEST_POOLER_URL") {
        Ok(u) if !u.is_empty() => u,
        _ => {
            if std::env::var("DATARIG_REQUIRE_POOLER").is_ok_and(|v| v == "1") {
                panic!("DATARIG_TEST_POOLER_URL must be set when DATARIG_REQUIRE_POOLER=1");
            }
            let _ = writeln!(std::io::stderr(), "SKIPPED an_export_through_a_pooler_streams_every_row");
            return;
        }
    };
    let Some(url) = pg_url("an_export_through_a_pooler_streams_every_row") else { return };
    let t = table("pooler");
    let _g = Guard(url.clone(), format!("DROP TABLE IF EXISTS {t}"));
    pg_clean::run_fresh(&url, &format!("CREATE TABLE {t} (id int PRIMARY KEY)")).unwrap();
    pg_clean::run_fresh(&url, &format!("INSERT INTO {t} SELECT generate_series(1, 7000)")).unwrap();
    let mut c = Conn::open(&pooler, SessionRole::Query, false).await;
    let (id, mut rx) = c.export(&format!("SELECT * FROM {t}"), ExportGuard::Repeatable, 1, 1_000);
    assert_eq!(drain(&mut rx).await.1.len(), 7_000);
    let read = c.exported(id).await.0.unwrap();
    assert!(read.complete && read.rows == 7_000);
    assert_eq!(c.rows("SELECT 1").await, [[Some("1".into())]]);
}

/// A table others inherit from (legacy inheritance): its key does not bind the children, which
/// `SELECT *` reads too, so the structure says so and no keyset is made of it.
#[tokio::test(flavor = "multi_thread")]
async fn a_table_with_inheritance_children_is_not_read_by_its_key() {
    let Some(url) = pg_url("a_table_with_inheritance_children_is_not_read_by_its_key") else { return };
    let parent = table("parent");
    let child = table("child");
    let _g = Guard(url.clone(), format!("DROP TABLE IF EXISTS {child}, {parent}"));
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    c.ok(&format!("CREATE TABLE {parent} (id int PRIMARY KEY)")).await;
    c.ok(&format!("CREATE TABLE {child} () INHERITS ({parent})")).await;
    c.ok(&format!("INSERT INTO {parent} VALUES (1), (2)")).await;
    c.ok(&format!("INSERT INTO {child} VALUES (2)")).await;
    let mut meta = Conn::open(&url, SessionRole::Meta, false).await;
    let name = parent.trim_start_matches("public.").to_string();
    meta.session.send(DbCommand::LoadStructure { schema: "public".into(), table: name });
    let DbEvent::Structure { result: Ok(s), .. } = meta.wait(|e| matches!(e, DbEvent::Structure { .. }), 20).await
    else {
        panic!("no structure")
    };
    assert!(s.inherited);
    assert_eq!(table_key(&s), Err(datarig_core::export::keyset::NotKeyset::NoKey));
}

/// Wide rows make smaller batches: memory is bounded by bytes, not only by rows.
#[tokio::test(flavor = "multi_thread")]
async fn wide_rows_come_in_batches_bounded_by_bytes() {
    let Some(url) = pg_url("wide_rows_come_in_batches_bounded_by_bytes") else { return };
    let mut c = Conn::open(&url, SessionRole::Query, false).await;
    let (id, mut rx) =
        c.export("SELECT g, repeat('x', 1000000) FROM generate_series(1, 120) g", ExportGuard::Confirmed, 1, 2_000);
    let mut largest = 0usize;
    let mut rows = 0;
    while let Some(b) = rx.recv().await {
        if let ExportBatch::Rows(r) = b {
            rows += r.len();
            largest =
                largest.max(r.iter().map(|r| r.iter().map(|v| v.as_ref().map_or(0, String::len)).sum::<usize>()).sum());
        }
    }
    assert_eq!(rows, 120);
    assert!(largest <= 2 * datarig_core::driver::EXPORT_BATCH_BYTES, "{largest}");
    assert!(c.exported(id).await.0.unwrap().complete);
}
