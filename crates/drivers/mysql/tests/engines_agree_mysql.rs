//! A MySQL-family session runs nothing on a server whose two reports of what it is disagree: the
//! greeting (which a proxy in front of the server may write itself) and the answer to
//! `SELECT VERSION() LIMIT 1`, read first on every session and in a test connection. Against a
//! real MySQL or MariaDB loaded with `dev/init-mysql/*.sql` (CI runs this file in both jobs),
//! through the test proxy (`my_proxy`), which rewrites the greeting or the version answer.
//!
//! Connection: `DATARIG_TEST_MYSQL_URL` (e.g. `mysql://datarig:datarig@127.0.0.1:53306/shop`).
//! * unset locally  -> each test prints a visible `SKIPPED` line to stderr and returns;
//! * unset with `DATARIG_REQUIRE_MYSQL=1` -> the test fails.
//!
//! The errors are compared by their debug text, which names the variant and its fields.

#[allow(dead_code)]
mod my_proxy;

use datarig_core::driver::{ConnectOptions, DbError, DbEvent, Driver, PingError, PingInfo, Session, SessionRole};
use datarig_core::driver::{EditFormat, ExplainFormat, MySqlEdit, SessionStatement};
use datarig_core::engine::{EngineId, Fact, ServerFacts, Version, Why};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::{Dialect, Language};
use datarig_driver_mysql::MyDriver;
use my_proxy::{Fault, Proxy, upstream};
use mysql_async::prelude::Queryable;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;

const PAGE: usize = 500;

/// What every session and a test connection ask first.
const VERSION_SQL: &str = "SELECT VERSION() LIMIT 1";

/// The start of [`VERSION_SQL`], and of the text a test connection asked before it.
const VERSION_START: &str = "SELECT VERSION()";

/// A query session's own start-up statement in these tests.
const STARTUP: &str = "SET @zz_x8_startup = 1";

const ROLES: [SessionRole; 3] = [SessionRole::Query, SessionRole::Meta, SessionRole::Monitor];

fn my_url(test: &str) -> Option<String> {
    match std::env::var("DATARIG_TEST_MYSQL_URL") {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            if std::env::var("DATARIG_REQUIRE_MYSQL").is_ok_and(|v| v == "1") {
                panic!("{test}: DATARIG_REQUIRE_MYSQL=1 but DATARIG_TEST_MYSQL_URL is not set");
            }
            let _ = writeln!(
                std::io::stderr(),
                "SKIPPED {test}: set DATARIG_TEST_MYSQL_URL (e.g. mysql://datarig:datarig@127.0.0.1:53306/shop)"
            );
            None
        }
    }
}

/// A profile of driver `driver` for `url`, plain (the proxy keeps sessions plain anyway).
fn profile(url: &str, driver: &str) -> ConnectionConfig {
    ConnectionConfig {
        name: "engines".into(),
        driver: driver.into(),
        dsn: Some(url.to_string()),
        sslmode: "DISABLED".into(),
        ..Default::default()
    }
}

/// What the server is by its own version line, asked on a plain client connection.
struct Server {
    /// The line `VERSION()` answers.
    line: String,
    mariadb: bool,
    version: Version,
}

impl Server {
    async fn of(url: &str) -> Server {
        let opts = mysql_async::Opts::from_url(url).expect("a mysql:// URL");
        let opts = mysql_async::OptsBuilder::from_opts(opts).prefer_socket(false);
        let mut c = mysql_async::Conn::new(opts).await.expect("the side connection opens");
        let line: String = c.query_first("SELECT VERSION()").await.unwrap().expect("a version");
        let _ = c.disconnect().await;
        let version = Version::parse(&line).expect("the version line starts with its number");
        Server { mariadb: line.contains("MariaDB"), line, version }
    }

    fn engine(&self) -> EngineId {
        if self.mariadb { EngineId::MariaDb } else { EngineId::MySql }
    }

    /// The server's greeting as the driver names it (`MySQL 8.4.6`, `MariaDB 11.4.13`).
    fn label(&self) -> String {
        let v = self.version;
        format!("{} {}.{}.{}", if self.mariadb { "MariaDB" } else { "MySQL" }, v.major, v.minor, v.patch)
    }

