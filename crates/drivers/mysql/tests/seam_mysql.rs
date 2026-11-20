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

#[allow(dead_code)]
mod my_proxy;

use datarig_core::driver::{
    CancelFailure, Cell, ConnectOptions, DbCommand, DbError, DbEvent, Driver, EditFormat, ExplainFormat, ExportBatch,
    ExportGuard, MySqlEdit, OwnSession, PagingMode, Session, SessionRole, SessionStatement, TxRule,
};
use datarig_core::engine::{EngineId, Fact, ServerFacts, Topology, Version, Why};
use datarig_core::export::keyset::{CHUNK_ROWS, Keyset};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::{Dialect, DollarQuotes, Language, MySqlMode};
use datarig_driver_mysql::MyDriver;
use my_proxy::{Fault, Proxy, upstream};
use mysql_async::prelude::Queryable;
use std::io::Write;
use std::sync::{Arc, OnceLock};
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
fn languages(evs: &[DbEvent]) -> Vec<(EngineId, MySqlMode)> {
    evs.iter()
        .filter_map(|e| match e {
            DbEvent::Language(Language::Sql(d @ (Dialect::MySql(m) | Dialect::MariaDb(m)))) => Some((d.engine(), *m)),
            _ => None,
        })
        .collect()
}

/// The engine of a MariaDB server (`maria`) or a MySQL one.
fn engine(maria: bool) -> EngineId {
    if maria { EngineId::MariaDb } else { EngineId::MySql }
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
        let DbEvent::Language(Language::Sql(d @ (Dialect::MySql(mode) | Dialect::MariaDb(mode)))) = last[0] else {
            panic!("{role:?}: a language third from last: {before:?}")
        };
        assert_eq!(d.engine(), engine(maria), "{role:?}: {d:?}");
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
    assert!(said[0].1.ansi_quotes && !said[0].1.no_backslash_escapes, "{said:?}");
    assert_eq!(said[0].0, engine(maria), "the server's engine stays: {said:?}");
    let at = |pred: fn(&DbEvent) -> bool| evs.iter().position(pred).unwrap();
    assert!(at(|e| matches!(e, DbEvent::Language(_))) < at(|e| matches!(e, DbEvent::Done { .. })), "{evs:?}");
    // In that mode `"x"` is a name: the session reads it so.
    let evs = c.ok(2, "SELECT 1 AS \"x\"").await;
    assert!(languages(&evs).is_empty(), "no change, nothing said: {evs:?}");
    let evs = c.ok(3, "SET SESSION sql_mode = DEFAULT").await;
    let said = languages(&evs);
    assert_eq!(said.len(), 1, "{evs:?}");
    assert!(!said[0].1.ansi_quotes && said[0].0 == engine(maria), "{said:?}");
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
    let dialect = if maria { Dialect::MariaDb(MySqlMode::default()) } else { Dialect::MySql(MySqlMode::default()) };
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

/// The facts a session said first, once, before `Connected` (`before`: every word it said
/// before it).
fn facts_first(role: SessionRole, before: &[DbEvent]) -> Arc<ServerFacts> {
    let said = before.iter().filter(|e| matches!(e, DbEvent::Server(_))).count();
    assert_eq!(said, 1, "{role:?}: once: {before:?}");
    let Some(DbEvent::Server(facts)) = before.first() else { panic!("{role:?}: the facts first: {before:?}") };
    facts.clone()
}

/// The language, plan format and edits a session said before `Connected`.
fn words(before: &[DbEvent]) -> (Vec<Language>, Vec<Option<ExplainFormat>>, Vec<Option<EditFormat>>) {
    let language = before.iter().filter_map(|e| if let DbEvent::Language(l) = e { Some(*l) } else { None });
    let explain = before.iter().filter_map(|e| if let DbEvent::Explain(f) = e { Some(*f) } else { None });
    let edit = before.iter().filter_map(|e| if let DbEvent::Edit(f) = e { Some(*f) } else { None });
    (language.collect(), explain.collect(), edit.collect())
}

/// Every role's session says what the server is first and once: the engine its own version line
/// names, the version of its greeting and the line `VERSION()` answers, reached directly, the provider not
/// checked. The plan format and the edits it says before `Connected` are the ones that engine
/// and version imply (MySQL's tree from 8.0.16 on, its edits from 8.0.17 on, none on MariaDB),
/// and the language is MariaDB's exactly when the engine is (with dollar quotes on MySQL 8.1
/// and later).
#[tokio::test(flavor = "multi_thread")]
async fn the_server_facts_come_first_once_and_the_words_before_connected_follow_from_them() {
    let test = "the_server_facts_come_first_once_and_the_words_before_connected_follow_from_them";
    let Some(url) = my_url(test) else { return };
    let maria = mariadb(&url).await;
    let mut c = side(&url).await;
    let line: String = c.query_first("SELECT VERSION()").await.unwrap().expect("a version");
    c.disconnect().await.unwrap();
    let version = Version::parse(&line).expect("the server's version line starts with its number");
    for role in [SessionRole::Query, SessionRole::Meta, SessionRole::Monitor] {
        let (_c, before) = Conn::start(&url, role, test).await;
        let f = facts_first(role, &before);
        let engine = if maria { EngineId::MariaDb } else { EngineId::MySql };
        assert_eq!(f.engine(), engine, "{role:?}: {f:?}");
        assert_eq!(f.version, Fact::Known(version), "{role:?}: {f:?}");
        assert_eq!(f.version_text.as_deref(), Some(line.as_str()), "{role:?}: {f:?}");
        assert_eq!(f.provider, Fact::Unknown(Why::NotChecked), "{role:?}: {f:?}");
        assert_eq!(f.topology, Fact::Known(Topology::Direct), "{role:?}: {f:?}");
        let (explain, edit, dollar_quotes) = match f.engine() {
            EngineId::MySql => (
                (version >= Version::new(8, 0, 16)).then_some(ExplainFormat::MySqlTree),
                (version >= Version::new(8, 0, 17)).then_some(EditFormat::MySql(MySqlEdit { nowait: true })),
                if version >= Version::new(8, 1, 0) { DollarQuotes::Read } else { DollarQuotes::NotRead },
            ),
            EngineId::MariaDb | EngineId::Postgres => (None, None, DollarQuotes::NotRead),
        };
        let (languages, explains, edits) = words(&before);
        assert_eq!(explains, [explain], "{role:?}: {before:?}");
        assert_eq!(edits, [edit], "{role:?}: {before:?}");
        let [Language::Sql(d @ (Dialect::MySql(mode) | Dialect::MariaDb(mode)))] = languages[..] else {
            panic!("{role:?}: {before:?}")
        };
        assert_eq!(d.engine(), f.engine(), "{role:?}: {d:?}");
        assert_eq!(mode.dollar_quotes, dollar_quotes, "{role:?}: {mode:?}");
    }
}

/// Where a session's facts say it reaches the server, by the connection's own check of its id
/// as it opened: straight to the server, directly; through a proxy whose greeting carries an id
/// of its own, pooled; through one that fails the check's read, not checked (the session still
/// opens). On each way every role's language, plan format and edits are the server's, as
/// straight to it, and a query session answers which server session is its own and takes a
/// cancel as it did before: straight to the server the statement is stopped (its `SLEEP`
/// answers 1 on MySQL, fails cancelled on MariaDB) and the session goes on; behind the proxy numbering its clients the session is
/// not its own, and after the failed read it checks its id when asked and finds it its own, but
/// on both ways its marker was never confirmed, so no kill is sent: the run ends cancelled and
/// the session is given up, as the cancel could not be confirmed.
#[tokio::test(flavor = "multi_thread")]
async fn the_facts_topology_is_the_own_id_check() {
    let test = "the_facts_topology_is_the_own_id_check";
    let Some((url, admin)) = urls(test) else { return };
    let maria = mariadb(&admin).await;
    let numbering = Proxy::start(&upstream(&url)).await;
    numbering.offset_ids(0x0100_0000);
    let failing = Proxy::start(&upstream(&url)).await;
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Way {
        Direct,
        Numbering,
        FailedRead,
    }
    let mut straight = Vec::new();
    for way in [Way::Direct, Way::Numbering, Way::FailedRead] {
        let at = match way {
            Way::Direct => url.clone(),
            Way::Numbering => numbering.url(&url),
            Way::FailedRead => failing.url(&url),
        };
        let topology = match way {
            Way::Direct => Fact::Known(Topology::Direct),
            Way::Numbering => Fact::Known(Topology::Pooled),
            Way::FailedRead => Fact::Unknown(Why::NotChecked),
        };
        for (n, role) in [SessionRole::Query, SessionRole::Meta, SessionRole::Monitor].into_iter().enumerate() {
            if way == Way::FailedRead {
                failing.fault_start("SELECT CONNECTION_ID(), ", Fault::Instead("SELECT zz_seam_no_such_column".into()));
            }
            let (mut c, before) = Conn::start(&at, role, test).await;
            assert!(!failing.fault_pending(), "{way:?} {role:?}: the read met its fault");
            let f = facts_first(role, &before);
            assert_eq!(f.topology, topology, "{way:?} {role:?}: {f:?}");
            match way {
                Way::Direct => straight.push(words(&before)),
                Way::Numbering | Way::FailedRead => assert_eq!(words(&before), straight[n], "{way:?} {role:?}"),
            }
            if role != SessionRole::Query {
                continue;
            }
            c.session.send(DbCommand::ServerSession { request: 7 });
            let own = loop {
                if let DbEvent::ServerSession { request: 7, own } = c.next(15).await {
                    break own;
                }
            };
            match way {
                Way::Direct | Way::FailedRead => assert!(matches!(own, OwnSession::Direct { .. }), "{way:?}: {own:?}"),
                Way::Numbering => assert_eq!(own, OwnSession::NotDirect, "{way:?}"),
            }
            let marker = format!("zz_seam_topology_{}_{n}_{way:?}", std::process::id());
            let mut a = side(&admin).await;
            let t0 = Instant::now();
            c.send(1, &[&format!("SELECT SLEEP(3) AS {marker}")]);
            running(&mut a, &marker).await;
            c.session.cancel();
            let evs = c.answer(1).await;
            a.disconnect().await.unwrap();
            match way {
                Way::Direct => {
                    // `SLEEP` stopped by a `KILL QUERY` answers 1 on MySQL, fails on MariaDB; the
                    // session goes on.
                    match maria {
                        false => {
                            let Some(DbEvent::Page { rows, .. }) = evs.last() else { panic!("{way:?}: {evs:?}") };
                            assert_eq!(rows[0][0].as_deref(), Some("1"), "{way:?}: stopped: {evs:?}");
                        }
                        true => assert!(
                            matches!(
                                evs.last(),
                                Some(DbEvent::Failed { error: DbError::Cancelled, cancelled: true, .. })
                            ),
                            "{way:?}: {evs:?}"
                        ),
                    }
                    assert!(t0.elapsed() < Duration::from_secs(3), "{way:?}: {:?}", t0.elapsed());
                    let evs = c.ok(2, "SELECT 1").await;
                    assert!(matches!(evs.last(), Some(DbEvent::Page { .. })), "{way:?}: {evs:?}");
                }
                Way::Numbering | Way::FailedRead => {
                    // No kill is sent for a session not confirmed: the run ends cancelled and the
                    // connection is given up.
                    assert!(
                        matches!(evs.last(), Some(DbEvent::Failed { error: DbError::Cancelled, cancelled: true, .. })),
                        "{way:?}: {evs:?}"
                    );
                    let lost = loop {
                        if let DbEvent::Lost { error } = c.next(15).await {
                            break error;
                        }
                    };
                    assert!(
                        matches!(&lost, DbError::CancelNotTaken(CancelFailure::NotSent(why)) if **why == DbError::CancelUnconfirmed),
                        "{way:?}: {lost:?}"
                    );
                }
            }
        }
    }
}

/// A test connection says the server's engine (MariaDB on a MariaDB server) and the version of
/// its greeting, the number its version line starts with.
#[tokio::test(flavor = "multi_thread")]
async fn a_test_connection_says_the_engine_and_version() {
    let test = "a_test_connection_says_the_engine_and_version";
    let Some(url) = my_url(test) else { return };
    let maria = mariadb(&url).await;
    let info = MyDriver.ping(&profile(&url), Duration::from_secs(10), None).await.expect("test connection");
    let engine = if maria { EngineId::MariaDb } else { EngineId::MySql };
    assert_eq!(info.identity.engine, Fact::Known(engine), "{info:?}");
    let version = Version::parse(&info.server_version).expect("the version line starts with its number");
    assert_eq!(info.identity.version, Fact::Known(version), "{info:?}");
}

// ── Dollar quotes when the server's own reports disagree ─────────────────────────────────────

/// Whether `e` is the refusal of a text that opens a dollar quote, on a session that does not
/// know whether its server reads them.
fn dollar_refusal(e: &DbError) -> bool {
    matches!(e, DbError::DollarQuotesUnknown)
}

/// The dollar-quote reading of the language a session said last before `evs` ended.
fn said_dollar_quotes(evs: &[DbEvent]) -> DollarQuotes {
    let said = languages(evs);
    let (_, mode) = said.last().unwrap_or_else(|| panic!("a language: {evs:?}"));
    mode.dollar_quotes
}

/// A proxy in front of the server of `url` whose sessions' two reports give different dollar
/// answers: on MySQL (8.1 or later) its greeting names MySQL 8.0.36, which does not read them;
/// on MariaDB ([`Disagreeing::arm`]) it answers `VERSION()` with a MySQL 8.4 line, which does.
struct Disagreeing {
    proxy: Proxy,
    maria: bool,
}

impl Disagreeing {
    async fn start(url: &str) -> Disagreeing {
        let maria = mariadb(url).await;
        let proxy = Proxy::start(&upstream(url)).await;
        if !maria {
            proxy.greet_as("8.0.36");
        }
        Disagreeing { proxy, maria }
    }

    /// Set up the next session's version read (once per session).
    fn arm(&self) {
        if self.maria {
            self.proxy.fault_start("SELECT VERSION()", Fault::Instead("SELECT '8.4.6'".into()));
        }
    }

    /// A query session through the proxy, and what it said before `Connected`.
    async fn open(&self, url: &str, test: &str) -> (Conn, Vec<DbEvent>) {
        self.arm();
        Conn::start(&self.proxy.url(url), SessionRole::Query, test).await
    }
}

impl Conn {
    /// The first event `f` takes, within `secs` each; `None` once the session ended.
    async fn until<T>(&mut self, secs: u64, f: impl Fn(&DbEvent) -> Option<T>) -> Option<T> {
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(secs), self.rx.recv()).await.expect("an event")?;
            if let Some(t) = f(&ev) {
                return Some(t);
            }
        }
    }

    /// The error command `cmd` (number `id`) failed with, or its answer in debug text (`ended`
    /// when the session ended without one).
    async fn failure(&mut self, id: u64, cmd: DbCommand) -> Result<String, DbError> {
        self.session.send(cmd);
        self.until(60, |ev| match ev {
            DbEvent::Failed { id: i, error, .. } if *i == id => Some(Err(error.clone())),
            DbEvent::Page { id: i, columns: Some(_), .. } | DbEvent::Done { id: i, .. } if *i == id => {
                Some(Ok(format!("{ev:?}")))
            }
            DbEvent::Counted { id: i, result, .. } if *i == id => Some(outcome(result, ev)),
            DbEvent::GroupCounted { id: i, result, .. } if *i == id => Some(outcome(result, ev)),
            DbEvent::Estimated { id: i, result } if *i == id => Some(outcome(result, ev)),
            DbEvent::RepeatChecked { id: i, result } if *i == id => Some(outcome(result, ev)),
            DbEvent::Exported { id: i, result, .. } if *i == id => Some(outcome(result, ev)),
            _ => None,
        })
        .await
        .unwrap_or_else(|| Ok(String::from("ended")))
    }
}

