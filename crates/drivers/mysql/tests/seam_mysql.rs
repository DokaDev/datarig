//! Pins of what a MySQL-family session says of its server and session today, against a real
//! MySQL or MariaDB loaded with `dev/init-mysql/*.sql` (CI runs this file in both jobs): the
//! order of the words a session says before `Connected` and their values on each server, a
//! changed sql mode said again before the run's answer, a run of several statements cancelled
//! during its first statement running no more of them, and the text a keyset chunk is sent as.
//!
//! Connection: `DATARIG_TEST_MYSQL_URL` (e.g. `mysql://datarig:datarig@127.0.0.1:53306/shop`),
//! and for the tests that look at the server from the side (`performance_schema`) or make a
//! table `DATARIG_TEST_MYSQL_ADMIN_URL` too (e.g. `mysql://root:datarig-root@127.0.0.1:53306/`).
//! * unset locally  -> each test prints a visible `SKIPPED` line to stderr and returns;
//! * unset with `DATARIG_REQUIRE_MYSQL=1` -> the test fails.
//!
//! Every table a test makes is `shop.zz_seam_*` of this process, dropped again when it ends.

use datarig_core::driver::{
    Cell, ConnectOptions, DbCommand, DbError, DbEvent, Driver, EditFormat, ExplainFormat, ExportBatch, ExportGuard,
    MySqlEdit, PagingMode, Session, SessionRole, TxRule,
};
use datarig_core::export::keyset::{CHUNK_ROWS, Keyset};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_driver_mysql::MyDriver;
use mysql_async::prelude::Queryable;
use std::io::Write;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

const PAGE: usize = 500;

fn env_url(var: &str, test: &str, example: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            if std::env::var("DATARIG_REQUIRE_MYSQL").is_ok_and(|v| v == "1") {
                panic!("{var} must be set when DATARIG_REQUIRE_MYSQL=1");
            }
            // Written to the real stderr handle so it is visible even with output capture.
            let _ = writeln!(std::io::stderr(), "SKIPPED {test}: set {var}={example}");
            None
        }
    }
}

fn my_url(test: &str) -> Option<String> {
    env_url("DATARIG_TEST_MYSQL_URL", test, "mysql://datarig:datarig@127.0.0.1:53306/shop")
}

/// The server's URL and an administrator's (root) URL.
fn urls(test: &str) -> Option<(String, String)> {
    let url = my_url(test)?;
    let admin = env_url("DATARIG_TEST_MYSQL_ADMIN_URL", test, "mysql://root:datarig-root@127.0.0.1:53306/")?;
    Some((url, admin))
}

/// The profile of `url`, in the TLS mode `DATARIG_TEST_MYSQL_SSLMODE` names (unset: `DISABLED`).
fn profile(url: &str) -> ConnectionConfig {
    let sslmode = std::env::var("DATARIG_TEST_MYSQL_SSLMODE").unwrap_or_else(|_| "DISABLED".into());
    ConnectionConfig {
        name: "seam".into(),
        driver: "mysql".into(),
        dsn: Some(url.to_string()),
        sslmode,
        ..Default::default()
    }
}

/// A plain client connection to look at the server from the side.
async fn side(url: &str) -> mysql_async::Conn {
    let opts = mysql_async::Opts::from_url(url).expect("a mysql:// URL");
    let opts = mysql_async::OptsBuilder::from_opts(opts).prefer_socket(false);
    mysql_async::Conn::new(opts).await.expect("the side connection opens")
}

/// Whether the server is MariaDB, from its own version line (asked once per process).
async fn mariadb(url: &str) -> bool {
    static MARIADB: OnceLock<bool> = OnceLock::new();
    if let Some(m) = MARIADB.get() {
        return *m;
    }
    let mut c = side(url).await;
    let line: String = c.query_first("SELECT VERSION()").await.unwrap().expect("a version");
    let _ = c.disconnect().await;
    *MARIADB.get_or_init(|| line.contains("MariaDB"))
}