    /// A greeting naming the other engine, and how the driver names it.
    fn other_greeting(&self) -> (&'static str, &'static str) {
        if self.mariadb { ("8.4.6", "MySQL 8.4.6") } else { ("5.5.5-11.4.13-MariaDB", "MariaDB 11.4.13") }
    }

    /// A version line of the other engine.
    fn other_line(&self) -> &'static str {
        if self.mariadb { "8.4.6" } else { "11.4.13-MariaDB-ubu2404" }
    }

    /// The driver's key of the other engine.
    fn other_key(&self) -> &'static str {
        if self.mariadb { "mysql" } else { "mariadb" }
    }
}

fn disagree(greeting: &str, version: &str) -> String {
    format!("EnginesDisagree {{ greeting: {greeting:?}, version: {version:?} }}")
}

fn unconfirmed(greeting: &str) -> String {
    format!("EngineUnconfirmed {{ greeting: {greeting:?} }}")
}

/// The options of a session of `role`: a query session on a read-only profile with a start-up
/// statement, so a refusal shows it ran none of the session's settings, its read-only check or
/// its start-up statements.
fn options(role: SessionRole, test: &str) -> ConnectOptions {
    let tag = format!("x8{}-{test}", std::process::id());
    let o = ConnectOptions::new(PAGE, role, &tag);
    match role {
        SessionRole::Query => o.read_only(true).session_sql(vec![SessionStatement {
            sql: STARTUP.into(),
            per_transaction: None,
            identity: false,
        }]),
        SessionRole::Meta | SessionRole::Monitor => o,
    }
}

/// A session of `role` on `url`: what it said up to (not with) `Connected`, or why it failed.
async fn open(url: &str, driver: &str, role: SessionRole, test: &str) -> Result<(Session, Vec<DbEvent>), DbError> {
    let (tx, mut rx) = unbounded_channel();
    let session = MyDriver.connect(&profile(url, driver), role, options(role, test), tx);
    let mut before = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(15), rx.recv()).await.expect("an answer") {
            Some(DbEvent::Connected) => return Ok((session, before)),
            Some(DbEvent::ConnectFailed { error, .. }) => return Err(error),
            Some(ev) => before.push(ev),
            None => panic!("event channel closed"),
        }
    }
}

async fn ping(url: &str) -> Result<PingInfo, PingError> {
    MyDriver.ping(&profile(url, "mysql"), Duration::from_secs(10), None).await
}

/// The facts a session said first.
fn facts(before: &[DbEvent]) -> Arc<ServerFacts> {
    let Some(DbEvent::Server(facts)) = before.first() else { panic!("the facts first: {before:?}") };
    facts.clone()
}

/// Why a session failed, in debug text; a session that connected fails the test.
fn refused(opened: Result<(Session, Vec<DbEvent>), DbError>, what: &str) -> String {
    match opened {
        Ok((_, before)) => panic!("{what}: connected: {before:?}"),
        Err(error) => format!("{error:?}"),
    }
}

/// Why a test connection failed, in debug text.
fn ping_refused(pinged: Result<PingInfo, PingError>, what: &str) -> String {
    match pinged {
        Ok(info) => panic!("{what}: the test connection succeeded: {info:?}"),
        Err(PingError::Failed(error)) => format!("{error:?}"),
        Err(PingError::Timeout(t)) => panic!("{what}: timed out after {t:?}"),
    }
}

/// The plan format, edits and dollar quotes a greeting of MySQL `v` implies (none on MariaDB).
fn words_of(engine: EngineId, v: Version) -> (Option<ExplainFormat>, Option<EditFormat>, bool) {
    match engine {
        EngineId::MySql => (
            (v >= Version::new(8, 0, 16)).then_some(ExplainFormat::MySqlTree),
            (v >= Version::new(8, 0, 17)).then_some(EditFormat::MySql(MySqlEdit { nowait: true })),
            v >= Version::new(8, 1, 0),
        ),
        EngineId::MariaDb | EngineId::Postgres => (None, None, false),
    }
}

