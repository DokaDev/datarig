//! Cancels against a real MariaDB with `performance_schema` off, as MariaDB ships, loaded with
//! `dev/init-mysql/*.sql`. There the driver cannot confirm which connection id is its session's
//! (it reads the sessions' markers there), so a cancel never sends a `KILL`: a statement that
//! runs is given up on at once (the session's connection is cut and the session is lost, saying
//! why), while a cancel of a run that has not started yet, or of the statements after the one
//! that ran, is the session's own and needs nothing of the server.
//!
//! Connection: `DATARIG_TEST_MARIADB_NOPS_URL` (CI's `integration (MariaDB 11.4)` job starts
//! such a server on port 13309: `mysql://datarig:datarig@127.0.0.1:13309/shop`).
//! * unset locally  -> each test prints a visible `SKIPPED` line to stderr and returns (the
//!   dev servers run with `performance_schema` on);
//! * unset with `DATARIG_REQUIRE_MARIADB_NOPS=1` -> the test fails; so does a server that is not
//!   MariaDB or has `performance_schema` on.
//!
//! Every table a test makes is `shop.zz_nops_*` of this process, dropped again when it ends.
//! The tests count the server's `KILL` statements (`Com_kill`), so they run one at a time.

use datarig_core::driver::{
    CancelFailure, ConnectOptions, DbCommand, DbError, DbEvent, Driver, PagingMode, Session, SessionRole, TxRule,
};
use datarig_core::profile::ConnectionConfig;
use datarig_driver_mysql::MyDriver;
use mysql_async::prelude::Queryable;
use std::io::Write;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

const PAGE: usize = 500;

/// The server, once it was checked to be MariaDB with `performance_schema` off.
async fn nops_url(test: &str) -> Option<String> {
    let url = match std::env::var("DATARIG_TEST_MARIADB_NOPS_URL") {
        Ok(u) if !u.is_empty() => u,
        _ => {
            if std::env::var("DATARIG_REQUIRE_MARIADB_NOPS").is_ok_and(|v| v == "1") {
                panic!("DATARIG_TEST_MARIADB_NOPS_URL must be set when DATARIG_REQUIRE_MARIADB_NOPS=1");
            }
            // Written to the real stderr handle so it is visible even with output capture.
            let _ = writeln!(
                std::io::stderr(),
                "SKIPPED {test}: start a MariaDB with performance_schema off \
                 (.github/scripts/start-mariadb.sh <image> 13309 <name> ps=off), then set \
                 DATARIG_TEST_MARIADB_NOPS_URL=mysql://datarig:datarig@127.0.0.1:13309/shop"
            );
            return None;
        }
    };
    let mut c = side(&url).await;
    let (version, ps): (String, u8) =
        c.query_first("SELECT VERSION(), @@GLOBAL.performance_schema").await.unwrap().expect("a row");
    c.disconnect().await.unwrap();
    assert!(version.contains("MariaDB"), "not a MariaDB server: {version}");
    assert_eq!(ps, 0, "performance_schema is on: {version}");
    Some(url)
}

/// One test at a time: they count the server's `KILL` statements.
static ALONE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn profile(url: &str) -> ConnectionConfig {
    ConnectionConfig {
        name: "nops".into(),
        driver: "mysql".into(),
        dsn: Some(url.to_string()),
        sslmode: "DISABLED".into(),
        ..Default::default()
    }
}

/// A plain client connection to look at the server from the side.
async fn side(url: &str) -> mysql_async::Conn {
    let opts = mysql_async::Opts::from_url(url).expect("a mysql:// URL");
    let opts = mysql_async::OptsBuilder::from_opts(opts).prefer_socket(false);
    mysql_async::Conn::new(opts).await.expect("the side connection opens")
}

/// The `KILL` statements the server ran since it started, and the connections it took.
async fn kills_and_connections(a: &mut mysql_async::Conn) -> (u64, u64) {
    let rows: Vec<(String, u64)> =
        a.query("SHOW GLOBAL STATUS WHERE Variable_name IN ('Com_kill', 'Connections')").await.unwrap();
    let get = |name: &str| rows.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| *v).expect(name);
    (get("Com_kill"), get("Connections"))
}

