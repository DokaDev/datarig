//! A MySQL-family session reads the server's own version line (`SELECT VERSION() LIMIT 1`) before
//! anything that depends on the engine, and its facts name the engine that line names: the
//! greeting may be a proxy's. A greeting naming the other engine is not refused: the session
//! says and sends what the greeting implies, as it did before the line was read, with the
//! version and how it reaches the server unknown. A line that confirms no engine closes the
//! session. Against a real MySQL or MariaDB loaded with `dev/init-mysql/*.sql` (CI runs this file
//! in both jobs), through the test proxy (`my_proxy`), which rewrites the greeting or the version
//! answer.
//!
//! Connection: `DATARIG_TEST_MYSQL_URL` (e.g. `mysql://datarig:datarig@127.0.0.1:53306/shop`).
//! * unset locally  -> each test prints a visible `SKIPPED` line to stderr and returns;
//! * unset with `DATARIG_REQUIRE_MYSQL=1` -> the test fails.
//!
//! The errors are compared by their debug text, which names the variant and its fields.

#[allow(dead_code)]
mod my_proxy;

use datarig_core::driver::{
    ConnectOptions, DbCommand, DbError, DbEvent, Driver, PagingMode, PingError, PingInfo, Session, SessionRole, TxRule,
};
use datarig_core::driver::{EditFormat, ExplainFormat, MySqlEdit, SessionStatement};
use datarig_core::engine::{EngineId, Fact, ServerFacts, Topology, Version, Why};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::{Dialect, DollarQuotes, Language};
use datarig_core::sql::risk::repeat::NotRepeatable;
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

/// What every session sends before its settings.
const OPENING: [&str; 1] = [VERSION_SQL];

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

    /// Greetings naming the other engine (a proxy's), each with a line of its engine that
    /// agrees with it.
    fn other_greetings(&self) -> &'static [(&'static str, &'static str)] {
        if self.mariadb {
            // A MySQL-format greeting, and MariaDB's number with no engine in it.
            &[("8.4.6", "8.4.6"), ("11.4.13", "11.4.13")]
        } else {
            &[("5.5.5-11.4.13-MariaDB", "11.4.13-MariaDB")]
        }
    }

    /// Version lines of the other engine (MariaDB's also in the form of its greeting).
    fn other_lines(&self) -> &'static [&'static str] {
        if self.mariadb { &["8.4.6"] } else { &["11.4.13-MariaDB-ubu2404", "5.5.5-11.4.13-MariaDB"] }
    }

    /// The engine a version line of the other engine names.
    fn other_engine(&self) -> EngineId {
        if self.mariadb { EngineId::MySql } else { EngineId::MariaDb }
    }

    /// The driver's key of the other engine.
    fn other_key(&self) -> &'static str {
        if self.mariadb { "mysql" } else { "mariadb" }
    }
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

/// One session through its own proxy, and what came of it.
struct Run {
    /// What it said before `Connected` but its facts, in debug text, or why it failed.
    outcome: Result<Vec<String>, String>,
    facts: Option<Arc<ServerFacts>>,
    /// The proxy's log once it connected or failed.
    log: Vec<String>,
}

/// How the proxy of a [`Run`] is set: the greeting it writes, the version answer it puts in place
/// of the server's (a statement), the results charset it leaves the session with.
#[derive(Clone, Copy, Default)]
struct Setup<'a> {
    greeting: Option<&'a str>,
    answer: Option<&'a str>,
    charset: Option<&'a str>,
}

impl Setup<'_> {
    async fn proxy(&self, url: &str) -> Proxy {
        let proxy = Proxy::start(&upstream(url)).await;
        if let Some(c) = self.charset {
            proxy.results_charset(c);
        }
        if let Some(g) = self.greeting {
            proxy.greet_as(g);
        }
        if let Some(a) = self.answer {
            proxy.fault_start(VERSION_START, Fault::Instead(a.into()));
        }
        proxy
    }

    async fn run(&self, url: &str, role: SessionRole, test: &str) -> Run {
        let proxy = self.proxy(url).await;
        let opened = open(&proxy.url(url), "mysql", role, test).await;
        let log = proxy.queries();
        let (outcome, facts) = match &opened {
            Ok((_, before)) => {
                let words = before.iter().filter(|e| !matches!(e, DbEvent::Server(_))).map(|e| format!("{e:?}"));
                (Ok(words.collect()), Some(facts(before)))
            }
            Err(e) => (Err(format!("{e:?}")), None),
        };
        Run { outcome, facts, log }
    }

    async fn ping(&self, url: &str) -> (Result<PingInfo, PingError>, Vec<String>) {
        let proxy = self.proxy(url).await;
        let pinged = ping(&proxy.url(url)).await;
        (pinged, proxy.queries())
    }
}