/// The error of `result`, or answer `ev` in debug text.
fn outcome<T>(result: &Result<T, DbError>, ev: &DbEvent) -> Result<String, DbError> {
    match result {
        Ok(_) => Ok(format!("{ev:?}")),
        Err(e) => Err(e.clone()),
    }
}

/// `Execute` of `statements` as run `id`.
fn execute(id: u64, statements: &[&str]) -> DbCommand {
    DbCommand::Execute {
        id,
        statements: statements.iter().map(|s| s.to_string()).collect(),
        paging: PagingMode::NoHold,
        tx: TxRule::Auto,
        max_rows: None,
    }
}

/// Each command that carries user text, with `sql` as its text, numbered from `id`.
fn every_send_path(id: u64, sql: &str) -> Vec<(&'static str, DbCommand)> {
    let (sink, _) = tokio::sync::mpsc::channel(4);
    vec![
        ("Execute", execute(id, &[sql])),
        (
            "Resume",
            DbCommand::Resume { id: id + 1, sql: sql.into(), skip: 0, paging: PagingMode::NoHold, max_rows: None },
        ),
        ("Export", DbCommand::Export { id: id + 2, sql: sql.into(), guard: ExportGuard::Confirmed, sink, batch: 10 }),
        ("Count", DbCommand::Count { id: id + 3, sql: sql.into(), limit: None }),
        ("GroupCount", DbCommand::GroupCount { id: id + 4, sql: sql.into(), limit: None }),
        ("Estimate", DbCommand::Estimate { id: id + 5, sql: sql.into() }),
        ("CheckRepeat", DbCommand::CheckRepeat { id: id + 6, sql: sql.into() }),
    ]
}

