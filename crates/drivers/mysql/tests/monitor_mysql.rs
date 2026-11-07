//! The monitor session (`SessionRole::Monitor`) against a real MySQL (`DATARIG_TEST_MYSQL_URL`
//! and `DATARIG_TEST_MYSQL_ADMIN_URL`, see `integration_mysql.rs`). In a binary of its own: it
//! makes an account (`zz_mon_<pid>`, dropped when the test ends).
//!
//! The monitor only reads. The tests make what it should see with sessions of their own: a
//! table `zz_it_mon_<pid>` (dropped when the test ends), a transaction holding a row of it, an
//! update waiting for that row and an `ALTER TABLE` waiting for the table.

use datarig_core::driver::TxRule;
use datarig_core::driver::{ConnectOptions, DbCommand, DbError, DbEvent, Driver, PagingMode, Session, SessionRole};
use datarig_core::monitor::{
    Hidden, MonitorFormat, MySqlMonitor, Need, Part, Readable, SessionRow, SessionState, SessionsWanted, Snapshot, Tx,
    Wait, WaitClass, Want,
};
use datarig_core::profile::ConnectionConfig;
use datarig_driver_mysql::MyDriver;
use mysql_async::prelude::Queryable;
use std::io::Write;
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

/// The server's URL and an administrator's.
fn urls(test: &str) -> Option<(String, String)> {
    let url = env_url("DATARIG_TEST_MYSQL_URL", test, "mysql://datarig:datarig@127.0.0.1:53306/shop")?;
    let admin = env_url("DATARIG_TEST_MYSQL_ADMIN_URL", test, "mysql://root:datarig-root@127.0.0.1:53306/")?;
    Some((url, admin))
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

/// `url` with another user and password, and no database (the account may use none).
fn as_user(url: &str, user: &str, password: &str) -> String {
    let mut d = datarig_core::profile::dsn::parse(url).expect("a mysql:// URL");
    d.user = user.to_string();
    d.password = Some(password.to_string());
    d.database = String::new();
    datarig_core::profile::dsn::format(&d)
}

const TICK: Want = Want { sessions: Some(SessionsWanted { idle: true }), server: true, limit: Duration::from_secs(5) };

struct Conn {
    session: Session,
    rx: UnboundedReceiver<DbEvent>,
    next: u64,
}

impl Conn {
    async fn open(url: &str, role: SessionRole, tag: &str) -> Conn {
        let (tx, rx) = unbounded_channel();
        let session = MyDriver.connect(&profile(url), role, ConnectOptions::new(500, role, tag), tx);
        let mut c = Conn { session, rx, next: 0 };
        c.wait(|e| matches!(e, DbEvent::Connected), 15).await;
        c
    }

    async fn monitor(url: &str, tag: &str) -> (Conn, MySqlMonitor) {
        let mut c = Conn::open(url, SessionRole::Monitor, tag).await;
        match c.wait(|e| matches!(e, DbEvent::Monitor(_)), 20).await {
            DbEvent::Monitor(Ok(MonitorFormat::MySql(my))) => (c, my),
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

    fn send(&mut self, sql: &str) -> u64 {
        self.next += 1;
        let id = self.next;
        let statements = vec![sql.to_string()];
        self.session.send(DbCommand::Execute { id, statements, paging: PagingMode::NoHold, tx: TxRule::Auto });
        id
    }

    async fn answer(&mut self, id: u64) -> Result<(), String> {
        let ev = self
            .wait(
                |e| matches!(e, DbEvent::Page { id: i, .. } | DbEvent::Done { id: i, .. } | DbEvent::Failed { id: i, .. } if *i == id),
                60,
            )
            .await;
        match ev {
            DbEvent::Failed { error, .. } => Err(format!("{error:?}")),
            _ => Ok(()),
        }
    }

    async fn run(&mut self, sql: &str) {
        let id = self.send(sql);
        self.answer(id).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
}

/// A plain client connection to look at the server from the side and clean up.
async fn side(url: &str) -> mysql_async::Conn {
    let opts = mysql_async::Opts::from_url(url).expect("a mysql:// URL");
    mysql_async::Conn::new(mysql_async::OptsBuilder::from_opts(opts).prefer_socket(false)).await.expect("side")
}

/// The connection id of the one session whose `program_name` is `program`.
async fn id_of(admin: &mut mysql_async::Conn, program: &str) -> i64 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let ids: Vec<i64> = admin
            .exec(
                "SELECT PROCESSLIST_ID FROM performance_schema.session_connect_attrs \
                 WHERE ATTR_NAME = 'program_name' AND ATTR_VALUE = ?",
                (program,),
            )
            .await
            .expect("session_connect_attrs reads");
        if let [id] = ids[..] {
            return id;
        }
        assert!(Instant::now() < deadline, "sessions of {program}: {ids:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Drops what a test made when it ends, on a connection of its own, also when it fails.
struct Cleanup {
    url: String,
    sql: String,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let (url, sql) = (self.url.clone(), self.sql.clone());
        let done = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
            rt.block_on(async move {
                let mut c = side(&url).await;
                c.query_drop(sql).await.map_err(|e| e.to_string())
            })
        })
        .join();
        if !matches!(done, Ok(Ok(()))) {
            let _ = writeln!(std::io::stderr(), "warning: {}: {done:?}", self.sql);
        }
    }
}

fn row(snapshot: &Snapshot, id: i64) -> &SessionRow {
    let list = snapshot.sessions.as_ref().expect("sessions were asked for");
    list.rows.iter().find(|r| r.key.id == id).unwrap_or_else(|| panic!("no session {id} in {:?}", list.rows))
}

/// Seen by an account with `PROCESS`: a transaction holding a row is idle in transaction since
/// its start, the update waiting for the row waits for a row lock, the `ALTER TABLE` behind the
/// transaction for the table's metadata lock; the monitor's own session is marked; each tick
/// counts the statements it sent.
#[tokio::test(flavor = "multi_thread")]
async fn sessions_show_row_and_metadata_lock_waits() {
    let test = "sessions_show_row_and_metadata_lock_waits";
    let Some((url, admin_url)) = urls(test) else { return };
    let pid = std::process::id();
    let table = format!("zz_it_mon_{pid}");
    let myisam = format!("zz_it_mon_m_{pid}");
    let _drop = Cleanup { url: url.clone(), sql: format!("DROP TABLE IF EXISTS {table}, {myisam}") };
    let tag = format!("mon{pid}");
    let mut holder = Conn::open(&url, SessionRole::Query, &format!("{tag}h")).await;
    holder.run(&format!("CREATE TABLE {table} (id INT PRIMARY KEY, v INT) ENGINE=InnoDB")).await;
    holder.run(&format!("INSERT INTO {table} VALUES (1, 0)")).await;
    holder.run(&format!("CREATE TABLE {myisam} (id INT) ENGINE=MyISAM")).await;
    // A transaction that read only a MyISAM table: no InnoDB transaction, its locks held.
    let mut reader = Conn::open(&url, SessionRole::Query, &format!("{tag}r")).await;
    reader.run("BEGIN").await;
    reader.run(&format!("SELECT COUNT(*) FROM {myisam}")).await;
    holder.run("BEGIN").await;
    holder.run(&format!("UPDATE {table} SET v = v + 1 WHERE id = 1")).await;
    let mut waiter = Conn::open(&url, SessionRole::Query, &format!("{tag}w")).await;
    waiter.run("SET SESSION innodb_lock_wait_timeout = 30").await;
    let waiting = waiter.send(&format!("UPDATE {table} SET v = v + 10 WHERE id = 1"));
    let mut alter = Conn::open(&url, SessionRole::Query, &format!("{tag}a")).await;
    alter.run("SET SESSION lock_wait_timeout = 30").await;
    let altering = alter.send(&format!("ALTER TABLE {table} ADD COLUMN x INT"));

    let mut admin = side(&admin_url).await;
    let (h, w, a) = (
        id_of(&mut admin, &format!("datarig-q-{tag}h")).await,
        id_of(&mut admin, &format!("datarig-q-{tag}w")).await,
        id_of(&mut admin, &format!("datarig-q-{tag}a")).await,
    );
    let r = id_of(&mut admin, &format!("datarig-q-{tag}r")).await;
    let (mut monitor, my) = Conn::monitor(&admin_url, &tag).await;
    assert_eq!((my.process, my.innodb_trx, my.metadata_locks), (Readable::Yes, Readable::Yes, Readable::Yes), "{my:?}");
    let deadline = Instant::now() + Duration::from_secs(10);
    let snapshot = loop {
        let s = monitor.tick(TICK).await.expect("tick");
        if (row(&s, w).waits_for_lock() && row(&s, a).waits_for_lock()) || Instant::now() > deadline {
            break s;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    let holder_row = row(&snapshot, h);
    assert_eq!(holder_row.state, SessionState::IdleInTx);
    assert!(matches!(holder_row.tx, Tx::Since(_)));
    assert_eq!(row(&snapshot, w).wait, Some(Wait { class: WaitClass::Lock, name: "row lock".into() }));
    assert_eq!(row(&snapshot, a).wait, Some(Wait { class: WaitClass::Lock, name: "table metadata lock".into() }));
    let reader_row = row(&snapshot, r);
    assert_eq!((&reader_row.state, reader_row.tx), (&SessionState::IdleInTx, Tx::Open));
    // Kept when idle sessions are left out: it holds locks.
    let busy = monitor.tick(Want { sessions: Some(SessionsWanted { idle: false }), ..TICK }).await.expect("tick");
    assert!(busy.sessions.as_ref().unwrap().idle_left_out);
    row(&busy, r);
    row(&busy, h);
    let own: Vec<_> = snapshot.sessions.as_ref().unwrap().rows.iter().filter(|r| r.monitor).collect();
    assert_eq!(own.len(), 1);
    assert!(snapshot.hidden.is_empty(), "{:?}", snapshot.hidden);
    assert!(snapshot.server.as_ref().is_some_and(|s| !s.version.is_empty()));

    // Counters only: the clock and the status, two statements of ours.
    let counted = snapshot.sample.own.statements;
    let only = monitor.tick(Want { sessions: None, server: false, ..TICK }).await.expect("tick");
    assert_eq!(
        only.sample.own.statements,
        counted + 2 + 1 + 2 + (2 + 1 + 2),
        "sessions and server info of the tick, the whole next tick, then two"
    );

    holder.run("ROLLBACK").await;
    reader.run("ROLLBACK").await;
    waiter.answer(waiting).await.expect("the waiter goes on once the holder is gone");
    alter.answer(altering).await.expect("the ALTER goes on once the transactions end");
}

/// An account without `PROCESS` sees only its own sessions and no transaction state, and the
/// monitor says which privilege would show them; granted `PROCESS` it sees every session, and
/// with `SELECT` on `performance_schema` too, every transaction.
#[tokio::test(flavor = "multi_thread")]
async fn what_an_account_cannot_see_names_the_privilege() {
    let test = "what_an_account_cannot_see_names_the_privilege";
    let Some((url, admin_url)) = urls(test) else { return };
    let pid = std::process::id();
    let user = format!("zz_mon_{pid}");
    let password = format!("zz-{pid}");
    let _drop = Cleanup { url: admin_url.clone(), sql: format!("DROP USER IF EXISTS '{user}'@'%'") };
    let mut admin = side(&admin_url).await;
    admin.query_drop(format!("CREATE USER '{user}'@'%' IDENTIFIED BY '{password}'")).await.expect("user");

    let (mut monitor, my) = Conn::monitor(&as_user(&url, &user, &password), &format!("monu{pid}")).await;
    assert_eq!(
        (my.process, my.innodb_trx, my.metadata_locks),
        (Readable::Unknown, Readable::Refused, Readable::Refused),
        "{my:?}"
    );
    let s = monitor.tick(TICK).await.expect("tick");
    assert_eq!(
        s.hidden,
        [
            Hidden { part: Part::OtherSessions, need: Need::Grant("PROCESS") },
            Hidden { part: Part::TransactionState, need: Need::Grant("PROCESS") },
        ]
    );
    let list = s.sessions.as_ref().unwrap();
    assert!(list.rows.iter().all(|r| r.user.as_deref() == Some(user.as_str())), "{:?}", list.rows);
    assert!(list.rows.iter().all(|r| r.tx == Tx::Unknown));
    drop(monitor);

    admin.query_drop(format!("GRANT PROCESS ON *.* TO '{user}'@'%'")).await.expect("grant");
    let (mut monitor, my) = Conn::monitor(&as_user(&url, &user, &password), &format!("monp{pid}")).await;
    assert_eq!(
        (my.process, my.innodb_trx, my.metadata_locks),
        (Readable::Yes, Readable::Yes, Readable::Refused),
        "{my:?}"
    );
    let s = monitor.tick(TICK).await.expect("tick");
    // Every session now; whether an idle one without an InnoDB transaction holds locks still
    // needs the metadata lock table.
    assert_eq!(s.hidden, [Hidden { part: Part::TransactionState, need: Need::Grant("SELECT on performance_schema") }]);
    assert!(s.sessions.as_ref().unwrap().rows.iter().any(|r| r.user.as_deref() != Some(user.as_str())));
    drop(monitor);

    admin.query_drop(format!("GRANT SELECT ON performance_schema.* TO '{user}'@'%'")).await.expect("grant");
    let (mut monitor, my) = Conn::monitor(&as_user(&url, &user, &password), &format!("monl{pid}")).await;
    assert_eq!(my.metadata_locks, Readable::Yes, "{my:?}");
    assert!(monitor.tick(TICK).await.expect("tick").hidden.is_empty());
}

/// The monitor session runs nothing but its own reads, and the other sessions answer a tick
/// sent to them, so nothing waits for it.
#[tokio::test(flavor = "multi_thread")]
async fn only_the_monitor_session_reads_ticks() {
    let Some((url, _)) = urls("only_the_monitor_session_reads_ticks") else { return };
    let pid = std::process::id();
    let (mut monitor, _) = Conn::monitor(&url, &format!("monx{pid}")).await;
    let id = monitor.send("SELECT 1");
    assert_eq!(monitor.answer(id).await, Err(format!("{:?}", DbError::NotSupported)));
    monitor.tick(TICK).await.expect("still reads");
    let mut query = Conn::open(&url, SessionRole::Query, &format!("monq{pid}")).await;
    assert_eq!(query.tick(TICK).await, Err(DbError::NotSupported));
    query.run("SELECT 1").await;
    let mut meta = Conn::open(&url, SessionRole::Meta, &format!("monm{pid}")).await;
    assert_eq!(meta.tick(TICK).await, Err(DbError::NotSupported));
}