/// `log` without what the session sends before its settings, and with the session's marker (a
/// random token of each session) left out.
fn legacy(log: &[String]) -> Vec<String> {
    let unmarked = |s: &str| match s.split_once("ATTR_VALUE = '") {
        Some((head, tail)) => format!("{head}ATTR_VALUE = '{}", tail.split_once('\'').map_or("", |(_, rest)| rest)),
        None => s.to_string(),
    };
    log.iter().filter(|s| !OPENING.contains(&s.as_str())).map(|s| unmarked(s)).collect()
}

/// `run` ends as `reference` does (connected with the same words, or failed with the same
/// error), but for the dollar quotes of the language it says: not known, as its two reports
/// disagree on them. It sends the same statements past the version read: all of them on a query
/// session or one that failed, the settings on the others (which go on reading once connected).
fn as_reference_but_dollar_quotes(run: &Run, reference: &Run, what: &str) {
    let not_known = reference.outcome.as_ref().map(|words| {
        words
            .iter()
            .map(|w| {
                w.replace("dollar_quotes: Read", "dollar_quotes: NotKnown")
                    .replace("dollar_quotes: NotRead", "dollar_quotes: NotKnown")
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(run.outcome, not_known.map_err(Clone::clone), "{what}");
    let (got, want) = (legacy(&run.log), legacy(&reference.log));
    if what.contains("Query") || run.outcome.is_err() {
        assert_eq!(got, want, "{what}");
    } else {
        assert_eq!(got.first(), want.first(), "{what}: the settings");
    }
}

/// Facts of a session whose greeting names another engine than its version line `line`.
fn assert_engines_disagree(f: &ServerFacts, engine: EngineId, line: &str, what: &str) {
    assert_eq!(f.engine(), engine, "{what}: {f:?}");
    assert_eq!(format!("{:?}", f.topology), "Unknown(EnginesDisagree)", "{what}: {f:?}");
    assert_eq!(format!("{:?}", f.version), "Unknown(EnginesDisagree)", "{what}: {f:?}");
    assert_eq!(f.version_text.as_deref(), Some(line), "{what}: {f:?}");
}

/// The plan format, edits and dollar quotes a greeting of MySQL `v` implies (none on MariaDB).
fn words_of(engine: EngineId, v: Version) -> (Option<ExplainFormat>, Option<EditFormat>, DollarQuotes) {
    match engine {
        EngineId::MySql => (
            (v >= Version::new(8, 0, 16)).then_some(ExplainFormat::MySqlTree),
            (v >= Version::new(8, 0, 17)).then_some(EditFormat::MySql(MySqlEdit { nowait: true })),
            if v >= Version::new(8, 1, 0) { DollarQuotes::Read } else { DollarQuotes::NotRead },
        ),
        EngineId::MariaDb | EngineId::Postgres => (None, None, DollarQuotes::NotRead),
    }
}

/// The plan format, edits and dollar quotes a session said before `Connected`.
fn words(before: &[DbEvent]) -> (Option<ExplainFormat>, Option<EditFormat>, DollarQuotes) {
    let explain = before.iter().find_map(|e| if let DbEvent::Explain(f) = e { Some(*f) } else { None });
    let edit = before.iter().find_map(|e| if let DbEvent::Edit(f) = e { Some(*f) } else { None });
    let dollar = before.iter().find_map(|e| match e {
        DbEvent::Language(Language::Sql(Dialect::MySql(m) | Dialect::MariaDb(m))) => Some(m.dollar_quotes),
        _ => None,
    });
    (explain.expect("a plan format"), edit.expect("edits"), dollar.expect("a language"))
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
        assert_eq!(log.get(..1), Some(&OPENING.map(String::from)[..]), "{role:?}: {log:?}");
        assert!(log.get(1).is_some_and(|s| s.starts_with("SET NAMES ")), "{role:?}: the settings second: {log:?}");
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
        assert_eq!(proxy.queries(), OPENING, "{role:?}");
        let proxy = Proxy::start(&upstream(&url)).await;
        proxy.fault_start(VERSION_START, Fault::Close);
        let error = open(&proxy.url(&url), "mysql", role, test).await.err();
        assert!(error.as_ref().is_some_and(is_closed), "{role:?}: {error:?}");
        assert_eq!(proxy.queries(), OPENING, "{role:?}");
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
            assert_eq!(proxy.queries(), OPENING, "{answer} {role:?}");
        }
        let proxy = Proxy::start(&upstream(&url)).await;
        proxy.fault_start(VERSION_START, Fault::Instead(answer.into()));
        let error = ping_refused(ping(&proxy.url(&url)).await, answer);
        assert_eq!(error, unconfirmed(&server.label()), "{answer}: ping");
    }
}

/// A session that a proxy (or the server's `init_connect`) leaves sending its results in a
/// character set that is not ASCII-compatible (`utf16`, `utf32`) gets even the column
/// definitions of an answer converted, which the client cannot read: the version read fails
/// with its own connection error and nothing else is sent, on every session and in a test
/// connection. Such a session never read an answer before the version read either (see
/// [`a_non_utf8_results_charset_leaves_no_answer_readable`]).
#[tokio::test(flavor = "multi_thread")]
async fn a_non_utf8_results_charset_fails_the_version_read_with_its_own_error() {
    let test = "a_non_utf8_results_charset_fails_the_version_read_with_its_own_error";
    let Some(url) = my_url(test) else { return };
    let server = Server::of(&url).await;
    let (other, _) = server.other_greeting();
    for charset in ["utf16", "utf32"] {
        for greeting in [None, Some(other)] {
            let setup = Setup { charset: Some(charset), greeting, ..Setup::default() };
            for role in ROLES {
                let what = format!("{charset} {greeting:?} {role:?}");
                let run = setup.run(&url, role, test).await;
                let error = run.outcome.err().unwrap_or_else(|| panic!("{what}: connected"));
                assert!(error.starts_with("Connection(") && error.contains("InvalidData"), "{what}: {error}");
                assert_eq!(run.log, [VERSION_SQL], "{what}");
            }
            let (pinged, log) = setup.ping(&url).await;
            let error = ping_refused(pinged, charset);
            assert!(error.starts_with("Connection(") && error.contains("InvalidData"), "{charset}: ping: {error}");
            assert_eq!(log, [VERSION_SQL], "{charset}: ping");
        }
    }
}

/// What the session's settings statement keeps of the results character set it starts with
/// (each value is set back to itself): under `utf16` or `utf32` no answer can be read on that
/// connection at all, with this client, whatever is asked; so sessions there never worked, and
/// the version read only fails first.
#[tokio::test(flavor = "multi_thread")]
async fn a_non_utf8_results_charset_leaves_no_answer_readable() {
    let test = "a_non_utf8_results_charset_leaves_no_answer_readable";
    let Some(url) = my_url(test) else { return };
    Server::of(&url).await;
    for charset in ["utf16", "utf32"] {
        let proxy = Setup { charset: Some(charset), ..Setup::default() }.proxy(&url).await;
        let opts = mysql_async::Opts::from_url(&proxy.url(&url)).expect("a mysql:// URL");
        // Known settings, so the client reads no answer of its own while it connects (as the
        // driver's sessions do not).
        let opts = mysql_async::OptsBuilder::from_opts(opts)
            .prefer_socket(false)
            .max_allowed_packet(Some(64 * 1024 * 1024))
            .wait_timeout(Some(28800));
        let mut c = mysql_async::Conn::new(opts).await.expect("the connection opens");
        let settings = "SET NAMES utf8mb4, SESSION character_set_results = @@SESSION.character_set_results";
        c.query_drop(settings).await.expect("the settings statement");
        let read: Result<Option<String>, _> = c.query_first("SELECT 1").await;
        let error = format!("{:?}", read.expect_err("no answer is readable"));
        assert!(error.contains("InvalidData") || error.contains("catalog"), "{charset}: {error}");
        let _ = c.disconnect().await;
    }
}

/// What the proxy sets, the values charset the session says, its `character_set_results` and
/// `character_set_connection`, and the text of `t` and `h`.
type Case = (&'static str, &'static str, [Option<&'static str>; 2], [&'static str; 2]);

/// A results or connection character set a proxy (or `init_connect`) leaves on the session is
/// kept by its settings, as it always was: the session reads its answers in it. The values and
/// bytes are the ones the session read before the version read was added (the same on MySQL 8.4
/// and MariaDB 11.4).
#[tokio::test(flavor = "multi_thread")]
async fn a_latin1_or_null_results_charset_stays_as_it_was() {
    let test = "a_latin1_or_null_results_charset_stays_as_it_was";
    let Some(url) = my_url(test) else { return };
    Server::of(&url).await;
    let cafe = "SELECT 'caf\u{e9}' AS t, HEX('caf\u{e9}') AS h";
    let vars = "SELECT @@SESSION.character_set_results, @@SESSION.character_set_connection";
    let cases: [Case; 3] = [
        ("latin1", "latin1", [Some("latin1"), Some("utf8mb4")], ["0x636166E9", "636166C3A9"]),
        ("NULL", "binary", [None, Some("utf8mb4")], ["caf\u{e9}", "636166C3A9"]),
        (
            "latin1, character_set_connection = latin1",
            "latin1",
            [Some("latin1"), Some("latin1")],
            ["0x636166E9", "636166E9"],
        ),
    ];
    for (setting, values, variables, bytes) in cases {
        let proxy = Setup { charset: Some(setting), ..Setup::default() }.proxy(&url).await;
        let mut c = Query::open(&proxy.url(&url), test).await;
        let said = c.values.clone().unwrap_or_else(|| panic!("{setting}: no values said"));
        assert!(said.contains(&format!("charset: {values:?}")), "{setting}: {said}");
        assert_eq!(c.rows(1, vars).await, [variables.map(|v| v.map(String::from))], "{setting}");
        assert_eq!(c.rows(2, cafe).await, [bytes.map(|b| Some(b.to_string()))], "{setting}");
    }
}

/// A query session to run statements on, and what it said of its values first.
struct Query {
    session: Session,
    rx: tokio::sync::mpsc::UnboundedReceiver<DbEvent>,
    values: Option<String>,
}

impl Query {
    async fn open(url: &str, test: &str) -> Query {
        let (tx, mut rx) = unbounded_channel();
        let role = SessionRole::Query;
        let session = MyDriver.connect(&profile(url, "mysql"), role, ConnectOptions::new(PAGE, role, test), tx);
        loop {
            match tokio::time::timeout(Duration::from_secs(15), rx.recv()).await.expect("an answer") {
                Some(DbEvent::Connected) => break,
                Some(DbEvent::ConnectFailed { error, .. }) => panic!("connect failed: {error:?}"),
                Some(_) => {}
                None => panic!("event channel closed"),
            }
        }
        let mut q = Query { session, rx, values: None };
        if let Some(DbEvent::SessionValues(v)) = q.next().await {
            q.values = Some(format!("{v:?}"));
        }
        q
    }

    async fn next(&mut self) -> Option<DbEvent> {
        tokio::time::timeout(Duration::from_secs(15), self.rx.recv()).await.expect("an event")
    }

    /// The rows of `sql`, run as run `id`.
    async fn rows(&mut self, id: u64, sql: &str) -> Vec<Vec<Option<String>>> {
        self.session.send(DbCommand::Execute {
            max_rows: None,
            id,
            statements: vec![sql.to_string()],
            paging: PagingMode::NoHold,
            tx: TxRule::Auto,
        });
        loop {
            match self.next().await {
                Some(DbEvent::Page { id: i, columns: Some(_), rows, .. }) if i == id => return rows,
                Some(ev @ (DbEvent::Failed { .. } | DbEvent::Lost { .. })) => panic!("{sql}: {ev:?}"),
                Some(_) => {}
                None => panic!("{sql}: event channel closed"),
            }
        }
    }
}

/// A version line with no number in it leaves the version unknown and the session connects:
/// `1` (what the text protocol sends for `SELECT 1`) under a MySQL greeting, a line that names
/// MariaDB and no number under MariaDB's. Its words are the greeting's. Under MariaDB's greeting
/// `1` names MySQL: the session connects with the engines disagreeing.
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
        let run = Setup { answer: Some("SELECT 1"), ..Setup::default() }.run(&url, SessionRole::Query, test).await;
        let f = run.facts.unwrap_or_else(|| panic!("1 on MariaDB: {:?}", run.outcome));
        assert_engines_disagree(&f, EngineId::MySql, "1", "1 on MariaDB");
    }
}