/// The plan format, edits and dollar quotes a session said before `Connected`.
fn words(before: &[DbEvent]) -> (Option<ExplainFormat>, Option<EditFormat>, bool) {
    let explain = before.iter().find_map(|e| if let DbEvent::Explain(f) = e { Some(*f) } else { None });
    let edit = before.iter().find_map(|e| if let DbEvent::Edit(f) = e { Some(*f) } else { None });
    let dollar = before.iter().find_map(|e| match e {
        DbEvent::Language(Language::Sql(Dialect::MySql(m))) => Some(m.dollar_quotes),
        _ => None,
    });
    (explain.expect("a plan format"), edit.expect("edits"), dollar.expect("a language"))
}

/// The failing-first case: a MySQL server behind a greeting naming MariaDB (and a MariaDB
/// server behind one naming MySQL) runs none of a query session's settings: the session is
/// refused after its version read, the only statement it sent.
#[tokio::test(flavor = "multi_thread")]
async fn a_greeting_naming_the_other_engine_is_refused_before_any_setting() {
    let test = "a_greeting_naming_the_other_engine_is_refused_before_any_setting";
    let Some(url) = my_url(test) else { return };
    let server = Server::of(&url).await;
    let proxy = Proxy::start(&upstream(&url)).await;
    let (greeting, label) = server.other_greeting();
    proxy.greet_as(greeting);
    let opened = open(&proxy.url(&url), "mysql", SessionRole::Query, test).await;
    let outcome = format!("{:?}", opened.as_ref().map(|(_, before)| before));
    assert_eq!(proxy.queries(), [VERSION_SQL], "{outcome}");
    assert_eq!(refused(opened, "query"), disagree(label, &server.line));
}

/// Every session reads the server's version first, before the settings it starts with (and a
/// query session before its start-up statements); a test connection reads only the version.
#[tokio::test(flavor = "multi_thread")]
async fn every_session_reads_the_version_before_its_settings() {
    let test = "every_session_reads_the_version_before_its_settings";
    let Some(url) = my_url(test) else { return };
    Server::of(&url).await;
    for role in ROLES {
        let proxy = Proxy::start(&upstream(&url)).await;
        let opened = open(&proxy.url(&url), "mysql", role, test).await;
        let (_session, _) = opened.unwrap_or_else(|e| panic!("{role:?}: {e:?}"));
        let log = proxy.queries();
        assert_eq!(log.first().map(String::as_str), Some(VERSION_SQL), "{role:?}: {log:?}");
        assert!(log.get(1).is_some_and(|s| s.starts_with("SET ")), "{role:?}: the settings second: {log:?}");
        if role == SessionRole::Query {
            assert!(log.iter().skip(2).any(|s| s == STARTUP), "{role:?}: {log:?}");
        }
    }
    let proxy = Proxy::start(&upstream(&url)).await;
    ping(&proxy.url(&url)).await.expect("test connection");
    assert_eq!(proxy.queries(), [VERSION_SQL]);
}

/// A session that a proxy (or the server's `init_connect`) leaves with a `sql_select_limit` of 0
/// still reads the server's version line: every session connects with the server's engine and
/// that line, and a test connection says it.
#[tokio::test(flavor = "multi_thread")]
async fn a_zero_select_limit_does_not_empty_the_version_answer() {
    let test = "a_zero_select_limit_does_not_empty_the_version_answer";
    let Some(url) = my_url(test) else { return };
    let server = Server::of(&url).await;
    let proxy = Proxy::start(&upstream(&url)).await;
    proxy.zero_select_limit();
    for role in ROLES {
        let opened = open(&proxy.url(&url), "mysql", role, test).await;
        let (_session, before) = opened.unwrap_or_else(|e| panic!("{role:?}: {e:?}"));
        let f = facts(&before);
        assert_eq!(f.engine(), server.engine(), "{role:?}: {f:?}");
        assert_eq!(f.version_text.as_deref(), Some(server.line.as_str()), "{role:?}: {f:?}");
        assert_eq!(f.version, Fact::Known(server.version), "{role:?}: {f:?}");
    }
    let info = ping(&proxy.url(&url)).await.expect("test connection");
    assert_eq!(info.server_version, server.line);
}

