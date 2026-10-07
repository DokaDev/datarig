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
    Cell, ConnectOptions, DbCommand, DbError, DbEvent, Driver, PagingMode, PingError, Session, SessionRole,
    StatementInfo, ValueKind,
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
        self.session.send(DbCommand::Execute { id, statements, paging });
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
        let _c = Conn::open(&url, role, true, "a_session_is_set_up_in_one_statement").await;
        let id = only_session(&mut admin, role, "a_session_is_set_up_in_one_statement").await;
        // The server's answer to the `SET` said the sql mode and read-only: nothing is asked
        // after it.
        let ran = statements_of(&mut admin, id).await;
        assert_eq!(ran.len(), 1, "{role:?}: {ran:?}");
        assert!(ran[0].starts_with("SET NAMES utf8mb4, SESSION session_track_system_variables"), "{ran:?}");
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
        let cfg =
            ConnectionConfig { server_public_key_file: Some(key.to_string()), ..profile(&as_user(&url, user, "pw")) };
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

/// The owner's worry: a result on screen holds no metadata lock, so an `ALTER TABLE` of another
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
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    a.query_drop(format!("KILL {id}")).await.unwrap();
    let evs = c.answer(1).await;
    assert!(matches!(evs.last(), Some(DbEvent::Failed { cancelled: false, .. })), "{evs:?}");
    assert!(matches!(c.next(10).await, DbEvent::Lost { .. }));
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
    a.query_drop(format!("CREATE PROCEDURE shop.{proc_name}() SET SESSION transaction_read_only = OFF")).await.unwrap();
    let mut c = Conn::open(&url, SessionRole::Query, true, "ro").await;
    // The server refuses a write.
    let evs = c.run(1, &[&format!("INSERT INTO {} VALUES (1)", t.q())]).await;
    match evs.last() {
        Some(DbEvent::Failed { error: DbError::Server(m), .. }) => assert!(m.starts_with("ERROR 1792"), "{m}"),
        other => panic!("{other:?}"),
    }
    // The session refuses to ask for read-write.
    for (id, sql) in [(2, "START TRANSACTION READ WRITE"), (3, "SET SESSION transaction_read_only = OFF")] {
        assert!(
            matches!(c.run(id, &[sql]).await.last(), Some(DbEvent::Failed { error: DbError::ReadWriteRefused, .. })),
            "{sql}"
        );
    }
    // A read-write transaction the text hides (an executable comment) is seen from the server's
    // flags: rolled back, and the session closes.
    let evs = c.run(4, &["/*!80000 START TRANSACTION READ WRITE */"]).await;
    assert!(matches!(evs.last(), Some(DbEvent::Failed { error: DbError::ReadOnlyLost, .. })), "{evs:?}");
    assert!(matches!(c.next(10).await, DbEvent::Lost { error: DbError::ReadOnlyLost }));
    // So is a procedure that turns read-only off.
    let mut c = Conn::open(&url, SessionRole::Query, true, "ro2").await;
    let evs = c.run(5, &[&format!("CALL shop.{proc_name}()")]).await;
    assert!(matches!(evs.last(), Some(DbEvent::Failed { error: DbError::ReadOnlyLost, .. })), "{evs:?}");
    assert!(matches!(c.next(10).await, DbEvent::Lost { error: DbError::ReadOnlyLost }));
    a.query_drop(format!("DROP PROCEDURE shop.{proc_name}")).await.unwrap();
    let n: u64 = a.query_first(format!("SELECT COUNT(*) FROM {}", t.q())).await.unwrap().unwrap();
    assert_eq!(n, 0, "nothing was written");
    a.disconnect().await.unwrap();
}