/// A blank version line (empty or only spaces) confirms no engine, whatever the greeting names:
/// every session is refused after the read and runs nothing else, a test connection fails, and
/// none of them goes on.
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
                assert_eq!(proxy.queries(), OPENING, "{answer} {greeting:?} {role:?}");
            }
            let proxy = start().await;
            let error = ping_refused(ping(&proxy.url(&url)).await, answer);
            assert_eq!(error, unconfirmed(label), "{answer} {greeting:?}: ping");
        }
    }
}

/// A greeting naming the other engine (a proxy's: MySQL's format in front of MariaDB, MariaDB's
/// in front of MySQL) is not refused: every session ends as the same proxy with a version line
/// that agrees with the greeting makes it end (connected with the same words, or failed on the
/// greeting's settings with the same error) and sends the same statements past the version read.
/// Its facts name the line's engine, with the version and how it reaches the server unknown. A
/// test connection succeeds and names the line's engine. A MariaDB greeting without `5.5.5-`
/// still names MariaDB. The dollar quotes the session says are not known: every greeting here
/// gives the other answer than the server's line (MariaDB's greeting none, MySQL 8.4's line
/// reads them; a MySQL-format greeting of 8.4.6 or 11.4.13 reads them, MariaDB's line none).
#[tokio::test(flavor = "multi_thread")]
async fn a_greeting_naming_the_other_engine_connects_as_today() {
    let test = "a_greeting_naming_the_other_engine_connects_as_today";
    let Some(url) = my_url(test) else { return };
    let server = Server::of(&url).await;
    for &(greeting, agreeing) in server.other_greetings() {
        let answer = format!("SELECT '{agreeing}'");
        let subject = Setup { greeting: Some(greeting), ..Setup::default() };
        let reference = Setup { greeting: Some(greeting), answer: Some(&answer), ..Setup::default() };
        for role in ROLES {
            let what = format!("{greeting} {role:?}");
            let run = subject.run(&url, role, test).await;
            as_reference_but_dollar_quotes(&run, &reference.run(&url, role, test).await, &what);
            assert_eq!(run.log.get(..1), Some(&OPENING.map(String::from)[..]), "{what}");
            if let Some(f) = &run.facts {
                assert_engines_disagree(f, server.engine(), &server.line, &what);
            }
        }
        let (pinged, log) = subject.ping(&url).await;
        let info = pinged.unwrap_or_else(|e| panic!("{greeting}: ping: {e:?}"));
        assert_eq!(info.server_version, server.line, "{greeting}: ping");
        assert_eq!(info.identity.engine, Fact::Known(server.engine()), "{greeting}: ping");
        assert_eq!(format!("{:?}", info.identity.version), "Unknown(EnginesDisagree)", "{greeting}: ping");
        assert_eq!(log, [VERSION_SQL], "{greeting}: ping");
    }
    if server.mariadb {
        let run = Setup { greeting: Some("11.4.13-MariaDB"), ..Setup::default() };
        let run = run.run(&url, SessionRole::Query, test).await;
        let f = run.facts.unwrap_or_else(|| panic!("no 5.5.5-: {:?}", run.outcome));
        assert_eq!(f.engine(), EngineId::MariaDb, "{f:?}");
        assert_eq!(f.topology, Fact::Known(Topology::Direct), "{f:?}");
    }
}

