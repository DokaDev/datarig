//! The monitor session (`SessionRole::Monitor`) against a real MySQL (`DATARIG_TEST_MYSQL_URL`
//! and `DATARIG_TEST_MYSQL_ADMIN_URL`, see `integration_mysql.rs`). In a binary of its own: it
//! makes an account (`zz_mon_<pid>`, dropped when the test ends).
//!
//! The monitor only reads. The tests make what it should see with sessions of their own: a
//! table `zz_it_mon_<pid>` (dropped when the test ends), a transaction holding a row of it, an
//! update waiting for that row and an `ALTER TABLE` waiting for the table, read as sessions and
//! as lock waits. The actions are sent only to sessions the tests open themselves.

use datarig_core::driver::TxRule;
use datarig_core::driver::{
    ConnectOptions, DbCommand, DbError, DbEvent, Driver, OwnSession, PagingMode, Session, SessionRole,
};
use datarig_core::monitor::action::{self, ActionOutcome, Denied, NotSent, Stale, Via};
use datarig_core::monitor::locks::{self, Blame, Holder, Tree};
use datarig_core::monitor::rate::{self, Pending, Rate};
use datarig_core::monitor::{
    ActionKind, ActionTarget, Confirmation, FollowUp, FullText, Hidden, MonitorFormat, MySqlMonitor, Need, Part, Proof,
    QUERY_CHARS, Readable, SessionKey, SessionRow, SessionState, SessionsWanted, Snapshot, StatementStamp,
    TEXT_MIN_AGE_US, TextAsk, Timestamp, Tx, Wait, WaitClass, Want,
};
use datarig_core::monitor::{CounterKind, GaugeKind};
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

const TICK: Want =
    Want { sessions: Some(SessionsWanted { idle: true }), locks: false, server: true, limit: Duration::from_secs(5) };

struct Conn {
    session: Session,
    rx: UnboundedReceiver<DbEvent>,
    next: u64,
}

impl Conn {
    async fn open(url: &str, role: SessionRole, tag: &str) -> Conn {
        Conn::open_with(url, role, ConnectOptions::new(500, role, tag)).await
    }