/// A version read that fails keeps its own error: the server's for a statement it refuses, the
/// closed connection's when the connection is closed on it; never one about the engine.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_version_read_keeps_its_own_error() {
    let test = "a_failed_version_read_keeps_its_own_error";
    let Some(url) = my_url(test) else { return };
    Server::of(&url).await;
    let syntax = || Fault::Instead("SELEC VERSION() LIMIT 1".into());
    let is_syntax = |e: &DbError| matches!(e, DbError::Server(_)) && e.raw().starts_with("ERROR 1064 ");
    let is_closed = |e: &DbError| matches!(e, DbError::Closed | DbError::Connection(_));
    for role in ROLES {
        let proxy = Proxy::start(&upstream(&url)).await;
        proxy.fault_start(VERSION_START, syntax());
        let error = open(&proxy.url(&url), "mysql", role, test).await.err();
        assert!(error.as_ref().is_some_and(is_syntax), "{role:?}: {error:?}");
        assert_eq!(proxy.queries(), [VERSION_SQL], "{role:?}");
        let proxy = Proxy::start(&upstream(&url)).await;
        proxy.fault_start(VERSION_START, Fault::Close);
        let error = open(&proxy.url(&url), "mysql", role, test).await.err();
        assert!(error.as_ref().is_some_and(is_closed), "{role:?}: {error:?}");
        assert_eq!(proxy.queries(), [VERSION_SQL], "{role:?}");
    }
    let proxy = Proxy::start(&upstream(&url)).await;
    proxy.fault_start(VERSION_START, syntax());
    match ping(&proxy.url(&url)).await {
        Err(PingError::Failed(e)) => assert!(is_syntax(&e), "ping: {e:?}"),
        other => panic!("ping: {other:?}"),
    }
    let proxy = Proxy::start(&upstream(&url)).await;
    proxy.fault_start(VERSION_START, Fault::Close);
    match ping(&proxy.url(&url)).await {
        Err(PingError::Failed(e)) => assert!(is_closed(&e), "ping: {e:?}"),
        other => panic!("ping: {other:?}"),
    }
}

/// A version answer of no row, `NULL` or bytes that are not UTF-8 text confirms no engine: every
/// session is refused after the read, and a test connection fails the same way.
#[tokio::test(flavor = "multi_thread")]
async fn an_empty_or_null_version_answer_is_unconfirmed() {
    let test = "an_empty_or_null_version_answer_is_unconfirmed";
    let Some(url) = my_url(test) else { return };
    let server = Server::of(&url).await;
    for answer in ["SELECT 1 FROM DUAL WHERE FALSE", "SELECT NULL", "SELECT X'FF'"] {
        for role in ROLES {
            let proxy = Proxy::start(&upstream(&url)).await;
            proxy.fault_start(VERSION_START, Fault::Instead(answer.into()));
            let error = refused(open(&proxy.url(&url), "mysql", role, test).await, answer);
            assert_eq!(error, unconfirmed(&server.label()), "{answer} {role:?}");
            assert_eq!(proxy.queries(), [VERSION_SQL], "{answer} {role:?}");
        }
        let proxy = Proxy::start(&upstream(&url)).await;
        proxy.fault_start(VERSION_START, Fault::Instead(answer.into()));
        let error = ping_refused(ping(&proxy.url(&url)).await, answer);
        assert_eq!(error, unconfirmed(&server.label()), "{answer}: ping");
    }
}