/// A version line naming the other engine under the server's own greeting (a proxy that answers
/// `VERSION()` itself) is followed: every session ends and sends what it does without the proxy
/// changing the answer, its facts name the line's engine with the version and how it reaches
/// the server unknown, and a test connection names the line's engine. The dollar quotes the
/// session says are not known: the server's greeting and the other engine's line give opposite
/// answers (MySQL 8.4 reads them, MariaDB does not).
#[tokio::test(flavor = "multi_thread")]
async fn a_rewritten_version_answer_follows_the_line() {
    let test = "a_rewritten_version_answer_follows_the_line";
    let Some(url) = my_url(test) else { return };
    let server = Server::of(&url).await;
    for &line in server.other_lines() {
        let answer = format!("SELECT '{line}'");
        let subject = Setup { answer: Some(&answer), ..Setup::default() };
        for role in ROLES {
            let what = format!("{line} {role:?}");
            let run = subject.run(&url, role, test).await;
            as_reference_but_dollar_quotes(&run, &Setup::default().run(&url, role, test).await, &what);
            let f = run.facts.as_ref().unwrap_or_else(|| panic!("{what}: {:?}", run.outcome));
            assert_engines_disagree(f, server.other_engine(), line, &what);
        }
        let (pinged, log) = subject.ping(&url).await;
        let info = pinged.unwrap_or_else(|e| panic!("{line}: ping: {e:?}"));
        assert_eq!(info.server_version, line, "{line}: ping");
        assert_eq!(info.identity.engine, Fact::Known(server.other_engine()), "{line}: ping");
        assert_eq!(format!("{:?}", info.identity.version), "Unknown(EnginesDisagree)", "{line}: ping");
        assert_eq!(log, [VERSION_SQL], "{line}: ping");
    }
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
/// implies, as before, but for the dollar quotes when the two numbers lie on either side of
/// MySQL 8.1.0 (a greeting of 8.0.40 in front of 8.4): those are not known.
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
        let (explain, edit, by_greeting) = words_of(server.engine(), greeted);
        let (_, _, by_line) = words_of(server.engine(), server.version);
        let dollar_quotes = if by_line == by_greeting { by_greeting } else { DollarQuotes::NotKnown };
        assert_eq!(words(&before), (explain, edit, dollar_quotes), "{role:?}");
    }
}