/// On a session whose greeting and `VERSION()` line disagree on whether the server reads dollar
/// quotes, the session says the reading is not known, and no user text whose bytes hold a
/// dollar-quote opener (`$`, tag characters, `$`) is sent: an `Execute` sent while the session
/// still opens, and each send path (`Execute`, `Resume`, `Export` confirmed by the user,
/// `Count`, `GroupCount`, `Estimate`, `CheckRepeat`), whatever the opener is inside (a string,
/// a name glued to it), and a run whose first statement changes the sql mode the second one
/// would be read in. Each fails with the refusal and the proxy sees none of it; the session
/// goes on.
#[tokio::test(flavor = "multi_thread")]
async fn a_dollar_opener_is_refused_on_every_send_path_when_the_reports_disagree() {
    let test = "a_dollar_opener_is_refused_on_every_send_path_when_the_reports_disagree";
    let Some(url) = my_url(test) else { return };
    let way = Disagreeing::start(&url).await;
    // Sent while the session still opens: it waits for the session's facts, then is refused.
    way.arm();
    let (tx, mut rx) = unbounded_channel();
    let tag = format!("seam{}-{test}", std::process::id());
    let at = way.proxy.url(&url);
    let session =
        MyDriver.connect(&profile(&at), SessionRole::Query, ConnectOptions::new(PAGE, SessionRole::Query, &tag), tx);
    session.send(execute(1, &["SELECT $$a; b$$"]));
    // Each way a command got through: (what, how it was answered, what the proxy saw of it).
    let mut through: Vec<(String, String, Vec<String>)> = Vec::new();
    let mut evs = Vec::new();
    let opening = loop {
        match tokio::time::timeout(Duration::from_secs(15), rx.recv()).await.expect("an answer") {
            Some(DbEvent::Failed { id: 1, error, .. }) => break Err(error),
            Some(ev @ (DbEvent::Page { id: 1, .. } | DbEvent::Done { id: 1, .. })) => break Ok(format!("{ev:?}")),
            Some(DbEvent::ConnectFailed { error, .. }) => panic!("connect failed: {error:?}"),
            Some(ev) => evs.push(ev),
            None => panic!("event channel closed"),
        }
    };
    assert!(evs.iter().any(|e| matches!(e, DbEvent::Connected)), "{evs:?}");
    let seen = |from: usize| way.proxy.queries()[from..].iter().filter(|q| q.contains('$')).cloned().collect();
    if !opening.as_ref().is_err_and(dollar_refusal) || !way.proxy.queries().iter().all(|q| !q.contains('$')) {
        through.push(("Execute while opening".into(), format!("{opening:?}"), seen(0)));
    }
    let language = said_dollar_quotes(&evs);
    let mut c = Conn { session, rx };
    let mut id = 10;
    let texts: [&[&str]; 5] = [
        &["SET SESSION sql_mode = 'NO_BACKSLASH_ESCAPES'", "SELECT '\\' AS a, $$x$$ AS b -- '"],
        &["SELECT '$$'"],
        &["SELECT $a$x$a$"],
        &["SELECT 1 AS x$y$z"],
        &["SELECT $$a; b$$"],
    ];
    'texts: for statements in texts {
        let mut paths = vec![("Execute", execute(id, statements))];
        if let [sql] = statements {
            paths = every_send_path(id, sql);
        }
        for (path, cmd) in paths {
            let n = match &cmd {
                DbCommand::Execute { id, .. }
                | DbCommand::Resume { id, .. }
                | DbCommand::Export { id, .. }
                | DbCommand::Count { id, .. }
                | DbCommand::GroupCount { id, .. }
                | DbCommand::Estimate { id, .. }
                | DbCommand::CheckRepeat { id, .. } => *id,
                _ => unreachable!(),
            };
            let from = way.proxy.queries().len();
            let got = c.failure(n, cmd).await;
            if !got.as_ref().is_err_and(dollar_refusal) || way.proxy.queries().len() != from {
                let sent = way.proxy.queries()[from..].to_vec();
                through.push((format!("{path} {statements:?}"), format!("{got:?}"), sent));
            }
            if got.as_deref() == Ok("ended") {
                break 'texts;
            }
        }
        id += 10;
    }
    let report: Vec<String> = through.iter().map(|(what, got, sent)| format!("{what}: {got}, sent {sent:?}")).collect();
    assert!(report.is_empty(), "{} got through:\n{}", report.len(), report.join("\n"));
    assert_eq!(language, DollarQuotes::NotKnown, "the session says the dollar reading is not known");
    // Neither the sql mode nor anything else changed, and the session goes on.
    let evs = c.ok(100, "SELECT @@SESSION.sql_mode LIKE '%NO_BACKSLASH_ESCAPES%' AS nbe").await;
    let Some(DbEvent::Page { rows, .. }) = evs.last() else { panic!("{evs:?}") };
    assert_eq!(rows[0][0].as_deref(), Some("0"), "the SET of the refused run was not sent");
    assert!(way.proxy.queries().iter().all(|q| !q.contains('$')), "{:?}", way.proxy.queries());
}