/// A version line with no number in it leaves the version unknown and the session connects:
/// `1` (what the text protocol sends for `SELECT 1`) under a MySQL greeting, a line that names
/// MariaDB and no number under MariaDB's. Its words are the greeting's. Under MariaDB's greeting
/// `1` names MySQL, so the reports disagree.
#[tokio::test(flavor = "multi_thread")]
async fn a_version_answer_without_a_number_connects_with_the_version_unknown() {
    let test = "a_version_answer_without_a_number_connects_with_the_version_unknown";
    let Some(url) = my_url(test) else { return };
    let server = Server::of(&url).await;
    let (answer, line) = if server.mariadb { ("SELECT 'x-MariaDB'", "x-MariaDB") } else { ("SELECT 1", "1") };
    for role in ROLES {
        let proxy = Proxy::start(&upstream(&url)).await;
        proxy.fault_start(VERSION_START, Fault::Instead(answer.into()));
        let opened = open(&proxy.url(&url), "mysql", role, test).await;
        let (_session, before) = opened.unwrap_or_else(|e| panic!("{role:?}: {e:?}"));
        assert!(!proxy.fault_pending(), "{role:?}: the version was read");
        let f = facts(&before);
        assert_eq!(f.engine(), server.engine(), "{role:?}: {f:?}");
        assert_eq!(f.version, Fact::Unknown(Why::VersionUnknown), "{role:?}: {f:?}");
        assert_eq!(f.version_text.as_deref(), Some(line), "{role:?}: {f:?}");
        assert_eq!(words(&before), words_of(server.engine(), server.version), "{role:?}");
    }
    if server.mariadb {
        let proxy = Proxy::start(&upstream(&url)).await;
        proxy.fault_start(VERSION_START, Fault::Instead("SELECT 1".into()));
        let error = refused(open(&proxy.url(&url), "mysql", SessionRole::Query, test).await, "1 on MariaDB");
        assert_eq!(error, disagree(&server.label(), "1"));
    }
}

/// A blank version line (empty or only spaces) confirms no engine, whatever the greeting names:
/// every session is refused after the read and runs nothing else, a test connection fails, and
/// none of them says the engines disagree.
#[tokio::test(flavor = "multi_thread")]
async fn a_blank_version_answer_is_unconfirmed_whatever_the_greeting() {
    let test = "a_blank_version_answer_is_unconfirmed_whatever_the_greeting";
    let Some(url) = my_url(test) else { return };
    let server = Server::of(&url).await;
    let (other, other_label) = server.other_greeting();
    let own_label = server.label();
    for answer in ["SELECT ''", "SELECT '  '"] {
        for (greeting, label) in [(None, own_label.as_str()), (Some(other), other_label)] {
            let start = || async {
                let proxy = Proxy::start(&upstream(&url)).await;
                if let Some(g) = greeting {
                    proxy.greet_as(g);
                }
                proxy.fault_start(VERSION_START, Fault::Instead(answer.into()));
                proxy
            };
            for role in ROLES {
                let proxy = start().await;
                let error = refused(open(&proxy.url(&url), "mysql", role, test).await, answer);
                assert_eq!(error, unconfirmed(label), "{answer} {greeting:?} {role:?}");
                assert_eq!(proxy.queries(), [VERSION_SQL], "{answer} {greeting:?} {role:?}");
            }
            let proxy = start().await;
            let error = ping_refused(ping(&proxy.url(&url)).await, answer);
            assert_eq!(error, unconfirmed(label), "{answer} {greeting:?}: ping");
        }
    }
}

/// A greeting naming the other engine is refused on every session, which then runs nothing
/// after its version read, and fails a test connection.
#[tokio::test(flavor = "multi_thread")]
async fn a_rewritten_greeting_is_refused_on_every_session_and_runs_nothing() {
    let test = "a_rewritten_greeting_is_refused_on_every_session_and_runs_nothing";
    let Some(url) = my_url(test) else { return };
    let server = Server::of(&url).await;
    let (greeting, label) = server.other_greeting();
    for role in ROLES {
        let proxy = Proxy::start(&upstream(&url)).await;
        proxy.greet_as(greeting);
        let error = refused(open(&proxy.url(&url), "mysql", role, test).await, "greeting");
        assert_eq!(error, disagree(label, &server.line), "{role:?}");
        assert_eq!(proxy.queries(), [VERSION_SQL], "{role:?}");
    }
    let proxy = Proxy::start(&upstream(&url)).await;
    proxy.greet_as(greeting);
    let error = ping_refused(ping(&proxy.url(&url)).await, "greeting");
    assert_eq!(error, disagree(label, &server.line), "ping");
    assert_eq!(proxy.queries(), [VERSION_SQL], "ping");
}