/// A new session of a profile (as the app opens on a reconnect) reads the version again: once
/// the greeting names the other engine, the next session's facts say the engines disagree.
#[tokio::test(flavor = "multi_thread")]
async fn a_reconnect_reads_the_version_again() {
    let test = "a_reconnect_reads_the_version_again";
    let Some(url) = my_url(test) else { return };
    let server = Server::of(&url).await;
    let proxy = Proxy::start(&upstream(&url)).await;
    let (session, before) = open(&proxy.url(&url), "mysql", SessionRole::Query, test).await.expect("first session");
    assert_eq!(proxy.queries().get(..1), Some(&OPENING.map(String::from)[..]));
    assert_eq!(facts(&before).topology, Fact::Known(Topology::Direct));
    drop(session);
    let (greeting, _) = server.other_greeting();
    proxy.greet_as(greeting);
    let sent = proxy.queries().len();
    let opened = open(&proxy.url(&url), "mysql", SessionRole::Query, test).await;
    assert_eq!(proxy.queries().get(sent..sent + 1), Some(&OPENING.map(String::from)[..]));
    if let Ok((_, before)) = &opened {
        assert_engines_disagree(&facts(before), server.engine(), &server.line, "reconnect");
    }
}

// ── A session whose reports name two engines is judged on both ──────────────────────────────