/// A `DROP PROCEDURE IF EXISTS` run on a side connection when dropped.
struct Dropped {
    url: String,
    sql: String,
}

impl Drop for Dropped {
    fn drop(&mut self) {
        let (url, sql) = (self.url.clone(), self.sql.clone());
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let mut c = side(&url).await;
                let _ = c.query_drop(sql).await;
                let _ = c.disconnect().await;
            });
        })
        .join();
    }
}

/// On the same disagreeing session, text without a dollar-quote opener runs as before: a `$`
/// at the end of a name, before a number or inside one is no opener, and a procedure and a
/// trigger with compound bodies, cut as the editor cuts them (`DELIMITER` lines read, not sent),
/// make their objects.
#[tokio::test(flavor = "multi_thread")]
async fn text_without_an_opener_runs_as_today_when_the_reports_disagree() {
    let test = "text_without_an_opener_runs_as_today_when_the_reports_disagree";
    let Some((_, admin)) = urls(test) else { return };
    let way = Disagreeing::start(&admin).await;
    let (mut c, before) = way.open(&admin, test).await;
    let f = facts_first(SessionRole::Query, &before);
    assert!(matches!(f.version, Fact::Unknown(_)), "the reports disagree: {f:?}");
    for (n, sql) in ["SELECT 1 AS price$", "SELECT 1 AS a$b", "SELECT $1"].into_iter().enumerate() {
        let id = 1 + n as u64;
        let got = c.failure(id, execute(id, &[sql])).await;
        if let Err(e) = &got {
            assert!(!dollar_refusal(e) && matches!(e, DbError::Server(_)), "{sql}: {got:?}");
        }
        assert!(way.proxy.queries().iter().any(|q| q == sql), "{sql}: sent");
    }
    let t = Table::make(&admin, "dq", &["CREATE TABLE {t} (id INT PRIMARY KEY, v VARCHAR(20))"]).await;
    let proc = format!("{}_p", t.name);
    let _proc = Dropped { url: admin.clone(), sql: format!("DROP PROCEDURE IF EXISTS shop.{proc}") };
    let text = format!(
        "DELIMITER ;;\nCREATE PROCEDURE shop.{proc}() BEGIN SELECT 1; SELECT 'a;b'; END ;;\n\
         CREATE TRIGGER shop.{}_bi BEFORE INSERT ON {} FOR EACH ROW BEGIN SET NEW.v = 'x;'; END ;;\nDELIMITER ;\n",
        t.name,
        t.q()
    );
    let mysql = Dialect::MySql(MySqlMode::default());
    let statements = datarig_core::sql::split::split_in(&text, mysql);
    assert_eq!(statements.len(), 2, "{text}");
    for (n, s) in statements.iter().enumerate() {
        let id = 10 + n as u64;
        let got = c.failure(id, execute(id, &[s.body(&text)])).await;
        assert!(got.is_ok(), "{}: {got:?}", s.body(&text));
    }
    let evs = c.ok(20, &format!("INSERT INTO {} (id) VALUES (1)", t.q())).await;
    assert!(matches!(evs.last(), Some(DbEvent::Done { .. })), "{evs:?}");
    let evs = c.ok(21, &format!("SELECT v FROM {}", t.q())).await;
    let Some(DbEvent::Page { rows, .. }) = evs.last() else { panic!("{evs:?}") };
    assert_eq!(rows[0][0].as_deref(), Some("x;"), "the trigger ran");
}