/// The connection that runs a statement containing `marker`, once it runs (at most 10 s).
async fn running(a: &mut mysql_async::Conn, marker: &str) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let id: Option<u64> = a
            .exec_first(
                "SELECT ID FROM information_schema.PROCESSLIST WHERE INFO LIKE CONCAT('%', ?, '%') \
                 AND ID <> CONNECTION_ID()",
                (marker,),
            )
            .await
            .unwrap();
        if let Some(id) = id {
            return id;
        }
        assert!(Instant::now() < deadline, "{marker} does not run");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Connection `id` is gone from the server, once it is (ended by the side connection, after the
/// test looked: the server notices a cut connection only once its statement next writes).
async fn end(a: &mut mysql_async::Conn, id: u64) {
    let _ = a.query_drop(format!("KILL {id}")).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let n: u64 = a
            .query_first(format!("SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE ID = {id}"))
            .await
            .unwrap()
            .unwrap();
        if n == 0 {
            return;
        }
        assert!(Instant::now() < deadline, "connection {id} stays");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `shop.zz_nops_<name>_<pid>`, dropped with the guard.
struct Table {
    url: String,
    name: String,
}

impl Table {
    async fn make(url: &str, name: &str) -> Table {
        let name = format!("zz_nops_{name}_{}", std::process::id());
        let mut c = side(url).await;
        c.query_drop(format!("DROP TABLE IF EXISTS shop.{name}")).await.unwrap();
        c.query_drop(format!("CREATE TABLE shop.{name} (x INT PRIMARY KEY)")).await.unwrap();
        c.disconnect().await.unwrap();
        Table { url: url.to_string(), name }
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

struct Conn {
    session: Session,
    rx: UnboundedReceiver<DbEvent>,
}

impl Conn {
    async fn open(url: &str, test: &str) -> Conn {
        let (tx, mut rx) = unbounded_channel();
        let tag = format!("nops{}-{test}", std::process::id());
        let session = MyDriver.connect(
            &profile(url),
            SessionRole::Query,
            ConnectOptions::new(PAGE, SessionRole::Query, &tag),
            tx,
        );
        loop {
            match tokio::time::timeout(Duration::from_secs(15), rx.recv()).await.expect("an answer") {
                Some(DbEvent::Connected) => return Conn { session, rx },
                Some(DbEvent::ConnectFailed { error, .. }) => panic!("connect failed: {error:?}"),
                Some(_) => {}
                None => panic!("event channel closed"),
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

    /// Every event within `secs` until `end` holds for one (with it).
    async fn until(&mut self, secs: u64, end: impl Fn(&DbEvent) -> bool) -> Vec<DbEvent> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut evs = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.rx.recv()).await {
                Ok(Some(ev)) => {
                    let done = end(&ev);
                    evs.push(ev);
                    if done {
                        return evs;
                    }
                }
                Ok(None) => panic!("event channel closed: {evs:?}"),
                Err(_) => panic!("nothing ended within {secs}s: {evs:?}"),
            }
        }
    }

    /// Every event until the session is lost.
    async fn until_lost(&mut self, secs: u64) -> Vec<DbEvent> {
        self.until(secs, |e| matches!(e, DbEvent::Lost { .. })).await
    }

    /// Every event up to and with run `id`'s answer.
    async fn answer(&mut self, id: u64) -> Vec<DbEvent> {
        self.until(60, |e| {
            matches!(e, DbEvent::Page { id: i, columns: Some(_), .. }
                | DbEvent::Done { id: i, .. } | DbEvent::Failed { id: i, .. } if *i == id)
        })
        .await
    }
}

/// Run `id`'s failures among `evs`.
fn failures(evs: &[DbEvent], id: u64) -> Vec<&DbEvent> {
    evs.iter().filter(|e| matches!(e, DbEvent::Failed { id: i, .. } if *i == id)).collect()
}

/// The cancel of a statement that runs, on a server without `performance_schema`: no `KILL` is
/// sent (none at all reaches the server, nor does the cancel open a connection to send one on),
/// the session's connection is cut at once instead of waiting for the statement, the run ends
/// cancelled once, and the session is lost saying the cancel was not sent because the server
/// could not confirm the session. A new session works.
#[tokio::test(flavor = "multi_thread")]
async fn an_in_flight_cancel_without_performance_schema_sends_no_kill_and_ends_the_read() {
    let test = "an_in_flight_cancel_without_performance_schema_sends_no_kill_and_ends_the_read";
    // Before any connection to the server: the other test's connections would count.
    let _alone = ALONE.lock().await;
    let Some(url) = nops_url(test).await else { return };
    let mut a = side(&url).await;
    let mut c = Conn::open(&url, test).await;
    let marker = format!("zz_nops_inflight_{}", std::process::id());
    let sql = format!("SELECT SLEEP(20) AS {marker}");
    c.send(1, &[&sql]);
    let id = running(&mut a, &marker).await;
    let before = kills_and_connections(&mut a).await;
    let t0 = Instant::now();
    c.session.cancel();
    let evs = c.until_lost(15).await;
    let took = t0.elapsed();
    let after = kills_and_connections(&mut a).await;
    // Within the cancel's wait, and long before the statement's own end: the read is not
    // waited for.
    assert!(took < Duration::from_secs(3), "{took:?}: {evs:?}");
    assert!(
        matches!(failures(&evs, 1)[..], [DbEvent::Failed { error: DbError::Cancelled, cancelled: true, .. }]),
        "{evs:?}"
    );
    assert!(!evs.iter().any(|e| matches!(e, DbEvent::Page { id: 1, .. } | DbEvent::Done { id: 1, .. })), "{evs:?}");
    match evs.last() {
        Some(DbEvent::Lost { error: DbError::CancelNotTaken(CancelFailure::NotSent(why)) }) => {
            assert_eq!(**why, DbError::CancelUnconfirmed, "{evs:?}")
        }
        other => panic!("{other:?}: {evs:?}"),
    }
    assert_eq!(after.0, before.0, "no KILL reached the server");
    assert_eq!(after.1, before.1, "the cancel opened no connection");
    end(&mut a, id).await;
    let mut c = Conn::open(&url, test).await;
    c.send(2, &["SELECT 1"]);
    let evs = c.answer(2).await;
    assert!(matches!(evs.last(), Some(DbEvent::Page { .. })), "{evs:?}");
    a.disconnect().await.unwrap();
}

/// Without `performance_schema` a cancel still stops what the session itself can stop: a run
/// waiting behind another is answered cancelled and never runs while the one before it ends as
/// it would (the session stays), and a run of several statements cancelled while its first one
/// runs runs none of the later ones. No `KILL` reaches the server either way.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_between_statements_works_without_performance_schema() {
    let test = "a_cancel_between_statements_works_without_performance_schema";
    // Before any connection to the server: the other test's connections would count.
    let _alone = ALONE.lock().await;
    let Some(url) = nops_url(test).await else { return };
    let t = Table::make(&url, "between").await;
    let mut a = side(&url).await;
    let kills = kills_and_connections(&mut a).await.0;
    let mut c = Conn::open(&url, test).await;
    let one = format!("INSERT INTO {} VALUES (1)", t.q());
    let two = format!("INSERT INTO {} VALUES (2)", t.q());
    // A run behind another: the cancel is the last run's, which has not started. The run before
    // it waits for a lock the side connection holds, so it cannot end before the cancel; the
    // lock is let go once the cancel was asked.
    let lock = format!("zz_nops_hold_{}", std::process::id());
    let held: Option<i64> = a.exec_first("SELECT GET_LOCK(?, 5)", (lock.clone(),)).await.unwrap();
    assert_eq!(held, Some(1), "the side connection holds {lock}");
    let marker = format!("zz_nops_first_{}", std::process::id());
    c.send(1, &[&format!("SELECT GET_LOCK('{lock}', 30) AS {marker}")]);
    c.send(2, &[&one]);
    running(&mut a, &marker).await;
    c.session.cancel();
    let let_go: Option<i64> = a.exec_first("SELECT RELEASE_LOCK(?)", (lock.clone(),)).await.unwrap();
    assert_eq!(let_go, Some(1));
    let evs = c.until(15, |e| matches!(e, DbEvent::Failed { id: 2, .. } | DbEvent::Lost { .. })).await;
    let first = evs.iter().find_map(|e| match e {
        DbEvent::Page { id: 1, columns: Some(_), rows, .. } => Some(rows.clone()),
        _ => None,
    });
    assert_eq!(first, Some(vec![vec![Some("1".to_string())]]), "the run before it got its lock: {evs:?}");
    assert!(failures(&evs, 1).is_empty(), "the run before it ends as it would: {evs:?}");
    assert!(
        matches!(failures(&evs, 2)[..], [DbEvent::Failed { error: DbError::Cancelled, cancelled: true, .. }]),
        "{evs:?}"
    );
    assert!(!evs.iter().any(|e| matches!(e, DbEvent::Lost { .. })), "{evs:?}");
    assert_eq!(t.count().await, 0, "the cancelled run did not run");
    c.send(3, &[&format!("SELECT RELEASE_LOCK('{lock}')")]);
    let evs = c.answer(3).await;
    let Some(DbEvent::Page { rows, .. }) = evs.last() else { panic!("the session goes on: {evs:?}") };
    assert_eq!(rows, &vec![vec![Some("1".to_string())]], "the session held the lock");
    // Several statements, cancelled while the first runs: the later ones never start.
    let marker = format!("zz_nops_many_{}", std::process::id());
    c.send(4, &[&format!("SELECT SLEEP(20) AS {marker}"), &one, &two]);
    let id = running(&mut a, &marker).await;
    c.session.cancel();
    let evs = c.until_lost(15).await;
    assert!(
        matches!(failures(&evs, 4)[..], [DbEvent::Failed { error: DbError::Cancelled, cancelled: true, .. }]),
        "{evs:?}"
    );
    let started: Vec<usize> = evs
        .iter()
        .filter_map(|e| if let DbEvent::Started { id: 4, index } = e { Some(*index) } else { None })
        .collect();
    assert_eq!(started, [0], "{evs:?}");
    assert!(
        matches!(evs.last(), Some(DbEvent::Lost { error: DbError::CancelNotTaken(CancelFailure::NotSent(_)) })),
        "{evs:?}"
    );
    assert_eq!(kills_and_connections(&mut a).await.0, kills, "no KILL reached the server");
    end(&mut a, id).await;
    assert_eq!(t.count().await, 0, "no later statement ran");
    a.disconnect().await.unwrap();
}