/// A query session through a proxy that greets as the other engine, with its events.
struct TwoEngines {
    proxy: Proxy,
    session: Session,
    rx: tokio::sync::mpsc::UnboundedReceiver<DbEvent>,
}

impl TwoEngines {
    /// A query session on `url` behind a greeting of the other engine: read-only with the
    /// tests' start-up statement when `read_only`, else with start-up statements `startup`.
    async fn open(url: &str, test: &str, read_only: bool, startup: &[&str]) -> TwoEngines {
        let server = Server::of(url).await;
        let proxy = Proxy::start(&upstream(url)).await;
        proxy.greet_as(server.other_greeting().0);
        let (tx, mut rx) = unbounded_channel();
        let tag = format!("x13{}-{test}", std::process::id());
        let o = match read_only {
            true => options(SessionRole::Query, test),
            false => ConnectOptions::new(PAGE, SessionRole::Query, &tag).session_sql(
                startup
                    .iter()
                    .map(|s| SessionStatement { sql: s.to_string(), per_transaction: None, identity: false })
                    .collect(),
            ),
        };
        let session = MyDriver.connect(&profile(&proxy.url(url), "mysql"), SessionRole::Query, o, tx);
        let mut before = Vec::new();
        loop {
            match tokio::time::timeout(Duration::from_secs(15), rx.recv()).await.expect("an answer") {
                Some(DbEvent::Connected) => break,
                Some(DbEvent::ConnectFailed { error, .. }) => panic!("connect failed: {error:?}"),
                Some(ev) => before.push(ev),
                None => panic!("event channel closed"),
            }
        }
        assert_engines_disagree(&facts(&before), server.engine(), &server.line, test);
        TwoEngines { proxy, session, rx }
    }

    /// The answer to command `cmd` (number `id`): its error, or the answer in debug text.
    async fn ask(&mut self, id: u64, cmd: DbCommand) -> Result<String, DbError> {
        self.session.send(cmd);
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(60), self.rx.recv())
                .await
                .expect("an event")
                .expect("the session goes on");
            let result = match &ev {
                DbEvent::Failed { id: i, error, .. } if *i == id => Err(error.clone()),
                DbEvent::Page { id: i, columns: Some(_), .. } | DbEvent::Done { id: i, .. } if *i == id => {
                    Ok(format!("{ev:?}"))
                }
                DbEvent::Counted { id: i, result, .. } if *i == id => result.clone().map(|n| n.to_string()),
                DbEvent::RepeatChecked { id: i, result } if *i == id => result.clone().map(|()| "repeatable".into()),
                DbEvent::Exported { id: i, result, .. } if *i == id => {
                    result.as_ref().map(|_| "exported".to_string()).map_err(Clone::clone)
                }
                _ => continue,
            };
            return result;
        }
    }

    /// [`TwoEngines::ask`] for `cmd` judged on both engines' rules, as the app judges a
    /// command once the session said its reports name two engines.
    async fn ask_both(&mut self, id: u64, cmd: DbCommand) -> Result<String, DbError> {
        let judged = datarig_core::sql::risk::mysql::Judged::WithSibling;
        self.ask(id, DbCommand::Judged { judged, command: Box::new(cmd) }).await
    }

    fn execute(id: u64, sql: &str) -> DbCommand {
        DbCommand::Execute {
            id,
            statements: vec![sql.to_string()],
            paging: PagingMode::NoHold,
            tx: TxRule::Auto,
            max_rows: None,
        }
    }

    fn count(id: u64, sql: &str) -> DbCommand {
        DbCommand::Count { id, sql: sql.into(), limit: None }
    }
}