/// A session whose two reports agree keeps today's dollar-quote reading, however it reaches the
/// server: straight through a proxy that changes nothing, behind one that numbers its clients
/// (pooled), through one that fails the own-id lookup at connect (not checked), and with a
/// version line of its engine that has no number (an Aurora-style line, which gives no answer;
/// a MariaDB line, whose answer needs none): MySQL 8.4
/// reads them, MariaDB does not. `SELECT $$a; b$$` is sent as one statement, and the answer is
/// the server's own (a syntax error on both: neither takes a dollar-quoted string there).
#[tokio::test(flavor = "multi_thread")]
async fn dollar_text_on_an_agreeing_session_runs_as_today() {
    let test = "dollar_text_on_an_agreeing_session_runs_as_today";
    let Some(url) = my_url(test) else { return };
    let maria = mariadb(&url).await;
    let want = if maria { DollarQuotes::NotRead } else { DollarQuotes::Read };
    for way in ["direct", "pooled", "own id not checked", "a line without a number"] {
        let proxy = Proxy::start(&upstream(&url)).await;
        match way {
            "pooled" => proxy.offset_ids(0x0100_0000),
            "own id not checked" => {
                proxy.fault_start("SELECT CONNECTION_ID(), ", Fault::Instead("SELECT zz_seam_no_such_column".into()))
            }
            // A line of the server's own engine.
            "a line without a number" => {
                let line = if maria { "SELECT 'MariaDB-custom'" } else { "SELECT '8.0.mysql_aurora.3.04.0'" };
                proxy.fault_start("SELECT VERSION()", Fault::Instead(line.into()))
            }
            _ => {}
        }
        let (mut c, before) = Conn::start(&proxy.url(&url), SessionRole::Query, test).await;
        assert_eq!(said_dollar_quotes(&before), want, "{way}: {before:?}");
        let sql = "SELECT $$a; b$$";
        let got = c.failure(1, execute(1, &[sql])).await;
        assert!(proxy.queries().iter().any(|q| q == sql), "{way}: sent: {got:?}");
        assert!(matches!(&got, Err(DbError::Server(_))), "{way}: the server's own answer: {got:?}");
    }
}

