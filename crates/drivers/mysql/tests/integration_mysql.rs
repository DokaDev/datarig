//! Integration tests against a real MySQL loaded with `dev/init-mysql/*.sql`.
//!
//! Connection: `DATARIG_TEST_MYSQL_URL` (e.g. `mysql://datarig:datarig@127.0.0.1:53306/shop`).
//! The tests that look at the server from the side (`performance_schema`) or make accounts need
//! `DATARIG_TEST_MYSQL_ADMIN_URL` too (e.g. `mysql://root:datarig-root@127.0.0.1:53306/`).
//! * unset locally  -> each test prints a visible `SKIPPED` line to stderr and returns;
//! * unset with `DATARIG_REQUIRE_MYSQL=1` (CI's `integration` job) -> the test fails, so that
//!   job can never silently skip.
//!
//! Every object a test makes is named `zz_it_…` and dropped again, also when the test fails.
//!
//! `DATARIG_TEST_DIAL=tcp` runs the suite with every session, test connection and cancel going
//! through a dialer (`transport::TcpDialer`), as they do through an SSH tunnel; CI runs the
//! suite both ways.

use datarig_core::driver::{
    CancelFailure, Cell, ConnectOptions, DbCommand, DbError, DbEvent, Driver, PagingMode, PingError, Session,
    SessionRole, StatementInfo, TxRule, ValueKind,
};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_core::transport::{Dialer, DialerRef, TcpDialer};
use datarig_driver_mysql::MyDriver;
use mysql_async::prelude::Queryable;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

const PAGE: usize = 500;

fn require() -> bool {
    std::env::var("DATARIG_REQUIRE_MYSQL").is_ok_and(|v| v == "1")
}