/// A session whose greeting names the other engine is judged on both engines' rules: on a
/// read-only profile the driver's re-run allowlist refuses what either engine refuses, whatever
/// command asks (`CheckRepeat`, a `Resume` of a later page and a `Count`, which ask the server
/// about the names first), and sends nothing of it. On the MariaDB server behind a MySQL 8.4.6
/// greeting `BIN_TO_UUID` (no built-in of MariaDB's, which MySQL's rules allow); on the MySQL
/// server behind a MariaDB 11.4.13 greeting a quoted `DATABASE` (which only MariaDB reads as
/// the built-in). A plain read runs and is asked about as before.
#[tokio::test(flavor = "multi_thread")]
async fn a_disagreeing_session_is_judged_on_both_engines() {
    let test = "a_disagreeing_session_is_judged_on_both_engines";
    let Some(url) = my_url(test) else { return };
    let server = Server::of(&url).await;
    let sql = if server.mariadb {
        "SELECT BIN_TO_UUID(UNHEX('00112233445566778899AABBCCDDEEFF')) AS u"
    } else {
        "SELECT `DATABASE`() AS d"
    };
    let mut s = TwoEngines::open(&url, test, true, &[]).await;
    let resume = |id| DbCommand::Resume { id, sql: sql.into(), skip: 1, paging: PagingMode::NoHold, max_rows: None };
    let commands = [
        ("CheckRepeat", 1, DbCommand::CheckRepeat { id: 1, sql: sql.into() }),
        ("Resume", 2, resume(2)),
        ("Count", 3, TwoEngines::count(3, sql)),
    ];
    for (what, id, cmd) in commands {
        let from = s.proxy.queries().len();
        let got = s.ask_both(id, cmd).await;
        let sent = s.proxy.queries()[from..].to_vec();
        assert!(
            matches!(got, Err(DbError::NotRepeatable(NotRepeatable::UserFunction))),
            "{what}: {got:?}, sent {sent:?}"
        );
        assert!(sent.is_empty(), "{what}: nothing sent: {sent:?}");
    }
    let plain = "SELECT 1 AS a";
    let got = s.ask_both(10, TwoEngines::execute(10, plain)).await;
    assert!(got.as_deref().is_ok_and(|ev| ev.starts_with("Page")), "{got:?}");
    let from = s.proxy.queries().len();
    let got = s.ask_both(11, DbCommand::CheckRepeat { id: 11, sql: plain.into() }).await;
    assert!(!matches!(got, Err(DbError::NotRepeatable(_))), "{got:?}");
    assert!(s.proxy.queries().len() > from, "the server was asked, as before: {got:?}");
}

/// MariaDB's own executable comments (`/*M! … */`) on a session whose greeting names the other
/// engine: either engine may be the one that runs the text, so a command holding one is refused
/// before anything of it is sent, whichever server is behind the greeting. A `/*! … */` comment
/// goes on as before.
#[tokio::test(flavor = "multi_thread")]
async fn a_disagreeing_session_sends_no_mariadb_only_comment() {
    let test = "a_disagreeing_session_sends_no_mariadb_only_comment";
    let Some(url) = my_url(test) else { return };
    let mut s = TwoEngines::open(&url, test, false, &[]).await;
    let sql = "SELECT 1 /*M! , 2 */";
    for (what, id, cmd) in [("Execute", 1, TwoEngines::execute(1, sql)), ("Count", 2, TwoEngines::count(2, sql))] {
        let from = s.proxy.queries().len();
        let got = s.ask_both(id, cmd).await;
        let sent = s.proxy.queries()[from..].to_vec();
        assert!(got.as_ref().is_err_and(|e| !matches!(e, DbError::Server(_))), "{what}: {got:?}, sent {sent:?}");
        assert!(sent.is_empty(), "{what}: nothing sent: {sent:?}");
    }
    let got = s.ask_both(3, TwoEngines::execute(3, "SELECT 1 /*! , 2 */")).await;
    assert!(got.as_deref().is_ok_and(|ev| ev.starts_with("Page")), "{got:?}");
}

/// The next transaction's settings on a session whose greeting names the other engine: a
/// statement counts as setting them when either engine's reading says so, the stronger answer
/// winning. An executable comment for MySQL 8 (`/*!80000 … */`, which MariaDB reads as a
/// comment) may set them: after it, whether it succeeded or failed, a count waits for a
/// transaction to start or end (`NextTransactionSet`); a `COMMIT` clears that. A plain `SET
/// TRANSACTION` sets them only once it succeeded. A start-up statement counts the same way. On
/// the MariaDB server (behind a MySQL greeting) MySQL's reading counted it before too; on the
/// MySQL server (behind a MariaDB greeting) MariaDB's reading alone counted it as nothing.
#[tokio::test(flavor = "multi_thread")]
async fn the_next_transaction_state_after_success_and_after_failure() {
    let test = "the_next_transaction_state_after_success_and_after_failure";
    let Some(url) = my_url(test) else { return };
    let mut s = TwoEngines::open(&url, test, false, &[]).await;
    let waits = |got: &Result<String, DbError>| matches!(got, Err(DbError::NextTransactionSet));
    let mut id = 0;
    let mut step = |sql: &'static str| {
        id += 2;
        (id, sql)
    };
    let steps: [(&str, Option<bool>, bool); 6] = [
        // (statement, whether it succeeds where known, whether a count then waits)
        ("/*!80000 SET TRANSACTION READ ONLY */", None, true),
        ("COMMIT", Some(true), false),
        // MySQL refuses the level; MariaDB reads only a comment, which it answers with OK.
        ("/*!80000 SET TRANSACTION ISOLATION LEVEL NO_SUCH_LEVEL */", None, true),
        ("COMMIT", Some(true), false),
        ("SET TRANSACTION ISOLATION LEVEL NO_SUCH_LEVEL", Some(false), false),
        ("SET TRANSACTION READ ONLY", Some(true), true),
    ];
    for (sql, succeeds, then_waits) in steps {
        let (n, sql) = step(sql);
        let ran = s.ask_both(n, TwoEngines::execute(n, sql)).await;
        if let Some(ok) = succeeds {
            assert_eq!(ran.is_ok(), ok, "{sql}: {ran:?}");
        }
        let got = s.ask_both(n + 1, TwoEngines::count(n + 1, "SELECT 1 AS a")).await;
        assert_eq!(waits(&got), then_waits, "after {sql} ({ran:?}): {got:?}");
    }
    // A start-up statement counts the same way.
    let mut s = TwoEngines::open(&url, test, false, &["/*!80000 SET TRANSACTION READ ONLY */"]).await;
    let got = s.ask_both(1, TwoEngines::count(1, "SELECT 1 AS a")).await;
    assert!(waits(&got), "after the start-up statement: {got:?}");
}