/// What a query session with start-up statement `startup`, through `proxy`, ended its opening
/// with: `Ok` once connected, else its error.
async fn open_with_startup(proxy: &Proxy, url: &str, test: &str, startup: &str) -> Result<Vec<DbEvent>, DbError> {
    let (tx, mut rx) = unbounded_channel();
    let tag = format!("seam{}-{test}", std::process::id());
    let statement = SessionStatement { sql: startup.into(), per_transaction: None, identity: false };
    let options = ConnectOptions::new(PAGE, SessionRole::Query, &tag).session_sql(vec![statement]);
    let _session = MyDriver.connect(&profile(&proxy.url(url)), SessionRole::Query, options, tx);
    let mut before = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(15), rx.recv()).await.expect("an answer") {
            Some(DbEvent::Connected) => return Ok(before),
            Some(DbEvent::ConnectFailed { error, .. }) => return Err(error),
            Some(ev) => before.push(ev),
            None => panic!("event channel closed"),
        }
    }
}

/// A start-up statement is user text too: on a session whose reports disagree on dollar quotes,
/// one whose bytes hold a dollar-quote opener (here inside a string, as a setting's value) fails
/// the session with the refusal as that statement's error, and nothing of it reaches the
/// server. One without an opener runs as before, and so does the same statement on a session
/// whose reports agree.
#[tokio::test(flavor = "multi_thread")]
async fn a_startup_statement_with_a_dollar_opener_is_refused_when_the_reports_disagree() {
    let test = "a_startup_statement_with_a_dollar_opener_is_refused_when_the_reports_disagree";
    let Some(url) = my_url(test) else { return };
    let opener = "SET @zz_x12_startup = '$$'";
    let way = Disagreeing::start(&url).await;
    way.arm();
    let got = open_with_startup(&way.proxy, &url, test, opener).await;
    match &got {
        Err(DbError::SessionSql { index: 0, statement, error }) => {
            assert_eq!(statement, opener);
            assert!(dollar_refusal(error), "{got:?}");
        }
        _ => panic!("refused as the start-up statement's error: {got:?}"),
    }
    assert!(way.proxy.queries().iter().all(|q| !q.contains('$')), "{:?}", way.proxy.queries());
    // No opener: it runs.
    let plain = "SET @zz_x12_startup = 'price$'";
    way.arm();
    let got = open_with_startup(&way.proxy, &url, test, plain).await;
    assert!(got.is_ok(), "{got:?}");
    assert!(way.proxy.queries().iter().any(|q| q == plain), "sent");
    // Agreeing reports: the opener runs as before.
    let proxy = Proxy::start(&upstream(&url)).await;
    let got = open_with_startup(&proxy, &url, test, opener).await;
    assert!(got.is_ok(), "{got:?}");
    assert!(proxy.queries().iter().any(|q| q == opener), "sent");
}

