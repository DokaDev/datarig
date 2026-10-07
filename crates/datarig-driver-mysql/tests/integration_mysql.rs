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

use datarig_core::driver::{ConnectOptions, DbError, DbEvent, Driver, PingError, Session, SessionRole};
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

fn profile(url: &str) -> ConnectionConfig {
    ConnectionConfig { name: "it".into(), driver: "mysql".into(), dsn: Some(url.to_string()), ..Default::default() }
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
    #[allow(dead_code)]
    session: Session,
    rx: UnboundedReceiver<DbEvent>,
}

impl Conn {
    /// A session of `url`'s profile for test `test`, read-only or not, once it connected.
    async fn open(url: &str, role: SessionRole, read_only: bool, test: &str) -> Conn {
        let mut c = Conn::start(url, role, read_only, test);
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

#[tokio::test(flavor = "multi_thread")]
async fn the_metadata_session_sends_no_language() {
    let Some(url) = my_url("the_metadata_session_sends_no_language") else { return };
    let mut c = Conn::start(&url, SessionRole::Meta, false, "the_metadata_session_sends_no_language");
    assert!(matches!(c.next(15).await, DbEvent::Connected));
    let next = tokio::time::timeout(Duration::from_millis(300), c.rx.recv()).await;
    assert!(next.is_err(), "nothing more: {next:?}");
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
        let c = Conn::open(&url, role, read_only, "sessions_are_set_up_on_the_server_by_role_and_policy").await;
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
        DbEvent::ConnectFailed { error: DbError::Server(m), auth: true } => {
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
    assert!(matches!(bad, Err(PingError::Failed(DbError::Server(m))) if m.starts_with("ERROR 1045")));
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
    assert!(matches!(first, Some(DbEvent::Connected)), "{first:?}");
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
    // Idle, nothing asked: the session sees the end itself.
    assert!(matches!(c.next(10).await, DbEvent::Lost { .. }));
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
        c.query_drop(format!("GRANT SELECT ON shop.* TO '{name}'@'%'")).await?;
        c.disconnect().await?;
        Ok(Account { admin: admin.to_string(), name })
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
    // over a plain connection goes encrypted with the server's public key; the second is
    // answered from the cache.
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