/// Whether `got` is the refusal of a command judged on fewer engines than the session's
/// server's reports name (its rules changed after it was judged: run it again).
fn judged_on_fewer(got: &Result<String, DbError>) -> bool {
    got.as_ref().is_err_and(|e| format!("{e:?}") == "ReportsChanged")
}

/// A command judged before the session said its server's reports name two engines (judged on
/// one engine's rules, as every command the app judged before such facts) reaches no server
/// whose session found two engines: `Execute`, `Resume`, `Count` and `Export` are refused with
/// the reason that the server's reports changed, and nothing of them is sent. The user runs it
/// again, judged anew.
#[tokio::test(flavor = "multi_thread")]
async fn a_command_judged_on_fewer_engines_than_the_session_is_not_sent() {
    let test = "a_command_judged_on_fewer_engines_than_the_session_is_not_sent";
    let Some(url) = my_url(test) else { return };
    let mut s = TwoEngines::open(&url, test, false, &[]).await;
    let (sink, _keep) = tokio::sync::mpsc::channel(4);
    let commands = [
        ("Execute", 1, TwoEngines::execute(1, "DO BIN_TO_UUID(UNHEX('00112233445566778899AABBCCDDEEFF'))")),
        (
            "Resume",
            2,
            DbCommand::Resume {
                id: 2,
                sql: "SELECT 1 AS a".into(),
                skip: 0,
                paging: PagingMode::NoHold,
                max_rows: None,
            },
        ),
        ("Count", 3, TwoEngines::count(3, "SELECT 1 AS a")),
        (
            "Export",
            4,
            DbCommand::Export {
                id: 4,
                sql: "SELECT 1 AS a".into(),
                guard: datarig_core::driver::ExportGuard::Confirmed,
                sink,
                batch: 10,
            },
        ),
    ];
    let mut through = Vec::new();
    for (what, id, cmd) in commands {
        let from = s.proxy.queries().len();
        let got = s.ask(id, cmd).await;
        let sent = s.proxy.queries()[from..].to_vec();
        if !judged_on_fewer(&got) || !sent.is_empty() {
            through.push(format!("{what}: {got:?}, sent {sent:?}"));
        }
    }
    assert!(through.is_empty(), "{} got through:\n{}", through.len(), through.join("\n"));
}

/// A session whose reports name one engine is unchanged: a command judged on that engine's
/// rules is sent and answered as before.
#[tokio::test(flavor = "multi_thread")]
async fn a_command_on_an_agreeing_session_is_sent_as_before() {
    let test = "a_command_on_an_agreeing_session_is_sent_as_before";
    let Some(url) = my_url(test) else { return };
    let proxy = Proxy::start(&upstream(&url)).await;
    let (tx, mut rx) = unbounded_channel();
    let o = ConnectOptions::new(PAGE, SessionRole::Query, &format!("x13{}-{test}", std::process::id()));
    let session = MyDriver.connect(&profile(&proxy.url(&url), "mysql"), SessionRole::Query, o, tx);
    loop {
        match tokio::time::timeout(Duration::from_secs(15), rx.recv()).await.expect("an answer") {
            Some(DbEvent::Connected) => break,
            Some(DbEvent::ConnectFailed { error, .. }) => panic!("connect failed: {error:?}"),
            Some(_) => {}
            None => panic!("event channel closed"),
        }
    }
    let mut s = TwoEngines { proxy, session, rx };
    let got = s.ask(1, TwoEngines::execute(1, "SELECT 1 AS a")).await;
    assert!(got.as_deref().is_ok_and(|ev| ev.starts_with("Page")), "{got:?}");
    assert!(s.proxy.queries().iter().any(|q| q == "SELECT 1 AS a"), "sent");
    let got = s.ask(2, TwoEngines::count(2, "SELECT 1 AS a")).await;
    assert!(!judged_on_fewer(&got), "{got:?}");
}