/// What MariaDB says otherwise, on stderr (as the `SKIPPED` lines).
fn on_mariadb(test: &str, what: &str) {
    let _ = writeln!(std::io::stderr(), "MARIADB {test}: {what}");
}

/// `shop.zz_seam_<name>_<pid>`, made on a side connection, dropped with the guard.
struct Table {
    url: String,
    name: String,
}

impl Table {
    async fn make(url: &str, name: &str, ddl: &[&str]) -> Table {
        let name = format!("zz_seam_{name}_{}", std::process::id());
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

    async fn count(&self) -> u64 {
        let mut c = side(&self.url).await;
        let n: u64 = c.query_first(format!("SELECT COUNT(*) FROM {}", self.q())).await.unwrap().unwrap();
        c.disconnect().await.unwrap();
        n
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

/// The server's count of `KILL` statements since it started.
async fn kills(a: &mut mysql_async::Conn) -> u64 {
    let row: Option<(String, u64)> = a.query_first("SHOW GLOBAL STATUS LIKE 'Com_kill'").await.unwrap();
    row.expect("Com_kill").1
}

/// The connection that runs a statement containing `marker` once the server runs it (at most
/// 10 s): a statement the session only set up is not there yet.
async fn running(a: &mut mysql_async::Conn, marker: &str) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(id) = on_server(a, marker).await {
            return id;
        }
        assert!(Instant::now() < deadline, "{marker} does not run");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The connection running a statement containing `marker` now, if any.
async fn on_server(a: &mut mysql_async::Conn, marker: &str) -> Option<u64> {
    a.exec_first(
        "SELECT ID FROM information_schema.PROCESSLIST WHERE INFO LIKE CONCAT('%', ?, '%') AND ID <> CONNECTION_ID()",
        (marker,),
    )
    .await
    .unwrap()
}

struct Conn {
    session: Session,
    rx: UnboundedReceiver<DbEvent>,
}

impl Conn {
    /// A session of role `role` on `url` for test `test`, and every event it said up to (not
    /// with) `Connected`.
    async fn start(url: &str, role: SessionRole, test: &str) -> (Conn, Vec<DbEvent>) {
        let (tx, mut rx) = unbounded_channel();
        let tag = format!("seam{}-{test}", std::process::id());
        let session = MyDriver.connect(&profile(url), role, ConnectOptions::new(PAGE, role, &tag), tx);
        let mut before = Vec::new();
        loop {
            match tokio::time::timeout(Duration::from_secs(15), rx.recv()).await.expect("an answer") {
                Some(DbEvent::Connected) => return (Conn { session, rx }, before),
                Some(DbEvent::ConnectFailed { error, .. }) => panic!("connect failed: {error:?}"),
                Some(ev) => before.push(ev),
                None => panic!("event channel closed"),
            }
        }
    }

    async fn open(url: &str, test: &str) -> Conn {
        Conn::start(url, SessionRole::Query, test).await.0
    }

    /// The first event, whatever it is.
    async fn next(&mut self, secs: u64) -> DbEvent {
        match tokio::time::timeout(Duration::from_secs(secs), self.rx.recv()).await {
            Ok(Some(ev)) => ev,
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out after {secs}s waiting for an event"),
        }
    }

    /// Every event up to and with the terminal one of run `id` (its first page, `Done` or
    /// `Failed`).
    async fn answer(&mut self, id: u64) -> Vec<DbEvent> {
        let mut out = Vec::new();
        loop {
            let ev = self.next(60).await;
            let end = matches!(&ev, DbEvent::Page { id: i, columns: Some(_), .. }
                | DbEvent::Done { id: i, .. } | DbEvent::Failed { id: i, .. } if *i == id);
            out.push(ev);
            if end {
                return out;
            }
        }
    }

    fn send(&self, id: u64, sql: &[&str]) {
        let statements = sql.iter().map(|s| s.to_string()).collect();
        self.session.send(DbCommand::Execute {
            max_rows: None,
            id,
            statements,
            paging: PagingMode::NoHold,
            tx: TxRule::Auto,
        });
    }

    async fn run(&mut self, id: u64, sql: &[&str]) -> Vec<DbEvent> {
        self.send(id, sql);
        self.answer(id).await
    }

    async fn ok(&mut self, id: u64, sql: &str) -> Vec<DbEvent> {
        let evs = self.run(id, &[sql]).await;
        assert!(!matches!(evs.last(), Some(DbEvent::Failed { .. })), "{sql}: {evs:?}");
        evs
    }
}

/// The languages `evs` says.
fn languages(evs: &[DbEvent]) -> Vec<MySqlMode> {
    evs.iter()
        .filter_map(|e| match e {
            DbEvent::Language(Language::Sql(Dialect::MySql(m))) => Some(*m),
            _ => None,
        })
        .collect()
}

/// Before `Connected` a query and a metadata session say, as their last three words and in
/// this order, the language the session starts in (MariaDB's on a MariaDB server, in the
/// server's default sql mode), the plan format the server gives (MySQL's tree; none on
/// MariaDB) and the edits it takes (MySQL's with `NOWAIT`; none on MariaDB). A query session's
/// first word after `Connected` is the values its results are read in.
#[tokio::test(flavor = "multi_thread")]
async fn the_session_says_language_explain_edit_then_connected() {
    let test = "the_session_says_language_explain_edit_then_connected";
    let Some(url) = my_url(test) else { return };
    let maria = mariadb(&url).await;
    if maria {
        on_mariadb(test, "the server gives no plan format and takes no edits: Explain(None), Edit(None)");
    }
    let (explain, edit) = match maria {
        true => (None, None),
        false => (Some(ExplainFormat::MySqlTree), Some(EditFormat::MySql(MySqlEdit { nowait: true }))),
    };
    for role in [SessionRole::Query, SessionRole::Meta] {
        let (mut c, before) = Conn::start(&url, role, test).await;
        let n = before.len();
        assert!(n >= 3, "{role:?}: {before:?}");
        let last = &before[n - 3..];
        let DbEvent::Language(Language::Sql(Dialect::MySql(mode))) = last[0] else {
            panic!("{role:?}: a language third from last: {before:?}")
        };
        assert_eq!(mode.mariadb, maria, "{role:?}: {mode:?}");
        assert!(!mode.ansi_quotes && !mode.no_backslash_escapes, "{role:?}: the server's default mode: {mode:?}");
        assert!(matches!(&last[1], DbEvent::Explain(e) if *e == explain), "{role:?}: {before:?}");
        assert!(matches!(&last[2], DbEvent::Edit(e) if *e == edit), "{role:?}: {before:?}");
        // Said once each.
        for (what, said) in [
            ("Language", before.iter().filter(|e| matches!(e, DbEvent::Language(_))).count()),
            ("Explain", before.iter().filter(|e| matches!(e, DbEvent::Explain(_))).count()),
            ("Edit", before.iter().filter(|e| matches!(e, DbEvent::Edit(_))).count()),
        ] {
            assert_eq!(said, 1, "{role:?}: {what}: {before:?}");
        }
        if role == SessionRole::Query {
            let after = c.next(10).await;
            assert!(matches!(after, DbEvent::SessionValues(_)), "{after:?}");
        }
    }
}

/// `SET SESSION sql_mode = 'ANSI_QUOTES'` is said again as a new language before the run's
/// answer (on MariaDB still MariaDB's); a run that changes no mode says none, and a mode set
/// back is said back.
#[tokio::test(flavor = "multi_thread")]
async fn a_sql_mode_change_is_said_again_after_the_run() {
    let test = "a_sql_mode_change_is_said_again_after_the_run";
    let Some(url) = my_url(test) else { return };
    let maria = mariadb(&url).await;
    let mut c = Conn::open(&url, test).await;
    let evs = c.ok(1, "SET SESSION sql_mode = 'ANSI_QUOTES'").await;
    let said = languages(&evs);
    assert_eq!(said.len(), 1, "{evs:?}");
    assert!(said[0].ansi_quotes && !said[0].no_backslash_escapes, "{said:?}");
    assert_eq!(said[0].mariadb, maria, "the server's engine stays: {said:?}");
    let at = |pred: fn(&DbEvent) -> bool| evs.iter().position(pred).unwrap();
    assert!(at(|e| matches!(e, DbEvent::Language(_))) < at(|e| matches!(e, DbEvent::Done { .. })), "{evs:?}");
    // In that mode `"x"` is a name: the session reads it so.
    let evs = c.ok(2, "SELECT 1 AS \"x\"").await;
    assert!(languages(&evs).is_empty(), "no change, nothing said: {evs:?}");
    let evs = c.ok(3, "SET SESSION sql_mode = DEFAULT").await;
    let said = languages(&evs);
    assert_eq!(said.len(), 1, "{evs:?}");
    assert!(!said[0].ansi_quotes && said[0].mariadb == maria, "{said:?}");
}

/// A run of several statements cancelled while its first one runs on the server (seen in its
/// process list, so not while the session sets it up): the server takes a `KILL` (its
/// `Com_kill` moves, as `performance_schema` is on there, as on the CI servers and the dev
/// ones) and the statement is gone, the run ends cancelled once, no later statement starts or
/// runs (their writes are not there), and the session goes on. A cancel in the moment between
/// two statements (one answered, the next not yet sent) is not reached: the test cannot hold the
/// session there, and a cancel while a statement is on its way is that statement's (even where
/// the killed statement answers a row, as `GET_LOCK` does on MariaDB).
#[tokio::test(flavor = "multi_thread")]
async fn a_multi_statement_run_cancelled_during_its_first_statement_runs_no_more() {
    let test = "a_multi_statement_run_cancelled_during_its_first_statement_runs_no_more";
    let Some((url, admin)) = urls(test) else { return };
    let t = Table::make(&admin, "cancel", &["CREATE TABLE {t} (x INT PRIMARY KEY)"]).await;
    let mut c = Conn::open(&url, test).await;
    let (one, two) = (format!("INSERT INTO {} VALUES (1)", t.q()), format!("INSERT INTO {} VALUES (2)", t.q()));
    let mut a = side(&admin).await;
    let marker = format!("zz_seam_cancel_{}", std::process::id());
    let t0 = Instant::now();
    c.send(1, &[&format!("SELECT SLEEP(30) AS {marker}"), &one, &two]);
    // Cancelled once the server runs it, not while the session sets it up.
    running(&mut a, &marker).await;
    let before = kills(&mut a).await;
    c.session.cancel();
    let evs = c.answer(1).await;
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
    assert!(kills(&mut a).await > before, "the server took a KILL");
    assert_eq!(on_server(&mut a, &marker).await, None, "the statement is gone from the server");
    a.disconnect().await.unwrap();
    let failed: Vec<&DbEvent> = evs.iter().filter(|e| matches!(e, DbEvent::Failed { id: 1, .. })).collect();
    assert!(matches!(failed[..], [DbEvent::Failed { error: DbError::Cancelled, cancelled: true, .. }]), "{evs:?}");
    let started: Vec<usize> = evs
        .iter()
        .filter_map(|e| if let DbEvent::Started { id: 1, index } = e { Some(*index) } else { None })
        .collect();
    assert_eq!(started, [0], "{evs:?}");
    assert!(!evs.iter().any(|e| matches!(e, DbEvent::Finished { id: 1, .. })), "{evs:?}");
    assert_eq!(t.count().await, 0, "no later statement ran");
    let evs = c.ok(2, "SELECT 1").await;
    assert!(matches!(evs.last(), Some(DbEvent::Page { .. })), "the session goes on: {evs:?}");
}

/// A keyset chunk is written by the core as a plain `SELECT` and sent by the driver as the
/// server's statement history shows it: on MySQL with the lock wait hint right after
/// `SELECT `, on MariaDB (which reads the hint as a comment) after a `SET STATEMENT … FOR`
/// prefix that never lengthens the session's own wait. The first chunk and the chunk after a
/// key, byte for byte.
#[tokio::test(flavor = "multi_thread")]
async fn the_keyset_chunk_is_sent_as_written() {
    let test = "the_keyset_chunk_is_sent_as_written";
    let Some((url, admin)) = urls(test) else { return };
    let maria = mariadb(&admin).await;
    let t = Table::make(
        &admin,
        "keyset",
        &[
            "CREATE TABLE {t} (id INT PRIMARY KEY, v VARCHAR(10))",
            "INSERT INTO {t} VALUES (1, 'a'), (2, 'b'), (3, 'c')",
        ],
    )
    .await;
    let mut c = Conn::open(&url, test).await;
    let evs = c.ok(1, "SELECT CONNECTION_ID()").await;
    let Some(DbEvent::Page { rows, .. }) = evs.last() else { panic!("{evs:?}") };
    let id: u64 = rows[0][0].as_deref().unwrap().parse().unwrap();
    let evs = c.ok(2, &format!("SELECT * FROM {} LIMIT 1", t.q())).await;
    let Some(DbEvent::Page { columns: Some(cols), .. }) = evs.last() else { panic!("{evs:?}") };
    let names = ["id".to_string(), "v".to_string()];
    let dialect = Dialect::MySql(MySqlMode { mariadb: maria, ..MySqlMode::default() });
    let k = Keyset::new(dialect, "shop", &t.name, &names[..1], cols, &names, true).unwrap();
    let last: Vec<Cell> = vec![Some("2".into()), Some("b".into())];
    let chunks = [k.first(), k.after(&last).expect("a chunk after key 2")];
    // The core's text: plain on every engine.
    let table = format!("`shop`.`{}`", t.name);
    assert_eq!(chunks[0], format!("SELECT * FROM {table} ORDER BY `id` LIMIT {CHUNK_ROWS}"));
    assert_eq!(chunks[1], format!("SELECT * FROM {table} WHERE `id` > 2 ORDER BY `id` LIMIT {CHUNK_ROWS}"));
    let mut read = Vec::new();
    for (n, chunk) in chunks.iter().enumerate() {
        let export = 10 + n as u64;
        let (sink, mut rx) = tokio::sync::mpsc::channel(4);
        c.session.send(DbCommand::Export {
            id: export,
            sql: chunk.clone(),
            guard: ExportGuard::Keyset,
            sink,
            batch: 2,
        });
        let mut rows = 0;
        while let Some(b) = rx.recv().await {
            if let ExportBatch::Rows(r) = b {
                rows += r.len();
            }
        }
        read.push(rows);
        loop {
            if let DbEvent::Exported { id: i, result, .. } = c.next(60).await
                && i == export
            {
                assert!(result.is_ok(), "{result:?}");
                break;
            }
        }
    }
    assert_eq!(read, [3, 1], "every row of the first chunk, then the one after key 2");
    let want: Vec<String> = match maria {
        true => {
            on_mariadb(test, "the chunk is bounded by a SET STATEMENT prefix, the hint being a comment there");
            let prefix = "SET STATEMENT lock_wait_timeout = LEAST(@@SESSION.lock_wait_timeout, 5) FOR ";
            chunks.iter().map(|c| format!("{prefix}{c}")).collect()
        }
        false => chunks
            .iter()
            .map(|c| format!("SELECT /*+ SET_VAR(lock_wait_timeout=5) */ {}", c.strip_prefix("SELECT ").unwrap()))
            .collect(),
    };
    let mut a = side(&admin).await;
    let sent: Vec<String> = a
        .exec(
            "SELECT h.SQL_TEXT FROM performance_schema.events_statements_history h \
             JOIN performance_schema.threads t ON t.THREAD_ID = h.THREAD_ID WHERE t.PROCESSLIST_ID = ? \
             AND h.SQL_TEXT LIKE CONCAT('%', ?, '%') ORDER BY h.EVENT_ID",
            (id, t.name.clone()),
        )
        .await
        .unwrap();
    a.disconnect().await.unwrap();
    // The statement history keeps the last statements of the session: the sample read first,
    // then each chunk once, as sent.
    let chunks_sent: Vec<&String> = sent.iter().filter(|s| s.contains("ORDER BY")).collect();
    assert_eq!(chunks_sent, want.iter().collect::<Vec<_>>(), "{sent:?}");
}