/// A version answer naming the other engine under the server's own greeting is refused the same
/// way.
#[tokio::test(flavor = "multi_thread")]
async fn a_rewritten_version_answer_is_refused() {
    let test = "a_rewritten_version_answer_is_refused";
    let Some(url) = my_url(test) else { return };
    let server = Server::of(&url).await;
    let line = server.other_line();
    let answer = format!("SELECT '{line}'");
    for role in ROLES {
        let proxy = Proxy::start(&upstream(&url)).await;
        proxy.fault_start(VERSION_START, Fault::Instead(answer.clone()));
        let error = refused(open(&proxy.url(&url), "mysql", role, test).await, "answer");
        assert_eq!(error, disagree(&server.label(), line), "{role:?}");
        assert_eq!(proxy.queries(), [VERSION_SQL], "{role:?}");
    }
    let proxy = Proxy::start(&upstream(&url)).await;
    proxy.fault_start(VERSION_START, Fault::Instead(answer.clone()));
    let error = ping_refused(ping(&proxy.url(&url)).await, "answer");
    assert_eq!(error, disagree(&server.label(), line), "ping");
}

/// The profile's key never decides the engine: a profile naming the sibling engine (`mariadb`
/// on MySQL, `mysql` on MariaDB) connects, and its facts name the server's engine.
#[tokio::test(flavor = "multi_thread")]
async fn a_profile_key_naming_the_sibling_connects() {
    let test = "a_profile_key_naming_the_sibling_connects";
    let Some(url) = my_url(test) else { return };
    let server = Server::of(&url).await;
    for role in ROLES {
        let opened = open(&url, server.other_key(), role, test).await;
        let (_session, before) = opened.unwrap_or_else(|e| panic!("{role:?}: {e:?}"));
        assert_eq!(facts(&before).engine(), server.engine(), "{role:?}");
    }
}

/// A greeting naming another version of the same engine leaves the version unknown (the reports
/// disagree) and the session connects; what it says before `Connected` is what the greeting
/// implies, as before.
#[tokio::test(flavor = "multi_thread")]
async fn a_version_number_the_reports_disagree_on_is_unknown_and_changes_no_word() {
    let test = "a_version_number_the_reports_disagree_on_is_unknown_and_changes_no_word";
    let Some(url) = my_url(test) else { return };
    let server = Server::of(&url).await;
    let (greeting, greeted) = if server.mariadb {
        ("5.5.5-10.11.9-MariaDB", Version::new(10, 11, 9))
    } else {
        ("8.0.40", Version::new(8, 0, 40))
    };
    for role in ROLES {
        let proxy = Proxy::start(&upstream(&url)).await;
        proxy.greet_as(greeting);
        let opened = open(&proxy.url(&url), "mysql", role, test).await;
        let (_session, before) = opened.unwrap_or_else(|e| panic!("{role:?}: {e:?}"));
        let f = facts(&before);
        assert_eq!(f.engine(), server.engine(), "{role:?}: {f:?}");
        assert_eq!(format!("{:?}", f.version), "Unknown(VersionsDisagree)", "{role:?}: {f:?}");
        assert_eq!(f.version_text.as_deref(), Some(server.line.as_str()), "{role:?}: {f:?}");
        assert_eq!(words(&before), words_of(server.engine(), greeted), "{role:?}");
    }
}

/// A new session of a profile (as the app opens on a reconnect) reads the version again: one
/// that connected before is refused once the greeting names the other engine.
#[tokio::test(flavor = "multi_thread")]
async fn a_reconnect_reads_the_version_again() {
    let test = "a_reconnect_reads_the_version_again";
    let Some(url) = my_url(test) else { return };
    let server = Server::of(&url).await;
    let proxy = Proxy::start(&upstream(&url)).await;
    let (session, _) = open(&proxy.url(&url), "mysql", SessionRole::Query, test).await.expect("first session");
    assert_eq!(proxy.queries().first().map(String::as_str), Some(VERSION_SQL));
    drop(session);
    let (greeting, label) = server.other_greeting();
    proxy.greet_as(greeting);
    let sent = proxy.queries().len();
    let error = refused(open(&proxy.url(&url), "mysql", SessionRole::Query, test).await, "reconnect");
    assert_eq!(error, disagree(label, &server.line));
    assert_eq!(proxy.queries()[sent..], [VERSION_SQL]);
}