    async fn open_with(url: &str, role: SessionRole, opts: ConnectOptions) -> Conn {
        let (tx, rx) = unbounded_channel();
        let session = MyDriver.connect(&profile(url), role, opts, tx);
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

    /// A monitor session of a profile that is read-only.
    async fn read_only_monitor(url: &str, tag: &str) -> (Conn, MySqlMonitor) {
        let opts = ConnectOptions::new(500, SessionRole::Monitor, tag).read_only(true);
        let mut c = Conn::open_with(url, SessionRole::Monitor, opts).await;
        match c.wait(|e| matches!(e, DbEvent::Monitor(_)), 20).await {
            DbEvent::Monitor(Ok(MonitorFormat::MySql(my))) => (c, my),
            other => panic!("probe: {other:?}"),
        }
    }

    async fn act(&mut self, kind: ActionKind, target: &ActionTarget) -> ActionOutcome {
        self.next += 1;
        let request = self.next;
        self.session.send(DbCommand::MonitorAction { request, kind, target: target.clone() });
        match self.wait(|e| matches!(e, DbEvent::MonitorActionDone { request: r, .. } if *r == request), 30).await {
            DbEvent::MonitorActionDone { outcome, .. } => outcome,
            _ => unreachable!(),
        }
    }

    async fn follow(&mut self, target: &ActionTarget) -> FollowUp {
        self.next += 1;
        let request = self.next;
        self.session.send(DbCommand::MonitorFollowUp { request, target: target.clone() });
        match self.wait(|e| matches!(e, DbEvent::MonitorFollowUp { request: r, .. } if *r == request), 30).await {
            DbEvent::MonitorFollowUp { result, .. } => result.expect("follow-up"),
            _ => unreachable!(),
        }
    }

    /// The row of session `id` once `ready` holds for it (at most ten seconds).
    async fn row_when(&mut self, id: i64, ready: impl Fn(&SessionRow) -> bool) -> SessionRow {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let s = self.tick(TICK).await.expect("tick");
            let r = row(&s, id).clone();
            if ready(&r) {
                return r;
            }
            assert!(Instant::now() < deadline, "session {id} never got ready: {r:?}");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// The row of session `id` once it runs `SLEEP`.
    async fn sleeping(&mut self, id: i64) -> SessionRow {
        self.row_when(id, |r| {
            r.state == SessionState::Active && r.query.as_ref().is_some_and(|q| q.text.contains("SLEEP"))
        })
        .await
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

    /// Which server session is the connection's own ([`DbCommand::ServerSession`]).
    async fn own_session(&mut self) -> OwnSession {
        self.next += 1;
        let request = self.next;
        self.session.send(DbCommand::ServerSession { request });
        match self.wait(|e| matches!(e, DbEvent::ServerSession { request: r, .. } if *r == request), 30).await {
            DbEvent::ServerSession { own, .. } => own,
            _ => unreachable!(),
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

    fn send(&mut self, sql: &str) -> u64 {
        self.next += 1;
        let id = self.next;
        let statements = vec![sql.to_string()];
        self.session.send(DbCommand::Execute {
            max_rows: None,
            id,
            statements,
            paging: PagingMode::NoHold,
            tx: TxRule::Auto,
        });
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

    // The Locks view's read: the row lock wait names its blocker; the ALTER's metadata lock
    // request is paired with the incompatible locks the two transactions on the table hold.
    let locked = monitor
        .tick(Want { sessions: Some(SessionsWanted { idle: false }), locks: true, server: false, ..TICK })
        .await
        .expect("tick");
    let lock_list = locked.locks.as_ref().expect("the locks were asked for");
    assert!(locked.hidden.is_empty(), "{:?}", locked.hidden);
    let row_wait = lock_list.waits_of(w).next().unwrap_or_else(|| panic!("{lock_list:?}"));
    assert_eq!(row_wait.object.kind, "RECORD");
    assert!(row_wait.holders.iter().any(|x| x.holder == Holder::Session(h) && x.blame == Blame::Proven));
    let mdl = lock_list.waits_of(a).find(|x| x.object.kind == "TABLE").unwrap_or_else(|| panic!("{lock_list:?}"));
    assert_eq!(mdl.mode, "EXCLUSIVE");
    let holders: Vec<&Holder> =
        mdl.holders.iter().filter(|x| x.blame == Blame::Incompatible).map(|x| &x.holder).collect();
    assert!(holders.contains(&&Holder::Session(h)) && holders.contains(&&Holder::Session(w)), "{mdl:?}");
    let tree = Tree::build(&locks::edges(locked.sessions.as_ref(), Some(lock_list), true));
    let line =
        |id: i64| tree.lines.iter().find(|l| l.node == Holder::Session(id)).unwrap_or_else(|| panic!("{tree:?}"));
    assert_eq!(line(h).depth, 0);
    assert!(line(w).depth >= 1 && line(a).depth >= 1, "{tree:?}");

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
    // The lock tables are refused: said, never an empty list of waits.
    let s = monitor.tick(Want { locks: true, server: false, ..TICK }).await.expect("tick");
    for part in [Part::RowLockWaits, Part::MetadataLockWaits] {
        let need = Need::Grant("SELECT on performance_schema");
        assert!(s.hidden.contains(&Hidden { part, need }), "{:?}", s.hidden);
    }
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

/// A statement longer than the list holds is read whole while its session runs it (likely the
/// same: the server tells its start in seconds); afterwards the read says the session runs
/// another statement or none, and once the session has ended, that it is gone.
#[tokio::test(flavor = "multi_thread")]
async fn a_statements_whole_text_is_read_only_while_it_runs() {
    let Some((url, _)) = urls("a_statements_whole_text_is_read_only_while_it_runs") else { return };
    let pid = std::process::id();
    let tag = format!("mont{pid}");
    let mut runner = Conn::open(&url, SessionRole::Query, &format!("{tag}r")).await;
    let marker = format!("t{pid}");
    let pad = "x".repeat(QUERY_CHARS);
    let sql = format!("SELECT '{marker}' AS m, SLEEP(9) AS s, '{pad}' AS pad, 'the end' AS e");
    let running = runner.send(&sql);
    let (mut monitor, _) = Conn::monitor(&url, &tag).await;
    // Listed young, it cannot be told apart from a later run yet; listed after five seconds, it
    // can.
    let mut young = false;
    let deadline = Instant::now() + Duration::from_secs(15);
    let (row, seen) = loop {
        let s = monitor.tick(TICK).await.expect("tick");
        let list = s.sessions.as_ref().expect("sessions were asked for");
        let found = list.rows.iter().find(|r| r.query.as_ref().is_some_and(|q| q.text.contains(&marker))).cloned();
        if let Some(row) = found {
            let age = row.query_started.map(|q| list.time.0 - q.0).unwrap_or(0);
            if age >= TEXT_MIN_AGE_US + 1_000_000 {
                break (row, list.time);
            }
            if age < TEXT_MIN_AGE_US - 1_000_000 && !young {
                assert_eq!(monitor.text(&row, list.time).await, Ok(FullText::NotYet));
                young = true;
            }
        }
        assert!(Instant::now() < deadline, "the statement is not listed long enough: {:?}", list.rows);
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    assert!(young, "seen young first");
    let shown = row.query.clone().expect("the statement is listed");
    assert!(shown.shortened && shown.text.chars().count() == QUERY_CHARS, "{shown:?}");
    match monitor.text(&row, seen).await.expect("read") {
        FullText::Text { text, proof } => {
            assert_eq!((text.text.as_str(), text.shortened, proof), (sql.as_str(), false, Proof::Likely));
        }
        other => panic!("{other:?}"),
    }

    runner.answer(running).await.expect("the statement ends");
    assert_eq!(monitor.text(&row, seen).await, Ok(FullText::Moved), "no statement now");
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

/// Statements by type count every client's but the monitor's own `SELECT`s, so an idle server
/// reads none; the threads are counted by command from the session list's process list.
#[tokio::test(flavor = "multi_thread")]
async fn statements_by_type_leave_out_our_own_selects() {
    let test = "statements_by_type_leave_out_our_own_selects";
    let Some((url, admin_url)) = urls(test) else { return };
    let pid = std::process::id();
    let table = format!("zz_it_mon_t_{pid}");
    let _drop = Cleanup { url: url.clone(), sql: format!("DROP TABLE IF EXISTS {table}, {table}_b") };
    let (mut monitor, _) = Conn::monitor(&admin_url, &format!("mont{pid}")).await;
    // Our own reads only between two ticks: none (tried again while another client is busy).
    let mut quiet = None;
    for _ in 0..10 {
        let a = monitor.tick(TICK).await.expect("tick");
        let b = monitor.tick(TICK).await.expect("tick");
        assert!(b.sample.own.selects > a.sample.own.selects, "each tick sends SELECTs");
        let r = rate::rate(&a.sample, &b.sample, CounterKind::Selects, &mut Pending::default());
        if r == Some(Rate::PerSecond(0.0)) {
            quiet = r;
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert_eq!(quiet, Some(Rate::PerSecond(0.0)), "an idle server reads 0 SELECT/s");
    let mut client = side(&url).await;
    let before = monitor.tick(TICK).await.expect("tick");
    for sql in [
        format!("CREATE TABLE {table} (id INT PRIMARY KEY, v INT)"),
        format!("CREATE TABLE {table}_b (id INT PRIMARY KEY)"),
        format!("INSERT INTO {table} VALUES (1, 0), (2, 0)"),
        format!("INSERT INTO {table}_b SELECT id FROM {table}"),
        format!("REPLACE INTO {table} VALUES (3, 0)"),
        format!("UPDATE {table} SET v = 1 WHERE id = 1"),
        format!("UPDATE {table} t JOIN {table}_b b ON b.id = t.id SET t.v = 2"),
        format!("DELETE FROM {table} WHERE id = 3"),
        format!("DELETE t FROM {table} t JOIN {table}_b b ON b.id = t.id WHERE t.id = 2"),
        format!("SELECT COUNT(*) FROM {table}"),
    ] {
        client.query_drop(&sql).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    let after = monitor.tick(TICK).await.expect("tick");
    let moved = |k| after.sample.get(k).unwrap() - before.sample.get(k).unwrap();
    // At least ours (another client may add to them).
    assert!(moved(CounterKind::Inserts) >= 3, "INSERT, INSERT ... SELECT, REPLACE: {}", moved(CounterKind::Inserts));
    assert!(moved(CounterKind::Updates) >= 2, "single and multiple table: {}", moved(CounterKind::Updates));
    assert!(moved(CounterKind::Deletes) >= 2, "single and multiple table: {}", moved(CounterKind::Deletes));
    let own = after.sample.own.selects - before.sample.own.selects;
    assert!(moved(CounterKind::Selects) > own, "the other client's SELECT counts");
    let gauge = |k| after.gauges.iter().find(|g| g.kind == k).map(|g| g.value).unwrap_or_else(|| panic!("{k:?}"));
    assert!(gauge(GaugeKind::CommandSleep) >= 1, "the other client's connection sleeps");
    assert!(!after.hidden.iter().any(|h| h.part == Part::OtherSessions), "root sees every thread");
}

/// What an action on `row` carries, as the app makes it.
fn target_of(my: &MySqlMonitor, row: &SessionRow) -> ActionTarget {
    action::target(&MonitorFormat::MySql(my.clone()), row).expect("a MySQL session is always told apart")
}

/// A table of one row: a `SLEEP` read from it is stopped by `KILL QUERY` with an error (alone it
/// would return 1 and no error).
async fn sleep_table(url: &str, table: &str) {
    let mut c = side(url).await;
    c.query_drop(format!("CREATE TABLE {table} (id INT PRIMARY KEY) ENGINE=InnoDB")).await.expect("table");
    c.query_drop(format!("INSERT INTO {table} VALUES (1)")).await.expect("row");
}

/// `KILL QUERY` goes only to the statement event the row showed: a later event sends nothing;
/// the right one stops the test's own `SLEEP`, and the statement history proves it
/// interrupted. The monitor's own connection is never killed.
#[tokio::test(flavor = "multi_thread")]
async fn kill_query_stops_only_the_statement_the_row_showed() {
    let test = "kill_query_stops_only_the_statement_the_row_showed";
    let Some((url, admin_url)) = urls(test) else { return };
    let pid = std::process::id();
    let table = format!("zz_it_mon_k_{pid}");
    let _drop = Cleanup { url: url.clone(), sql: format!("DROP TABLE IF EXISTS {table}") };
    sleep_table(&url, &table).await;
    let tag = format!("monk{pid}");
    let mut sleeper = Conn::open(&url, SessionRole::Query, &format!("{tag}s")).await;
    let sleeping = sleeper.send(&format!("SELECT SLEEP(60) FROM {table}"));
    let mut admin = side(&admin_url).await;
    let id = id_of(&mut admin, &format!("datarig-q-{tag}s")).await;
    let (mut monitor, my) = Conn::monitor(&admin_url, &tag).await;
    assert_eq!(my.statements, Readable::Yes, "{my:?}");
    let r = monitor.sleeping(id).await;
    let target = target_of(&my, &r);
    let Some(StatementStamp::MySqlEvent { thread_id, event_id }) = target.stamp else { panic!("{target:?}") };

    let later = ActionTarget {
        stamp: Some(StatementStamp::MySqlEvent { thread_id, event_id: event_id + 1 }),
        ..target.clone()
    };
    assert_eq!(
        monitor.act(ActionKind::Cancel, &later).await,
        ActionOutcome::NotSent(NotSent::Stale(Stale::StatementChanged))
    );
    let other_thread = ActionTarget { thread: Some(thread_id + 100_000), ..target.clone() };
    assert_eq!(
        monitor.act(ActionKind::Terminate, &other_thread).await,
        ActionOutcome::NotSent(NotSent::Stale(Stale::SessionReplaced))
    );
    let still = monitor.follow(&target).await;
    assert_eq!(action::confirm(ActionKind::Cancel, &target, &still), Confirmation::StillRunning);

    assert_eq!(monitor.act(ActionKind::Cancel, &target).await, ActionOutcome::Acknowledged { via: Via::Standard });
    // The query session reads the server's 1317 as a cancel.
    let error = sleeper.answer(sleeping).await.expect_err("the statement was stopped");
    assert_eq!(error, "Cancelled");
    let after = monitor.follow(&target).await;
    assert_eq!(after.interrupted, Some(true), "{after:?}");
    assert_eq!(action::confirm(ActionKind::Cancel, &target, &after), Confirmation::Interrupted);
    sleeper.run("SELECT 1").await;

    let own = monitor.row_when(id_of(&mut admin, &format!("datarig-mon-{tag}")).await, |r| r.monitor).await;
    let own = target_of(&my, &own);
    for kind in [ActionKind::Cancel, ActionKind::Terminate] {
        assert_eq!(monitor.act(kind, &own).await, ActionOutcome::NotSent(NotSent::OwnConnection), "{kind:?}");
    }
    monitor.tick(TICK).await.expect("the monitor goes on");
}

/// `KILL` of a test connection is confirmed only when a later read no longer finds it.
#[tokio::test(flavor = "multi_thread")]
async fn kill_is_confirmed_when_the_session_is_gone() {
    let Some((url, admin_url)) = urls("kill_is_confirmed_when_the_session_is_gone") else { return };
    let tag = format!("monkt{}", std::process::id());
    let mut victim = Conn::open(&url, SessionRole::Query, &format!("{tag}v")).await;
    victim.run("SELECT 1").await;
    let mut admin = side(&admin_url).await;
    let id = id_of(&mut admin, &format!("datarig-q-{tag}v")).await;
    let (mut monitor, my) = Conn::monitor(&admin_url, &tag).await;
    let r = monitor.row_when(id, |r| r.state == SessionState::Idle).await;
    let target = target_of(&my, &r);
    assert_eq!(monitor.act(ActionKind::Terminate, &target).await, ActionOutcome::Acknowledged { via: Via::Standard });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let follow = monitor.follow(&target).await;
        match action::confirm(ActionKind::Terminate, &target, &follow) {
            Confirmation::Gone => break,
            other => assert!(Instant::now() < deadline, "still there: {other:?} {follow:?}"),
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(
        monitor.act(ActionKind::Terminate, &target).await,
        ActionOutcome::NotSent(NotSent::Stale(Stale::SessionGone)),
        "never sent twice to a session that is gone"
    );
}

/// On a read-only profile neither action is sent, nor the read before it: the session goes on
/// sleeping.
#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_profile_sends_no_action() {
    let Some((url, admin_url)) = urls("a_read_only_profile_sends_no_action") else { return };
    let pid = std::process::id();
    let table = format!("zz_it_mon_r_{pid}");
    let _drop = Cleanup { url: url.clone(), sql: format!("DROP TABLE IF EXISTS {table}") };
    sleep_table(&url, &table).await;
    let tag = format!("monr{pid}");
    let mut sleeper = Conn::open(&url, SessionRole::Query, &format!("{tag}s")).await;
    let sleeping = sleeper.send(&format!("SELECT SLEEP(60) FROM {table}"));
    let mut admin = side(&admin_url).await;
    let id = id_of(&mut admin, &format!("datarig-q-{tag}s")).await;
    let (mut monitor, my) = Conn::read_only_monitor(&admin_url, &tag).await;
    let target = target_of(&my, &monitor.sleeping(id).await);
    for kind in [ActionKind::Cancel, ActionKind::Terminate] {
        assert_eq!(monitor.act(kind, &target).await, ActionOutcome::NotSent(NotSent::ReadOnly), "{kind:?}");
    }
    let monitor_id = id_of(&mut admin, &format!("datarig-mon-{tag}")).await;
    let sent: Vec<String> = admin
        .exec(
            "SELECT h.SQL_TEXT FROM performance_schema.events_statements_history h \
             JOIN performance_schema.threads t ON t.THREAD_ID = h.THREAD_ID WHERE t.PROCESSLIST_ID = ?",
            (monitor_id,),
        )
        .await
        .expect("the history reads");
    assert!(sent.iter().all(|s| !s.starts_with("KILL") && !s.contains("WHERE p.ID IN (")), "{sent:?}");
    let still = monitor.follow(&target).await;
    assert_eq!(action::confirm(ActionKind::Cancel, &target, &still), Confirmation::StillRunning);
    admin.query_drop(format!("KILL QUERY {id}")).await.expect("kill");
    assert!(sleeper.answer(sleeping).await.is_err());
}

/// An account without `CONNECTION_ADMIN` is refused another account's session and told the
/// privilege (a self-managed server has no procedure to fall back on); one with it is refused
/// a session holding `SYSTEM_USER`; an account may stop its own sessions without either.
#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_names_what_would_allow_it() {
    let Some((url, admin_url)) = urls("a_refusal_names_what_would_allow_it") else { return };
    let pid = std::process::id();
    let (plain, admin_user) = (format!("zz_mon_act_{pid}"), format!("zz_mon_a_{pid}"));
    let password = format!("zz-{pid}");
    let table = format!("zz_it_mon_g_{pid}");
    let _drop_table = Cleanup { url: url.clone(), sql: format!("DROP TABLE IF EXISTS {table}") };
    let _drop_users =
        Cleanup { url: admin_url.clone(), sql: format!("DROP USER IF EXISTS '{plain}'@'%', '{admin_user}'@'%'") };
    let mut admin = side(&admin_url).await;
    for user in [&plain, &admin_user] {
        admin.query_drop(format!("CREATE USER '{user}'@'%' IDENTIFIED BY '{password}'")).await.expect("user");
        admin.query_drop(format!("GRANT PROCESS ON *.* TO '{user}'@'%'")).await.expect("grant");
        admin.query_drop(format!("GRANT SELECT ON performance_schema.* TO '{user}'@'%'")).await.expect("grant");
    }
    admin.query_drop(format!("GRANT CONNECTION_ADMIN ON *.* TO '{admin_user}'@'%'")).await.expect("grant");
    sleep_table(&url, &table).await;

    let tag = format!("mong{pid}");
    let mut sleeper = Conn::open(&url, SessionRole::Query, &format!("{tag}s")).await;
    let sleeping = sleeper.send(&format!("SELECT SLEEP(60) FROM {table}"));
    let id = id_of(&mut admin, &format!("datarig-q-{tag}s")).await;
    let (mut monitor, my) = Conn::monitor(&as_user(&url, &plain, &password), &format!("{tag}p")).await;
    let target = target_of(&my, &monitor.sleeping(id).await);
    for kind in [ActionKind::Cancel, ActionKind::Terminate] {
        assert_eq!(
            monitor.act(kind, &target).await,
            ActionOutcome::NotSent(NotSent::Refused(Denied::GrantOrSystemUser(&["CONNECTION_ADMIN"]))),
            "{kind:?}"
        );
    }
    admin.query_drop(format!("KILL QUERY {id}")).await.expect("kill");
    assert!(sleeper.answer(sleeping).await.is_err());

    // The administrator's own session holds SYSTEM_USER: an account with CONNECTION_ADMIN but
    // without it is refused as one without the grant (the server answers 1095 for both).
    let mut root_sleeper = side(&admin_url).await;
    let root_id: i64 = root_sleeper.query_first("SELECT CONNECTION_ID()").await.expect("id").expect("a row");
    let root_sleep = tokio::spawn(async move {
        let _ = root_sleeper.query_drop("SELECT SLEEP(60)").await;
    });
    let (mut privileged, my_admin) = Conn::monitor(&as_user(&url, &admin_user, &password), &format!("{tag}a")).await;
    let root_target = target_of(&my_admin, &privileged.sleeping(root_id).await);
    assert_eq!(
        privileged.act(ActionKind::Cancel, &root_target).await,
        ActionOutcome::NotSent(NotSent::Refused(Denied::GrantOrSystemUser(&["CONNECTION_ADMIN"])))
    );
    admin.query_drop(format!("KILL QUERY {root_id}")).await.expect("kill");
    root_sleep.await.expect("the sleep ends");

    // The account's own session: no privilege needed.
    let mut own = Conn::open(&as_user(&url, &plain, &password), SessionRole::Query, &format!("{tag}o")).await;
    let own_sleep = own.send("SELECT SLEEP(60)");
    let own_id = id_of(&mut admin, &format!("datarig-q-{tag}o")).await;
    let own_target = target_of(&my, &monitor.sleeping(own_id).await);
    assert_eq!(
        monitor.act(ActionKind::Terminate, &own_target).await,
        ActionOutcome::Acknowledged { via: Via::Standard }
    );
    assert!(own.answer(own_sleep).await.is_err());
    drop((monitor, privileged, own));
}

/// A query session says which server session is its own.
#[tokio::test(flavor = "multi_thread")]
async fn a_query_session_names_its_server_session() {
    let Some((url, admin_url)) = urls("a_query_session_names_its_server_session") else { return };
    let tag = format!("mons{}", std::process::id());
    let mut query = Conn::open(&url, SessionRole::Query, &tag).await;
    let mut admin = side(&admin_url).await;
    let id = id_of(&mut admin, &format!("datarig-q-{tag}")).await;
    assert_eq!(query.own_session().await, OwnSession::Direct { id });
}