fn env_url(var: &str, test: &str, example: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            if require() {
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

/// The server's URL and an administrator's (root) URL, for the tests that need both.
fn urls(test: &str) -> Option<(String, String)> {
    let url = my_url(test)?;
    let admin = env_url("DATARIG_TEST_MYSQL_ADMIN_URL", test, "mysql://root:datarig-root@127.0.0.1:53306/")?;
    Some((url, admin))
}

/// The tag of test `test`'s sessions in this run (their `program_name` is
/// `datarig-<role>-<tag>`), so a test finds its own sessions on the server.
fn tag(test: &str) -> String {
    format!("it{}-{test}", std::process::id())
}

fn opts(role: SessionRole, test: &str) -> ConnectOptions {
    ConnectOptions::new(PAGE, role, &tag(test)).dialer(dialer())
}

/// The dialer every session of the run goes through (`DATARIG_TEST_DIAL=tcp`), or none.
fn dialer() -> Option<DialerRef> {
    run_dialer().map(|d| DialerRef(d as Arc<dyn Dialer>))
}

fn run_dialer() -> Option<Arc<TcpDialer>> {
    static DIALER: std::sync::OnceLock<Option<Arc<TcpDialer>>> = std::sync::OnceLock::new();
    DIALER
        .get_or_init(|| match std::env::var("DATARIG_TEST_DIAL").as_deref() {
            Ok("tcp") => Some(Arc::new(TcpDialer::default())),
            Ok("") | Err(_) => None,
            Ok(other) => panic!("DATARIG_TEST_DIAL={other}: only `tcp` is known"),
        })
        .clone()
}

/// The profile of `url`, in the TLS mode `DATARIG_TEST_MYSQL_SSLMODE` names (unset: `DISABLED`,
/// as these tests were written; CI runs them once more with `PREFERRED`, over TLS).
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

/// `url` with another user and password.
fn as_user(url: &str, user: &str, password: &str) -> String {
    let mut d = datarig_core::profile::dsn::parse(url).expect("a mysql:// URL");
    d.user = user.to_string();
    d.password = Some(password.to_string());
    datarig_core::profile::dsn::format(&d)
}

/// `url` with another database (`""`: none).
fn in_database(url: &str, database: &str) -> String {
    let mut d = datarig_core::profile::dsn::parse(url).expect("a mysql:// URL");
    d.database = database.to_string();
    datarig_core::profile::dsn::format(&d)
}

struct Conn {
    session: Session,
    rx: UnboundedReceiver<DbEvent>,
}

impl Conn {
    /// A session of `url`'s profile for test `test`, read-only or not, once it connected.
    async fn open(url: &str, role: SessionRole, read_only: bool, test: &str) -> Conn {
        let mut c = Conn::start(url, role, read_only, test);
        let encryption = c.wait(|e| matches!(e, DbEvent::Encryption(_)), 15).await;
        // A run over TLS (`DATARIG_TEST_MYSQL_SSLMODE`) is over TLS indeed: the server offers it.
        let tls = std::env::var("DATARIG_TEST_MYSQL_SSLMODE").is_ok_and(|m| !m.eq_ignore_ascii_case("DISABLED"));
        if tls {
            let encrypted = matches!(encryption, DbEvent::Encryption(datarig_core::tls::Encryption::Tls { .. }));
            assert!(encrypted, "{encryption:?}");
        }
        c.wait(|e| matches!(e, DbEvent::Connected), 15).await;
        c
    }

    fn start(url: &str, role: SessionRole, read_only: bool, test: &str) -> Conn {
        let (tx, rx) = unbounded_channel();
        let session = MyDriver.connect(&profile(url), role, opts(role, test).read_only(read_only), tx);
        Conn { session, rx }
    }

    /// Wait for the first event matching `pred` (other events are dropped).
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

    /// Every event up to and with the terminal one of run `id` (its first page, `Done` or
    /// `Failed`).
    async fn answer(&mut self, id: u64) -> Vec<DbEvent> {
        let mut out = Vec::new();
        loop {
            let ev = self.next(60).await;
            let end = matches!(&ev, DbEvent::Page { id: i, columns: Some(_), .. }
                | DbEvent::Done { id: i, .. } | DbEvent::Failed { id: i, .. } if *i == id);
            if let DbEvent::ConnectFailed { error, .. } = &ev {
                panic!("connect failed: {error:?}");
            }
            out.push(ev);
            if end {
                return out;
            }
        }
    }

    /// Run `sql` (one statement or several) as run `id`, not held: its events up to its answer.
    async fn run(&mut self, id: u64, sql: &[&str]) -> Vec<DbEvent> {
        self.run_as(id, sql, PagingMode::NoHold).await
    }

    async fn run_as(&mut self, id: u64, sql: &[&str], paging: PagingMode) -> Vec<DbEvent> {
        let statements = sql.iter().map(|s| s.to_string()).collect();
        self.session.send(DbCommand::Execute { id, statements, paging, tx: TxRule::Auto });
        self.answer(id).await
    }

    /// Run `sql` and expect it to succeed (its answer).
    async fn ok(&mut self, id: u64, sql: &str) -> DbEvent {
        let evs = self.run(id, &[sql]).await;
        let last = evs.into_iter().last().unwrap();
        assert!(!matches!(last, DbEvent::Failed { .. }), "{sql}: {last:?}");
        last
    }

    /// The first event, whatever it is.
    async fn next(&mut self, secs: u64) -> DbEvent {
        match tokio::time::timeout(Duration::from_secs(secs), self.rx.recv()).await {
            Ok(Some(ev)) => ev,
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out after {secs}s waiting for an event"),
        }
    }
}

/// A plain client connection to look at the server from the side.
async fn side(url: &str) -> mysql_async::Conn {
    let opts = mysql_async::Opts::from_url(url).expect("a mysql:// URL");
    let opts = mysql_async::OptsBuilder::from_opts(opts).prefer_socket(false);
    mysql_async::Conn::new(opts).await.expect("the side connection opens")
}

/// The connection ids of test `test`'s sessions of role `role`, by their `program_name`.
async fn sessions_of(admin: &mut mysql_async::Conn, role: SessionRole, test: &str) -> Vec<u64> {
    let program = format!("datarig-{}-{}", role.tag(), tag(test));
    admin
        .exec(
            "SELECT PROCESSLIST_ID FROM performance_schema.session_connect_attrs \
             WHERE ATTR_NAME = 'program_name' AND ATTR_VALUE = ? ORDER BY PROCESSLIST_ID",
            (program,),
        )
        .await
        .expect("session_connect_attrs reads")
}

/// A session variable of connection `id`, as the server has it.
async fn variable_of(admin: &mut mysql_async::Conn, id: u64, name: &str) -> Option<String> {
    admin
        .exec_first(
            "SELECT v.VARIABLE_VALUE FROM performance_schema.variables_by_thread v \
             JOIN performance_schema.threads t ON t.THREAD_ID = v.THREAD_ID \
             WHERE t.PROCESSLIST_ID = ? AND v.VARIABLE_NAME = ?",
            (id, name),
        )
        .await
        .expect("variables_by_thread reads")
}

/// Test `test`'s one session of `role`.
async fn only_session(admin: &mut mysql_async::Conn, role: SessionRole, test: &str) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let ids = sessions_of(admin, role, test).await;
        if let [id] = ids[..] {
            return id;
        }
        assert!(Instant::now() < deadline, "sessions of {role:?}: {ids:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_query_session_says_how_its_text_is_read() {
    let Some(url) = my_url("a_query_session_says_how_its_text_is_read") else { return };
    let mut c = Conn::start(&url, SessionRole::Query, false, "a_query_session_says_how_its_text_is_read");
    assert!(matches!(c.next(15).await, DbEvent::Encryption(_)), "how its traffic goes, first");
    assert!(matches!(c.next(15).await, DbEvent::Connected));
    let DbEvent::Language(Language::Sql(Dialect::MySql(mode))) = c.next(5).await else {
        panic!("the language follows Connected")
    };
    // The dev server runs the default sql mode.
    assert_eq!((mode.ansi_quotes, mode.no_backslash_escapes), (false, false));
    let mut probe = side(&url).await;
    let version: String = probe.query_first("SELECT VERSION()").await.unwrap().unwrap();
    let mut parts = version.split(|c: char| !c.is_ascii_digit()).map(|n| n.parse::<u16>().unwrap_or(0));
    let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    // `select $$` is a syntax error from 8.1 on (MySQL's client then reads dollar quotes).
    assert_eq!(mode, MySqlMode { dollar_quotes: (major, minor) >= (8, 1), ..MySqlMode::default() }, "{version}");
    probe.disconnect().await.unwrap();
}

/// The metadata session says the mode a new session starts in, then lists the databases, reads
/// the catalog and the key metadata as soon as it connects.
#[tokio::test(flavor = "multi_thread")]
async fn the_metadata_session_reads_the_databases_and_the_catalog() {
    let Some(url) = my_url("the_metadata_session_reads_the_databases_and_the_catalog") else { return };
    let mut c = Conn::start(&url, SessionRole::Meta, false, "metaread");
    assert!(matches!(c.next(15).await, DbEvent::Encryption(_)), "how its traffic goes, first");
    assert!(matches!(c.next(15).await, DbEvent::Connected));
    let lang = c.next(15).await;
    assert!(matches!(lang, DbEvent::Language(Language::Sql(Dialect::MySql(_)))), "{lang:?}");
    match c.next(15).await {
        DbEvent::Schemas(Ok(s)) => {
            assert!(s.contains(&"shop".to_string()), "{s:?}");
            assert!(
                !s.iter().any(|n| ["mysql", "sys", "information_schema", "performance_schema"].contains(&n.as_str()))
            );
        }
        other => panic!("{other:?}"),
    }
    match c.next(30).await {
        DbEvent::Catalog(Ok(cat)) => {
            let users = cat.relations.iter().find(|r| r.schema == "shop" && r.name == "users").expect("shop.users");
            assert!(!users.is_view);
            assert_eq!(users.columns[0].name, "id");
            assert_eq!(users.columns[0].type_name, "bigint unsigned");
            assert!(cat.relations.iter().any(|r| r.name == "order_summary" && r.is_view));
        }
        other => panic!("{other:?}"),
    }
    // Then the key metadata.
    match c.next(30).await {
        DbEvent::Keys(Ok(k)) => assert!(k.marks_by_name("shop", "users", "id").pk),
        other => panic!("{other:?}"),
    }
    c.session.send(DbCommand::LoadObjects { schema: "shop".into() });
    match c.next(15).await {
        DbEvent::Objects { schema, result: Ok(o) } => {
            assert_eq!(schema, "shop");
            assert!(o.tables.contains(&"events".to_string()) && o.views == ["order_summary"], "{o:?}");
            let events = o.stats["events"];
            assert!(events.rows.is_some_and(|r| r > 500_000), "an estimate: {events:?}");
            assert!(events.bytes.is_some_and(|b| b > 1 << 20), "{events:?}");
            assert!(!o.stats.contains_key("order_summary"), "a view has no estimates");
        }
        other => panic!("{other:?}"),
    }
    let next = tokio::time::timeout(Duration::from_millis(300), c.rx.recv()).await;
    assert!(next.is_err(), "nothing more: {next:?}");
}

/// A read of the metadata session never queues behind another session's metadata lock (held,
/// or asked for by an `ALTER TABLE` that waits) beyond its lock wait, and never holds up that
/// session: while the reads run, and right after, the server shows no other session waiting
/// for a lock of the metadata session's but the momentary high-priority one of a table's
/// estimates, and the metadata session keeps none ([`watched`]: a sampled guard; with `LOCK
/// TABLES` the other session can wait for nothing, so there the fact is that the lookups did not
/// wait).
#[tokio::test(flavor = "multi_thread")]
async fn lookups_never_queue_behind_a_metadata_lock_nor_hold_one_up() {
    let Some((url, admin)) = urls("lookups_never_queue_behind_a_metadata_lock") else { return };
    let t = numbers(&admin, "lookup", 200_000).await;
    let mut wide = side(&admin).await;
    wide.query_drop(format!("ALTER TABLE {} MODIFY v VARCHAR(200)", t.q())).await.unwrap();
    wide.query_drop(format!("UPDATE {} SET v = REPEAT('x', 200)", t.q())).await.unwrap();
    wide.disconnect().await.unwrap();
    let mut m = Conn::start(&url, SessionRole::Meta, false, "lookup");
    m.wait(|e| matches!(e, DbEvent::Catalog(_)), 30).await;
    let mut side_admin = side(&admin).await;
    let meta = only_session(&mut side_admin, SessionRole::Meta, "lookup").await;
    let lookups = |m: &mut Conn| {
        m.session.send(DbCommand::LoadObjects { schema: "shop".into() });
        m.session.send(DbCommand::LoadCatalog);
    };
    // 1. Another session holds `LOCK TABLES … WRITE`, and writes while the lookups run.
    let mut lock = side(&admin).await;
    lock.query_drop(format!("LOCK TABLES {} WRITE", t.q())).await.unwrap();
    let locker = u64::from(lock.id());
    let ((), seen) = watched(&admin, meta, &[locker], &t.name, async {
        lookups(&mut m);
        lock.query_drop(format!("INSERT INTO {} VALUES (999999, 'w')", t.q())).await.unwrap();
        m.wait(|e| matches!(e, DbEvent::Objects { .. }), 10).await;
        m.wait(|e| matches!(e, DbEvent::Catalog(_)), 10).await;
    })
    .await;
    // The locking session holds the strongest lock already: it waits for nothing of ours by
    // construction, and the fact here is that the lookups did not wait for it.
    assert!(seen.queued.is_empty(), "the lookups waited for the locking session: {:?}", seen.queued);
    assert!(seen.held_up.is_empty(), "the locking session waited for the lookups: {:?}", seen.held_up);
    assert_eq!(metadata_locks_of(&mut side_admin, meta).await, 0, "no lock kept");
    lock.query_drop("UNLOCK TABLES").await.unwrap();
    // 2. A statement holds a shared lock (a held result), and an `ALTER TABLE` waits for it: a
    //    lookup does not queue behind the waiting ALTER beyond its lock wait.
    let mut q = Conn::open(&url, SessionRole::Query, false, "lookup").await;
    let evs = q.run_as(1, &[&format!("SELECT * FROM {}", t.q())], PagingMode::Hold).await;
    assert!(page_of(&evs).2);
    let (alter_id, alter) = waiting_alter(&admin, &t.q()).await;
    let ((objects, catalog), seen) = watched(&admin, meta, &[alter_id], &t.name, async {
        lookups(&mut m);
        let objects = m.wait(|e| matches!(e, DbEvent::Objects { .. }), 10).await;
        let catalog = m.wait(|e| matches!(e, DbEvent::Catalog(_)), 10).await;
        (objects, catalog)
    })
    .await;
    // The estimates of the tables (`TABLE_ROWS`, `DATA_LENGTH + INDEX_LENGTH`) may open a table
    // whose statistics the server has not kept, for the moment they are read, with a high-priority
    // shared lock granted past the waiting ALTER (which waits for the held result whatever the
    // lookup does) and gone once it has answered: the only lock of the lookups' the ALTER can be
    // seen behind is that one, never a shared read or write lock or a stronger one.
    assert!(
        seen.held_up.iter().all(|w| w.ends_with(": ours SHARED_HIGH_PRIO GRANTED")),
        "the ALTER waited for a lock of the lookups: {:?}",
        seen.held_up
    );
    assert_eq!(metadata_locks_of(&mut side_admin, meta).await, 0, "no lock kept");
    assert!(alter_waits(&mut side_admin, alter_id).await, "the ALTER still waits for the held result");
    // A lookup may wait behind the waiting ALTER, for the session's lock wait only, and then
    // gives up: every wait seen ended `Locked`.
    let wait = variable_of(&mut side_admin, meta, "lock_wait_timeout").await;
    assert_eq!(wait.as_deref(), Some("2"), "the metadata session's lock wait");
    let locked = [&objects, &catalog]
        .iter()
        .filter(|ev| {
            matches!(ev, DbEvent::Objects { result: Err(DbError::Locked), .. } | DbEvent::Catalog(Err(DbError::Locked)))
        })
        .count();
    for ev in [&objects, &catalog] {
        match ev {
            DbEvent::Objects { result: Err(e), .. } | DbEvent::Catalog(Err(e)) => {
                assert_eq!(*e, DbError::Locked, "{ev:?}")
            }
            _ => {}
        }
    }
    assert!(seen.queued.is_empty() || locked > 0, "a lookup waited and was answered: {:?}", seen.queued);
    // The ALTER goes through once the held result is stopped: the lookups held nothing up (the
    // limit only guards against a hang).
    let t1 = Instant::now();
    q.session.send(DbCommand::ClosePortal { id: 1 });
    q.ok(2, "SELECT 1").await;
    let r = alter.await.unwrap();
    assert!(r.is_ok(), "{r:?}");
    assert!(t1.elapsed() < Duration::from_secs(10), "{:?}", t1.elapsed());
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_are_set_up_on_the_server_by_role_and_policy() {
    let Some((url, admin)) = urls("sessions_are_set_up_on_the_server_by_role_and_policy") else { return };
    let mut admin = side(&admin).await;
    for (role, read_only, ro, limit) in [
        (SessionRole::Query, false, "OFF", (PAGE + 1).to_string()),
        (SessionRole::Query, true, "ON", (PAGE + 1).to_string()),
        (SessionRole::Meta, false, "ON", "18446744073709551615".to_string()),
    ] {
        let mut c = Conn::open(&url, role, read_only, "sessions_are_set_up_on_the_server_by_role_and_policy").await;
        // The metadata session reads at once: its variables are looked at once it is idle, after
        // its last read at connect (the keys). While a session reads `information_schema`, the
        // server shows its `character_set_client` and `character_set_connection` as the views'
        // own (`utf8mb3`), not the session's.
        if role == SessionRole::Meta {
            c.wait(|e| matches!(e, DbEvent::Keys(_)), 30).await;
        }
        let id = only_session(&mut admin, role, "sessions_are_set_up_on_the_server_by_role_and_policy").await;
        let mut vars = std::collections::HashMap::new();
        for name in [
            "transaction_read_only",
            "sql_select_limit",
            "character_set_client",
            "character_set_connection",
            "character_set_results",
            "lock_wait_timeout",
            "max_execution_time",
            "sql_quote_show_create",
        ] {
            vars.insert(name, variable_of(&mut admin, id, name).await.unwrap_or_default());
        }
        let what = format!("{role:?} read_only={read_only}");
        assert_eq!(vars["transaction_read_only"], ro, "{what}");
        assert_eq!(vars["sql_select_limit"], limit, "{what}");
        for name in ["character_set_client", "character_set_connection", "character_set_results"] {
            assert_eq!(vars[name], "utf8mb4", "{what}: {name}");
        }
        let meta = role == SessionRole::Meta;
        assert_eq!(vars["lock_wait_timeout"] == "2", meta, "{what}");
        assert_eq!(vars["max_execution_time"] == "10000", meta, "{what}");
        // `SHOW CREATE …` quotes every name on the metadata session, whatever the server's default.
        if meta {
            assert_eq!(vars["sql_quote_show_create"], "ON", "{what}");
        }
        drop(c);
        // Closed: the connection goes.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !sessions_of(&mut admin, role, "sessions_are_set_up_on_the_server_by_role_and_policy").await.is_empty() {
            assert!(Instant::now() < deadline, "{what}: still connected");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    admin.disconnect().await.unwrap();
}

/// The statements test `test`'s session `id` ran, as the server recorded them
/// (`events_statements_history`, its last ten), oldest first. The server records a statement
/// after it answered it: an empty history is asked again for a while.
async fn statements_of(admin: &mut mysql_async::Conn, id: u64) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let ran: Vec<String> = admin
            .exec(
                "SELECT h.SQL_TEXT FROM performance_schema.events_statements_history h \
                 JOIN performance_schema.threads t ON t.THREAD_ID = h.THREAD_ID \
                 WHERE t.PROCESSLIST_ID = ? ORDER BY h.EVENT_ID",
                (id,),
            )
            .await
            .expect("events_statements_history reads");
        if !ran.is_empty() || Instant::now() > deadline {
            return ran;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_is_set_up_in_one_statement() {
    let Some((url, admin)) = urls("a_session_is_set_up_in_one_statement") else { return };
    let mut admin = side(&admin).await;
    for role in [SessionRole::Query, SessionRole::Meta] {
        let mut c = Conn::open(&url, role, true, "a_session_is_set_up_in_one_statement").await;
        if role == SessionRole::Meta {
            // Its reads at connect, the last being the keys, are in the history before it is read.
            c.wait(|e| matches!(e, DbEvent::Keys(_)), 15).await;
        }
        let id = only_session(&mut admin, role, "a_session_is_set_up_in_one_statement").await;
        // The server's answer to the `SET` said the session's values: none is asked after it
        // (the metadata session goes on to its own reads: the databases, the catalog, the keys,
        // which read the server's own `@@lower_case_table_names` and `@@system_time_zone`).
        let ran = statements_of(&mut admin, id).await;
        assert!(ran[0].starts_with("SET NAMES utf8mb4, SESSION session_track_system_variables"), "{ran:?}");
        assert!(ran[1..].iter().all(|s| !s.contains("@@SESSION")), "{role:?}: {ran:?}");
        if role == SessionRole::Query {
            assert_eq!(ran.len(), 1, "{ran:?}");
        }
    }
    admin.disconnect().await.unwrap();
}

/// What the client asks for at login, held against the server: one request runs one statement.
#[tokio::test(flavor = "multi_thread")]
async fn one_request_never_runs_two_statements() {
    let Some(url) = my_url("one_request_never_runs_two_statements") else { return };
    let d = datarig_core::profile::dsn::parse(&url).unwrap();
    let opts = mysql_async::OptsBuilder::default()
        .user(Some(d.user.clone()))
        .pass(d.password.clone())
        .db_name(Some(d.database.clone()))
        .prefer_socket(false);
    let stream = tokio::net::TcpStream::connect((d.host.as_str(), d.port.unwrap_or(3306))).await.unwrap();
    let mut c = mysql_async::Conn::connect_with_stream(opts, Box::new(stream)).await.unwrap();
    match c.query_drop("SELECT 1; SELECT 2").await {
        Err(mysql_async::Error::Server(e)) => assert_eq!(e.code, 1064, "{e}"),
        other => panic!("{other:?}"),
    }
    c.disconnect().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_password_asks_for_one() {
    let Some(url) = my_url("a_wrong_password_asks_for_one") else { return };
    let mut c = Conn::start(
        &as_user(&url, "datarig", "not-the-password"),
        SessionRole::Meta,
        false,
        "a_wrong_password_asks_for_one",
    );
    match c.next(15).await {
        DbEvent::ConnectFailed { error: DbError::AccessDenied(m), auth: true } => {
            assert!(m.starts_with("ERROR 1045 (28000): "), "{m}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_database_fails_without_asking_for_a_password() {
    let Some(url) = my_url("an_unknown_database_fails_without_asking_for_a_password") else { return };
    let mut c = Conn::start(
        &in_database(&url, "zz_it_no_such_database"),
        SessionRole::Meta,
        false,
        "an_unknown_database_fails_without_asking_for_a_password",
    );
    match c.next(15).await {
        // A user without privileges on it hears "access denied" (1044), not "unknown" (1049).
        DbEvent::ConnectFailed { error: DbError::Server(m), auth: false } => {
            assert!(m.starts_with("ERROR 1049 (42000): ") || m.starts_with("ERROR 1044 (42000): "), "{m}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_may_start_without_a_database() {
    let Some(url) = my_url("a_session_may_start_without_a_database") else { return };
    let mut c =
        Conn::start(&in_database(&url, ""), SessionRole::Query, false, "a_session_may_start_without_a_database");
    assert!(matches!(c.next(15).await, DbEvent::Encryption(_)), "how its traffic goes, first");
    assert!(matches!(c.next(15).await, DbEvent::Connected));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_test_connection_says_the_server_version() {
    let Some(url) = my_url("a_test_connection_says_the_server_version") else { return };
    let info = MyDriver.ping(&profile(&url), Duration::from_secs(10), dialer()).await.expect("the server answers");
    let mut probe = side(&url).await;
    let version: String = probe.query_first("SELECT VERSION()").await.unwrap().unwrap();
    probe.disconnect().await.unwrap();
    assert_eq!(info.server_version, version);
    let bad = MyDriver.ping(&profile(&as_user(&url, "datarig", "nope")), Duration::from_secs(10), dialer()).await;
    assert!(matches!(bad, Err(PingError::Failed(DbError::AccessDenied(m))) if m.starts_with("ERROR 1045")));
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_and_test_connections_go_through_a_dialer() {
    let Some(url) = my_url("sessions_and_test_connections_go_through_a_dialer") else { return };
    let d = Arc::new(TcpDialer::default());
    let via = Some(DialerRef(d.clone() as Arc<dyn Dialer>));
    let (tx, mut rx) = unbounded_channel();
    let _s = MyDriver.connect(
        &profile(&url),
        SessionRole::Query,
        opts(SessionRole::Query, "dialer").dialer(via.clone()),
        tx,
    );
    let first = tokio::time::timeout(Duration::from_secs(15), rx.recv()).await.unwrap();
    assert!(matches!(first, Some(DbEvent::Encryption(_))), "{first:?}");
    let next = tokio::time::timeout(Duration::from_secs(15), rx.recv()).await.unwrap();
    assert!(matches!(next, Some(DbEvent::Connected)), "{next:?}");
    MyDriver.ping(&profile(&url), Duration::from_secs(10), via).await.expect("the server answers");
    assert_eq!(d.dials.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_the_server_ends_is_lost_at_once() {
    let Some((url, admin)) = urls("a_connection_the_server_ends_is_lost_at_once") else { return };
    let mut admin = side(&admin).await;
    let mut c = Conn::open(&url, SessionRole::Meta, false, "a_connection_the_server_ends_is_lost_at_once").await;
    let id = only_session(&mut admin, SessionRole::Meta, "a_connection_the_server_ends_is_lost_at_once").await;
    admin.query_drop(format!("KILL {id}")).await.unwrap();
    // Idle, nothing asked: the session sees the end itself (after what it read at connect).
    c.wait(|e| matches!(e, DbEvent::Lost { .. }), 10).await;
    admin.disconnect().await.unwrap();
}

/// An account `zz_it_<what>_<pid>` with password `pw`, made with `identified` (`WITH
/// caching_sha2_password BY 'pw'`), dropped with the guard; `None` when the server refuses to
/// make it (a plugin it does not load).
struct Account {
    admin: String,
    name: String,
}

impl Account {
    async fn make(admin: &str, what: &str, identified: &str) -> Result<Account, mysql_async::Error> {
        let name = format!("zz_it_{what}_{}", std::process::id());
        let mut c = side(admin).await;
        c.query_drop(format!("DROP USER IF EXISTS '{name}'@'%'")).await?;
        c.query_drop(format!("CREATE USER '{name}'@'%' IDENTIFIED {identified}")).await?;
        // Dropped with the guard from here on, also when the grant fails.
        let account = Account { admin: admin.to_string(), name };
        c.query_drop(format!("GRANT SELECT ON shop.* TO '{}'@'%'", account.name)).await?;
        c.disconnect().await?;
        Ok(account)
    }
}

impl Drop for Account {
    fn drop(&mut self) {
        let (admin, name) = (self.admin.clone(), self.name.clone());
        // Dropped on a thread of its own: the test's runtime may be shutting down.
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let mut c = side(&admin).await;
                let _ = c.query_drop(format!("DROP USER IF EXISTS '{name}'@'%'")).await;
                let _ = c.disconnect().await;
            });
        })
        .join();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn caching_sha2_logs_in_on_a_plain_connection_first_and_cached() {
    let Some((url, admin)) = urls("caching_sha2_logs_in_on_a_plain_connection_first_and_cached") else { return };
    let account = Account::make(&admin, "sha2", "WITH caching_sha2_password BY 'pw'").await.unwrap();
    // A new account is not in the server's cache: the first login sends the password, which
    // over a plain connection (TLS off, as by default here) goes encrypted with the server's
    // public key, and over TLS inside TLS (this machine); the second is answered from the cache.
    for _ in 0..2 {
        let c = Conn::open(
            &as_user(&url, &account.name, "pw"),
            SessionRole::Meta,
            false,
            "caching_sha2_logs_in_on_a_plain_connection_first_and_cached",
        )
        .await;
        drop(c);
    }
    let mut c = Conn::start(
        &as_user(&url, &account.name, "wrong"),
        SessionRole::Meta,
        false,
        "caching_sha2_logs_in_on_a_plain_connection_first_and_cached",
    );
    assert!(matches!(c.next(15).await, DbEvent::ConnectFailed { auth: true, .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_native_password_logs_in_where_the_server_has_it() {
    let Some((url, admin)) = urls("mysql_native_password_logs_in_where_the_server_has_it") else { return };
    let account = match Account::make(&admin, "native", "WITH mysql_native_password BY 'pw'").await {
        Ok(a) => a,
        // MySQL 8.4 loads it only when asked (`--mysql-native-password=ON`); 9.x has none.
        Err(mysql_async::Error::Server(e)) if e.code == 1524 => {
            let _ = writeln!(std::io::stderr(), "SKIPPED mysql_native_password: the server does not load it");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    let c = Conn::open(
        &as_user(&url, &account.name, "pw"),
        SessionRole::Meta,
        false,
        "mysql_native_password_logs_in_where_the_server_has_it",
    )
    .await;
    drop(c);
}

/// A file with `text` in a directory of this test's own; returns its path.
fn temp_file(name: &str, text: &str) -> String {
    let dir = std::env::temp_dir().join(format!("datarig-it-mysql-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path.to_string_lossy().into_owned()
}

/// A 2048-bit RSA public key that is not the server's.
const OTHER_KEY: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAs7uesYv+jCjfQLX/mnir
NMpG8TBaLvuQ1Fgv3j/4ipvicNTp+Ft9h7uAdntx//JbZOFe1iu/0IhP/r77pbYQ
LwJSvozvsJ3AJW/J7ZMTLJ5c4RfciGUUz2ec7W6FKVNDqhASZmXSNhgAiEx7nEFM
MEhuntdG/HpqvDebbleNmJlGJwpfDiGmeteYQTAv/3I965LS4njdMkV2asMPN5JV
+v6visukUX0tlXxm/kaKMQBgyt369mAVCfQG9nRCgsQu1pmS4jZS573aCCFkv5Ql
V9BJoVLx22kBsyDRsu9XjrotHMQNZ16kiw4Pz8IERxgau+7hzMOY40CLaaq0Gac0
twIDAQAB
-----END PUBLIC KEY-----
";

/// The first login of a new account (not in the server's cache) with the server's public key
/// in the profile: the password goes encrypted with that key. With another key the server
/// cannot read it and refuses the login.
#[tokio::test(flavor = "multi_thread")]
async fn a_pinned_server_key_encrypts_the_first_login() {
    let test = "a_pinned_server_key_encrypts_the_first_login";
    let Some((url, admin)) = urls(test) else { return };
    let mut probe = side(&admin).await;
    let key: Option<(String, String)> =
        probe.query_first("SHOW STATUS LIKE 'Caching_sha2_password_rsa_public_key'").await.unwrap();
    probe.disconnect().await.unwrap();
    let server_key = temp_file("server.pem", &key.expect("the server's public key").1);
    let other_key = temp_file("other.pem", OTHER_KEY);
    let start = |user: &str, key: &str| {
        // A pinned key is for an unencrypted login (over TLS the password goes inside TLS).
        let cfg = ConnectionConfig {
            server_public_key_file: Some(key.to_string()),
            sslmode: "DISABLED".into(),
            ..profile(&as_user(&url, user, "pw"))
        };
        let (tx, rx) = unbounded_channel();
        let session = MyDriver.connect(&cfg, SessionRole::Meta, opts(SessionRole::Meta, test), tx);
        Conn { session, rx }
    };
    let pinned = Account::make(&admin, "pin", "WITH caching_sha2_password BY 'pw'").await.unwrap();
    start(&pinned.name, &server_key).wait(|e| matches!(e, DbEvent::Connected), 15).await;
    let other = Account::make(&admin, "pinx", "WITH caching_sha2_password BY 'pw'").await.unwrap();
    match start(&other.name, &other_key).next(15).await {
        DbEvent::ConnectFailed { error: DbError::AccessDenied(m), auth: true } => assert!(m.contains("1045"), "{m}"),
        other => panic!("{other:?}"),
    }
}

// ── statements ─────────────────────────────────────────────────────────────

/// A table `shop.zz_it_<name>_<pid>` made on a side connection (`ddl`, with `{t}` for its
/// name), dropped with the guard, also when the test fails.
struct Table {
    url: String,
    name: String,
}

impl Table {
    async fn make(url: &str, name: &str, ddl: &[&str]) -> Table {
        let name = format!("zz_it_{name}_{}", std::process::id());
        let mut c = side(url).await;
        c.query_drop(format!("DROP TABLE IF EXISTS shop.{name}")).await.unwrap();
        let t = Table { url: url.to_string(), name };
        for sql in ddl {
            c.query_drop(sql.replace("{t}", &format!("shop.{}", t.name))).await.unwrap();
        }
        c.disconnect().await.unwrap();
        t
    }

    /// `shop.<name>`.
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

/// A statement run from the side when the guard goes, whatever the test did (a panic too): the
/// drop of what the test made.
struct Cleanup {
    url: String,
    sql: String,
}

impl Drop for Cleanup {
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

/// Ends test `test`'s own sessions (`KILL` of the connections whose program name carries its
/// tag) when it ends, also when an assertion panics: a statement it left running goes with them
/// (and the cleanup after it does not wait behind its locks).
struct EndSessions {
    admin: String,
    test: &'static str,
}

impl Drop for EndSessions {
    fn drop(&mut self) {
        // The test's own tag, or one of its parts' (`<test>_<part>`).
        let tag = tag(self.test).replace('_', "\\_");
        let (admin, pattern) = (self.admin.clone(), format!("datarig-%-{tag}"));
        let parts = format!("{pattern}\\_%");
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let mut c = side(&admin).await;
                let ids: Vec<u64> = c
                    .exec(
                        "SELECT PROCESSLIST_ID FROM performance_schema.session_connect_attrs \
                         WHERE ATTR_NAME = 'program_name' AND (ATTR_VALUE LIKE ? OR ATTR_VALUE LIKE ?)",
                        (pattern, parts),
                    )
                    .await
                    .unwrap_or_default();
                for id in ids {
                    let _ = c.query_drop(format!("KILL {id}")).await;
                }
                let _ = c.disconnect().await;
            });
        })
        .join();
    }
}

/// A table of `n` rows (`id` 1..=n, `v` text), `n` under 1,000,000 (made from a cross join, so
/// no recursion goes past the server's `cte_max_recursion_depth`).
async fn numbers(url: &str, name: &str, n: u32) -> Table {
    let fill = format!(
        "INSERT INTO {{t}} (id, v) WITH RECURSIVE d (i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM d WHERE i < 99) \
         SELECT g.n, CONCAT('row ', g.n) FROM (SELECT a.i + b.i * 100 + c.i * 10000 + 1 AS n FROM d a, d b, d c) g \
         WHERE g.n <= {n}"
    );
    Table::make(url, name, &["CREATE TABLE {t} (id INT PRIMARY KEY, v VARCHAR(40))", &fill]).await
}

fn page_of(evs: &[DbEvent]) -> (&Vec<datarig_core::driver::ColumnMeta>, &Vec<Vec<Cell>>, bool) {
    match evs.last() {
        Some(DbEvent::Page { columns: Some(c), rows, more, .. }) => (c, rows, *more),
        other => panic!("not a page: {other:?}"),
    }
}

fn released(evs: &[DbEvent]) -> bool {
    evs.iter().any(|e| matches!(e, DbEvent::Released { .. }))
}

/// The run's `Info`, if any.
fn info(evs: &[DbEvent]) -> Option<StatementInfo> {
    evs.iter().find_map(|e| match e {
        DbEvent::Info { info, .. } => Some(info.clone()),
        _ => None,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_small_result_is_one_page_and_nothing_stays_open() {
    let Some(url) = my_url("a_small_result_is_one_page_and_nothing_stays_open") else { return };
    let mut c = Conn::open(&url, SessionRole::Query, false, "small").await;
    let evs = c.run(1, &["SELECT 1 AS a, 'x' AS b, NULL AS c"]).await;
    let (cols, rows, more) = page_of(&evs);
    assert_eq!(cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
    assert_eq!((cols[0].kind, cols[1].kind), (ValueKind::Integer, ValueKind::Text));
    assert_eq!(rows, &vec![vec![Some("1".to_string()), Some("x".to_string()), None]]);
    assert!(!more && !released(&evs));
    assert!(!evs.iter().any(|e| matches!(e, DbEvent::TxOpen(true))));
}

/// The metadata locks connection `id` holds on table `shop.<name>`.
async fn table_locks(a: &mut mysql_async::Conn, id: u64, name: &str) -> u64 {
    let sql = format!(
        "SELECT COUNT(*) FROM performance_schema.metadata_locks m JOIN performance_schema.threads th \
         ON th.THREAD_ID = m.OWNER_THREAD_ID WHERE th.PROCESSLIST_ID = {id} AND m.OBJECT_TYPE = 'TABLE' \
         AND m.OBJECT_NAME = '{name}'"
    );
    a.query_first::<u64, _>(sql).await.unwrap().unwrap()
}

/// Every metadata lock connection `id` holds or asked for, of any object.
async fn metadata_locks_of(a: &mut mysql_async::Conn, id: u64) -> u64 {
    let sql = format!(
        "SELECT COUNT(*) FROM performance_schema.metadata_locks m JOIN performance_schema.threads th \
         ON th.THREAD_ID = m.OWNER_THREAD_ID WHERE th.PROCESSLIST_ID = {id}"
    );
    a.query_first::<u64, _>(sql).await.unwrap().unwrap()
}

/// What connection `other` waits for that connection `ours` holds: a metadata lock pending on
/// an object `ours` has a granted lock on, or an InnoDB row lock `ours` blocks it with; each as a
/// line that says what.
async fn waits_on(a: &mut mysql_async::Conn, other: u64, ours: u64) -> Vec<String> {
    let sql = format!(
        "SELECT CONCAT('metadata ', w.OBJECT_TYPE, ' ', IFNULL(w.OBJECT_SCHEMA, ''), '.', IFNULL(w.OBJECT_NAME, ''), \
           ': ours ', m.LOCK_TYPE, ' ', m.LOCK_STATUS) \
         FROM performance_schema.metadata_locks w \
         JOIN performance_schema.threads tw ON tw.THREAD_ID = w.OWNER_THREAD_ID \
         JOIN performance_schema.metadata_locks m ON m.OBJECT_TYPE = w.OBJECT_TYPE \
           AND m.OBJECT_SCHEMA <=> w.OBJECT_SCHEMA AND m.OBJECT_NAME <=> w.OBJECT_NAME \
         JOIN performance_schema.threads tm ON tm.THREAD_ID = m.OWNER_THREAD_ID \
         WHERE tw.PROCESSLIST_ID = {other} AND w.LOCK_STATUS = 'PENDING' AND tm.PROCESSLIST_ID = {ours} \
           AND m.LOCK_STATUS = 'GRANTED' \
         UNION ALL SELECT CONCAT('row lock of ', w.OBJECT_SCHEMA, '.', w.OBJECT_NAME) \
         FROM performance_schema.data_lock_waits l \
         JOIN performance_schema.data_locks w ON w.ENGINE_LOCK_ID = l.REQUESTING_ENGINE_LOCK_ID \
         JOIN performance_schema.threads tw ON tw.THREAD_ID = l.REQUESTING_THREAD_ID \
         JOIN performance_schema.threads tb ON tb.THREAD_ID = l.BLOCKING_THREAD_ID \
         WHERE tw.PROCESSLIST_ID = {other} AND tb.PROCESSLIST_ID = {ours}"
    );
    a.query(sql).await.unwrap()
}

/// The metadata locks connection `id` waits for (pending) on table `shop.<table>`, each as a
/// line that says what. Only the test's own table: DDL of other tests (or of anyone on the
/// server) elsewhere, and server-wide locks (the privilege cache), are not the lookup's doing.
async fn queued(a: &mut mysql_async::Conn, id: u64, table: &str) -> Vec<String> {
    let sql = format!(
        "SELECT CONCAT(m.OBJECT_TYPE, ' ', IFNULL(m.OBJECT_SCHEMA, ''), '.', IFNULL(m.OBJECT_NAME, ''), ' ', m.LOCK_TYPE) \
         FROM performance_schema.metadata_locks m JOIN performance_schema.threads th \
         ON th.THREAD_ID = m.OWNER_THREAD_ID WHERE th.PROCESSLIST_ID = {id} AND m.LOCK_STATUS = 'PENDING' \
         AND m.OBJECT_TYPE = 'TABLE' AND m.OBJECT_SCHEMA = 'shop' AND m.OBJECT_NAME = '{table}'"
    );
    a.query(sql).await.unwrap()
}

/// What the side saw while lookups of the metadata session ran ([`watched`]).
struct Seen {
    /// Another session waiting for a lock the metadata session holds ([`waits_on`]).
    held_up: Vec<String>,
    /// The metadata session waiting for a table's lock ([`queued`]).
    queued: Vec<String>,
}

/// `run` (lookups of metadata session `ours` in flight, and whatever else it does), watched from
/// the side through `admin` while it runs and once right after: whether any of connections
/// `others` waits for a lock `ours` holds, and whether `ours` waits for a lock of table
/// `shop.<table>`. Returns what `run` gives and what was seen. It is what the server shows, never
/// how long anything took, and it is sampled: a guard (a lock held between two looks would not
/// be seen), with `queued` the stronger fact (a wait lasts the session's lock wait, 2 s).
async fn watched<T>(
    admin: &str,
    ours: u64,
    others: &[u64],
    table: &str,
    run: impl std::future::Future<Output = T>,
) -> (T, Seen) {
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (url, others_owned, stop2, table) = (admin.to_string(), others.to_vec(), stop.clone(), table.to_string());
    let watch = tokio::spawn(async move {
        let mut a = side(&url).await;
        let mut seen = Seen { held_up: Vec::new(), queued: Vec::new() };
        loop {
            // One more look after `run` ended.
            let last = stop2.load(std::sync::atomic::Ordering::SeqCst);
            for other in &others_owned {
                for w in waits_on(&mut a, *other, ours).await {
                    let w = format!("{other}: {w}");
                    if !seen.held_up.contains(&w) {
                        seen.held_up.push(w);
                    }
                }
            }
            for q in queued(&mut a, ours, &table).await {
                if !seen.queued.contains(&q) {
                    seen.queued.push(q);
                }
            }
            if last {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        a.disconnect().await.unwrap();
        seen
    });
    let out = run.await;
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    (out, watch.await.unwrap())
}

/// An `ALTER TABLE` of `table` (adding column `c9`) from a connection of its own, which waits for
/// the metadata lock another session holds: its connection id once the server shows it waiting
/// (`Waiting for table metadata lock`), and the task that ends with its outcome.
async fn waiting_alter(admin: &str, table: &str) -> (u64, tokio::task::JoinHandle<Result<(), String>>) {
    let (url, table) = (admin.to_string(), table.to_string());
    let (id_tx, id_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut a = side(&url).await;
        a.query_drop("SET SESSION lock_wait_timeout = 60").await.unwrap();
        let _ = id_tx.send(u64::from(a.id()));
        let r = a.query_drop(format!("ALTER TABLE {table} ADD COLUMN c9 INT")).await;
        a.disconnect().await.unwrap();
        r.map_err(|e| e.to_string())
    });
    let id = id_rx.await.unwrap();
    let mut a = side(admin).await;
    // A hang guard, not a measure: the ALTER is seen waiting well within it.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !alter_waits(&mut a, id).await {
        assert!(Instant::now() < deadline, "the ALTER never waited");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    a.disconnect().await.unwrap();
    (id, task)
}

/// Whether connection `id` waits for a table's metadata lock, as the server shows it.
async fn alter_waits(a: &mut mysql_async::Conn, id: u64) -> bool {
    let state: Option<String> =
        a.query_first(format!("SELECT STATE FROM information_schema.PROCESSLIST WHERE ID = {id}")).await.unwrap();
    state.as_deref() == Some("Waiting for table metadata lock")
}

/// A result on screen holds no metadata lock, so an `ALTER TABLE` of another
/// session goes through; a held result does hold one while its statement runs (one larger than
/// what the network buffers take), until it is read to its end or stopped.
#[tokio::test(flavor = "multi_thread")]
async fn a_first_page_holds_no_metadata_lock_and_a_held_result_does_until_read() {
    let Some((url, admin)) = urls("a_first_page_holds_no_metadata_lock") else { return };
    let test = "mdl";
    // About 60 MB of rows: far more than the server's and the client's socket buffers take.
    let t = numbers(&admin, "mdl", 300_000).await;
    let mut wide = side(&admin).await;
    wide.query_drop(format!("ALTER TABLE {} MODIFY v VARCHAR(200)", t.q())).await.unwrap();
    wide.query_drop(format!("UPDATE {} SET v = REPEAT('x', 200)", t.q())).await.unwrap();
    wide.disconnect().await.unwrap();
    let mut a = side(&admin).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, test).await;
    let id = only_session(&mut a, SessionRole::Query, test).await;
    let sql = format!("SELECT * FROM {} ORDER BY id", t.q());
    // Not held (the default): the first page, then nothing open on the server.
    let evs = c.run(1, &[&sql]).await;
    let (_, rows, more) = page_of(&evs);
    assert_eq!((rows.len(), more, released(&evs)), (PAGE, true, true));
    assert_eq!(table_locks(&mut a, id, &t.name).await, 0, "no metadata lock while the page is on screen");
    let trx: u64 = a
        .query_first(format!("SELECT COUNT(*) FROM information_schema.innodb_trx WHERE trx_mysql_thread_id = {id}"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(trx, 0, "no transaction");
    let mut alter = side(&admin).await;
    alter.query_drop("SET SESSION lock_wait_timeout = 5").await.unwrap();
    let t0 = Instant::now();
    alter.query_drop(format!("ALTER TABLE {} ADD COLUMN c1 INT", t.q())).await.expect("the ALTER goes through");
    assert!(t0.elapsed() < Duration::from_secs(5));
    // Held: the statement runs on (and keeps its lock) while the result is not read to its end.
    let evs = c.run_as(2, &[&sql], PagingMode::Hold).await;
    let (_, rows, more) = page_of(&evs);
    assert_eq!((rows.len(), more, released(&evs)), (PAGE, true, false));
    assert!(table_locks(&mut a, id, &t.name).await > 0, "the held statement keeps its metadata lock");
    alter.query_drop("SET SESSION lock_wait_timeout = 1").await.unwrap();
    match alter.query_drop(format!("ALTER TABLE {} ADD COLUMN c2 INT", t.q())).await {
        Err(mysql_async::Error::Server(e)) => assert_eq!(e.code, 1205, "{e}"),
        other => panic!("the ALTER waited for the held statement: {other:?}"),
    }
    // Stopped (the app closes it at its spill limit): stopped on the server, the lock goes.
    c.session.send(DbCommand::ClosePortal { id: 2 });
    c.ok(3, "SELECT 1").await;
    assert_eq!(table_locks(&mut a, id, &t.name).await, 0, "stopped: the lock is gone");
    alter.query_drop(format!("ALTER TABLE {} ADD COLUMN c2 INT", t.q())).await.expect("the ALTER goes through now");
    alter.disconnect().await.unwrap();
    a.disconnect().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_next_page_runs_the_statement_again_past_the_rows_it_has() {
    let Some((url, admin)) = urls("the_next_page_runs_the_statement_again") else { return };
    let t = numbers(&admin, "resume", 1200).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, "resume").await;
    let sql = format!("SELECT id FROM {} ORDER BY id", t.q());
    let evs = c.run(1, &[&sql]).await;
    assert_eq!(page_of(&evs).1.len(), PAGE);
    c.session.send(DbCommand::Resume { id: 2, sql: sql.clone(), skip: PAGE as u64, paging: PagingMode::NoHold });
    let evs = c.answer(2).await;

    let (_, rows, more) = page_of(&evs);
    assert_eq!((rows.first().cloned(), rows.len(), more), (Some(vec![Some("501".into())]), PAGE, true));
    assert!(released(&evs));
    c.session.send(DbCommand::Resume { id: 3, sql: sql.clone(), skip: 1000, paging: PagingMode::NoHold });
    let evs = c.answer(3).await;
    let (_, rows, more) = page_of(&evs);
    assert_eq!((rows.len(), more, released(&evs)), (200, false, false));
    // Past its end: an empty last page.
    c.session.send(DbCommand::Resume { id: 4, sql, skip: 5000, paging: PagingMode::NoHold });
    let evs = c.answer(4).await;
    let (_, rows, more) = page_of(&evs);
    assert_eq!((rows.len(), more), (0, false));
    // A view, or a text off the allowlist, is not run again.
    c.session.send(DbCommand::Resume {
        id: 5,
        sql: "SELECT * FROM shop.order_summary".into(),
        skip: 1,
        paging: PagingMode::NoHold,
    });
    match c.answer(5).await.pop() {
        Some(DbEvent::Failed { error: DbError::NotRepeatable(r), .. }) => {
            assert_eq!(r, datarig_core::sql::risk::repeat::NotRepeatable::NotATable("shop.order_summary".into()))
        }
        other => panic!("{other:?}"),
    }
    c.session.send(DbCommand::Resume { id: 6, sql: "SELECT RAND()".into(), skip: 1, paging: PagingMode::NoHold });
    assert!(matches!(c.answer(6).await.pop(), Some(DbEvent::Failed { error: DbError::NotRepeatable(_), .. })));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_limit_the_user_sets_is_kept_and_the_session_pages_again_after_it() {
    let Some((url, admin)) = urls("a_limit_the_user_sets_is_kept") else { return };
    let t = numbers(&admin, "limit", 1200).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, "limit").await;
    let sql = format!("SELECT id FROM {} ORDER BY id", t.q());
    c.ok(1, "SET sql_select_limit = 3").await;
    let evs = c.run(2, &[&sql]).await;
    let (_, rows, more) = page_of(&evs);
    assert_eq!((rows.len(), more), (3, false), "the user's limit wins");
    // Its warnings stay readable: no setting of the session's runs before a SHOW.
    let evs = c.run(3, &["SELECT 1 / 0"]).await;
    assert_eq!(info(&evs).map(|i| i.warnings), Some(1));
    let evs = c.run(4, &["SHOW WARNINGS"]).await;
    assert_eq!(page_of(&evs).1.len(), 1, "the division's warning");
    c.ok(5, "SET sql_select_limit = DEFAULT").await;
    let evs = c.run(6, &[&sql]).await;
    let (_, rows, more) = page_of(&evs);
    assert_eq!((rows.len(), more), (PAGE, true));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_result_the_limit_does_not_stop_is_read_and_only_its_page_kept() {
    let Some((url, admin)) = urls("a_result_the_limit_does_not_stop") else { return };
    let t = numbers(&admin, "explicit", 1200).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, "explicit").await;
    let evs = c.run(1, &[&format!("SELECT id FROM {} ORDER BY id LIMIT 1100", t.q())]).await;
    let (_, rows, more) = page_of(&evs);
    assert_eq!((rows.len(), more, released(&evs)), (PAGE, true, true));
    // The session is free at once (the rest was read).
    let evs = c.run(2, &["SELECT 2"]).await;
    assert_eq!(page_of(&evs).1, &vec![vec![Some("2".to_string())]]);
}

#[tokio::test(flavor = "multi_thread")]
async fn statements_of_a_run_go_one_by_one_and_say_what_they_did() {
    let Some((url, admin)) = urls("statements_of_a_run_go_one_by_one") else { return };
    let t = Table::make(&admin, "run", &["CREATE TABLE {t} (id INT AUTO_INCREMENT PRIMARY KEY, v VARCHAR(10))"]).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, "run").await;
    let ins = format!("INSERT INTO {} (v) VALUES ('a'), ('b')", t.q());
    let upd = format!("UPDATE {} SET v = 'c' WHERE id = 1", t.q());
    let sel = format!("SELECT v FROM {} ORDER BY id", t.q());
    let evs = c.run(1, &[&ins, &upd, &sel]).await;
    let finished: Vec<(usize, String)> = evs
        .iter()
        .filter_map(|e| match e {
            DbEvent::Finished { index, outcome, .. } => Some((*index, format!("{outcome:?}"))),
            _ => None,
        })
        .collect();
    assert_eq!(finished, [(0, "Affected(2)".to_string()), (1, "Affected(1)".to_string())]);
    let infos: Vec<(usize, StatementInfo)> = evs
        .iter()
        .filter_map(|e| match e {
            DbEvent::Info { index, info, .. } => Some((*index, info.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(infos[0].0, 0);
    assert_eq!(infos[0].1.insert_id, Some(1), "the first id an INSERT of several rows generated");
    assert!(infos[0].1.message.as_deref().is_some_and(|m| m.starts_with("Records: 2")), "{infos:?}");
    assert_eq!(infos[1].1.message.as_deref(), Some("Rows matched: 1  Changed: 1  Warnings: 0"));
    assert_eq!(page_of(&evs).1, &vec![vec![Some("c".to_string())], vec![Some("b".to_string())]]);
    // A failure stops the run there.
    let evs = c.run(2, &["SELECT * FROM shop.zz_it_no_such_table", "SELECT 1"]).await;
    match evs.last() {
        Some(DbEvent::Failed { error: DbError::Server(m), cancelled: false, .. }) => {
            assert!(m.starts_with("ERROR 1146 (42S02): "), "{m}")
        }
        other => panic!("{other:?}"),
    }
    assert!(!evs.iter().any(|e| matches!(e, DbEvent::Started { index: 1, .. })));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_users_transaction_follows_the_servers_flags() {
    let Some((url, admin)) = urls("the_users_transaction_follows_the_servers_flags") else { return };
    let t = Table::make(&admin, "tx", &["CREATE TABLE {t} (id INT PRIMARY KEY)"]).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, "tx").await;
    let tx_events = |evs: &[DbEvent]| -> Vec<String> {
        evs.iter()
            .filter_map(|e| match e {
                DbEvent::Block(b) => Some(format!("block {b}")),
                DbEvent::TxOpen(b) => Some(format!("tx {b}")),
                _ => None,
            })
            .collect()
    };
    assert_eq!(tx_events(&c.run(1, &["BEGIN"]).await), ["block true", "tx true"]);
    c.ok(2, &format!("INSERT INTO {} VALUES (1)", t.q())).await;
    // A failure does not end it (MySQL keeps the transaction).
    let evs = c.run(3, &[&format!("INSERT INTO {} VALUES (1)", t.q())]).await;
    assert!(matches!(evs.last(), Some(DbEvent::Failed { .. })));
    assert!(tx_events(&evs).is_empty(), "still open: {evs:?}");
    // Another BEGIN commits it and opens a new one: both said.
    assert_eq!(
        tx_events(&c.run(4, &["START TRANSACTION"]).await),
        ["block false", "tx false", "block true", "tx true"]
    );
    // DDL commits it implicitly.
    let evs = c.run(5, &[&format!("CREATE TABLE {}_x (a INT)", t.q())]).await;
    assert_eq!(tx_events(&evs), ["block false", "tx false"]);
    c.ok(6, &format!("DROP TABLE {}_x", t.q())).await;
    // The row of the first transaction was committed by the second BEGIN.
    let evs = c.run(7, &[&format!("SELECT id FROM {}", t.q())]).await;
    assert_eq!(page_of(&evs).1.len(), 1);
    assert_eq!(tx_events(&c.run(8, &["BEGIN"]).await), ["block true", "tx true"]);
    assert_eq!(tx_events(&c.run(9, &["COMMIT AND CHAIN"]).await), ["block false", "tx false", "block true", "tx true"]);
    assert_eq!(tx_events(&c.run(10, &["ROLLBACK"]).await), ["block false", "tx false"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn results_inside_the_users_transaction_are_held_and_read_on() {
    let Some((url, admin)) = urls("results_inside_the_users_transaction_are_held") else { return };
    let t = numbers(&admin, "inblock", 700).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, "inblock").await;
    c.ok(1, "BEGIN").await;
    let evs = c.run(2, &[&format!("SELECT id FROM {} ORDER BY id", t.q())]).await;
    let (_, rows, more) = page_of(&evs);
    assert_eq!((rows.len(), more, released(&evs)), (PAGE, true, false));
    c.session.send(DbCommand::FetchMore { id: 2 });
    match c.next(30).await {
        DbEvent::Page { id: 2, columns: None, rows, more: false, .. } => assert_eq!(rows.len(), 200),
        other => panic!("{other:?}"),
    }
    c.ok(3, "ROLLBACK").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_result_another_statement_ends_is_stopped_on_the_server() {
    let Some((url, admin)) = urls("a_held_result_another_statement_ends") else { return };
    let test = "heldstop";
    let mut a = side(&admin).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, test).await;
    let id = only_session(&mut a, SessionRole::Query, test).await;
    // A result far longer than a page, held, then another statement: the first one stops.
    let evs = c.run_as(1, &["SELECT * FROM shop.events"], PagingMode::Hold).await;
    assert!(page_of(&evs).2);
    let t0 = Instant::now();
    let evs = c.run(2, &["SELECT 7"]).await;
    assert_eq!(page_of(&evs).1, &vec![vec![Some("7".to_string())]]);
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
    let running: u64 = a
        .query_first(format!(
            "SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE ID = {id} AND COMMAND = 'Query'"
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(running, 0);
    // Closed by the app (its spill limit): the same.
    let evs = c.run_as(3, &["SELECT * FROM shop.events"], PagingMode::Hold).await;
    assert!(page_of(&evs).2);
    c.session.send(DbCommand::ClosePortal { id: 3 });
    let evs = c.run(4, &["SELECT 8"]).await;
    assert_eq!(page_of(&evs).1, &vec![vec![Some("8".to_string())]]);
    a.disconnect().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_stops_the_statement_on_the_server() {
    let Some(url) = my_url("a_cancel_stops_the_statement_on_the_server") else { return };
    let mut c = Conn::open(&url, SessionRole::Query, false, "cancel").await;
    let t0 = Instant::now();
    c.session.send(DbCommand::Execute {
        id: 1,
        statements: vec!["SELECT SLEEP(30)".into()],
        paging: PagingMode::NoHold,
        tx: TxRule::Auto,
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.session.cancel();
    match c.answer(1).await.pop() {
        Some(DbEvent::Failed { error: DbError::Cancelled, cancelled: true, .. }) => {}
        // `SLEEP` interrupted answers its row (1) instead of an error on some servers.
        Some(DbEvent::Page { rows, .. }) => assert_eq!(rows, vec![vec![Some("1".to_string())]]),
        other => panic!("{other:?}"),
    }
    assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    // The session goes on.
    c.ok(2, "SELECT 1").await;
    // Between two statements of a run: the rest does not run.
    c.session.send(DbCommand::Execute {
        id: 3,
        statements: vec!["SELECT SLEEP(30)".into(), "SELECT 2".into()],
        paging: PagingMode::NoHold,
        tx: TxRule::Auto,
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.session.cancel();
    let evs = c.answer(3).await;
    assert!(matches!(evs.last(), Some(DbEvent::Failed { cancelled: true, .. })), "{evs:?}");
    assert!(!evs.iter().any(|e| matches!(e, DbEvent::Started { index: 1, .. })));
    c.ok(4, "SELECT 1").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_session_stops_what_it_runs() {
    let Some((url, admin)) = urls("closing_a_session_stops_what_it_runs") else { return };
    let test = "closing";
    let mut a = side(&admin).await;
    let c = Conn::open(&url, SessionRole::Query, false, test).await;
    let id = only_session(&mut a, SessionRole::Query, test).await;
    c.session.send(DbCommand::Execute {
        id: 1,
        statements: vec!["SELECT SLEEP(30)".into()],
        paging: PagingMode::NoHold,
        tx: TxRule::Auto,
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(c);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let n: u64 = a
            .query_first(format!("SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE ID = {id}"))
            .await
            .unwrap()
            .unwrap();
        if n == 0 {
            break;
        }
        assert!(Instant::now() < deadline, "the connection stays");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    a.disconnect().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_lost_while_a_statement_runs_fails_it_then_is_lost() {
    let Some((url, admin)) = urls("a_connection_lost_while_a_statement_runs") else { return };
    let test = "lostrun";
    let mut a = side(&admin).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, test).await;
    let id = only_session(&mut a, SessionRole::Query, test).await;
    c.session.send(DbCommand::Execute {
        id: 1,
        statements: vec!["SELECT SLEEP(30)".into()],
        paging: PagingMode::NoHold,
        tx: TxRule::Auto,
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    a.query_drop(format!("KILL {id}")).await.unwrap();
    let evs = c.answer(1).await;
    assert!(matches!(evs.last(), Some(DbEvent::Failed { cancelled: false, .. })), "{evs:?}");
    assert!(matches!(c.next(10).await, DbEvent::Lost { .. }));
    a.disconnect().await.unwrap();
}

/// A procedure of the administrator's that runs as its definer (`SQL SECURITY DEFINER`) and
/// whose result has no end in practice (a table of a million rows joined with itself): the
/// session's user may run it, and may not `KILL QUERY` it (while it runs, the statement is the
/// definer's). Joined by a range on the key, so the server streams rows from the first one on
/// (a cross join builds a hash table first, saying nothing for as long as 25 s on a loaded
/// server). Dropped again at the end.
async fn endless_definer_procedure(admin: &str, name: &str) -> (String, Cleanup) {
    let p = format!("zz_it_{name}_{}", std::process::id());
    let cleanup = Cleanup { url: admin.to_string(), sql: format!("DROP PROCEDURE IF EXISTS shop.{p}") };
    let mut a = side(admin).await;
    a.query_drop(format!(
        "CREATE PROCEDURE shop.{p}() SQL SECURITY DEFINER SELECT a.id FROM shop.events a JOIN shop.events b ON b.id >= a.id"
    ))
    .await
    .unwrap();
    a.disconnect().await.unwrap();
    (p, cleanup)
}

/// Every event of `c` until its session is lost (at most `secs`); each page of a held result
/// asks for the next one, as the app reads a held result to its end.
async fn until_lost(c: &mut Conn, secs: u64) -> Vec<DbEvent> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut evs = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let ev = match tokio::time::timeout(left, c.rx.recv()).await {
            Ok(Some(ev)) => ev,
            Ok(None) => panic!("event channel closed: {evs:?}"),
            Err(_) => panic!("the session is not lost within {secs}s: {} events, last {:?}", evs.len(), evs.last()),
        };
        if let DbEvent::Page { id, more: true, .. } = ev {
            c.session.send(DbCommand::FetchMore { id });
        }
        let lost = matches!(ev, DbEvent::Lost { .. });
        evs.push(ev);
        if lost {
            return evs;
        }
    }
}

/// Wait (at most `secs`) for the server to end connection `id`. A connection the client closed
/// ends when its statement next sends or checks: within a second mostly, some seconds while a
/// join builds its hash table on a loaded server.
async fn thread_ends(a: &mut mysql_async::Conn, id: u64, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let n: u64 = a
            .query_first(format!("SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE ID = {id}"))
            .await
            .unwrap()
            .unwrap();
        if n == 0 {
            return;
        }
        assert!(Instant::now() < deadline, "connection {id} is still on the server after {secs}s");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The failures of run `id` among `evs`.
fn failures(evs: &[DbEvent], id: u64) -> Vec<&DbEvent> {
    evs.iter().filter(|e| matches!(e, DbEvent::Failed { id: i, .. } if *i == id)).collect()
}

/// A cancel the server refuses (the statement runs as another account) does not leave the
/// session reading a result without end: it cuts the connection at once, the run ends cancelled
/// (once), the session is lost and says why, the server ends the statement, and a new session
/// works.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_the_server_refuses_ends_the_session_instead_of_reading_on() {
    let test = "a_cancel_the_server_refuses_ends_the_session";
    let Some((url, admin)) = urls(test) else { return };
    let (p, _p) = endless_definer_procedure(&admin, "refused").await;
    // Dropped first: nothing of the test's keeps running past it.
    let _end = EndSessions { admin: admin.clone(), test };
    let mut a = side(&admin).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, test).await;
    let id = only_session(&mut a, SessionRole::Query, test).await;
    c.session.send(DbCommand::Execute {
        id: 1,
        statements: vec![format!("CALL shop.{p}()")],
        paging: PagingMode::NoHold,
        tx: TxRule::Auto,
    });
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let t0 = Instant::now();
    c.session.cancel();
    let evs = until_lost(&mut c, 20).await;
    // Bounded by the kill's own time limit and the ticks rows must keep coming in.
    assert!(t0.elapsed() < Duration::from_secs(15), "{:?}", t0.elapsed());
    let answers: Vec<_> = evs
        .iter()
        .filter(|e| matches!(e, DbEvent::Page { id: 1, columns: Some(_), .. } | DbEvent::Done { id: 1, .. }))
        .collect();
    assert!(answers.is_empty(), "{answers:?}");
    assert!(
        matches!(failures(&evs, 1)[..], [DbEvent::Failed { error: DbError::Cancelled, cancelled: true, .. }]),
        "{evs:?}"
    );
    assert!(
        matches!(evs.last(), Some(DbEvent::Lost { error: DbError::CancelNotTaken(CancelFailure::Refused) })),
        "{evs:?}"
    );
    thread_ends(&mut a, id, 30).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, test).await;
    c.ok(2, "SELECT 1").await;
    a.disconnect().await.unwrap();
}

/// A cancel the server takes whose statement then rolls back for a long time (a large `UPDATE`
/// undone row by row, saying nothing meanwhile) is waited for: the run ends cancelled once, the
/// session stays, and so does the user's transaction with what it did before the statement.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_taken_by_a_long_rollback_keeps_the_session_and_its_transaction() {
    let test = "a_cancel_taken_by_a_long_rollback";
    let Some((url, admin)) = urls(test) else { return };
    let t = format!("shop.zz_it_rollback_{}", std::process::id());
    let _t = Cleanup { url: admin.clone(), sql: format!("DROP TABLE IF EXISTS {t}") };
    // Dropped first: nothing of the test's keeps running past it.
    let _end = EndSessions { admin: admin.clone(), test };
    let mut a = side(&admin).await;
    a.query_drop(format!("CREATE TABLE {t} (id INT PRIMARY KEY AUTO_INCREMENT, v INT, pad CHAR(100), KEY (v))"))
        .await
        .unwrap();
    for _ in 0..3 {
        a.query_drop(format!("INSERT INTO {t} (v, pad) SELECT 0, 'x' FROM shop.events")).await.unwrap();
    }
    a.query_drop(format!("INSERT INTO {t} (v, pad) SELECT v, pad FROM {t}")).await.unwrap();
    let mut c = Conn::open(&url, SessionRole::Query, false, test).await;
    c.ok(1, "BEGIN").await;
    c.ok(2, &format!("INSERT INTO {t} (v, pad) VALUES (-1, 'marker')")).await;
    c.session.send(DbCommand::Execute {
        id: 3,
        statements: vec![format!("UPDATE {t} SET v = v + 1, pad = 'y'")],
        paging: PagingMode::NoHold,
        tx: TxRule::Auto,
    });
    // Long enough for its rollback to take longer than any bound a cancel could be given.
    tokio::time::sleep(Duration::from_secs(15)).await;
    c.session.cancel();
    let cancelled = Instant::now();
    let mut evs = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(240);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let ev = match tokio::time::timeout(left, c.rx.recv()).await {
            Ok(Some(ev)) => ev,
            other => panic!("no answer: {other:?} after {evs:?}"),
        };
        let end = matches!(ev, DbEvent::Failed { id: 3, .. } | DbEvent::Done { id: 3, .. } | DbEvent::Lost { .. });
        evs.push(ev);
        if end {
            break;
        }
    }
    assert!(!evs.iter().any(|e| matches!(e, DbEvent::Done { id: 3, .. })), "the update ended before the cancel");
    assert!(
        matches!(failures(&evs, 3)[..], [DbEvent::Failed { error: DbError::Cancelled, cancelled: true, .. }]),
        "{evs:?}"
    );
    // A long wait says, every few seconds, that the server still stops the statement.
    if cancelled.elapsed() > Duration::from_secs(8) {
        assert!(evs.iter().any(|e| matches!(e, DbEvent::Stopping { id: 3 })), "{evs:?}");
    }
    // The session and its transaction go on: the marker is still there, and nothing is lost.
    let more = c.run(4, &[&format!("SELECT v FROM {t} WHERE pad = 'marker'")]).await;
    assert!(!more.iter().any(|e| matches!(e, DbEvent::Lost { .. })), "{more:?}");
    assert_eq!(page_of(&more).1, &vec![vec![Some("-1".to_string())]], "{more:?}");
    c.ok(5, "ROLLBACK").await;
    a.disconnect().await.unwrap();
}

/// A statement that runs as another account only for a moment at a time (a `SQL SECURITY
/// DEFINER` function called for every row): a kill that lands inside the function is refused,
/// one between two calls is not. The refusal is tried again (at once, then while the session
/// waits: the statement sends nothing), and the run ends cancelled with the session kept.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_refused_for_a_moment_is_tried_again() {
    let test = "a_cancel_refused_for_a_moment";
    let Some((url, admin)) = urls(test) else { return };
    let f = format!("zz_it_rowfn_{}", std::process::id());
    let _f = Cleanup { url: admin.clone(), sql: format!("DROP FUNCTION IF EXISTS shop.{f}") };
    // Dropped first: nothing of the test's keeps running past it.
    let _end = EndSessions { admin: admin.clone(), test };
    let mut a = side(&admin).await;
    a.query_drop(format!(
        "CREATE FUNCTION shop.{f}(x INT) RETURNS INT DETERMINISTIC SQL SECURITY DEFINER RETURN x + 1"
    ))
    .await
    .unwrap();
    a.disconnect().await.unwrap();
    let mut c = Conn::open(&url, SessionRole::Query, false, test).await;
    c.session.send(DbCommand::Execute {
        id: 1,
        statements: vec![format!(
            "SELECT COUNT(*) FROM shop.events a CROSS JOIN shop.events b WHERE shop.{f}(a.id) > 0"
        )],
        paging: PagingMode::NoHold,
        tx: TxRule::Auto,
    });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    c.session.cancel();
    let evs = c.answer(1).await;
    assert!(matches!(evs.last(), Some(DbEvent::Failed { error: DbError::Cancelled, cancelled: true, .. })), "{evs:?}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let lost = c.rx.try_recv();
    assert!(!matches!(lost, Ok(DbEvent::Lost { .. })), "{lost:?}");
    let more = c.run(2, &["SELECT 1"]).await;
    assert!(!more.iter().any(|e| matches!(e, DbEvent::Lost { .. })), "{more:?}");
    assert_eq!(page_of(&more).1, &vec![vec![Some("1".to_string())]]);
}

/// A count and the server's question run only what can be run again, which is never code of
/// the server's (a function, a view): nothing in them runs as another account, so no kill of
/// theirs is refused. What a definer function in them gets is a refusal before anything runs;
/// a count stopped by a cancel says so, and the session goes on.
#[tokio::test(flavor = "multi_thread")]
async fn counts_and_questions_never_run_another_accounts_code() {
    let test = "counts_and_questions_never_run";
    let Some((url, admin)) = urls(test) else { return };
    let f = format!("zz_it_cntfn_{}", std::process::id());
    let _f = Cleanup { url: admin.clone(), sql: format!("DROP FUNCTION IF EXISTS shop.{f}") };
    // Dropped first: nothing of the test's keeps running past it.
    let _end = EndSessions { admin: admin.clone(), test };
    let mut a = side(&admin).await;
    a.query_drop(format!(
        "CREATE FUNCTION shop.{f}(x INT) RETURNS INT DETERMINISTIC SQL SECURITY DEFINER RETURN SLEEP(1) + x"
    ))
    .await
    .unwrap();
    a.disconnect().await.unwrap();
    let mut c = Conn::open(&url, SessionRole::Query, false, test).await;
    let with_fn = format!("SELECT COUNT(*) FROM (\nSELECT shop.{f}(id) FROM shop.events\n) AS datarig_count");
    let t0 = Instant::now();
    assert!(matches!(count(&mut c, 1, with_fn).await.0, Err(DbError::NotRepeatable(_))));
    assert!(t0.elapsed() < Duration::from_secs(5), "nothing ran: {:?}", t0.elapsed());
    c.session.send(DbCommand::CheckRepeat { id: 2, sql: format!("SELECT shop.{f}(id) FROM shop.events") });
    assert!(matches!(
        c.wait(|e| matches!(e, DbEvent::RepeatChecked { .. }), 30).await,
        DbEvent::RepeatChecked { result: Err(DbError::NotRepeatable(_)), .. }
    ));
    // A long count of tables (every pair is looked at: no shortcut from the tables' sizes),
    // cancelled.
    let long =
        "SELECT COUNT(*) FROM (\nSELECT a.id FROM shop.events a JOIN shop.events b ON a.id < b.id\n) AS datarig_count";
    c.session.send(DbCommand::Count { id: 3, sql: long.to_string() });
    tokio::time::sleep(Duration::from_millis(1000)).await;
    c.session.cancel();
    match c.wait(|e| matches!(e, DbEvent::Counted { .. } | DbEvent::Lost { .. }), 30).await {
        DbEvent::Counted { id: 3, result: Err(DbError::Cancelled), .. } => {}
        other => panic!("{other:?}"),
    }
    c.ok(4, "SELECT 1").await;
}

/// A cancel the server refuses of a statement that sends nothing (a `SQL SECURITY DEFINER`
/// procedure that counts): cutting the connection would not stop it (the server does not look
/// at the connection meanwhile), so the session keeps waiting, says why every few seconds, and
/// stays; the statement is still on the server. Once an administrator kills it, the run ends
/// and the session goes on.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_cancel_of_a_silent_statement_keeps_the_session_and_says_why() {
    let test = "a_refused_cancel_of_a_silent_statement";
    let Some((url, admin)) = urls(test) else { return };
    let p = format!("zz_it_silent_{}", std::process::id());
    let _p = Cleanup { url: admin.clone(), sql: format!("DROP PROCEDURE IF EXISTS shop.{p}") };
    let mut a = side(&admin).await;
    a.query_drop(format!(
        "CREATE PROCEDURE shop.{p}() SQL SECURITY DEFINER \
         SELECT COUNT(*) FROM shop.events a JOIN shop.events b ON a.id < b.id"
    ))
    .await
    .unwrap();
    // Dropped first: the statement the test leaves running goes, and the drop does not wait
    // behind it.
    let _end = EndSessions { admin: admin.clone(), test };
    let mut c = Conn::open(&url, SessionRole::Query, false, test).await;
    let id = only_session(&mut a, SessionRole::Query, test).await;
    c.session.send(DbCommand::Execute {
        id: 1,
        statements: vec![format!("CALL shop.{p}()")],
        paging: PagingMode::NoHold,
        tx: TxRule::Auto,
    });
    tokio::time::sleep(Duration::from_millis(1000)).await;
    c.session.cancel();
    // Two of its words, and nothing else: no answer, no loss.
    let mut said = 0;
    let deadline = Instant::now() + Duration::from_secs(20);
    while said < 2 {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, c.rx.recv()).await {
            Ok(Some(DbEvent::CancelRefused { id: 1 })) => said += 1,
            Ok(Some(
                ev @ (DbEvent::Failed { .. } | DbEvent::Done { .. } | DbEvent::Page { .. } | DbEvent::Lost { .. }),
            )) => {
                panic!("{ev:?}")
            }
            Ok(Some(_)) => {}
            other => panic!("{other:?} after {said} words"),
        }
    }
    let running: Option<String> =
        a.query_first(format!("SELECT COMMAND FROM information_schema.PROCESSLIST WHERE ID = {id}")).await.unwrap();
    assert_eq!(running.as_deref(), Some("Query"), "the statement is still on the server");
    // An administrator stops it: the run ends, the session goes on.
    a.query_drop(format!("KILL QUERY {id}")).await.unwrap();
    let evs = c.answer(1).await;
    assert!(matches!(evs.last(), Some(DbEvent::Failed { error: DbError::Cancelled, cancelled: true, .. })), "{evs:?}");
    c.ok(2, "SELECT 1").await;
    a.disconnect().await.unwrap();
}

/// A held result of a procedure that runs as another account and goes silent after it (its
/// rows all sent, a long count next): a refused cancel while the app has not asked for the next
/// page keeps the session, as for a statement that sends nothing, and says why; once an
/// administrator stops the procedure, the result reads to its end and the session goes on.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_cancel_of_a_held_result_that_went_silent_keeps_the_session() {
    // A tag no other test's begins with (their sessions' guards match by it).
    let test = "held_went_silent_refused";
    let Some((url, admin)) = urls(test) else { return };
    let p = format!("zz_it_heldsilent_{}", std::process::id());
    let _p = Cleanup { url: admin.clone(), sql: format!("DROP PROCEDURE IF EXISTS shop.{p}") };
    let mut a = side(&admin).await;
    a.query_drop(format!(
        "CREATE PROCEDURE shop.{p}() SQL SECURITY DEFINER BEGIN \
         SELECT id FROM shop.events ORDER BY id LIMIT 600; \
         SELECT COUNT(*) FROM shop.events a JOIN shop.events b ON a.id < b.id; END"
    ))
    .await
    .unwrap();
    // Dropped first: the statement the test leaves running goes, and the drop does not wait
    // behind it.
    let _end = EndSessions { admin: admin.clone(), test };
    let mut c = Conn::open(&url, SessionRole::Query, false, test).await;
    let id = only_session(&mut a, SessionRole::Query, test).await;
    let first = c.run_as(1, &[&format!("CALL shop.{p}()")], PagingMode::Hold).await;
    assert!(matches!(first.last(), Some(DbEvent::Page { id: 1, columns: Some(_), more: true, .. })), "{first:?}");
    // Its rows are all on their way: the count runs, saying nothing.
    tokio::time::sleep(Duration::from_millis(1000)).await;
    c.session.cancel();
    let mut said = 0;
    let deadline = Instant::now() + Duration::from_secs(20);
    while said < 2 {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, c.rx.recv()).await {
            Ok(Some(DbEvent::CancelRefused { id: 1 })) => said += 1,
            Ok(Some(ev @ (DbEvent::Failed { .. } | DbEvent::Lost { .. }))) => panic!("{ev:?}"),
            Ok(Some(_)) => {}
            other => panic!("{other:?} after {said} words"),
        }
    }
    let running: Option<String> =
        a.query_first(format!("SELECT COMMAND FROM information_schema.PROCESSLIST WHERE ID = {id}")).await.unwrap();
    assert_eq!(running.as_deref(), Some("Query"), "the procedure is still on the server");
    // An administrator stops it: the app reads the result on, and the session goes on.
    a.query_drop(format!("KILL QUERY {id}")).await.unwrap();
    let mut evs = Vec::new();
    loop {
        c.session.send(DbCommand::FetchMore { id: 1 });
        let ev = c
            .wait(
                |e| matches!(e, DbEvent::Page { id: 1, .. } | DbEvent::Failed { id: 1, .. } | DbEvent::Lost { .. }),
                30,
            )
            .await;
        let end = matches!(ev, DbEvent::Page { more: false, .. } | DbEvent::Failed { .. } | DbEvent::Lost { .. });
        evs.push(ev);
        if end {
            break;
        }
    }
    assert!(!evs.iter().any(|e| matches!(e, DbEvent::Lost { .. })), "{evs:?}");
    c.ok(2, "SELECT 1").await;
    a.disconnect().await.unwrap();
}

/// The same for a held result the app reads on after the cancel: its first page was the
/// answer, then one cancelled failure, then the session is lost.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_cancel_of_a_held_result_ends_the_session() {
    let test = "a_refused_cancel_of_a_held_result";
    let Some((url, admin)) = urls(test) else { return };
    let (p, _p) = endless_definer_procedure(&admin, "heldrefused").await;
    // Dropped first: nothing of the test's keeps running past it.
    let _end = EndSessions { admin: admin.clone(), test };
    let mut a = side(&admin).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, test).await;
    let id = only_session(&mut a, SessionRole::Query, test).await;
    let first = c.run_as(1, &[&format!("CALL shop.{p}()")], PagingMode::Hold).await;
    assert!(matches!(first.last(), Some(DbEvent::Page { id: 1, columns: Some(_), more: true, .. })), "{first:?}");
    c.session.send(DbCommand::FetchMore { id: 1 });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let t0 = Instant::now();
    c.session.cancel();
    let evs = until_lost(&mut c, 20).await;
    // Bounded by the kill's own time limit and the ticks rows must keep coming in.
    assert!(t0.elapsed() < Duration::from_secs(15), "{:?}", t0.elapsed());
    assert!(
        matches!(failures(&evs, 1)[..], [DbEvent::Failed { error: DbError::Cancelled, cancelled: true, .. }]),
        "{evs:?}"
    );
    assert!(
        matches!(evs.last(), Some(DbEvent::Lost { error: DbError::CancelNotTaken(CancelFailure::Refused) })),
        "{evs:?}"
    );
    thread_ends(&mut a, id, 30).await;
    a.disconnect().await.unwrap();
}

/// A held result the app stops itself (it closes it, or the user runs another statement) whose
/// statement the server will not stop is not read to its end either: the session is lost at
/// once (the run was answered with its first page: nothing more for it). Closing a session
/// while such a result is held ends its statement on the server too.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_result_the_server_will_not_stop_is_not_read_to_its_end() {
    let test = "a_held_result_the_server_will_not_stop";
    let Some((url, admin)) = urls(test) else { return };
    let (p, _p) = endless_definer_procedure(&admin, "heldstop").await;
    // Dropped first: nothing of the test's keeps running past it.
    let _end = EndSessions { admin: admin.clone(), test };
    let mut a = side(&admin).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, test).await;
    let id = only_session(&mut a, SessionRole::Query, test).await;
    let first = c.run_as(1, &[&format!("CALL shop.{p}()")], PagingMode::Hold).await;
    assert!(matches!(first.last(), Some(DbEvent::Page { id: 1, columns: Some(_), more: true, .. })), "{first:?}");
    let t0 = Instant::now();
    c.session.send(DbCommand::ClosePortal { id: 1 });
    let evs = until_lost(&mut c, 20).await;
    // Bounded by the kill's own time limit and the ticks rows must keep coming in.
    assert!(t0.elapsed() < Duration::from_secs(15), "{:?}", t0.elapsed());
    assert!(failures(&evs, 1).is_empty(), "{evs:?}");
    assert!(
        matches!(evs.last(), Some(DbEvent::Lost { error: DbError::CancelNotTaken(CancelFailure::Refused) })),
        "{evs:?}"
    );
    thread_ends(&mut a, id, 30).await;
    // Another statement stops it: that run, which never got to run, is answered before the
    // session says it is lost.
    let test = "a_held_result_the_server_will_not_stop_next";
    let mut c = Conn::open(&url, SessionRole::Query, false, test).await;
    let id = only_session(&mut a, SessionRole::Query, test).await;
    let first = c.run_as(1, &[&format!("CALL shop.{p}()")], PagingMode::Hold).await;
    assert!(matches!(first.last(), Some(DbEvent::Page { id: 1, columns: Some(_), more: true, .. })), "{first:?}");
    c.session.send(DbCommand::Execute {
        id: 2,
        statements: vec!["SELECT 1".into()],
        paging: PagingMode::NoHold,
        tx: TxRule::Auto,
    });
    let evs = until_lost(&mut c, 20).await;
    assert!(failures(&evs, 1).is_empty(), "{evs:?}");
    let refused = DbError::CancelNotTaken(CancelFailure::Refused);
    assert!(
        matches!(&failures(&evs, 2)[..], [DbEvent::Failed { error, cancelled: false, .. }] if *error == refused),
        "{evs:?}"
    );
    assert!(matches!(evs.last(), Some(DbEvent::Lost { error }) if *error == refused), "{evs:?}");
    thread_ends(&mut a, id, 30).await;
    // Closing the session.
    let test = "a_held_result_the_server_will_not_stop_closing";
    let mut c = Conn::open(&url, SessionRole::Query, false, test).await;
    let id = only_session(&mut a, SessionRole::Query, test).await;
    let first = c.run_as(1, &[&format!("CALL shop.{p}()")], PagingMode::Hold).await;
    assert!(matches!(first.last(), Some(DbEvent::Page { id: 1, columns: Some(_), more: true, .. })), "{first:?}");
    drop(c);
    thread_ends(&mut a, id, 30).await;
    // Closing it while the result is read past its first page, not held (the app gives up on
    // a cancel that is not answered by closing the session): never read on in the background.
    let test = "a_held_result_the_server_will_not_stop_closing_not_held";
    let c = Conn::open(&url, SessionRole::Query, false, test).await;
    let id = only_session(&mut a, SessionRole::Query, test).await;
    c.session.send(DbCommand::Execute {
        id: 1,
        statements: vec![format!("CALL shop.{p}()")],
        paging: PagingMode::NoHold,
        tx: TxRule::Auto,
    });
    tokio::time::sleep(Duration::from_millis(1000)).await;
    drop(c);
    thread_ends(&mut a, id, 30).await;
    a.disconnect().await.unwrap();
}

/// How many rows table `q` has, counted by session `c` (run `id`).
async fn rows_in(c: &mut Conn, id: u64, q: &str) -> String {
    let evs = c.run(id, &[&format!("SELECT COUNT(*) FROM {q}")]).await;
    page_of(&evs).1[0][0].clone().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn explain_analyze_keeps_nothing_it_ran() {
    let Some((url, admin)) = urls("explain_analyze_keeps_nothing_it_ran") else { return };
    let t = numbers(&admin, "explain", 10).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, "explain").await;
    // The multi-table form: MySQL measures it (a single-table one it does not run at all).
    let delete = format!("EXPLAIN ANALYZE DELETE x FROM {q} x JOIN {q} y ON y.id = x.id WHERE x.id > 5", q = t.q());
    let evs = c.run(1, &[&delete]).await;
    assert!(page_of(&evs).1[0][0].as_deref().is_some_and(|p| p.contains("Delete")), "{evs:?}");
    assert_eq!(rows_in(&mut c, 2, &t.q()).await, "10");
    assert!(!evs.iter().any(|e| matches!(e, DbEvent::TxOpen(true))), "its transaction is not the user's");
    // Inside the user's transaction: what the user did stays, the statement's changes go.
    c.ok(3, "BEGIN").await;
    c.ok(4, &format!("DELETE FROM {} WHERE id = 1", t.q())).await;
    c.ok(5, &delete).await;
    assert_eq!(rows_in(&mut c, 6, &t.q()).await, "9");
    let evs = c.run(7, &["RELEASE SAVEPOINT datarig_explain"]).await;
    assert!(matches!(evs.last(), Some(DbEvent::Failed { .. })), "the savepoint is gone");
    c.ok(8, "ROLLBACK").await;
    assert_eq!(rows_in(&mut c, 9, &t.q()).await, "10");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_sql_mode_changes_the_language() {
    let Some(url) = my_url("a_changed_sql_mode_changes_the_language") else { return };
    let mut c = Conn::open(&url, SessionRole::Query, false, "mode").await;
    let evs = c.run(1, &["SET SESSION sql_mode = 'ANSI_QUOTES,NO_BACKSLASH_ESCAPES'"]).await;
    let mode = evs.iter().rev().find_map(|e| match e {
        DbEvent::Language(Language::Sql(Dialect::MySql(m))) => Some(*m),
        _ => None,
    });
    assert!(mode.is_some_and(|m| m.ansi_quotes && m.no_backslash_escapes), "{evs:?}");
    // Said before the run's answer, so the next statement is read in it.
    let at = |pred: fn(&DbEvent) -> bool| evs.iter().rposition(pred).unwrap();
    assert!(at(|e| matches!(e, DbEvent::Language(_))) < at(|e| matches!(e, DbEvent::Done { .. })));
    let evs = c.run(2, &["SET SESSION sql_mode = DEFAULT"]).await;
    assert!(evs.iter().any(|e| matches!(e, DbEvent::Language(Language::Sql(Dialect::MySql(m))) if !m.ansi_quotes)));
}

#[tokio::test(flavor = "multi_thread")]
async fn values_of_every_type_read_as_the_server_writes_them() {
    let Some((url, admin)) = urls("values_of_every_type") else { return };
    let t = Table::make(
        &admin,
        "types",
        &[
            "CREATE TABLE {t} (
               i BIGINT UNSIGNED, d DECIMAL(65, 30), f DOUBLE, b BIT(4), t1 TINYINT(1), y YEAR,
               dt DATETIME(6), ts TIMESTAMP(3) NULL, da DATE, tm TIME(2), j JSON, e ENUM('a', 'b'), s SET('x', 'y'),
               bl BLOB, vb VARBINARY(8), g GEOMETRY, c CHAR(3), tx TEXT)",
            "SET SESSION sql_mode = ''",
        ],
    )
    .await;
    let mut c = Conn::open(&url, SessionRole::Query, false, "types").await;
    c.ok(1, "SET SESSION sql_mode = '', time_zone = '+09:00'").await;
    c.ok(
        2,
        &format!(
            "INSERT INTO {} VALUES (18446744073709551615, 12345678901234567890.123456789012345678901234567890, 1.5e300, b'0101', 2, 2024,
             '2024-02-29 23:59:59.123456', '2024-01-01 00:00:00.5', '0000-00-00', '-838:59:59.5', '{{\"a\": [1, 2]}}', 'b', 'x,y',
             X'00FF', X'41', ST_GeomFromText('POINT(1 2)'), 'ab', '\\u{{d55c}}')",
            t.q()
        ),
    )
    .await;
    let evs = c.run(3, &[&format!("SELECT * FROM {}", t.q())]).await;
    let (cols, rows, _) = page_of(&evs);
    let got: Vec<(String, String, ValueKind, Option<String>)> =
        cols.iter().zip(&rows[0]).map(|(c, v)| (c.name.clone(), c.type_name.clone(), c.kind, v.clone())).collect();
    let want = [
        ("i", "bigint unsigned", ValueKind::Integer, "18446744073709551615"),
        ("d", "decimal", ValueKind::Decimal, "12345678901234567890.123456789012345678901234567890"),
        ("f", "double", ValueKind::Float, "1.5e300"),
        ("b", "bit", ValueKind::Bit, "b'0101'"),
        ("t1", "tinyint(1)", ValueKind::Integer, "2"),
        ("y", "year", ValueKind::Integer, "2024"),
        ("dt", "datetime", ValueKind::Timestamp, "2024-02-29 23:59:59.123456"),
        ("ts", "timestamp", ValueKind::TimestampTz, "2024-01-01 00:00:00.500"),
        ("da", "date", ValueKind::Date, "0000-00-00"),
        ("tm", "time", ValueKind::Interval, "-838:59:59.00"),
        ("j", "json", ValueKind::Json, "{\"a\": [1, 2]}"),
        ("e", "enum", ValueKind::Text, "b"),
        ("s", "set", ValueKind::Text, "x,y"),
        ("bl", "blob", ValueKind::Bytes, "0x00FF"),
        ("vb", "varbinary", ValueKind::Bytes, "0x41"),
        ("c", "char", ValueKind::Text, "ab"),
    ];
    for (name, ty, kind, value) in want {
        let g = got.iter().find(|g| g.0 == name).unwrap();
        assert_eq!((g.1.as_str(), g.2, g.3.as_deref()), (ty, kind, Some(value)), "{name}");
    }
    let g = got.iter().find(|g| g.0 == "g").unwrap();
    assert_eq!((g.1.as_str(), g.2), ("geometry", ValueKind::Bytes));
    assert!(g.3.as_deref().is_some_and(|v| v.starts_with("0x00000000")), "SRID 0, then WKB: {:?}", g.3);
    // Every column names the table column it comes from.
    assert!(cols.iter().all(|c| matches!(&c.origin,
        Some(datarig_core::driver::ColumnOrigin::Named { schema, table, column }) if schema == "shop" && *table == t.name && *column == c.name)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_procedures_further_result_sets_are_read_and_said() {
    let Some((url, admin)) = urls("a_procedures_further_result_sets") else { return };
    let name = format!("zz_it_proc_{}", std::process::id());
    let mut a = side(&admin).await;
    a.query_drop(format!("DROP PROCEDURE IF EXISTS shop.{name}")).await.unwrap();
    a.query_drop(format!("CREATE PROCEDURE shop.{name}() BEGIN SELECT 1 AS a; SELECT 2 AS b; END")).await.unwrap();
    let mut c = Conn::open(&url, SessionRole::Query, false, "proc").await;
    let evs = c.run(1, &[&format!("CALL shop.{name}()")]).await;
    a.query_drop(format!("DROP PROCEDURE shop.{name}")).await.unwrap();
    a.disconnect().await.unwrap();
    assert_eq!(page_of(&evs).1, &vec![vec![Some("1".to_string())]]);
    assert_eq!(info(&evs).map(|i| i.more_results), Some(1));
    c.ok(2, "SELECT 1").await;
}

/// Count with `sql` (run `id`): the count and whether it saw the transaction's snapshot.
async fn count(c: &mut Conn, id: u64, sql: String) -> (Result<u64, DbError>, bool) {
    c.session.send(DbCommand::Count { id, sql });
    match c.wait(|e| matches!(e, DbEvent::Counted { .. }), 30).await {
        DbEvent::Counted { result, snapshot, .. } => (result, snapshot),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn counts_and_the_servers_question() {
    let Some((url, admin)) = urls("counts_and_the_servers_question") else { return };
    let t = numbers(&admin, "count", 1234).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, "count").await;
    let sql = format!("SELECT COUNT(*) FROM (\nSELECT id FROM {}\n) AS datarig_count", t.q());
    assert_eq!(count(&mut c, 1, sql.clone()).await, (Ok(1234), false));
    c.ok(2, "BEGIN").await;
    assert_eq!(count(&mut c, 3, sql).await, (Ok(1234), true), "REPEATABLE READ: the transaction's snapshot");
    c.ok(4, "ROLLBACK").await;
    let view = "SELECT COUNT(*) FROM (\nSELECT * FROM shop.order_summary\n) AS datarig_count".to_string();
    assert!(matches!(count(&mut c, 5, view).await.0, Err(DbError::NotRepeatable(_))));
    c.session.send(DbCommand::CheckRepeat { id: 6, sql: format!("SELECT id FROM {}", t.q()) });
    assert!(matches!(
        c.wait(|e| matches!(e, DbEvent::RepeatChecked { .. }), 30).await,
        DbEvent::RepeatChecked { result: Ok(()), .. }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_session_refuses_and_never_stays_writable() {
    let Some((url, admin)) = urls("a_read_only_session_refuses") else { return };
    let t = Table::make(&admin, "ro", &["CREATE TABLE {t} (id INT PRIMARY KEY)"]).await;
    let proc_name = format!("zz_it_ro_off_{}", std::process::id());
    let mut a = side(&admin).await;
    a.query_drop(format!("DROP PROCEDURE IF EXISTS shop.{proc_name}")).await.unwrap();
    let _gone = Cleanup { url: admin.clone(), sql: format!("DROP PROCEDURE IF EXISTS shop.{proc_name}") };
    a.query_drop(format!(
        "CREATE PROCEDURE shop.{proc_name}() BEGIN SET SESSION transaction_read_only = OFF; \
         INSERT INTO {} VALUES (1); SET SESSION transaction_read_only = ON; END",
        t.q()
    ))
    .await
    .unwrap();
    let mut c = Conn::open(&url, SessionRole::Query, true, "ro").await;
    // A write is refused before the server sees it (the server's read-only, set up at connect,
    // is the layer behind).
    let evs = c.run(1, &[&format!("INSERT INTO {} VALUES (1)", t.q())]).await;
    assert!(matches!(evs.last(), Some(DbEvent::Failed { error: DbError::ReadWriteRefused, .. })), "{evs:?}");
    // The session refuses to ask for read-write.
    for (id, sql) in [(2, "START TRANSACTION READ WRITE"), (3, "SET SESSION transaction_read_only = OFF")] {
        assert!(
            matches!(c.run(id, &[sql]).await.last(), Some(DbEvent::Failed { error: DbError::ReadWriteRefused, .. })),
            "{sql}"
        );
    }
    // What the server's read-only does not stop is refused as the app refuses it: an executable
    // comment, a procedure (this one turns read-only off, writes, and turns it on again, which
    // the server's report after it would not show), turning the session's reports off.
    for (id, sql) in [
        (4, "/*!80000 START TRANSACTION READ WRITE */".to_string()),
        (5, format!("CALL shop.{proc_name}()")),
        (6, "SET SESSION session_track_system_variables = ''".to_string()),
    ] {
        let evs = c.run(id, &[&sql]).await;
        assert!(matches!(evs.last(), Some(DbEvent::Failed { error: DbError::ReadWriteRefused, .. })), "{sql}: {evs:?}");
    }
    c.ok(7, "SELECT 1").await;
    a.query_drop(format!("DROP PROCEDURE shop.{proc_name}")).await.unwrap();
    let n: u64 = a.query_first(format!("SELECT COUNT(*) FROM {}", t.q())).await.unwrap().unwrap();
    assert_eq!(n, 0, "nothing was written");
    a.disconnect().await.unwrap();
}

/// A listing the server cuts at `sql_select_limit` (`SHOW`, `DESCRIBE`) comes whole, last or not,
/// held or not; `SHOW WARNINGS` still lists the statement before.
#[tokio::test(flavor = "multi_thread")]
async fn a_listing_is_never_cut_at_a_page() {
    let Some(url) = my_url("a_listing_is_never_cut_at_a_page") else { return };
    let mut c = Conn::open(&url, SessionRole::Query, false, "listing").await;
    let mut probe = side(&url).await;
    let all: Vec<(String, String)> = probe.query("SHOW SESSION VARIABLES").await.unwrap();
    probe.disconnect().await.unwrap();
    assert!(all.len() > PAGE + 1, "{}", all.len());
    let rows = |evs: &[DbEvent], index: Option<usize>| -> usize {
        evs.iter()
            .map(|e| match e {
                DbEvent::Page { rows, .. } if index.is_none() => rows.len(),
                DbEvent::StepRows { index: i, rows, .. } if Some(*i) == index => rows.len(),
                _ => 0,
            })
            .sum()
    };
    // After a query set the page limit: last (read on to its end), and before the last.
    c.ok(1, "SELECT 1").await;
    let evs = c.run(2, &["SHOW SESSION VARIABLES"]).await;
    let mut got = rows(&evs, None);
    let mut more = page_of(&evs).2;
    while more {
        c.session.send(DbCommand::FetchMore { id: 2 });
        match c.wait(|e| matches!(e, DbEvent::Page { .. }), 30).await {
            DbEvent::Page { rows, more: m, .. } => (got, more) = (got + rows.len(), m),
            _ => unreachable!(),
        }
    }
    assert_eq!(got, all.len());
    c.ok(3, "SELECT 1").await;
    let evs = c.run(4, &["SHOW SESSION VARIABLES", "SELECT 1"]).await;
    assert_eq!(rows(&evs, Some(0)), all.len(), "{evs:?}");
    let evs = c.run_as(5, &["SELECT 1", "DESCRIBE shop.users", "SELECT 1"], PagingMode::Hold).await;
    assert!(rows(&evs, Some(1)) > 1, "{evs:?}");
    // The warnings of the statement before, not those of a `SET` of the session's.
    let evs = c.run(6, &["SELECT CAST('1a' AS SIGNED)", "SHOW WARNINGS"]).await;
    assert_eq!(rows(&evs, None), 1, "{evs:?}");
}

/// A statement that reads what the one before did reads that, not a `SET` of the session's.
#[tokio::test(flavor = "multi_thread")]
async fn row_count_reads_the_statement_before() {
    let Some((url, admin)) = urls("row_count_reads_the_statement_before") else { return };
    let t = Table::make(&admin, "rowcount", &["CREATE TABLE {t} (id INT PRIMARY KEY)"]).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, "rowcount").await;
    let evs =
        c.run(1, &["SELECT 1", &format!("INSERT INTO {} VALUES (1), (2), (3)", t.q()), "SELECT ROW_COUNT()"]).await;
    match evs.last() {
        Some(DbEvent::Page { rows, .. }) => assert_eq!(rows, &vec![vec![Some("3".to_string())]]),
        other => panic!("{other:?}"),
    }
}

/// A dialer whose dials after the first `slow` one take `delay`: a cancel's login that comes late.
struct SlowDialer {
    inner: TcpDialer,
    delay: Duration,
}

impl Dialer for SlowDialer {
    fn dial(
        &self,
        host: &str,
        port: u16,
    ) -> futures::future::BoxFuture<
        'static,
        Result<datarig_core::transport::BoxedStream, datarig_core::transport::DialError>,
    > {
        let first = self.inner.dials.load(std::sync::atomic::Ordering::SeqCst) == 0;
        let (dial, delay) = (self.inner.dial(host, port), if first { Duration::ZERO } else { self.delay });
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            dial.await
        })
    }
}

/// A cancel whose kill comes after its statement ended and another run began stops nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_late_cancel_does_not_stop_the_next_run() {
    let Some(url) = my_url("a_late_cancel_does_not_stop_the_next_run") else { return };
    let d = Arc::new(SlowDialer { inner: TcpDialer::default(), delay: Duration::from_millis(1500) });
    let (tx, rx) = unbounded_channel();
    let o =
        ConnectOptions::new(PAGE, SessionRole::Query, &tag("latecancel")).dialer(Some(DialerRef(d as Arc<dyn Dialer>)));
    let session = MyDriver.connect(&profile(&url), SessionRole::Query, o, tx);
    let mut c = Conn { session, rx };
    c.wait(|e| matches!(e, DbEvent::Connected), 15).await;
    c.session.send(DbCommand::Execute {
        id: 1,
        statements: vec!["SELECT SLEEP(0.5)".into()],
        paging: PagingMode::NoHold,
        tx: TxRule::Auto,
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    c.session.cancel();
    c.answer(1).await;
    // The kill's login is still on its way when the next run begins.
    let evs = c.run(2, &["SELECT SLEEP(2.5)"]).await;
    match evs.last() {
        Some(DbEvent::Page { rows, .. }) => assert_eq!(rows, &vec![vec![Some("0".to_string())]], "not stopped"),
        other => panic!("{other:?}"),
    }
}

/// Every row of run `id` in mode `paging` (its pages asked for to the end).
async fn all_rows(c: &mut Conn, id: u64, sql: &str, paging: PagingMode) -> usize {
    let evs = c.run_as(id, &[sql], paging).await;
    let (_, rows, mut more) = page_of(&evs);
    let mut got = rows.len();
    while more {
        c.session.send(DbCommand::FetchMore { id });
        match c.wait(|e| matches!(e, DbEvent::Page { .. } | DbEvent::Failed { .. }), 30).await {
            DbEvent::Page { rows, more: m, .. } => (got, more) = (got + rows.len(), m),
            other => panic!("{other:?}"),
        }
    }
    got
}

/// A locking read is read whole: the page limit would lock fewer rows than it asks for, and
/// leave the rest unread.
#[tokio::test(flavor = "multi_thread")]
async fn a_locking_read_is_read_whole() {
    let Some((url, admin)) = urls("a_locking_read_is_read_whole") else { return };
    let t = numbers(&admin, "locking", 1234).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, "locking").await;
    for (id, clause) in [(1, "FOR UPDATE"), (2, "FOR SHARE"), (3, "LOCK IN SHARE MODE")] {
        let sql = format!("SELECT id FROM {} {clause}", t.q());
        assert_eq!(all_rows(&mut c, id, &sql, PagingMode::NoHold).await, 1234, "{clause}");
    }
}

/// A column named as a function that reads the statement before is only a name: a held result
/// is never cut at the page limit the session has.
#[tokio::test(flavor = "multi_thread")]
async fn a_column_named_row_count_is_read_whole() {
    let Some((url, admin)) = urls("a_column_named_row_count_is_read_whole") else { return };
    let t = numbers(&admin, "rcname", 1234).await;
    let mut c = Conn::open(&url, SessionRole::Query, false, "rcname").await;
    c.ok(1, &format!("SELECT id FROM {}", t.q())).await;
    let sql = format!("SELECT id AS row_count FROM {}", t.q());
    assert_eq!(all_rows(&mut c, 2, &sql, PagingMode::Hold).await, 1234);
}

/// The structure test `test`'s metadata session reads of `shop.<table>`.
async fn structure_of(
    m: &mut Conn,
    table: &str,
) -> Result<Box<datarig_core::driver::structure::TableStructure>, DbError> {
    m.session.send(DbCommand::LoadStructure { schema: "shop".into(), table: table.into() });
    match m.wait(|e| matches!(e, DbEvent::Structure { .. }), 15).await {
        DbEvent::Structure { schema, table: t, result } => {
            assert_eq!((schema.as_str(), t.as_str()), ("shop", table));
            result
        }
        _ => unreachable!(),
    }
}

/// What metadata session `m` reads of the structure of `schema.<table>`.
async fn structure_in(
    m: &mut Conn,
    schema: &str,
    table: &str,
) -> Result<Box<datarig_core::driver::structure::TableStructure>, DbError> {
    m.session.send(DbCommand::LoadStructure { schema: schema.into(), table: table.into() });
    match m.wait(|e| matches!(e, DbEvent::Structure { schema: s, table: t, .. } if s == schema && t == table), 15).await
    {
        DbEvent::Structure { result, .. } => result,
        _ => unreachable!(),
    }
}

/// A table's structure in one statement: its columns (`AUTO_INCREMENT`, defaults as SQL,
/// generated columns), its primary key, a composite foreign key to a table of another database,
/// its indexes (prefix, descending, functional, full-text, unique), its checks (one the server
/// does not enforce) and its triggers (which call no function); a view; a table that is not
/// there; and a user who may not see the triggers, to whom they are unknown, not none.
#[tokio::test(flavor = "multi_thread")]
async fn the_metadata_session_reads_a_tables_structure() {
    use datarig_core::driver::structure::*;
    let Some((url, admin)) = urls("the_metadata_session_reads_a_tables_structure") else { return };
    let pid = std::process::id();
    let (child, parent, view) =
        (format!("zz_it_child_{pid}"), format!("zz_it_parent_{pid}"), format!("zz_it_view_{pid}"));
    // One statement each, dropped in the reverse order: the view, the table, the one it
    // references.
    let _parent = Cleanup { url: admin.clone(), sql: format!("DROP TABLE IF EXISTS datarig.{parent}") };
    let _child = Cleanup { url: admin.clone(), sql: format!("DROP TABLE IF EXISTS shop.{child}") };
    // The view is in `datarig`: other tests list the views of `shop`.
    let _view = Cleanup { url: admin.clone(), sql: format!("DROP VIEW IF EXISTS datarig.{view}") };
    let mut a = side(&admin).await;
    for sql in [
        format!("CREATE TABLE datarig.{parent} (a INT NOT NULL, b VARCHAR(10) NOT NULL, PRIMARY KEY (a, b))"),
        format!(
            "CREATE TABLE shop.{child} (
               id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY,
               pa INT, pb VARCHAR(10),
               name VARCHAR(100) NOT NULL DEFAULT 'it''s',
               qty INT NOT NULL DEFAULT 0,
               price DECIMAL(10,2) DEFAULT (1.5),
               created TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),
               body TEXT,
               twice INT GENERATED ALWAYS AS (qty * 2) STORED,
               low VARCHAR(100) GENERATED ALWAYS AS (lower(name)) VIRTUAL,
               UNIQUE KEY uq_name (name),
               KEY ix_prefix (body(10)),
               KEY ix_desc (qty DESC, name),
               KEY ix_fn ((upper(name))),
               FULLTEXT KEY ft_body (body),
               CONSTRAINT zz_it_fk_{pid} FOREIGN KEY (pa, pb) REFERENCES datarig.{parent} (a, b) ON DELETE CASCADE,
               CONSTRAINT zz_it_ck_{pid} CHECK (qty >= 0),
               CONSTRAINT zz_it_loose_{pid} CHECK (price < 1000) NOT ENFORCED)"
        ),
        format!(
            "CREATE TRIGGER shop.zz_it_bi_{pid} BEFORE INSERT ON shop.{child} FOR EACH ROW SET NEW.qty = NEW.qty + 0"
        ),
        format!(
            "CREATE TRIGGER shop.zz_it_au_{pid} AFTER UPDATE ON shop.{child} FOR EACH ROW \
             BEGIN DECLARE x INT; SET x = 1; END"
        ),
        format!("CREATE VIEW datarig.{view} AS SELECT id, name FROM shop.{child}"),
    ] {
        a.query_drop(sql).await.unwrap();
    }
    a.disconnect().await.unwrap();

    let mut m = Conn::start(&url, SessionRole::Meta, false, "structure");
    m.wait(|e| matches!(e, DbEvent::Catalog(_)), 30).await;
    let s = structure_of(&mut m, &child).await.unwrap();
    assert_eq!(s.kind, RelationKind::Table);
    assert_eq!(s.stats(), None, "the estimates are the listing's");
    let cols: Vec<(&str, &str, bool, Option<&str>, &ColumnFill)> = s
        .columns
        .iter()
        .map(|c| (c.name.as_str(), c.type_name.as_str(), c.not_null, c.default.as_deref(), &c.fill))
        .collect();
    assert_eq!(
        cols,
        [
            ("id", "bigint unsigned", true, None, &ColumnFill::AutoIncrement),
            ("pa", "int", false, None, &ColumnFill::Default),
            ("pb", "varchar(10)", false, None, &ColumnFill::Default),
            ("name", "varchar(100)", true, Some("'it''s'"), &ColumnFill::Default),
            ("qty", "int", true, Some("0"), &ColumnFill::Default),
            ("price", "decimal(10,2)", false, Some("(1.5)"), &ColumnFill::Default),
            (
                "created",
                "timestamp(3)",
                true,
                Some("CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3)"),
                &ColumnFill::Default
            ),
            ("body", "text", false, None, &ColumnFill::Default),
            ("twice", "int", false, None, &ColumnFill::Stored("`qty` * 2".into())),
            ("low", "varchar(100)", false, None, &ColumnFill::Virtual("lower(`name`)".into())),
        ]
    );
    assert_eq!(
        s.primary_key.as_ref().map(|p| (p.name.as_str(), p.columns.clone())),
        Some(("PRIMARY", vec!["id".into()]))
    );
    assert_eq!(
        s.foreign_keys,
        [ForeignKey {
            name: format!("zz_it_fk_{pid}"),
            columns: vec!["pa".into(), "pb".into()],
            ref_schema: "datarig".into(),
            ref_table: parent.clone(),
            ref_columns: vec!["a".into(), "b".into()],
            on_delete: FkAction::Cascade,
            on_update: FkAction::NoAction,
            definition: format!(
                "CONSTRAINT zz_it_fk_{pid} FOREIGN KEY (pa, pb) REFERENCES datarig.{parent} (a, b) ON DELETE CASCADE"
            ),
        }]
    );
    let keys = |name: &str| {
        let x = s.indexes.iter().find(|x| x.name == name).unwrap_or_else(|| panic!("{name}: {:?}", s.indexes));
        (x.keys(), x.key_columns.clone(), x.unique, x.method.clone())
    };
    let some = |v: &[&str]| v.iter().map(|c| Some(c.to_string())).collect::<Vec<_>>();
    assert_eq!(keys("PRIMARY"), (vec!["id".into()], some(&["id"]), true, "BTREE".into()));
    assert_eq!(keys("ix_prefix"), (vec!["body(10)".into()], some(&["body"]), false, "BTREE".into()));
    assert_eq!(
        keys("ix_desc"),
        (vec!["qty DESC".into(), "name".into()], some(&["qty", "name"]), false, "BTREE".into())
    );
    assert_eq!(keys("ix_fn"), (vec!["(upper(`name`))".into()], vec![None], false, "BTREE".into()));
    assert_eq!(keys("ft_body"), (vec!["body".into()], some(&["body"]), false, "FULLTEXT".into()));
    assert_eq!(keys("uq_name"), (vec!["name".into()], some(&["name"]), true, "BTREE".into()));
    // The foreign key's own index.
    assert_eq!(keys(&format!("zz_it_fk_{pid}")).0, ["pa", "pb"]);
    let names: Vec<&str> = s.indexes.iter().map(|x| x.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "by name");
    assert_eq!(s.unique_constraints.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(), ["uq_name"]);
    let checks: Vec<(&str, &str, Vec<String>, bool)> =
        s.checks.iter().map(|c| (c.name.as_str(), c.expression.as_str(), c.columns.clone(), c.enforced())).collect();
    assert_eq!(
        checks,
        [
            (format!("zz_it_ck_{pid}").as_str(), "`qty` >= 0", vec!["qty".to_string()], true),
            (format!("zz_it_loose_{pid}").as_str(), "`price` < 1000", vec!["price".to_string()], false),
        ]
    );
    let triggers: Vec<(String, TriggerTiming, Vec<TriggerEvent>, bool, &str)> = s
        .triggers
        .iter()
        .map(|t| (t.name.clone(), t.timing, t.events.clone(), t.for_each_row, t.function.as_str()))
        .collect();
    assert_eq!(
        triggers,
        [
            (format!("zz_it_bi_{pid}"), TriggerTiming::Before, vec![TriggerEvent::Insert], true, ""),
            (format!("zz_it_au_{pid}"), TriggerTiming::After, vec![TriggerEvent::Update], true, ""),
        ],
        "in the order they fire"
    );
    assert!(
        s.triggers[1].definition.ends_with("FOR EACH ROW BEGIN DECLARE x INT; SET x = 1; END"),
        "{}",
        s.triggers[1].definition
    );
    assert!(s.hidden.is_empty());
    // Every definition is MySQL that runs: the indexes and checks on a copy of the table.
    let mut a = side(&admin).await;
    let copy = format!("zz_it_copy_{pid}");
    let _copy = Cleanup { url: admin.clone(), sql: format!("DROP TABLE IF EXISTS shop.{copy}") };
    a.query_drop(format!(
        "CREATE TABLE shop.{copy} (id BIGINT UNSIGNED NOT NULL, name VARCHAR(100) NOT NULL, qty INT, price DECIMAL(10,2), body TEXT)"
    ))
    .await
    .unwrap();
    for x in s
        .indexes
        .iter()
        .filter(|x| ["PRIMARY", "ix_prefix", "ix_desc", "ix_fn", "ft_body", "uq_name"].contains(&x.name.as_str()))
    {
        let sql = x.definition.replace(&format!("shop.{child}"), &format!("shop.{copy}"));
        a.query_drop(&sql).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    for c in &s.checks {
        let sql = format!("ALTER TABLE shop.{copy} ADD {}", c.definition.replace("zz_it_", "zz_it_copy_"));
        a.query_drop(&sql).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    a.disconnect().await.unwrap();

    let v = structure_in(&mut m, "datarig", &view).await.unwrap();
    assert_eq!((v.kind, v.stats()), (RelationKind::View, None));
    assert_eq!(v.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id", "name"]);
    assert!(v.triggers.is_empty() && v.hidden.is_empty());
    assert_eq!(structure_of(&mut m, &format!("zz_it_none_{pid}")).await, Err(DbError::NotFound));

    // A user who may read the table but not see its triggers.
    let account = Account::make(&admin, "notrig", "BY 'pw'").await.unwrap();
    let mut n = Conn::start(&as_user(&url, &account.name, "pw"), SessionRole::Meta, false, "structure_notrig");
    n.wait(|e| matches!(e, DbEvent::Catalog(_)), 30).await;
    let hidden = structure_of(&mut n, &child).await.unwrap();
    assert!(hidden.triggers.is_empty());
    assert_eq!(hidden.hidden, [StructureGroup::Triggers], "unknown, not none");
    assert_eq!(hidden.columns.len(), 10);
}

/// Whether table `name` of `shop` is in the server's table cache: a table some session opened
/// since it was flushed (`SHOW OPEN TABLES`). Unlike a count of the tables a session opened, the
/// server's own dictionary tables and other tests' tables do not count.
async fn is_open(a: &mut mysql_async::Conn, name: &str) -> bool {
    let open: Vec<(String, String, u64, u64)> =
        a.query(format!("SHOW OPEN TABLES FROM shop LIKE '{name}'")).await.unwrap();
    open.iter().any(|(_, table, _, _)| table == name)
}

/// Reading a table's structure does not open the table (step 1 is a guard against that, not a
/// proof: on the metadata session, which is read-only, a read of the estimates did not open a
/// table without cached statistics either), never queues behind another session's metadata lock
/// (held by `LOCK TABLES … WRITE`, or asked for by an `ALTER TABLE` that waits behind a held
/// result) nor waits for any lock, and answers with the structure; no other session waits for a
/// lock of its while it runs, and it keeps none once it has answered (as the server shows them,
/// [`watched`]).
#[tokio::test(flavor = "multi_thread")]
async fn a_structure_read_never_queues_behind_a_metadata_lock_nor_keeps_one() {
    let test = "a_structure_read_never_queues_behind_a_metadata_lock_nor_keeps_one";
    let Some((url, admin)) = urls(test) else { return };
    let t = numbers(&admin, "structlock", 200_000).await;
    // Rows wide enough that a held result is more than the network buffers take.
    let mut wide = side(&admin).await;
    wide.query_drop(format!("ALTER TABLE {} MODIFY v VARCHAR(200)", t.q())).await.unwrap();
    wide.query_drop(format!("UPDATE {} SET v = REPEAT('x', 200)", t.q())).await.unwrap();
    wide.disconnect().await.unwrap();
    let mut m = Conn::start(&url, SessionRole::Meta, false, test);
    m.wait(|e| matches!(e, DbEvent::Catalog(_)), 30).await;
    let mut side_admin = side(&admin).await;
    let meta = only_session(&mut side_admin, SessionRole::Meta, test).await;
    let read = |r: Result<Box<datarig_core::driver::structure::TableStructure>, DbError>, columns: usize| {
        let s = r.expect("the structure, never Locked");
        assert_eq!(s.columns.len(), columns);
    };
    // 1. A guard: a new table out of the server's cache, no statistics of it cached, and the
    //    read opens nothing (a read-only session did not open it for its estimates either, so
    //    this does not tell whether they are read).
    let fresh = numbers(&admin, "structopen", 10).await;
    side_admin.query_drop(format!("FLUSH TABLES {}", fresh.q())).await.unwrap();
    read(structure_of(&mut m, &t.name).await, 2);
    assert!(!is_open(&mut side_admin, &fresh.name).await, "flushed");
    read(structure_of(&mut m, &fresh.name).await, 2);
    assert!(!is_open(&mut side_admin, &fresh.name).await, "the table was not opened");
    // 2. Another session holds `LOCK TABLES … WRITE`, and writes while the read runs.
    let mut lock = side(&admin).await;
    lock.query_drop(format!("LOCK TABLES {} WRITE", t.q())).await.unwrap();
    let locker = u64::from(lock.id());
    let (r, seen) = watched(&admin, meta, &[locker], &t.name, async {
        m.session.send(DbCommand::LoadStructure { schema: "shop".into(), table: t.name.clone() });
        lock.query_drop(format!("INSERT INTO {} VALUES (999999, 'w')", t.q())).await.unwrap();
        match m.wait(|e| matches!(e, DbEvent::Structure { .. }), 10).await {
            DbEvent::Structure { result, .. } => result,
            _ => unreachable!(),
        }
    })
    .await;
    assert!(seen.held_up.is_empty(), "the locking session waited for the read: {:?}", seen.held_up);
    assert!(seen.queued.is_empty(), "the read waited for a lock: {:?}", seen.queued);
    read(r, 2);
    assert_eq!(metadata_locks_of(&mut side_admin, meta).await, 0, "no lock kept");
    lock.query_drop("UNLOCK TABLES").await.unwrap();
    // 3. A held result keeps a shared lock and an `ALTER TABLE` waits for it.
    let mut q = Conn::open(&url, SessionRole::Query, false, test).await;
    let evs = q.run_as(1, &[&format!("SELECT * FROM {}", t.q())], PagingMode::Hold).await;
    assert!(page_of(&evs).2);
    let (alter_id, alter) = waiting_alter(&admin, &t.q()).await;
    let (r, seen) = watched(&admin, meta, &[alter_id], &t.name, structure_of(&mut m, &t.name)).await;
    assert!(seen.held_up.is_empty(), "the ALTER waited for the read: {:?}", seen.held_up);
    assert!(seen.queued.is_empty(), "the read queued behind the ALTER: {:?}", seen.queued);
    read(r, 2);
    assert_eq!(metadata_locks_of(&mut side_admin, meta).await, 0, "no lock kept");
    assert!(alter_waits(&mut side_admin, alter_id).await, "the ALTER still waits for the held result");
    // The ALTER goes through once the held result is stopped: the read held nothing up (the
    // limit only guards against a hang).
    let t1 = Instant::now();
    q.session.send(DbCommand::ClosePortal { id: 1 });
    q.ok(2, "SELECT 1").await;
    assert!(alter.await.unwrap().is_ok());
    assert!(t1.elapsed() < Duration::from_secs(10), "{:?}", t1.elapsed());
    // Read again once nothing is in the way: the new column is there.
    let s = structure_of(&mut m, &t.name).await.unwrap();
    assert_eq!(s.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id", "v", "c9"]);
    side_admin.disconnect().await.unwrap();
}

/// Completion's values: an `ENUM` column's, from the `COLUMN_TYPE` the catalog reads (a quote
/// and a backslash in a value as the server writes them there), and those a check lists, from
/// the table's structure (`IN` and an `OR` of `=`, charset introducers and escapes as the server
/// keeps them); the dev database's `shop.orders.status` is an `ENUM`.
#[tokio::test(flavor = "multi_thread")]
async fn enum_and_check_values_for_completion() {
    use datarig_core::sql::values::{CheckValues, check_values};
    let test = "enum_and_check_values_for_completion";
    let Some((url, admin)) = urls(test) else { return };
    let pid = std::process::id();
    let t = Table::make(
        &admin,
        "values",
        &[&format!(
            r#"CREATE TABLE {{t}} (id INT PRIMARY KEY, s ENUM('a', 'it''s', 'b\\c', 'd"q', 'x,y', ''),
               st VARCHAR(20), u VARCHAR(10), n INT,
               CONSTRAINT zz_it_values_st_{pid} CHECK (st IN ('p', 'it''s', 'b\\c')),
               CONSTRAINT zz_it_values_u_{pid} CHECK (u = 'k' OR u = 'm'),
               CONSTRAINT zz_it_values_n_{pid} CHECK (n IN (1, 2)))"#
        )],
    )
    .await;
    let mut m = Conn::start(&url, SessionRole::Meta, false, test);
    let DbEvent::Catalog(Ok(cat)) = m.wait(|e| matches!(e, DbEvent::Catalog(_)), 30).await else {
        panic!("catalog failed")
    };
    let values = |table: &str, column: &str| {
        let rel = cat.relations.iter().find(|r| r.schema == "shop" && r.name == table).expect(table);
        rel.columns.iter().find(|c| c.name == column).expect(column).values.as_ref().map(|v| v.to_vec())
    };
    let strings = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(values(&t.name, "s"), Some(strings(&["a", "it's", "b\\c", "d\"q", "x,y", ""])));
    assert_eq!((values(&t.name, "st"), values(&t.name, "n")), (None, None));
    assert_eq!(values("orders", "status"), Some(strings(&["pending", "paid", "shipped", "cancelled"])));
    let st = structure_of(&mut m, &t.name).await.unwrap();
    let mut found = check_values(Dialect::MySql(MySqlMode::default()), &st.checks);
    found.sort_by(|a, b| a.column.cmp(&b.column));
    let v = |column: &str, xs: &[&str]| CheckValues { column: column.into(), values: strings(xs) };
    assert_eq!(found, [v("st", &["p", "it's", "b\\c"]), v("u", &["k", "m"])], "{:?}", st.checks);
}

/// What MySQL keeps of expressions with string literals (a check, a generated column, a
/// functional key, an expression default), keys of other kinds (spatial, invisible, a unique
/// prefix) and an `ON UPDATE` column that is invisible read back as SQL: each definition runs on
/// a copy of the table, which then behaves as the table does.
#[tokio::test(flavor = "multi_thread")]
async fn definitions_with_literals_and_other_keys_run_back() {
    use datarig_core::driver::structure::*;
    let test = "definitions_with_literals_and_other_keys_run_back";
    let Some((url, admin)) = urls(test) else { return };
    let pid = std::process::id();
    let t = Table::make(
        &admin,
        "literals",
        &[&format!(
            r"CREATE TABLE {{t}} (id INT PRIMARY KEY, s VARCHAR(20) DEFAULT (concat('a', '''b')),
               g VARCHAR(40) GENERATED ALWAYS AS (concat(s, 'x''y')) VIRTUAL, QtY INT, b TEXT,
               pt POINT NOT NULL SRID 4326,
               ts TIMESTAMP NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP INVISIBLE,
               KEY fx ((concat(s, 'q''r'))), UNIQUE KEY uq_pre (b(10)), SPATIAL KEY sp (pt), KEY inv (QtY) INVISIBLE,
               CONSTRAINT zz_it_lit_ck_{pid} CHECK (s <> 'a''b\\c' AND qty > 0))"
        )],
    )
    .await;
    let mut m = Conn::start(&url, SessionRole::Meta, false, test);
    m.wait(|e| matches!(e, DbEvent::Catalog(_)), 30).await;
    let s = structure_of(&mut m, &t.name).await.unwrap();
    let column = |name: &str| s.columns.iter().find(|c| c.name == name).unwrap();
    assert_eq!(column("s").default.as_deref(), Some(r"(concat(_utf8mb4'a',_utf8mb4'\'b'))"));
    assert_eq!(column("g").fill, ColumnFill::Virtual(r"concat(`s`,_utf8mb4'x\'y')".into()));
    assert_eq!(column("ts").default.as_deref(), Some("CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP"));
    let index = |name: &str| s.indexes.iter().find(|x| x.name == name).unwrap();
    assert_eq!(index("fx").columns, [r"(concat(`s`,_utf8mb4'q\'r'))"]);
    assert!(index("uq_pre").unique && !index("uq_pre").constraint);
    assert!(s.unique_constraints.is_empty(), "a unique prefix is no unique column");
    assert_eq!(index("sp").columns, ["pt"]);
    assert!(index("inv").definition.ends_with(" INVISIBLE"), "{}", index("inv").definition);
    let ck = &s.checks[0];
    assert_eq!(ck.expression, r"(`s` <> _utf8mb4'a\'b\\c') and (`qty` > 0)");
    assert_eq!(ck.columns, ["s", "QtY"]);
    // The copy: its columns as the structure has them, then every index and check.
    let copy = Table::make(&admin, "literals_copy", &[]).await;
    let mut a = side(&admin).await;
    let generated = match &column("g").fill {
        ColumnFill::Virtual(e) => e.clone(),
        other => panic!("{other:?}"),
    };
    let create = format!(
        "CREATE TABLE {} (id INT PRIMARY KEY, s VARCHAR(20) DEFAULT {}, \
         g VARCHAR(40) GENERATED ALWAYS AS ({generated}) VIRTUAL, \
         QtY INT, b TEXT, pt POINT NOT NULL SRID 4326, ts TIMESTAMP NULL DEFAULT {})",
        copy.q(),
        column("s").default.as_deref().unwrap(),
        column("ts").default.as_deref().unwrap()
    );
    a.query_drop(&create).await.unwrap_or_else(|e| panic!("{create}: {e}"));
    for x in s.indexes.iter().filter(|x| !x.primary) {
        let sql = x.definition.replace(&t.q(), &copy.q());
        a.query_drop(&sql).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    for c in &s.checks {
        let sql = format!("ALTER TABLE {} ADD {}", copy.q(), c.definition.replace("zz_it_lit_", "zz_it_litc_"));
        a.query_drop(&sql).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    // It behaves as the table: the default, the generated value, the check on the literal.
    let pt = "ST_GeomFromText('POINT(1 1)', 4326)";
    a.query_drop(format!("INSERT INTO {} (id, QtY, pt) VALUES (1, 1, {pt})", copy.q())).await.unwrap();
    let row: Option<(String, String)> = a.query_first(format!("SELECT s, g FROM {}", copy.q())).await.unwrap();
    assert_eq!(row, Some(("a'b".to_string(), "a'bx'y".to_string())));
    let refused =
        a.query_drop(format!(r"INSERT INTO {} (id, s, QtY, pt) VALUES (2, 'a''b\\c', 1, {pt})", copy.q())).await;
    assert!(matches!(refused, Err(mysql_async::Error::Server(ref e)) if e.code == 3819), "{refused:?}");
    a.disconnect().await.unwrap();
}

/// A user whose name holds an `@`, with the `TRIGGER` privilege on a table without triggers:
/// the privilege is found (the grantee's host is after the last `@`), so the empty list is
/// none, not unknown.
#[tokio::test(flavor = "multi_thread")]
async fn a_user_name_with_an_at_sign_finds_its_trigger_privilege() {
    let test = "a_user_name_with_an_at_sign_finds_its_trigger_privilege";
    let Some((url, admin)) = urls(test) else { return };
    let t = numbers(&admin, "atsign", 10).await;
    let account = Account::make(&admin, "a@b", "BY 'pw'").await.unwrap();
    let mut a = side(&admin).await;
    a.query_drop(format!("GRANT TRIGGER ON {} TO '{}'@'%'", t.q(), account.name)).await.unwrap();
    a.disconnect().await.unwrap();
    let mut m = Conn::start(&as_user(&url, &account.name, "pw"), SessionRole::Meta, false, test);
    m.wait(|e| matches!(e, DbEvent::Catalog(_)), 30).await;
    let s = structure_of(&mut m, &t.name).await.unwrap();
    assert!(s.triggers.is_empty());
    assert!(s.hidden.is_empty(), "the privilege is the user's: {:?}", s.hidden);
}

// ── DDL ────────────────────────────────────────────────────────────────────

/// The user of the tests' URL (`DATARIG_TEST_MYSQL_URL`) as a definer.
const DEFINER: &str = "`datarig`@`%`";

/// The DDL the metadata session `m` reads of `object`.
async fn ddl_of(
    m: &mut Conn,
    object: datarig_core::driver::ddl::DdlObject,
) -> Result<datarig_core::driver::ddl::DdlSource, DbError> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    m.session.send(DbCommand::LoadDdl { id, object });
    match m.wait(|e| matches!(e, DbEvent::Ddl { id: i, .. } if *i == id), 15).await {
        DbEvent::Ddl { result, .. } => result,
        _ => unreachable!(),
    }
}

/// The name and text of a DDL the server wrote.
fn verbatim(r: Result<datarig_core::driver::ddl::DdlSource, DbError>) -> (String, String) {
    match r {
        Ok(datarig_core::driver::ddl::DdlSource::Verbatim { name, text, .. }) => (name, text),
        other => panic!("not the server's text: {other:?}"),
    }
}

/// Run DDL text `text` as the editor runs it (the statements its splitter finds, `DELIMITER`
/// lines read by it and not sent) on query session `q`: each must succeed.
async fn replay(q: &mut Conn, id: &mut u64, text: &str) {
    let mysql = Dialect::MySql(MySqlMode::default());
    let statements = datarig_core::sql::split::split_in(text, mysql);
    assert!(!statements.is_empty(), "{text}");
    for s in statements {
        *id += 1;
        q.ok(*id, s.body(text)).await;
    }
}

/// The DDL of each kind of object, the server's own text: a table's and a view's (`SHOW CREATE
/// TABLE`, ended with `;`), a trigger's, a procedure's, a function's and an event's (between
/// `DELIMITER` lines), named `database.object`; a name typed bare, qualified or quoted is looked
/// up in the session's database or the one it names. The text of the table, view, trigger and
/// procedure runs again as the editor runs it (on copies with other names) and makes the same
/// objects. A name of a table and a procedure is ambiguous; one of nothing, an object that is
/// not there, is not found; an index and a trigger's function have no DDL of their own here.
#[tokio::test(flavor = "multi_thread")]
async fn the_metadata_session_shows_the_servers_ddl() {
    use datarig_core::driver::ddl::{DdlObject, ObjectKind};
    let test = "the_metadata_session_shows_the_servers_ddl";
    let Some((url, admin)) = urls(test) else { return };
    let b = format!("zz_it_ddl_{}", std::process::id());
    // Dropped also when the test fails (a table's triggers with it). The views are in `datarig`:
    // other tests list the views of `shop`.
    let mut guards = Vec::new();
    for (what, name) in [
        ("TABLE", format!("shop.{b}")),
        ("TABLE", format!("shop.{b}c")),
        ("VIEW", format!("datarig.{b}_v")),
        ("VIEW", format!("datarig.{b}_vc")),
        ("PROCEDURE", format!("shop.{b}")),
        ("PROCEDURE", format!("shop.{b}_p")),
        ("PROCEDURE", format!("shop.{b}_pc")),
        ("PROCEDURE", format!("shop.{b}_h")),
        ("FUNCTION", format!("shop.{b}_f")),
        ("EVENT", format!("shop.{b}_e")),
    ] {
        guards.push(Cleanup { url: admin.clone(), sql: format!("DROP {what} IF EXISTS {name}") });
    }
    let mut a = side(&admin).await;
    for sql in [
        format!(
            "CREATE TABLE shop.{b} (id INT NOT NULL AUTO_INCREMENT PRIMARY KEY, \
             name VARCHAR(40) NOT NULL DEFAULT 'a;b' COMMENT 'it''s; \"quoted\"', \
             twice INT GENERATED ALWAYS AS (CHAR_LENGTH(name) * 2) STORED, \
             KEY ix_name (name(8), id DESC)) COMMENT 'x;y'"
        ),
        format!("CREATE VIEW datarig.{b}_v AS SELECT id, CONCAT(name, ';') AS n FROM shop.{b} WHERE name <> ';'"),
        format!(
            "CREATE TRIGGER shop.{b}_bi BEFORE INSERT ON shop.{b} FOR EACH ROW BEGIN \
             SET NEW.name = CONCAT(NEW.name, ';;'); -- a comment; with a ;\n SET NEW.name = UPPER(NEW.name); END"
        ),
        // The session's user is their definer: it may see their text.
        format!("CREATE DEFINER = {DEFINER} PROCEDURE shop.{b}_p(IN a INT) BEGIN SELECT a; SELECT ';', a + 1; END"),
        format!(
            "CREATE DEFINER = {DEFINER} FUNCTION shop.{b}_f(a INT) RETURNS INT DETERMINISTIC BEGIN RETURN a + 1; END"
        ),
        format!("CREATE EVENT shop.{b}_e ON SCHEDULE EVERY 1 DAY DISABLE DO BEGIN SELECT 1; END"),
        format!("CREATE PROCEDURE shop.{b}() SELECT 1"),
        // Another's: the session's user may run it, not see it.
        format!("CREATE PROCEDURE shop.{b}_h() SELECT 1"),
    ] {
        a.query_drop(sql).await.unwrap();
    }
    a.disconnect().await.unwrap();
    let mut m = Conn::start(&url, SessionRole::Meta, false, test);
    m.wait(|e| matches!(e, DbEvent::Catalog(_)), 30).await;
    let shop = || "shop".to_string();
    let named =
        |name: &str, schema: Option<&str>| DdlObject::Named { name: name.into(), schema: schema.map(Into::into) };

    let (name, table) = verbatim(ddl_of(&mut m, DdlObject::Relation { schema: shop(), name: b.clone() }).await);
    assert_eq!(name, format!("shop.{b}"));
    assert!(table.starts_with(&format!("CREATE TABLE `{b}` (\n")), "{table}");
    assert!(table.ends_with(" COMMENT='x;y';\n"), "{table}");
    assert!(table.contains("KEY `ix_name` (`name`(8),`id` DESC)"), "{table}");
    let datarig = || "datarig".to_string();
    let (name, view) =
        verbatim(ddl_of(&mut m, DdlObject::Relation { schema: datarig(), name: format!("{b}_v") }).await);
    assert_eq!(name, format!("datarig.{b}_v"));
    assert!(view.starts_with("CREATE ALGORITHM=UNDEFINED DEFINER="), "{view}");
    // Read in its own database: its own name bare, the table of another database qualified.
    assert!(view.contains(&format!(" VIEW `{b}_v` AS select ")) && view.ends_with(";\n"), "{view}");
    assert!(view.contains(&format!(" from `shop`.`{b}` ")), "{view}");
    let trigger = DdlObject::Trigger { schema: shop(), table: b.clone(), name: format!("{b}_bi") };
    let (name, trig) = verbatim(ddl_of(&mut m, trigger).await);
    assert_eq!(name, format!("shop.{b}.{b}_bi"), "a trigger is named with its table");
    assert!(trig.starts_with("DELIMITER ;;\nCREATE DEFINER="), "{trig}");
    assert!(trig.contains(&format!(" TRIGGER `{b}_bi` BEFORE INSERT ON `{b}` FOR EACH ROW BEGIN")), "{trig}");
    assert!(trig.ends_with(" END\n;;\nDELIMITER ;\n"), "{trig}");
    let (name, proc) = verbatim(ddl_of(&mut m, named(&format!("{b}_p"), Some("shop"))).await);
    assert_eq!(name, format!("shop.{b}_p()"), "a routine is named as one");
    assert!(proc.starts_with("DELIMITER ;;\nCREATE DEFINER=") && proc.ends_with(" END\n;;\nDELIMITER ;\n"), "{proc}");
    assert!(proc.contains(&format!(" PROCEDURE `{b}_p`(IN a INT)")), "{proc}");
    let (name, func) =
        verbatim(ddl_of(&mut m, named(&format!("shop.{}", format!("{b}_f").to_uppercase()), None)).await);
    assert_eq!(name, format!("shop.{b}_f()"), "a routine's name is the same in any case");
    assert!(func.contains(&format!(" FUNCTION `{b}_f`(a INT) RETURNS int")), "{func}");
    let (name, event) = verbatim(ddl_of(&mut m, named(&format!("`{b}_e`"), None)).await);
    assert_eq!(name, format!("shop.{b}_e (event)"), "in the database the session opened in");
    assert!(
        event.contains(&format!(" EVENT `{b}_e` ON SCHEDULE EVERY 1 DAY")) && event.ends_with(";;\nDELIMITER ;\n"),
        "{event}"
    );
    // The same objects through `:ddl name`.
    assert_eq!(verbatim(ddl_of(&mut m, named(&format!("{b}_v"), Some("datarig"))).await).1, view);
    // A table and a view are one kind to a kind word.
    assert_eq!(verbatim(ddl_of(&mut m, named(&format!("table {b}_v"), Some("datarig"))).await).1, view);
    assert_eq!(verbatim(ddl_of(&mut m, named(&format!("view {b}"), Some("shop"))).await).1, table);
    assert_eq!(verbatim(ddl_of(&mut m, named(&format!("`shop`.`{b}_bi`"), None)).await).1, trig);
    let elsewhere = ddl_of(&mut m, named(&format!("{b}_p"), Some("datarig"))).await;
    assert_eq!(elsewhere, Err(DbError::NotFound), "looked up in the database it is given");
    // Ambiguous, not there, none of its own.
    assert_eq!(
        ddl_of(&mut m, named(&b, Some("shop"))).await,
        Err(DbError::Ambiguous(vec![ObjectKind::Table, ObjectKind::Procedure]))
    );
    assert_eq!(ddl_of(&mut m, named(&format!("{b}_h"), Some("shop"))).await, Err(DbError::DefinitionHidden));
    for object in [
        named("zz_it_nothing_here", Some("shop")),
        named("zz_it_no_db.x", None),
        DdlObject::Relation { schema: shop(), name: "zz_it_nothing_here".into() },
        DdlObject::Trigger { schema: shop(), table: b.clone(), name: "zz_it_nothing_here".into() },
    ] {
        assert_eq!(ddl_of(&mut m, object.clone()).await, Err(DbError::NotFound), "{object:?}");
    }
    assert_eq!(ddl_of(&mut m, named("a.b.c", None)).await, Err(DbError::NotAName));
    // A database the user may not see: the server says so, as it would of one that is not there.
    let denied = ddl_of(&mut m, DdlObject::Relation { schema: "zz_it_no_db".into(), name: "x".into() }).await;
    assert!(matches!(&denied, Err(DbError::Server(e)) if e.starts_with("ERROR 1044 ")), "{denied:?}");
    for object in [
        DdlObject::Index { schema: shop(), name: "ix_name".into() },
        DdlObject::TriggerFunction { schema: shop(), table: b.clone(), trigger: format!("{b}_bi") },
    ] {
        assert_eq!(ddl_of(&mut m, object).await, Err(DbError::NotSupported));
    }

    // Run again under other names, as the editor runs it: the same objects.
    let mut q = Conn::open(&in_database(&admin, "shop"), SessionRole::Query, false, test).await;
    let mut id = 0;
    let rename = |text: &str, from: &[(String, String)]| {
        from.iter().fold(text.to_string(), |t, (a, b)| t.replace(&format!("`{a}`"), &format!("`{b}`")))
    };
    let copies = [
        (b.clone(), format!("{b}c")),
        (format!("{b}_v"), format!("{b}_vc")),
        (format!("{b}_bi"), format!("{b}_bic")),
        (format!("{b}_p"), format!("{b}_pc")),
    ];
    // Each in its own database, as `o` opens a console.
    for text in [&table, &trig, &proc] {
        replay(&mut q, &mut id, &rename(text, &copies)).await;
    }
    let mut qd = Conn::open(&in_database(&admin, "datarig"), SessionRole::Query, false, test).await;
    replay(&mut qd, &mut id, &rename(&view, &copies)).await;
    let again = |name: String| DdlObject::Relation { schema: shop(), name };
    assert_eq!(verbatim(ddl_of(&mut m, again(format!("{b}c"))).await).1, rename(&table, &copies));
    let view_copy = DdlObject::Relation { schema: datarig(), name: format!("{b}_vc") };
    assert_eq!(verbatim(ddl_of(&mut m, view_copy).await).1, rename(&view, &copies));
    let trigger = DdlObject::Trigger { schema: shop(), table: format!("{b}c"), name: format!("{b}_bic") };
    assert_eq!(verbatim(ddl_of(&mut m, trigger).await).1, rename(&trig, &copies));
    assert_eq!(verbatim(ddl_of(&mut m, named(&format!("{b}_pc"), Some("shop"))).await).1, rename(&proc, &copies));
    // The copied trigger and procedure work.
    q.ok(100, &format!("INSERT INTO {b}c (name) VALUES ('x')")).await;
    let evs = q.run(101, &[&format!("SELECT name FROM {b}c")]).await;
    assert_eq!(page_of(&evs).1[0][0].as_deref(), Some("X;;"));
}

/// A DDL read behind another session's metadata lock never holds that session up and keeps no
/// metadata lock once it has answered. `SHOW CREATE TABLE` of a table, of a view over it and of a
/// view over that view (and a view by name) answers with the DDL without waiting for any lock,
/// whether the table is held with `LOCK TABLES … WRITE` or an `ALTER TABLE` waits behind a held
/// result's shared lock. `SHOW CREATE TRIGGER` opens the trigger's table: it answers under
/// `LOCK TABLES`, and is `Locked` after the session's lock wait behind the waiting `ALTER`, which
/// still waits after it (as the server shows them, [`watched`]).
#[tokio::test(flavor = "multi_thread")]
async fn a_ddl_read_never_queues_behind_a_metadata_lock_nor_keeps_one() {
    use datarig_core::driver::ddl::DdlObject;
    let test = "a_ddl_read_never_queues_behind_a_metadata_lock_nor_keeps_one";
    let Some((url, admin)) = urls(test) else { return };
    let t = numbers(&admin, "ddllock", 200_000).await;
    let (view, view2, trigger) = (format!("{}_v", t.name), format!("{}_vv", t.name), format!("{}_bi", t.name));
    // The views are in `datarig`: other tests list the views of `shop`.
    let _view2 = Cleanup { url: admin.clone(), sql: format!("DROP VIEW IF EXISTS datarig.{view2}") };
    let _view = Cleanup { url: admin.clone(), sql: format!("DROP VIEW IF EXISTS datarig.{view}") };
    let mut wide = side(&admin).await;
    wide.query_drop(format!("ALTER TABLE {} MODIFY v VARCHAR(200)", t.q())).await.unwrap();
    wide.query_drop(format!("UPDATE {} SET v = REPEAT('x', 200)", t.q())).await.unwrap();
    wide.query_drop(format!("CREATE VIEW datarig.{view} AS SELECT id FROM {}", t.q())).await.unwrap();
    wide.query_drop(format!("CREATE VIEW datarig.{view2} AS SELECT id FROM datarig.{view}")).await.unwrap();
    let body = "BEGIN SET NEW.v = CONCAT(NEW.v, ';'); END";
    wide.query_drop(format!("CREATE TRIGGER shop.{trigger} BEFORE INSERT ON {} FOR EACH ROW {body}", t.q()))
        .await
        .unwrap();
    wide.disconnect().await.unwrap();
    let mut m = Conn::start(&url, SessionRole::Meta, false, test);
    m.wait(|e| matches!(e, DbEvent::Catalog(_)), 30).await;
    let mut side_admin = side(&admin).await;
    let meta = only_session(&mut side_admin, SessionRole::Meta, test).await;
    let relation = |schema: &str, name: &str| DdlObject::Relation { schema: schema.into(), name: name.into() };
    let trigger = DdlObject::Trigger { schema: "shop".into(), table: t.name.clone(), name: trigger.clone() };
    let relations = [
        relation("shop", &t.name),
        relation("datarig", &view),
        relation("datarig", &view2),
        DdlObject::Named { name: view.clone(), schema: Some("datarig".into()) },
    ];
    /// Read each of `objects`: each answers with its DDL, or (`locked`) `Locked`; no metadata
    /// lock stays.
    async fn reads(m: &mut Conn, admin: &mut mysql_async::Conn, meta: u64, objects: &[DdlObject], locked: bool) {
        for object in objects {
            let r = ddl_of(m, object.clone()).await;
            if locked {
                assert_eq!(r, Err(DbError::Locked), "{object:?}");
            } else {
                assert!(verbatim(r).1.contains("CREATE "), "{object:?}");
            }
            assert_eq!(metadata_locks_of(admin, meta).await, 0, "{object:?}: no lock kept");
        }
    }
    // 1. Another session holds `LOCK TABLES … WRITE`, and writes after the reads.
    let mut lock = side(&admin).await;
    lock.query_drop(format!("LOCK TABLES {} WRITE", t.q())).await.unwrap();
    let locker = u64::from(lock.id());
    let ((), seen) = watched(&admin, meta, &[locker], &t.name, async {
        reads(&mut m, &mut side_admin, meta, &relations, false).await;
        reads(&mut m, &mut side_admin, meta, std::slice::from_ref(&trigger), false).await;
        lock.query_drop(format!("INSERT INTO {} VALUES (999999, 'w')", t.q())).await.unwrap();
    })
    .await;
    assert!(seen.held_up.is_empty(), "the locking session waited for a read: {:?}", seen.held_up);
    assert!(seen.queued.is_empty(), "a read waited for a lock: {:?}", seen.queued);
    lock.query_drop("UNLOCK TABLES").await.unwrap();
    // 2. A held result keeps a shared lock and an `ALTER TABLE` waits for it.
    let mut q = Conn::open(&url, SessionRole::Query, false, test).await;
    let evs = q.run_as(1, &[&format!("SELECT * FROM {}", t.q())], PagingMode::Hold).await;
    assert!(page_of(&evs).2);
    let (alter_id, alter) = waiting_alter(&admin, &t.q()).await;
    // `SHOW CREATE TABLE` takes a high-priority shared lock for the moment it reads, granted past
    // the waiting ALTER (which waits for the held result whatever the read does) and gone once it
    // has answered: the read never queues, keeps nothing, and the only lock of its the ALTER can
    // be seen behind is that one, never a shared read or write lock or a stronger one.
    let ((), seen) = watched(&admin, meta, &[alter_id], &t.name, async {
        reads(&mut m, &mut side_admin, meta, &relations, false).await;
    })
    .await;
    assert!(
        seen.held_up.iter().all(|w| w.ends_with(": ours SHARED_HIGH_PRIO GRANTED")),
        "the ALTER waited for a lock of a read: {:?}",
        seen.held_up
    );
    assert!(seen.queued.is_empty(), "a read queued behind the ALTER: {:?}", seen.queued);
    let ((), seen) = watched(&admin, meta, &[alter_id], &t.name, async {
        reads(&mut m, &mut side_admin, meta, std::slice::from_ref(&trigger), true).await;
    })
    .await;
    // The trigger's read waits its lock wait (2 s) and gives up; the ALTER never waits for it.
    assert!(seen.held_up.is_empty(), "the ALTER waited for the trigger's read: {:?}", seen.held_up);
    assert!(alter_waits(&mut side_admin, alter_id).await, "the ALTER still waits, for the held result only");
    // The ALTER goes through once the held result is stopped: the reads held nothing up (the
    // limit only guards against a hang).
    let t1 = Instant::now();
    q.session.send(DbCommand::ClosePortal { id: 1 });
    q.ok(2, "SELECT 1").await;
    assert!(alter.await.unwrap().is_ok());
    assert!(t1.elapsed() < Duration::from_secs(10), "{:?}", t1.elapsed());
    // Read again once nothing is in the way: the new column is there, the trigger is read.
    let ddl = ddl_of(&mut m, relations[0].clone()).await;
    assert!(verbatim(ddl).1.contains("`c9` int DEFAULT NULL"));
    assert!(verbatim(ddl_of(&mut m, trigger).await).1.contains("CREATE "));
    side_admin.disconnect().await.unwrap();
}

/// A user without the privileges to see an object's definition hears the server's own message
/// (a trigger's needs `TRIGGER`), or that the definition is hidden from it (a routine it may run
/// but not see).
#[tokio::test(flavor = "multi_thread")]
async fn a_ddl_the_user_may_not_see_says_why() {
    use datarig_core::driver::ddl::DdlObject;
    let test = "a_ddl_the_user_may_not_see_says_why";
    let Some((url, admin)) = urls(test) else { return };
    let t = Table::make(&admin, "ddlpriv", &["CREATE TABLE {t} (id INT PRIMARY KEY)"]).await;
    let (trigger, proc) = (format!("{}_bi", t.name), format!("{}_p", t.name));
    let _proc = Cleanup { url: admin.clone(), sql: format!("DROP PROCEDURE IF EXISTS shop.{proc}") };
    let mut a = side(&admin).await;
    a.query_drop(format!("CREATE TRIGGER shop.{trigger} BEFORE INSERT ON {} FOR EACH ROW SET NEW.id = NEW.id", t.q()))
        .await
        .unwrap();
    a.query_drop(format!("CREATE PROCEDURE shop.{proc}() SELECT 1")).await.unwrap();
    let account = Account::make(&admin, "ddlpriv", "BY 'pw'").await.unwrap();
    a.query_drop(format!("GRANT EXECUTE ON PROCEDURE shop.{proc} TO '{}'@'%'", account.name)).await.unwrap();
    a.disconnect().await.unwrap();
    let mut m = Conn::start(&as_user(&url, &account.name, "pw"), SessionRole::Meta, false, test);
    m.wait(|e| matches!(e, DbEvent::Catalog(_)), 30).await;
    let r = ddl_of(&mut m, DdlObject::Trigger { schema: "shop".into(), table: t.name.clone(), name: trigger }).await;
    assert!(matches!(&r, Err(DbError::Server(e)) if e.contains("the TRIGGER privilege")), "{r:?}");
    let r = ddl_of(&mut m, DdlObject::Named { name: proc.clone(), schema: None }).await;
    assert_eq!(r, Err(DbError::DefinitionHidden));
    // Its table it may read: shown.
    let r = ddl_of(&mut m, DdlObject::Relation { schema: "shop".into(), name: t.name.clone() }).await;
    assert!(verbatim(r).1.starts_with("CREATE TABLE "));
}

/// A trigger's or a routine's body of one statement may end in a comment (`-- note`, `# note`): the
/// server keeps it when the client sends it (not after a compound body's `END`). The DDL ends the
/// statement on a line of its own, so the comment does not swallow the end, and the text runs again
/// as the editor runs it (on copies) and makes the same objects (without that last comment, which
/// the editor does not send).
#[tokio::test(flavor = "multi_thread")]
async fn a_body_ending_in_a_comment_runs_again() {
    use datarig_core::driver::ddl::DdlObject;
    let test = "a_body_ending_in_a_comment_runs_again";
    let Some((url, admin)) = urls(test) else { return };
    let t = Table::make(&admin, "ddlnote", &["CREATE TABLE {t} (id INT PRIMARY KEY, v VARCHAR(20))"]).await;
    let (trigger, proc) = (format!("{}_bi", t.name), format!("{}_p", t.name));
    let _procs = [proc.clone(), format!("{proc}c")]
        .map(|p| Cleanup { url: admin.clone(), sql: format!("DROP PROCEDURE IF EXISTS shop.{p}") });
    // The query session sends the text as it is, comments too.
    let mut q = Conn::open(&in_database(&admin, "shop"), SessionRole::Query, false, test).await;
    let made_trigger =
        format!("CREATE TRIGGER {trigger} BEFORE INSERT ON {} FOR EACH ROW SET NEW.v = 'x;' -- note", t.name);
    q.ok(1, &made_trigger).await;
    q.ok(2, &format!("CREATE DEFINER = {DEFINER} PROCEDURE {proc}() SELECT 1 # note")).await;
    let mut m = Conn::start(&url, SessionRole::Meta, false, test);
    m.wait(|e| matches!(e, DbEvent::Catalog(_)), 30).await;
    let of_trigger = |name: String| DdlObject::Trigger { schema: "shop".into(), table: t.name.clone(), name };
    let of_proc = |name: String| DdlObject::Named { name, schema: Some("shop".into()) };
    let (_, trig) = verbatim(ddl_of(&mut m, of_trigger(trigger.clone())).await);
    assert!(trig.ends_with(" SET NEW.v = 'x;' -- note\n;;\nDELIMITER ;\n"), "{trig}");
    let (_, routine) = verbatim(ddl_of(&mut m, of_proc(proc.clone())).await);
    assert!(routine.ends_with("\nSELECT 1 # note\n;;\nDELIMITER ;\n"), "{routine}");
    let mut id = 10;
    let copy = |text: &str, name: &str| text.replace(&format!("`{name}`"), &format!("`{name}c`"));
    replay(&mut q, &mut id, &copy(&trig, &trigger)).await;
    replay(&mut q, &mut id, &copy(&routine, &proc)).await;
    // The same objects; the editor sends a statement without its last comment.
    let (trig_copy, routine_copy) = (copy(&trig, &trigger), copy(&routine, &proc));
    let trig_copy = trig_copy.replace(" -- note\n", "\n");
    assert_eq!(verbatim(ddl_of(&mut m, of_trigger(format!("{trigger}c"))).await).1, trig_copy);
    let routine_copy = routine_copy.replace(" # note\n", "\n");
    assert_eq!(verbatim(ddl_of(&mut m, of_proc(format!("{proc}c"))).await).1, routine_copy);
}

/// A routine made under another `sql_mode` than the metadata session's (here
/// `NO_BACKSLASH_ESCAPES`, which changes how its strings read) says so in a comment line first;
/// one made under the session's mode does not.
#[tokio::test(flavor = "multi_thread")]
async fn a_routines_own_sql_mode_is_said_when_it_differs() {
    use datarig_core::driver::ddl::DdlObject;
    let test = "a_routines_own_sql_mode_is_said_when_it_differs";
    let Some((url, admin)) = urls(test) else { return };
    let (odd, plain) =
        (format!("zz_it_ddlmode_{}", std::process::id()), format!("zz_it_ddlmode_{}_d", std::process::id()));
    let _procs = [odd.clone(), plain.clone()]
        .map(|p| Cleanup { url: admin.clone(), sql: format!("DROP PROCEDURE IF EXISTS shop.{p}") });
    let mut q = Conn::open(&in_database(&admin, "shop"), SessionRole::Query, false, test).await;
    q.ok(1, &format!("CREATE DEFINER = {DEFINER} PROCEDURE {plain}() SELECT 'a\\\\b'")).await;
    q.ok(2, "SET SESSION sql_mode = CONCAT(@@SESSION.sql_mode, ',NO_BACKSLASH_ESCAPES')").await;
    q.ok(3, &format!("CREATE DEFINER = {DEFINER} PROCEDURE {odd}() SELECT 'a\\b'")).await;
    let mut m = Conn::start(&url, SessionRole::Meta, false, test);
    m.wait(|e| matches!(e, DbEvent::Catalog(_)), 30).await;
    let of = |name: &String| DdlObject::Named { name: name.clone(), schema: Some("shop".into()) };
    let (_, text) = verbatim(ddl_of(&mut m, of(&odd)).await);
    let (first, rest) = text.split_once('\n').unwrap();
    assert!(first.starts_with("-- sql_mode: ") && first.contains("NO_BACKSLASH_ESCAPES"), "{text}");
    assert!(rest.starts_with("DELIMITER ;;\n"), "{text}");
    let (_, text) = verbatim(ddl_of(&mut m, of(&plain)).await);
    assert!(text.starts_with("DELIMITER ;;\n"), "{text}");
}

/// `:ddl kind name` picks the object of that kind when the name names several; a kind it does not
/// name is not found. A name with a character MySQL never has in a name (above U+FFFF) names
/// nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_kind_word_picks_one_object_of_a_shared_name() {
    use datarig_core::driver::ddl::{DdlObject, ObjectKind};
    let test = "a_kind_word_picks_one_object_of_a_shared_name";
    let Some((url, admin)) = urls(test) else { return };
    let t = Table::make(&admin, "ddlkind", &["CREATE TABLE {t} (id INT PRIMARY KEY)"]).await;
    let _proc = Cleanup { url: admin.clone(), sql: format!("DROP PROCEDURE IF EXISTS shop.{}", t.name) };
    let mut a = side(&admin).await;
    a.query_drop(format!("CREATE DEFINER = {DEFINER} PROCEDURE {}() SELECT 1", t.q())).await.unwrap();
    a.disconnect().await.unwrap();
    let mut m = Conn::start(&url, SessionRole::Meta, false, test);
    m.wait(|e| matches!(e, DbEvent::Catalog(_)), 30).await;
    let named = |name: String| DdlObject::Named { name, schema: Some("shop".into()) };
    let n = &t.name;
    assert_eq!(
        ddl_of(&mut m, named(n.clone())).await,
        Err(DbError::Ambiguous(vec![ObjectKind::Table, ObjectKind::Procedure]))
    );
    let (name, text) = verbatim(ddl_of(&mut m, named(format!("procedure {n}"))).await);
    assert_eq!(name, format!("shop.{n}()"));
    assert!(text.contains(&format!(" PROCEDURE `{n}`()")), "{text}");
    let (_, text) = verbatim(ddl_of(&mut m, named(format!("TABLE shop.`{n}`"))).await);
    assert!(text.starts_with(&format!("CREATE TABLE `{n}` (")), "{text}");
    // A table and a view are one kind to a kind word: one statement reads both.
    let (_, as_view) = verbatim(ddl_of(&mut m, named(format!("view {n}"))).await);
    assert!(as_view.starts_with(&format!("CREATE TABLE `{n}` (")), "{as_view}");
    for kind in ["function", "trigger", "event"] {
        assert_eq!(ddl_of(&mut m, named(format!("{kind} {n}"))).await, Err(DbError::NotFound), "{kind}");
    }
    assert_eq!(ddl_of(&mut m, named("zz_it_\u{1F600}".into())).await, Err(DbError::NotFound));
}

/// A bare name on a session without a database says to name the database; a name of three parts
/// is not a name here.
#[tokio::test(flavor = "multi_thread")]
async fn a_name_that_cannot_be_looked_up_says_why() {
    use datarig_core::driver::ddl::DdlObject;
    let test = "a_name_that_cannot_be_looked_up_says_why";
    let Some(url) = my_url(test) else { return };
    let mut m = Conn::start(&in_database(&url, ""), SessionRole::Meta, false, test);
    m.wait(|e| matches!(e, DbEvent::Catalog(_)), 30).await;
    let named = |name: &str| DdlObject::Named { name: name.into(), schema: None };
    assert_eq!(ddl_of(&mut m, named("orders")).await, Err(DbError::NoDatabase));
    assert!(verbatim(ddl_of(&mut m, named("shop.orders")).await).1.starts_with("CREATE TABLE `orders`"));
    assert_eq!(ddl_of(&mut m, named("a.b.c")).await, Err(DbError::NotAName));
}

/// A trigger's DDL is `SHOW CREATE TRIGGER`'s text, as it was written (`information_schema`
/// keeps the statement with its strings unescaped): a body with `''` and `\\` in its strings runs
/// again on a copy, reads back the same, and the copy writes the same value.
#[tokio::test(flavor = "multi_thread")]
async fn a_triggers_strings_run_again_as_written() {
    use datarig_core::driver::ddl::DdlObject;
    let test = "a_triggers_strings_run_again_as_written";
    let Some((url, admin)) = urls(test) else { return };
    let ddl = ["CREATE TABLE {t} (id INT PRIMARY KEY, v VARCHAR(40))"];
    let (t, c) = (Table::make(&admin, "ddlesc", &ddl).await, Table::make(&admin, "ddlescc", &ddl).await);
    let (trigger, copy) = (format!("{}_bi", t.name), format!("{}_bi", c.name));
    let mut q = Conn::open(&in_database(&admin, "shop"), SessionRole::Query, false, test).await;
    let body = r"SET NEW.v = CONCAT('it''s', '\\', 'a\'b', NEW.v)";
    q.ok(1, &format!("CREATE TRIGGER {trigger} BEFORE INSERT ON {} FOR EACH ROW {body}", t.name)).await;
    let mut m = Conn::start(&url, SessionRole::Meta, false, test);
    m.wait(|e| matches!(e, DbEvent::Catalog(_)), 30).await;
    let of =
        |table: &str, name: &str| DdlObject::Trigger { schema: "shop".into(), table: table.into(), name: name.into() };
    let (_, text) = verbatim(ddl_of(&mut m, of(&t.name, &trigger)).await);
    assert!(text.contains(body), "as written: {text}");
    let renamed = text
        .replace(&format!("`{trigger}`"), &format!("`{copy}`"))
        .replace(&format!("`{}`", t.name), &format!("`{}`", c.name));
    let mut id = 10;
    replay(&mut q, &mut id, &renamed).await;
    assert_eq!(verbatim(ddl_of(&mut m, of(&c.name, &copy)).await).1, renamed);
    for table in [&t.name, &c.name] {
        q.ok(20, &format!("INSERT INTO {table} (id, v) VALUES (1, '!')")).await;
    }
    let value = |evs: Vec<DbEvent>| page_of(&evs).1[0][0].clone();
    let a = value(q.run(21, &[&format!("SELECT v FROM {}", t.name)]).await);
    let b = value(q.run(22, &[&format!("SELECT v FROM {}", c.name)]).await);
    assert_eq!(a.as_deref(), Some(r"it's\a'b!"));
    assert_eq!(a, b);
}

/// The text of the DDL `object`, the database it runs in and the dialect it is written in.
async fn ddl_written(m: &mut Conn, object: datarig_core::driver::ddl::DdlObject) -> (String, Option<String>, Dialect) {
    match ddl_of(m, object).await {
        Ok(datarig_core::driver::ddl::DdlSource::Verbatim { text, database, written_in: Some(d), .. }) => {
            (text, database, d)
        }
        other => panic!("not the server's text with its dialect: {other:?}"),
    }
}

/// `text` with each `` `a` `` of `names` written `` `b` ``.
fn renamed(text: &str, names: &[(&str, String)]) -> String {
    names.iter().fold(text.to_string(), |t, (a, b)| t.replace(&format!("`{a}`"), &format!("`{b}`")))
}

/// Strings with a backslash and a quote, in a table's defaults and check, a view's query (in
/// another database), a function's body and a trigger's, all made in the server's default mode:
/// opened in a console of either mode (`o`: the strings written as that console reads them,
/// `sql::requote`) and run there, each makes the same object. A table's and a view's DDL read back
/// the same; the function returns, and the trigger writes, the same values.
#[tokio::test(flavor = "multi_thread")]
async fn a_ddl_opened_in_a_console_of_either_mode_makes_the_same_object() {
    use datarig_core::driver::ddl::DdlObject;
    use datarig_core::sql::requote::requote_strings;
    let test = "a_ddl_opened_in_a_console_of_either_mode_makes_the_same_object";
    let Some((url, admin)) = urls(test) else { return };
    // A check's name is the database's: named after the table. An expression (an expression
    // default, a check) with a quote in a string cannot be made under NO_BACKSLASH_ESCAPES at all
    // (the server prints it with backslash escapes and reads that back in the session's mode),
    // so those have a backslash only.
    let ddl = format!(
        r"CREATE TABLE {{t}} (id INT PRIMARY KEY, v VARCHAR(40) DEFAULT 'a\\b''c',
        w VARCHAR(20) DEFAULT ('x\\y'), CONSTRAINT zz_it_ddlq_{}_chk CHECK (v <> 'q\\r'))",
        std::process::id()
    );
    let t = Table::make(&admin, "ddlq", &[&ddl]).await;
    let n = t.name.clone();
    let (view, func, trig) = (format!("{n}_v"), format!("{n}_f"), format!("{n}_bi"));
    let modes = [("e", false), ("n", true)];
    let mut guards = vec![
        Cleanup { url: admin.clone(), sql: format!("DROP VIEW IF EXISTS datarig.{view}") },
        Cleanup { url: admin.clone(), sql: format!("DROP FUNCTION IF EXISTS shop.{func}") },
    ];
    for (c, _) in modes {
        for sql in [
            format!("DROP TABLE IF EXISTS shop.{n}{c}"),
            format!("DROP VIEW IF EXISTS datarig.{view}{c}"),
            format!("DROP FUNCTION IF EXISTS shop.{func}{c}"),
        ] {
            guards.push(Cleanup { url: admin.clone(), sql });
        }
    }
    let mut a = side(&admin).await;
    for sql in [
        format!(r"CREATE VIEW datarig.{view} AS SELECT id, 'a\\b''c' AS x FROM shop.{n}"),
        format!(
            r"CREATE DEFINER = {DEFINER} FUNCTION shop.{func}() RETURNS VARCHAR(40) DETERMINISTIC
              RETURN CONCAT('a\\b', 'it''s')"
        ),
        format!(
            r"CREATE TRIGGER shop.{trig} BEFORE INSERT ON shop.{n} FOR EACH ROW SET NEW.v = CONCAT('p\\q''r', NEW.v)"
        ),
    ] {
        a.query_drop(sql).await.unwrap();
    }
    a.disconnect().await.unwrap();
    let mut m = Conn::start(&url, SessionRole::Meta, false, test);
    m.wait(|e| matches!(e, DbEvent::Catalog(_)), 30).await;
    let rel = |schema: &str, name: &str| DdlObject::Relation { schema: schema.into(), name: name.into() };
    let table = ddl_written(&mut m, rel("shop", &n)).await;
    let view_ddl = ddl_written(&mut m, rel("datarig", &view)).await;
    let func_ddl =
        ddl_written(&mut m, DdlObject::Named { name: format!("function {func}"), schema: Some("shop".into()) }).await;
    let trig_ddl =
        ddl_written(&mut m, DdlObject::Trigger { schema: "shop".into(), table: n.clone(), name: trig.clone() }).await;
    let mut id = 0;
    for (c, nbe) in modes {
        let console = Dialect::MySql(MySqlMode { no_backslash_escapes: nbe, ..MySqlMode::default() });
        let mut q = Conn::open(&in_database(&admin, "shop"), SessionRole::Query, false, test).await;
        let mut qd = Conn::open(&in_database(&admin, "datarig"), SessionRole::Query, false, test).await;
        if nbe {
            for s in [&mut q, &mut qd] {
                s.ok(1, "SET SESSION sql_mode = CONCAT(@@SESSION.sql_mode, ',NO_BACKSLASH_ESCAPES')").await;
            }
        }
        // As `o` opens each: in its own database, the strings as the console reads them.
        let open = |(text, database, from): &(String, Option<String>, Dialect)| {
            (requote_strings(text, *from, console).expect("requoted"), database.clone())
        };
        let (text, db) = open(&table);
        assert_eq!(db.as_deref(), Some("shop"));
        let table_names = [(n.as_str(), format!("{n}{c}")), (&format!("{n}_chk"), format!("{n}{c}_chk"))];
        replay(&mut q, &mut id, &renamed(&text, &table_names)).await;
        let (text, db) = open(&view_ddl);
        assert_eq!(db.as_deref(), Some("datarig"));
        replay(&mut qd, &mut id, &renamed(&text, &[(view.as_str(), format!("{view}{c}"))])).await;
        replay(&mut q, &mut id, &renamed(&open(&func_ddl).0, &[(func.as_str(), format!("{func}{c}"))])).await;
        let trig_names = [(n.as_str(), format!("{n}{c}")), (trig.as_str(), format!("{trig}{c}"))];
        replay(&mut q, &mut id, &renamed(&open(&trig_ddl).0, &trig_names)).await;
        // The table and the view read back as the originals do.
        let back = ddl_written(&mut m, rel("shop", &format!("{n}{c}"))).await.0;
        assert_eq!(
            renamed(&back, &[(&format!("{n}{c}"), n.clone()), (&format!("{n}{c}_chk"), format!("{n}_chk"))]),
            table.0,
            "{c}"
        );
        let back = ddl_written(&mut m, rel("datarig", &format!("{view}{c}"))).await.0;
        assert_eq!(renamed(&back, &[(&format!("{view}{c}"), view.clone())]), view_ddl.0, "{c}");
    }
    // The function returns, and the trigger and the defaults write, the same values.
    let mut q = Conn::open(&in_database(&admin, "shop"), SessionRole::Query, false, test).await;
    let one = |evs: Vec<DbEvent>| page_of(&evs).1[0].clone();
    let funcs = one(q.run(200, &[&format!("SELECT {func}(), {func}e(), {func}n()")]).await);
    assert_eq!(funcs, vec![Some(r"a\bit's".to_string()); 3]);
    let mut rows = Vec::new();
    for (i, table) in [n.clone(), format!("{n}e"), format!("{n}n")].iter().enumerate() {
        q.ok(210 + i as u64, &format!("INSERT INTO {table} (id) VALUES (1)")).await;
        rows.push(one(q.run(220 + i as u64, &[&format!("SELECT v, w FROM {table}")]).await));
    }
    assert_eq!(rows[0], vec![Some(r"p\q'ra\b'c".to_string()), Some(r"x\y".to_string())]);
    assert!(rows.iter().all(|r| *r == rows[0]), "{rows:?}");
    drop(guards);
}

/// An object of another database than the metadata session's: read with that database current,
/// so its text runs as written in a console there (where `o` opens it: the answer names it). The
/// session's other reads name their database and go on as before, and a name typed without one
/// is looked up in the database the request names, else the one the session opened in, never
/// the one it has current.
#[tokio::test(flavor = "multi_thread")]
async fn another_databases_ddl_runs_in_that_database() {
    use datarig_core::driver::ddl::DdlObject;
    let test = "another_databases_ddl_runs_in_that_database";
    let Some((url, admin)) = urls(test) else { return };
    let n = format!("zz_it_ddlother_{}", std::process::id());
    let (view, tc, vc) = (format!("{n}_v"), format!("{n}c"), format!("{n}_vc"));
    let _guards = [
        format!("VIEW IF EXISTS datarig.{vc}"),
        format!("VIEW IF EXISTS datarig.{view}"),
        format!("TABLE IF EXISTS datarig.{tc}"),
        format!("TABLE IF EXISTS datarig.{n}"),
    ]
    .map(|w| Cleanup { url: admin.clone(), sql: format!("DROP {w}") });
    let mut a = side(&admin).await;
    a.query_drop(format!("CREATE TABLE datarig.{n} (id INT PRIMARY KEY, v VARCHAR(9) DEFAULT 'x')")).await.unwrap();
    a.query_drop(format!("CREATE VIEW datarig.{view} AS SELECT id, v FROM datarig.{n}")).await.unwrap();
    a.disconnect().await.unwrap();
    let mut m = Conn::start(&url, SessionRole::Meta, false, test);
    m.wait(|e| matches!(e, DbEvent::Catalog(_)), 30).await;
    let rel = |name: &str| DdlObject::Relation { schema: "datarig".into(), name: name.into() };
    let (table, db, _) = ddl_written(&mut m, rel(&n)).await;
    assert_eq!(db.as_deref(), Some("datarig"));
    assert!(table.starts_with(&format!("CREATE TABLE `{n}` (")), "{table}");
    let (view_text, db, _) = ddl_written(&mut m, rel(&view)).await;
    assert_eq!(db.as_deref(), Some("datarig"));
    assert!(
        view_text.contains(&format!(" VIEW `{view}` AS select ")) && view_text.contains(&format!(" from `{n}`")),
        "{view_text}"
    );
    // Run in a console in that database, under other names: the same objects there.
    let mut q = Conn::open(&in_database(&admin, "datarig"), SessionRole::Query, false, test).await;
    let mut id = 0;
    let names = [(n.as_str(), tc.clone()), (view.as_str(), vc.clone())];
    replay(&mut q, &mut id, &renamed(&table, &names)).await;
    replay(&mut q, &mut id, &renamed(&view_text, &names)).await;
    assert_eq!(ddl_written(&mut m, rel(&tc)).await.0, renamed(&table, &names));
    assert_eq!(ddl_written(&mut m, rel(&vc)).await.0, renamed(&view_text, &names));
    // The session's other reads.
    m.session.send(DbCommand::LoadObjects { schema: "datarig".into() });
    match m.wait(|e| matches!(e, DbEvent::Objects { .. }), 15).await {
        DbEvent::Objects { result: Ok(o), .. } => {
            assert!(o.tables.contains(&n) && o.tables.contains(&tc), "{o:?}");
            assert!(o.views.contains(&view) && o.views.contains(&vc), "{o:?}");
        }
        other => panic!("{other:?}"),
    }
    m.session.send(DbCommand::LoadObjects { schema: "shop".into() });
    match m.wait(|e| matches!(e, DbEvent::Objects { .. }), 15).await {
        DbEvent::Objects { result: Ok(o), .. } => {
            assert!(o.tables.contains(&"users".to_string()) && !o.tables.contains(&n), "{o:?}")
        }
        other => panic!("{other:?}"),
    }
    m.session.send(DbCommand::LoadCatalog);
    match m.wait(|e| matches!(e, DbEvent::Catalog(_)), 15).await {
        DbEvent::Catalog(Ok(c)) => {
            assert!(c.relations.iter().any(|r| r.schema == "shop" && r.name == "users"));
            assert!(c.relations.iter().any(|r| r.schema == "datarig" && r.name == tc));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(structure_of(&mut m, "users").await.unwrap().columns[0].name, "id");
    // A typed name: in the database the request names, else the one the session opened in.
    let named =
        |name: &str, schema: Option<&str>| DdlObject::Named { name: name.into(), schema: schema.map(Into::into) };
    assert_eq!(ddl_written(&mut m, named(&n, Some("datarig"))).await.0, table);
    assert_eq!(ddl_of(&mut m, named(&n, None)).await, Err(DbError::NotFound), "not the current database's");
    assert!(ddl_written(&mut m, named("users", None)).await.0.starts_with("CREATE TABLE `users`"));
}

/// The key metadata `m` reads on `LoadKeys`.
async fn keys_of(m: &mut Conn) -> Result<datarig_core::driver::keys::KeyCatalog, DbError> {
    m.session.send(DbCommand::LoadKeys);
    match m.wait(|e| matches!(e, DbEvent::Keys(_)), 15).await {
        DbEvent::Keys(r) => r,
        _ => unreachable!(),
    }
}

/// The key metadata of every database: a primary key, a composite foreign key to another
/// database, unique keys (whole columns only: not a prefix, not an expression), generated
/// columns, `AUTO_INCREMENT`, views, and names compared as the server compares them.
#[tokio::test(flavor = "multi_thread")]
async fn the_metadata_session_reads_the_key_metadata() {
    use datarig_core::driver::keys::{Generated, KeyMarks, NameRule};
    let test = "the_metadata_session_reads_the_key_metadata";
    let Some((url, admin)) = urls(test) else { return };
    let pid = std::process::id();
    let parent = format!("zz_it_kparent_{pid}");
    let _parent = Cleanup { url: admin.clone(), sql: format!("DROP TABLE IF EXISTS datarig.{parent}") };
    let mut a = side(&admin).await;
    a.query_drop(format!("CREATE TABLE datarig.{parent} (a INT NOT NULL, b VARCHAR(10) NOT NULL, PRIMARY KEY (a, b))"))
        .await
        .unwrap();
    a.disconnect().await.unwrap();
    let t = Table::make(
        &admin,
        "keys",
        &[&format!(
            "CREATE TABLE {{t}} (id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY, pa INT, pb VARCHAR(10),
               Code VARCHAR(20), note TEXT, qty INT, twice INT GENERATED ALWAYS AS (qty * 2) STORED,
               low VARCHAR(20) GENERATED ALWAYS AS (lower(Code)) VIRTUAL,
               UNIQUE KEY uq_code (Code), UNIQUE KEY uq_note (note(10)), UNIQUE KEY uq_fn ((qty + 1)),
               CONSTRAINT zz_it_kfk_{pid} FOREIGN KEY (pa, pb) REFERENCES datarig.{parent} (a, b))"
        )],
    )
    .await;
    let view = Table::make(&admin, "kview", &[]).await;
    let _view = Cleanup { url: admin.clone(), sql: format!("DROP VIEW IF EXISTS {}", view.q()) };
    let mut a = side(&admin).await;
    a.query_drop(format!("CREATE VIEW {} AS SELECT id, Code FROM {}", view.q(), t.q())).await.unwrap();
    a.disconnect().await.unwrap();
    let mut m = Conn::start(&url, SessionRole::Meta, false, test);
    m.wait(|e| matches!(e, DbEvent::Keys(_)), 30).await;
    let k = keys_of(&mut m).await.unwrap();
    let marks = |c: &str| k.marks_by_name("shop", &t.name, c);
    let (pk, fk, uq, none) = (
        KeyMarks { pk: true, ..KeyMarks::default() },
        KeyMarks { fk: true, ..KeyMarks::default() },
        KeyMarks { unique: true, ..KeyMarks::default() },
        KeyMarks::default(),
    );
    assert_eq!([marks("id"), marks("pa"), marks("pb"), marks("Code")], [pk, fk, fk, uq]);
    assert_eq!([marks("note"), marks("qty")], [none, none], "a prefix or an expression proves no column unique");
    let table = k.find(Some("shop"), &t.name).unwrap();
    let generated: Vec<(&str, Generated)> = table.columns.values().map(|c| (c.name.as_str(), c.generated)).collect();
    assert_eq!(generated[0], ("id", Generated::AutoIncrement), "AUTO_INCREMENT takes a value, 0 the next number");
    assert!(
        generated.contains(&("twice", Generated::Expression)) && generated.contains(&("low", Generated::Expression))
    );
    assert!(!table.view);
    assert!(k.find(Some("shop"), &view.name).unwrap().view);
    assert!(k.marks_by_name("datarig", &parent, "a").pk);
    // As the server compares names: a column's in any case.
    assert!(k.marks_by_name("shop", &t.name, "CODE").unique);
    let mut probe = side(&admin).await;
    let lctn: u8 = probe.query_first("SELECT @@lower_case_table_names").await.unwrap().unwrap();
    probe.disconnect().await.unwrap();
    assert_eq!(k.names(), NameRule::MySql { tables_ignore_case: lctn != 0 });
    if lctn == 0 {
        assert!(k.find(Some("shop"), &t.name.to_uppercase()).is_err(), "a table's case matters");
    } else {
        assert!(k.find(Some("SHOP"), &t.name.to_uppercase()).is_ok(), "lower_case_table_names = {lctn}");
    }
}

/// Reading the key metadata never queues behind another session's metadata lock (held by
/// `LOCK TABLES … WRITE`, or asked for by an `ALTER TABLE` that waits behind a held result) nor
/// waits for any lock, and answers with the keys; no other session waits for a lock of its, and it
/// keeps none (as the server shows them, [`watched`]).
#[tokio::test(flavor = "multi_thread")]
async fn the_keys_read_never_queues_behind_a_metadata_lock_nor_keeps_one() {
    let test = "the_keys_read_never_queues_behind_a_metadata_lock_nor_keeps_one";
    let Some((url, admin)) = urls(test) else { return };
    let t = numbers(&admin, "keyslock", 200_000).await;
    let mut wide = side(&admin).await;
    wide.query_drop(format!("ALTER TABLE {} MODIFY v VARCHAR(200)", t.q())).await.unwrap();
    wide.query_drop(format!("UPDATE {} SET v = REPEAT('x', 200)", t.q())).await.unwrap();
    wide.disconnect().await.unwrap();
    let mut m = Conn::start(&url, SessionRole::Meta, false, test);
    m.wait(|e| matches!(e, DbEvent::Keys(_)), 30).await;
    let mut side_admin = side(&admin).await;
    let meta = only_session(&mut side_admin, SessionRole::Meta, test).await;
    let read = |k: Result<datarig_core::driver::keys::KeyCatalog, DbError>, name: &str| {
        let k = k.expect("the keys, never Locked");
        assert!(k.marks_by_name("shop", name, "id").pk);
    };
    // A guard, as for the structure: a new table out of the server's cache, and the read opens
    // nothing.
    let fresh = numbers(&admin, "keysopen", 10).await;
    side_admin.query_drop(format!("FLUSH TABLES {}", fresh.q())).await.unwrap();
    read(keys_of(&mut m).await, &t.name);
    assert!(!is_open(&mut side_admin, &fresh.name).await, "flushed");
    read(keys_of(&mut m).await, &fresh.name);
    assert!(!is_open(&mut side_admin, &fresh.name).await, "no table was opened");
    // 1. Another session holds `LOCK TABLES … WRITE`, and writes while the read runs.
    let mut lock = side(&admin).await;
    lock.query_drop(format!("LOCK TABLES {} WRITE", t.q())).await.unwrap();
    let locker = u64::from(lock.id());
    let (k, seen) = watched(&admin, meta, &[locker], &t.name, async {
        m.session.send(DbCommand::LoadKeys);
        lock.query_drop(format!("INSERT INTO {} VALUES (999999, 'w')", t.q())).await.unwrap();
        match m.wait(|e| matches!(e, DbEvent::Keys(_)), 10).await {
            DbEvent::Keys(k) => k,
            _ => unreachable!(),
        }
    })
    .await;
    assert!(seen.held_up.is_empty(), "the locking session waited for the read: {:?}", seen.held_up);
    assert!(seen.queued.is_empty(), "the read waited for a lock: {:?}", seen.queued);
    read(k, &t.name);
    assert_eq!(metadata_locks_of(&mut side_admin, meta).await, 0, "no lock kept");
    lock.query_drop("UNLOCK TABLES").await.unwrap();
    // 2. A held result keeps a shared lock and an `ALTER TABLE` waits for it.
    let mut q = Conn::open(&url, SessionRole::Query, false, test).await;
    let evs = q.run_as(1, &[&format!("SELECT * FROM {}", t.q())], PagingMode::Hold).await;
    assert!(page_of(&evs).2);
    let (alter_id, alter) = waiting_alter(&admin, &t.q()).await;
    let (k, seen) = watched(&admin, meta, &[alter_id], &t.name, keys_of(&mut m)).await;
    assert!(seen.held_up.is_empty(), "the ALTER waited for the read: {:?}", seen.held_up);
    assert!(seen.queued.is_empty(), "the read queued behind the ALTER: {:?}", seen.queued);
    read(k, &t.name);
    assert_eq!(metadata_locks_of(&mut side_admin, meta).await, 0, "no lock kept");
    assert!(alter_waits(&mut side_admin, alter_id).await, "the ALTER still waits for the held result");
    q.session.send(DbCommand::ClosePortal { id: 1 });
    q.ok(2, "SELECT 1").await;
    assert!(alter.await.unwrap().is_ok());
    side_admin.disconnect().await.unwrap();
}

/// Every column of `table` as one hash per row, by `id`: its values' bytes (a NULL apart).
async fn row_hashes(c: &mut mysql_async::Conn, table: &str, columns: &[&str]) -> Vec<(i64, String)> {
    let parts: Vec<String> = columns.iter().map(|c| format!("IFNULL(HEX(`{c}`), 'NULL')")).collect();
    let sql = format!("SELECT id, SHA2(CONCAT_WS('|', {}), 256) FROM {table} ORDER BY id", parts.join(", "));
    c.query(sql).await.unwrap()
}

/// The last [`SessionValues`](datarig_core::driver::SessionValues) and sql mode `evs` report.
fn reported(evs: &[DbEvent]) -> (Option<datarig_core::driver::SessionValues>, Option<MySqlMode>) {
    let values = evs.iter().rev().find_map(|e| match e {
        DbEvent::SessionValues(v) => Some(v.clone()),
        _ => None,
    });
    let mode = evs.iter().rev().find_map(|e| match e {
        DbEvent::Language(Language::Sql(Dialect::MySql(m))) => Some(*m),
        _ => None,
    });
    (values, mode)
}

/// The rows of `SELECT * FROM table` on query session `q`, copied as SQL by the app's own path
/// ([`export::sql_cells`], then [`insert_source`] and the writer for an INSERT, `update` true
/// [`update_source`] for an UPDATE), in the session's mode `mode` and values `values`; or the
/// column the copy refuses.
async fn copied_as_sql(
    q: &mut Conn,
    id: u64,
    keys: &datarig_core::driver::keys::KeyCatalog,
    table: &Table,
    mode: MySqlMode,
    values: &datarig_core::driver::SessionValues,
    update: bool,
) -> Result<String, datarig_core::export::Refused> {
    use datarig_core::driver::keys::{Generated, insert_source, update_source};
    use datarig_core::export::{self, Format, Kind, SqlColumn, Target, UpdateTarget, Writer};
    let sql = format!("SELECT * FROM `shop`.`{}`", table.name);
    let evs = q.run(id, &[&sql]).await;
    let (cols, rows, more) = page_of(&evs);
    assert!(!more);
    let d = Dialect::MySql(mode);
    let origins: Vec<_> = cols.iter().map(|c| c.origin.clone()).collect();
    let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
    let columns: Vec<export::Column> =
        cols.iter().map(|c| export::Column { name: &c.name, kind: Kind::of_column(c) }).collect();
    let sql_columns: Vec<SqlColumn> = cols
        .iter()
        .map(|c| SqlColumn {
            name: &c.name,
            type_name: &c.type_name,
            kind: c.kind,
            auto_increment: !update
                && c.origin
                    .as_ref()
                    .and_then(|o| keys.column(o))
                    .is_some_and(|k| k.generated == Generated::AutoIncrement),
        })
        .collect();
    let read_in = datarig_core::driver::ReadIn::Values(values.clone());
    let cells = export::sql_cells(d, &sql_columns, &read_in, keys.server_zone())?;
    let owned: Vec<Vec<Option<String>>> = rows
        .iter()
        .map(|r| {
            r.iter()
                .enumerate()
                .map(|(i, v)| v.as_deref().map(|v| cells.cell(i, v).map(|c| c.into_owned())).transpose())
                .collect::<Result<_, _>>()
        })
        .collect::<Result<_, _>>()?;
    let rows: Vec<export::Row> = owned.iter().map(|r| r.iter().map(|v| v.as_deref()).collect()).collect();
    if update {
        let plan = update_source(d, Some(keys), &origins, &names, &sql).expect("rows of one table with its key");
        let target =
            UpdateTarget { schema: &plan.table.schema, name: &plan.table.name, set: plan.set.clone(), keys: plan.keys };
        let mut w = Writer::new(Format::Update(d, target), &columns);
        w.rows(&rows);
        return Ok(w.finish());
    }
    let plan = insert_source(d, Some(keys), &origins, &names, &sql).expect("rows of one table");
    let target = Target::Table {
        schema: &plan.table.schema,
        name: &plan.table.name,
        columns: plan.columns.iter().map(|c| c.1).collect(),
        overriding: plan.overriding,
    };
    let written: Vec<export::Column> = plan.columns.iter().map(|c| columns[c.0]).collect();
    let rows: Vec<export::Row> = rows.iter().map(|r| plan.columns.iter().map(|c| r[c.0]).collect()).collect();
    let mut w = Writer::new(Format::Sql(d, target), &written);
    w.rows(&rows);
    Ok(w.finish())
}

/// Run every statement of `text` (split as the editor splits what it runs, in dialect `d`) on
/// `q`, each succeeding.
async fn run_all(q: &mut Conn, first_id: u64, text: &str, d: Dialect) {
    let statements: Vec<String> =
        datarig_core::sql::split::split_in(text, d).iter().map(|st| st.body(text).to_string()).collect();
    assert!(!statements.is_empty(), "{text}");
    for (i, statement) in statements.iter().enumerate() {
        let evs = q.run(first_id + i as u64, &[statement.as_str()]).await;
        assert!(!matches!(evs.last(), Some(DbEvent::Failed { .. })), "{statement}: {evs:?}");
    }
}

/// Rows of every kind of value copied as SQL `INSERT` and `UPDATE` statements by the app's own
/// path, run back into a copy of the table, are the rows byte for byte: in the server's default
/// escaping, `NO_BACKSLASH_ESCAPES` and `ANSI_QUOTES`, and run in a session in another mode and
/// another time zone than the one they were copied in (a string with a backslash is written as
/// its bytes, a `TIMESTAMP` with its offset). What would not be written whole is refused: a
/// `FLOAT`, text read in latin1, a `TIMESTAMP` in a zone with daylight saving (where one shown
/// time is two instants).
#[tokio::test(flavor = "multi_thread")]
async fn rows_copied_as_sql_run_back_byte_for_byte() {
    use datarig_core::export::NotWhole;
    let test = "rows_copied_as_sql_run_back_byte_for_byte";
    let Some((url, admin)) = urls(test) else { return };
    let columns = [
        "id", "txt", "js", "bin", "blb", "bt", "d", "dt", "ts", "tm", "yr", "big", "amount", "nul", "dbl", "en", "st",
        "geo", "bn", "lat", "twice",
    ];
    let create = "CREATE TABLE {t} (id INT PRIMARY KEY, txt VARCHAR(100), js JSON, bin VARBINARY(20), blb BLOB, \
                  bt BIT(10), d DATE, dt DATETIME(6), ts TIMESTAMP(3) NULL, tm TIME(2), yr YEAR, \
                  big BIGINT UNSIGNED, amount DECIMAL(30,10), nul VARCHAR(5), dbl DOUBLE, \
                  en ENUM('a', 'b c', 'it''s'), st SET('x', 'y', 'z'), geo GEOMETRY NULL, bn BINARY(4), \
                  lat VARCHAR(20) CHARACTER SET latin1, twice INT GENERATED ALWAYS AS (id * 2) STORED)";
    // Zero dates are kept: the modes leave out NO_ZERO_DATE and NO_ZERO_IN_DATE. The rows are
    // written in UTC: rows 1 and 4 are the two instants Europe/Berlin shows as 02:30 on the
    // night its clocks go back.
    let base = "STRICT_TRANS_TABLES,ERROR_FOR_DIVISION_BY_ZERO,NO_ENGINE_SUBSTITUTION";
    let setup = [format!("SET SESSION sql_mode = '{base}'"), "SET SESSION time_zone = '+00:00'".to_string()];
    let fill = "INSERT INTO {t} (id, txt, js, bin, blb, bt, d, dt, ts, tm, yr, big, amount, nul, dbl, en, st, geo, bn, lat) \
        VALUES \
        (1, 'back\\\\slash \\'q\\' \"dq\" tab\\tnl\\n', '{\"a\": \"b\\\\\\\\c\", \"q\": \"\\\\\"x\\\\\"\"}', \
         0x00FF275C22, 0x00, b'1010101010', '2024-02-29', '2024-02-29 23:59:59.123456', \
         '2024-10-27 00:30:00.500', '-838:59:59.00', 2024, 18446744073709551615, \
         -12345678901234567890.0123456789, NULL, 1.2345678901234567, 'it''s', 'x,z', \
         ST_GeomFromText('POINT(1 2)'), 0x0102, CONCAT('caf', _latin1 X'E9')), \
        (2, CONCAT('nul', CHAR(0), 'byte ', _utf8mb4 X'F09F9880', ' emoji'), '[1, \"\\\\u00e9\", null]', \
         '', NULL, b'0', '0000-00-00', '0000-00-00 00:00:00', '0000-00-00 00:00:00', '00:00:00', 0000, 0, 0, '', \
         -0.0, 'b c', '', NULL, 0x00000000, _latin1 X'E9'), \
        (3, '', 'null', NULL, 0x0A0D1A, b'1111111111', NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'x', \
         1e308, NULL, NULL, NULL, NULL, NULL), \
        (4, 'second', '{}', NULL, NULL, NULL, NULL, NULL, '2024-10-27 01:30:00', NULL, NULL, NULL, NULL, NULL, \
         NULL, NULL, NULL, NULL, NULL, NULL)";
    for (n, flag) in ["", ",NO_BACKSLASH_ESCAPES", ",ANSI_QUOTES"].iter().enumerate() {
        let mode = format!("{base}{flag}");
        let ddl = |sql: &str| vec![setup[0].clone(), setup[1].clone(), sql.to_string()];
        let src = Table::make(
            &admin,
            &format!("golden{n}"),
            &ddl(create).iter().chain([&fill.to_string()]).map(String::as_str).collect::<Vec<_>>(),
        )
        .await;
        let copy = Table::make(
            &admin,
            &format!("golden{n}_copy"),
            &ddl(create).iter().map(String::as_str).collect::<Vec<_>>(),
        )
        .await;
        // The table is read from the session's catalog: the keys read after it was made.
        let mut m = Conn::start(&url, SessionRole::Meta, false, test);
        m.wait(|e| matches!(e, DbEvent::Keys(_)), 30).await;
        let keys = keys_of(&mut m).await.unwrap();
        let mut q = Conn::open(&url, SessionRole::Query, false, test).await;
        let evs = q.run(1, &[&format!("SET SESSION sql_mode = '{mode}'")]).await;
        let evs2 = q.run(2, &["SET SESSION time_zone = '+00:00'"]).await;
        let (_, session_mode) = reported(&evs);
        let (values, _) = reported(&evs2);
        let (session_mode, values) = (session_mode.expect("the new mode"), values.expect("the new zone"));
        assert_eq!(values.time_zone, "+00:00");
        assert_eq!(
            (session_mode.no_backslash_escapes, session_mode.ansi_quotes),
            (flag.contains("BACKSLASH"), flag.contains("ANSI")),
            "{mode}"
        );
        let insert = copied_as_sql(&mut q, 3, &keys, &src, session_mode, &values, false).await.unwrap();
        let insert = insert.replace(&format!("`{}`", src.name), &format!("`{}`", copy.name));
        assert!(!insert.contains("OVERRIDING") && !insert.contains("`twice`"), "{insert}");
        assert!(insert.contains("'2024-10-27 00:30:00.500+00:00'"), "{insert}");
        // Run in another mode and another zone than the copy was made in.
        let other = if flag.is_empty() { format!("{base},NO_BACKSLASH_ESCAPES") } else { base.to_string() };
        q.ok(10, &format!("SET SESSION sql_mode = '{other}'")).await;
        q.ok(11, "SET SESSION time_zone = '+05:00'").await;
        let other_mode = MySqlMode { no_backslash_escapes: other.contains("BACKSLASH"), ..session_mode };
        run_all(&mut q, 20, &insert, Dialect::MySql(MySqlMode { ansi_quotes: false, ..other_mode })).await;
        let mut a = side(&admin).await;
        let want = row_hashes(&mut a, &src.q(), &columns).await;
        assert_eq!(row_hashes(&mut a, &copy.q(), &columns).await, want, "INSERT, {mode}:\n{insert}");
        // UPDATE: the copy's rows changed, then set back by the statements.
        a.query_drop(format!(
            "UPDATE {} SET txt = 'changed', js = '{{}}', bin = NULL, blb = 0x01, bt = b'0', d = NULL, \
             dt = '2000-01-01', ts = '2000-01-01 00:00:00', tm = '01:00:00', yr = 1999, big = 1, amount = 2, \
             nul = 'y', dbl = 3, en = 'a', st = 'y', geo = ST_GeomFromText('POINT(9 9)'), bn = 0x09, lat = 'z'",
            copy.q()
        ))
        .await
        .unwrap();
        assert_ne!(row_hashes(&mut a, &copy.q(), &columns).await, want);
        q.ok(30, &format!("SET SESSION sql_mode = '{mode}'")).await;
        q.ok(31, "SET SESSION time_zone = '+00:00'").await;
        let update = copied_as_sql(&mut q, 32, &keys, &src, session_mode, &values, true).await.unwrap();
        let update = update.replace(&format!("`{}`", src.name), &format!("`{}`", copy.name));
        q.ok(33, &format!("SET SESSION sql_mode = '{other}'")).await;
        q.ok(34, "SET SESSION time_zone = '-03:00'").await;
        run_all(&mut q, 40, &update, Dialect::MySql(MySqlMode { ansi_quotes: false, ..other_mode })).await;
        assert_eq!(row_hashes(&mut a, &copy.q(), &columns).await, want, "UPDATE, {mode}:\n{update}");
        a.disconnect().await.unwrap();
        if n > 0 {
            continue;
        }
        // Refused: text read in latin1.
        let evs = q.run(50, &["SET NAMES latin1"]).await;
        let (latin1, _) = reported(&evs);
        let latin1 = latin1.expect("the session says its character set");
        assert_eq!(latin1.charset, "latin1");
        let refused = copied_as_sql(&mut q, 51, &keys, &src, session_mode, &latin1, false).await.unwrap_err();
        assert_eq!((refused.column.as_str(), refused.why), ("txt", NotWhole::Charset("latin1".into())));
        // Refused: text read in utf8mb3, where the emoji of row 2 arrives as `?`.
        let evs = q.run(52, &["SET NAMES utf8mb3"]).await;
        let (mb3, _) = reported(&evs);
        let mb3 = mb3.expect("the session says its character set");
        assert_eq!(mb3.charset, "utf8mb3");
        let refused = copied_as_sql(&mut q, 58, &keys, &src, session_mode, &mb3, false).await.unwrap_err();
        assert_eq!((refused.column.as_str(), refused.why), ("txt", NotWhole::Charset("utf8mb3".into())));
        let evs = q.run(59, &["SET NAMES utf8mb4"]).await;
        assert_eq!(reported(&evs).0.map(|v| v.charset).as_deref(), Some("utf8mb4"));
        // Refused: a zone with daylight saving, where rows 1 and 4 show the same time.
        match q.run(53, &["SET SESSION time_zone = 'Europe/Berlin'"]).await.last() {
            Some(DbEvent::Failed { error, .. }) => {
                let _ = writeln!(std::io::stderr(), "SKIPPED the Europe/Berlin part: no time zone tables ({error:?})");
            }
            _ => {
                let evs = q.run(54, &[&format!("SELECT ts FROM {} WHERE id IN (1, 4) ORDER BY id", src.q())]).await;
                let shown: Vec<_> = page_of(&evs).1.iter().map(|r| r[0].clone()).collect();
                assert_eq!(
                    shown,
                    [Some("2024-10-27 02:30:00.500".to_string()), Some("2024-10-27 02:30:00.000".to_string())]
                );
                let berlin =
                    datarig_core::driver::SessionValues { time_zone: "Europe/Berlin".into(), ..values.clone() };
                let refused = copied_as_sql(&mut q, 55, &keys, &src, session_mode, &berlin, false).await.unwrap_err();
                assert_eq!((refused.column.as_str(), refused.why), ("ts", NotWhole::TimeZone("Europe/Berlin".into())));
            }
        }
        // Refused: a FLOAT (sent rounded: 16777215 shows as 16777200).
        let floats = Table::make(
            &admin,
            "golden_float",
            &["CREATE TABLE {t} (id INT PRIMARY KEY, f FLOAT)", "INSERT INTO {t} VALUES (1, 16777215), (2, 1.2345678)"],
        )
        .await;
        let keys = keys_of(&mut m).await.unwrap();
        let evs = q.run(56, &[&format!("SELECT f FROM {} ORDER BY id", floats.q())]).await;
        let shown: Vec<_> = page_of(&evs).1.iter().map(|r| r[0].clone()).collect();
        assert_eq!(shown, [Some("16777200".to_string()), Some("1.23457".to_string())], "rounded");
        let refused = copied_as_sql(&mut q, 57, &keys, &floats, session_mode, &values, false).await.unwrap_err();
        assert_eq!((refused.column.as_str(), refused.why), ("f", NotWhole::Float));
    }
}

/// A trigger's definition in the structure is for display: `ACTION_STATEMENT` gives its body
/// with the escapes of its string literals undone, so a literal with a doubled quote or a
/// backslash no longer reads as SQL (the DDL comes from `SHOW CREATE TRIGGER`).
#[tokio::test(flavor = "multi_thread")]
async fn a_mysql_trigger_definition_is_for_display_only() {
    let test = "a_mysql_trigger_definition_is_for_display_only";
    let Some((url, admin)) = urls(test) else { return };
    let pid = std::process::id();
    let t = Table::make(
        &admin,
        "trigtext",
        &[
            "CREATE TABLE {t} (id INT PRIMARY KEY, s VARCHAR(40))",
            &format!(
                r"CREATE TRIGGER shop.zz_it_tt_{pid} BEFORE INSERT ON {{t}} FOR EACH ROW SET NEW.s = 'it''s \\ x'"
            ),
        ],
    )
    .await;
    let mut m = Conn::start(&url, SessionRole::Meta, false, test);
    m.wait(|e| matches!(e, DbEvent::Keys(_)), 30).await;
    let s = structure_of(&mut m, &t.name).await.unwrap();
    assert_eq!(s.triggers.len(), 1);
    assert!(s.triggers[0].definition.ends_with(r"FOR EACH ROW SET NEW.s = 'it's \ x'"), "{}", s.triggers[0].definition);
}

/// A foreign key may be named `PRIMARY` (on a column with an index of its own): it is a
/// foreign key, not the primary key, so a table without one is copied as UPDATE by none. The
/// structure read says the same.
#[tokio::test(flavor = "multi_thread")]
async fn a_foreign_key_named_primary_is_no_primary_key() {
    use datarig_core::driver::keys::{KeyMarks, NotUpdatable, update_source};
    let test = "a_foreign_key_named_primary_is_no_primary_key";
    let Some((url, admin)) = urls(test) else { return };
    let parent = Table::make(&admin, "fkp_parent", &["CREATE TABLE {t} (a INT PRIMARY KEY)"]).await;
    let child = Table::make(
        &admin,
        "fkp_child",
        &[&format!(
            "CREATE TABLE {{t}} (x INT, v INT, KEY ix (x), CONSTRAINT `PRIMARY` FOREIGN KEY (x) REFERENCES {} (a))",
            parent.q()
        )],
    )
    .await;
    let mut m = Conn::start(&url, SessionRole::Meta, false, test);
    m.wait(|e| matches!(e, DbEvent::Keys(_)), 30).await;
    let k = keys_of(&mut m).await.unwrap();
    assert_eq!(k.marks_by_name("shop", &child.name, "x"), KeyMarks { fk: true, ..KeyMarks::default() });
    let origin = |c: &str| {
        Some(datarig_core::driver::ColumnOrigin::Named {
            schema: "shop".into(),
            table: child.name.clone(),
            column: c.into(),
        })
    };
    let sql = format!("SELECT * FROM `shop`.`{}`", child.name);
    let mode = Dialect::MySql(MySqlMode::default());
    assert_eq!(
        update_source(mode, Some(&k), &[origin("x"), origin("v")], &["x", "v"], &sql).map(|p| p.keys),
        Err(NotUpdatable::NoPrimaryKey)
    );
    let s = structure_of(&mut m, &child.name).await.unwrap();
    assert!(s.primary_key.is_none(), "{:?}", s.primary_key);
    assert_eq!(s.foreign_keys.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["PRIMARY"]);
}

/// Code of the server's that changes the session: a procedure's `SET time_zone` and `SET NAMES`
/// are not reported by the server when it returns rows (no OK packet has them), so the session
/// asks after a `CALL` and says what it found; its `SET sql_mode` does not outlive it (a routine
/// runs in the mode it was made in); a stored function's and a trigger's are reported with the
/// statement that ran them, and said as well.
#[tokio::test(flavor = "multi_thread")]
async fn what_server_code_changes_in_the_session_is_followed() {
    let test = "what_server_code_changes_in_the_session_is_followed";
    let Some((url, admin)) = urls(test) else { return };
    let pid = std::process::id();
    let t = Table::make(&admin, "routine", &["CREATE TABLE {t} (id INT, ts TIMESTAMP NULL)"]).await;
    let (tz, names, mode, f) = (
        format!("zz_it_ptz_{pid}"),
        format!("zz_it_pnames_{pid}"),
        format!("zz_it_pmode_{pid}"),
        format!("zz_it_f_{pid}"),
    );
    let _tz = Cleanup { url: admin.clone(), sql: format!("DROP PROCEDURE IF EXISTS shop.{tz}") };
    let _names = Cleanup { url: admin.clone(), sql: format!("DROP PROCEDURE IF EXISTS shop.{names}") };
    let _mode = Cleanup { url: admin.clone(), sql: format!("DROP PROCEDURE IF EXISTS shop.{mode}") };
    let _f = Cleanup { url: admin.clone(), sql: format!("DROP FUNCTION IF EXISTS shop.{f}") };
    let mut a = side(&admin).await;
    for sql in [
        format!(
            "CREATE PROCEDURE shop.{tz}() BEGIN SET time_zone = '+09:00'; SELECT NOW() AS now, @@time_zone AS zone; END"
        ),
        format!("CREATE PROCEDURE shop.{names}() BEGIN SET NAMES latin1; SELECT 'x' AS x; END"),
        format!("CREATE PROCEDURE shop.{mode}() BEGIN SET sql_mode = 'NO_BACKSLASH_ESCAPES'; END"),
        format!("CREATE FUNCTION shop.{f}() RETURNS INT DETERMINISTIC BEGIN SET time_zone = '+05:00'; RETURN 1; END"),
        format!("CREATE TRIGGER shop.zz_it_tr_{pid} BEFORE INSERT ON {} FOR EACH ROW SET time_zone = '+03:00'", t.q()),
    ] {
        a.query_drop(sql).await.unwrap();
    }
    a.disconnect().await.unwrap();
    let mut q = Conn::open(&url, SessionRole::Query, false, test).await;
    q.ok(1, "SET time_zone = '+00:00'").await;
    let zone = |evs: &[DbEvent]| reported(evs).0.map(|v| v.time_zone);
    let mut evs = q.run(2, &[&format!("CALL shop.{tz}()")]).await;
    // The run's events up to its first page; the session's report comes once the CALL ended.
    evs.extend(q.run(3, &["SELECT @@time_zone"]).await);
    assert_eq!(zone(&evs).as_deref(), Some("+09:00"), "{evs:?}");
    let mut evs = q.run(4, &[&format!("CALL shop.{names}()")]).await;
    evs.extend(q.run(5, &["SELECT 1"]).await);
    assert_eq!(reported(&evs).0.map(|v| v.charset).as_deref(), Some("latin1"), "{evs:?}");
    q.ok(6, "SET NAMES utf8mb4").await;
    // A routine runs in the sql mode it was made in, and the session's is back after it: no
    // change to say (the session asked, and the mode it found is the same).
    let mut evs = q.run(7, &[&format!("CALL shop.{mode}()")]).await;
    evs.extend(q.run(11, &["SELECT @@SESSION.sql_mode"]).await);
    assert!(reported(&evs).1.is_none(), "{evs:?}");
    let mode_now = page_of(&evs).1[0][0].clone().unwrap_or_default();
    assert!(!mode_now.contains("NO_BACKSLASH_ESCAPES"), "{mode_now}");
    let mut evs = q.run(8, &[&format!("SELECT shop.{f}()")]).await;
    evs.extend(q.run(9, &["SELECT 1"]).await);
    assert_eq!(zone(&evs).as_deref(), Some("+05:00"), "a stored function: {evs:?}");
    let mut evs = q.run(10, &[&format!("INSERT INTO {} (id) VALUES (1)", t.q())]).await;
    evs.extend(q.run(12, &["SELECT 1"]).await);
    assert_eq!(zone(&evs).as_deref(), Some("+03:00"), "a trigger: {evs:?}");
    // A procedure the text hides: an executable comment, a prepared statement. Its result is
    // said to be read in values not known, before its page.
    let not_known = |evs: &[DbEvent], id: u64| {
        let at = |pred: &dyn Fn(&DbEvent) -> bool| evs.iter().position(pred);
        let signal = at(&|e| matches!(e, DbEvent::ValuesNotKnown { id: i } if *i == id));
        let page = at(&|e| matches!(e, DbEvent::Page { id: i, .. } if *i == id));
        signal.is_some() && signal < page
    };
    let evs = q.run(13, &[&format!("CALL shop.{tz}()")]).await;
    assert!(not_known(&evs, 13), "{evs:?}");
    for (id, sql) in [(20, format!("/*! CALL shop.{tz}() */")), (30, "EXECUTE zz_it_s".to_string())] {
        q.ok(id, "SET time_zone = '+00:00'").await;
        q.ok(id + 1, &format!("PREPARE zz_it_s FROM 'CALL shop.{tz}()'")).await;
        let mut evs = q.run(id + 2, &[&sql]).await;
        assert!(not_known(&evs, id + 2), "{sql}: {evs:?}");
        evs.extend(q.run(id + 3, &["SELECT 1"]).await);
        assert_eq!(zone(&evs).as_deref(), Some("+09:00"), "{sql}: {evs:?}");
    }
}

/// A procedure whose result is stopped (a cancel while its rows are read) leaves the session's
/// values not known: what it changed is never taken for what the session had.
#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_procedure_leaves_the_values_not_known() {
    let test = "a_stopped_procedure_leaves_the_values_not_known";
    let Some((url, admin)) = urls(test) else { return };
    let pid = std::process::id();
    let t = numbers(&admin, "slowproc", 200_000).await;
    let p = format!("zz_it_pslow_{pid}");
    let _p = Cleanup { url: admin.clone(), sql: format!("DROP PROCEDURE IF EXISTS shop.{p}") };
    // Dropped first: nothing of the test's keeps running past it.
    let _end = EndSessions { admin: admin.clone(), test };
    let mut a = side(&admin).await;
    a.query_drop(format!(
        "CREATE PROCEDURE shop.{p}() SQL SECURITY INVOKER BEGIN SET time_zone = '+09:00'; \
         SELECT a.id, a.v FROM {} a CROSS JOIN {} b LIMIT 2000000; END",
        t.q(),
        t.q()
    ))
    .await
    .unwrap();
    a.disconnect().await.unwrap();
    let mut q = Conn::open(&url, SessionRole::Query, false, test).await;
    q.ok(1, "SET time_zone = '+00:00'").await;
    // The procedure's rows as a statement before the last: read chunk by chunk, stopped after
    // the first. The procedure runs as the session's user (`SQL SECURITY INVOKER`), so the
    // cancel's `KILL QUERY` stops it and the session goes on (one that runs as another account
    // cannot be stopped so, see
    // `a_cancel_the_server_refuses_ends_the_session_instead_of_reading_on`).
    let statements = vec![format!("CALL shop.{p}()"), "SELECT 1".to_string()];
    q.session.send(DbCommand::Execute { id: 2, statements, paging: PagingMode::NoHold, tx: TxRule::Auto });
    let mut evs = Vec::new();
    loop {
        let ev = q.next(60).await;
        let first_rows = matches!(ev, DbEvent::StepRows { .. });
        let end =
            matches!(ev, DbEvent::Failed { id: 2, .. } | DbEvent::Done { id: 2, .. } | DbEvent::Page { id: 2, .. });
        evs.push(ev);
        if first_rows && evs.iter().filter(|e| matches!(e, DbEvent::StepRows { .. })).count() == 1 {
            q.session.cancel();
        }
        if end {
            break;
        }
    }
    assert!(matches!(evs.last(), Some(DbEvent::Failed { error: DbError::Cancelled, .. })), "{evs:?}");
    evs.extend(q.run(3, &["SELECT 1"]).await);
    let zone = reported(&evs).0.map(|v| v.time_zone);
    assert_ne!(zone.as_deref(), Some("+00:00"), "{evs:?}");
    assert!(zone.is_some(), "the session said its values are not known, or asked: {evs:?}");
}