/// A `VERSION()` line of the other engine without a number still answers for dollar quotes as
/// far as it can: a MariaDB line never reads them, so behind a greeting of MySQL 8.4 (which
/// does) the reading is not known; a MySQL line without a number gives no answer, so behind a
/// MariaDB greeting the reading is not known either, and is not taken from the greeting. An
/// opener is then refused and not sent.
#[tokio::test(flavor = "multi_thread")]
async fn a_line_of_the_other_engine_without_a_number_leaves_dollar_quotes_unknown() {
    let test = "a_line_of_the_other_engine_without_a_number_leaves_dollar_quotes_unknown";
    let Some(url) = my_url(test) else { return };
    let maria = mariadb(&url).await;
    let proxy = Proxy::start(&upstream(&url)).await;
    let (greeting, line) =
        if maria { ("8.4.6", "MariaDB-custom") } else { ("5.5.5-11.4.13-MariaDB", "8.0.mysql_aurora.3.04.0") };
    proxy.greet_as(greeting);
    proxy.fault_start("SELECT VERSION()", Fault::Instead(format!("SELECT '{line}'")));
    let (mut c, before) = Conn::start(&proxy.url(&url), SessionRole::Query, test).await;
    assert_eq!(said_dollar_quotes(&before), DollarQuotes::NotKnown, "{greeting} / {line}: {before:?}");
    let got = c.failure(1, execute(1, &["SELECT $$a; b$$"])).await;
    assert!(got.as_ref().is_err_and(dollar_refusal), "{got:?}");
    assert!(proxy.queries().iter().all(|q| !q.contains('$')), "{:?}", proxy.queries());
}
