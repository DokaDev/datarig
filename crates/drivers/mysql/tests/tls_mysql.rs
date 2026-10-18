//! TLS against a real MySQL 8.4 that takes encrypted connections only
//! (`require_secure_transport=ON`, `dev/tls/up.sh`, certificates of `dev/tls/make-certs.sh`).
//!
//! `DATARIG_TEST_MYSQL_TLS_URL` (`mysql://datarig:datarig@localhost:53307/datarig`, by the name
//! the certificate has), `DATARIG_TEST_MYSQL_TLS_ADMIN_URL`
//! (`mysql://root:datarig-root@localhost:53307/`) and `DATARIG_TEST_TLS_CERTS` (the
//! certificates' directory). Unset: each test prints `SKIPPED` and returns; with
//! `DATARIG_REQUIRE_TLS=1` (CI's integration job) it fails instead.
//!
//! Each login that matters is a full `caching_sha2_password` one: the server's cache of
//! verified logins is emptied first (`FLUSH PRIVILEGES`), so the server asks for the password
//! itself.

use datarig_core::driver::{
    ConnectOptions, DbCommand, DbError, DbEvent, Driver, PagingMode, Session, SessionRole, TxRule,
};
use datarig_core::fault::Fault;
use datarig_core::profile::ConnectionConfig;
use datarig_core::profile::dsn;
use datarig_core::tls::{Encryption, TlsProblem, TlsVersion, Verified};
use datarig_core::transport::{BoxedStream, DialError, Dialer, DialerRef};
use datarig_driver_mysql::MyDriver;
use futures::future::BoxFuture;
use std::io::Write;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

struct Env {
    url: String,
    admin: String,
    certs: String,
}

/// The tests of this file one at a time: emptying the server's login cache in one would turn
/// another's login into a full one halfway.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn tls_env(test: &str) -> Option<Env> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    match (var("DATARIG_TEST_MYSQL_TLS_URL"), var("DATARIG_TEST_MYSQL_TLS_ADMIN_URL"), var("DATARIG_TEST_TLS_CERTS")) {
        (Some(url), Some(admin), Some(certs)) => Some(Env { url, admin, certs }),
        _ => {
            if std::env::var("DATARIG_REQUIRE_TLS").is_ok_and(|v| v == "1") {
                panic!(
                    "DATARIG_TEST_MYSQL_TLS_URL, DATARIG_TEST_MYSQL_TLS_ADMIN_URL and DATARIG_TEST_TLS_CERTS must be set when DATARIG_REQUIRE_TLS=1"
                );
            }
            let _ = writeln!(
                std::io::stderr(),
                "SKIPPED {test}: start dev/tls/up.sh, then set DATARIG_TEST_MYSQL_TLS_URL=mysql://datarig:datarig@localhost:53307/datarig \
                 DATARIG_TEST_MYSQL_TLS_ADMIN_URL=mysql://root:datarig-root@localhost:53307/ DATARIG_TEST_TLS_CERTS=<dir>"
            );
            None
        }
    }
}

/// The profile of `url` (fields) with a TLS mode (empty: none) and a CA file of `certs`.
fn profile(url: &str, mode: &str, certs: &str, ca: Option<&str>) -> ConnectionConfig {
    let d = dsn::parse(url).expect("a URL");
    ConnectionConfig {
        name: "tls".into(),
        driver: "mysql".into(),
        host: d.host,
        port: d.port.unwrap_or(3306),
        user: d.user,
        password: d.password.unwrap_or_default(),
        database: d.database,
        sslmode: mode.into(),
        sslrootcert: ca.map(|c| format!("{certs}/{c}")),
        ..ConnectionConfig::default()
    }
}

fn start(
    cfg: &ConnectionConfig,
    role: SessionRole,
    dialer: Option<DialerRef>,
) -> (Session, UnboundedReceiver<DbEvent>) {
    let (tx, rx) = unbounded_channel();
    let opts = ConnectOptions::new(100, role, "datarig-tls-test").dialer(dialer);
    (MyDriver.connect(cfg, role, opts, tx), rx)
}

async fn next(rx: &mut UnboundedReceiver<DbEvent>) -> DbEvent {
    tokio::time::timeout(Duration::from_secs(20), rx.recv()).await.expect("an event in time").expect("open")
}

/// How a session of `cfg` connected: what it said about its traffic, or why it failed.
async fn connect(cfg: &ConnectionConfig, dialer: Option<DialerRef>) -> Result<Encryption, DbError> {
    let (_session, mut rx) = start(cfg, SessionRole::Meta, dialer);
    let mut encryption = None;
    loop {
        match next(&mut rx).await {
            DbEvent::Encryption(e) => encryption = Some(e),
            DbEvent::Connected => return Ok(encryption.expect("Encryption before Connected")),
            DbEvent::ConnectFailed { error, .. } => return Err(error),
            _ => {}
        }
    }
}

/// The first cell of `sql` run on a query session of `cfg` (connected).
async fn cell(cfg: &ConnectionConfig, sql: &str) -> Option<String> {
    let (session, mut rx) = start(cfg, SessionRole::Query, None);
    session.send(DbCommand::Execute {
        id: 1,
        statements: vec![sql.into()],
        paging: PagingMode::Hold,
        tx: TxRule::Auto,
    });
    loop {
        match next(&mut rx).await {
            DbEvent::Page { rows, .. } => return rows.first().and_then(|r| r.get(1).or(r.first())).cloned().flatten(),
            DbEvent::Done { .. } => return None,
            DbEvent::ConnectFailed { error, .. } | DbEvent::Failed { error, .. } => panic!("{sql}: {error:?}"),
            _ => {}
        }
    }
}

/// Empty the server's cache of verified logins: the next login of each account is a full one.
async fn forget_logins(env: &Env) {
    let mut admin = profile(&env.admin, "REQUIRED", &env.certs, None);
    admin.database = String::new();
    let (session, mut rx) = start(&admin, SessionRole::Query, None);
    session.send(DbCommand::Execute {
        id: 1,
        statements: vec!["FLUSH PRIVILEGES".into()],
        paging: PagingMode::Hold,
        tx: TxRule::Auto,
    });
    loop {
        match next(&mut rx).await {
            DbEvent::Done { .. } => return,
            DbEvent::ConnectFailed { error, .. } | DbEvent::Failed { error, .. } => {
                panic!("FLUSH PRIVILEGES: {error:?}")
            }
            _ => {}
        }
    }
}

fn tls(verified: Verified) -> Result<Encryption, DbError> {
    Ok(Encryption::Tls { version: TlsVersion::Tls13, verified })
}

#[tokio::test(flavor = "multi_thread")]
async fn every_mode_against_a_server_that_takes_tls_only() {
    let Some(env) = tls_env("every_mode_against_a_server_that_takes_tls_only") else { return };
    let _serial = SERIAL.lock().await;
    let at = |mode: &str, ca: Option<&str>| profile(&env.url, mode, &env.certs, ca);
    let by_ip = |mut c: ConnectionConfig| {
        c.host = "127.0.0.1".into();
        c
    };
    let cases: Vec<(ConnectionConfig, Result<Encryption, DbError>)> = vec![
        // No mode: PREFERRED (the server offers TLS), not checked; a full login sends the
        // password inside TLS on this machine.
        (at("", None), tls(Verified::No)),
        (at("REQUIRED", None), tls(Verified::No)),
        // REQUIRED with a CA file checks the chain.
        (at("REQUIRED", Some("ca.crt")), tls(Verified::Chain)),
        (at("VERIFY_CA", Some("ca.crt")), tls(Verified::Chain)),
        (by_ip(at("VERIFY_CA", Some("ca.crt"))), tls(Verified::Chain)),
        (at("VERIFY_CA", Some("other-ca.crt")), Err(DbError::Tls(TlsProblem::Untrusted))),
        (at("VERIFY_IDENTITY", Some("ca.crt")), tls(Verified::Full)),
        (
            by_ip(at("VERIFY_IDENTITY", Some("ca.crt"))),
            Err(DbError::Tls(TlsProblem::NameMismatch { host: "127.0.0.1".into() })),
        ),
        (at("VERIFY_IDENTITY", None), Err(DbError::Tls(TlsProblem::Untrusted))),
        // TLS off: the server refuses.
        (at("DISABLED", None), Err(DbError::TlsRequired)),
    ];
    for (cfg, want) in cases {
        forget_logins(&env).await;
        let got = connect(&cfg, None).await;
        assert_eq!(got, want, "{} {:?} {}", cfg.sslmode, cfg.sslrootcert, cfg.host);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_sees_the_session_encrypted_and_the_test_says_how() {
    let Some(env) = tls_env("the_server_sees_the_session_encrypted_and_the_test_says_how") else { return };
    let _serial = SERIAL.lock().await;
    let cfg = profile(&env.url, "VERIFY_IDENTITY", &env.certs, Some("ca.crt"));
    forget_logins(&env).await;
    assert_eq!(cell(&cfg, "SHOW SESSION STATUS LIKE 'Ssl_version'").await.as_deref(), Some("TLSv1.3"));
    // The login over TLS still carries the connection's attributes (its program name; root
    // may read them).
    let mut root = profile(&env.admin, "VERIFY_IDENTITY", &env.certs, Some("ca.crt"));
    root.database = String::new();
    let program = cell(
        &root,
        "SELECT 'program', ATTR_VALUE FROM performance_schema.session_connect_attrs \
         WHERE PROCESSLIST_ID = CONNECTION_ID() AND ATTR_NAME = 'program_name'",
    )
    .await;
    assert!(program.as_deref().is_some_and(|p| p.contains("tls-test")), "{program:?}");
    let test = MyDriver.ping(&cfg, Duration::from_secs(10), None).await.expect("the test connects");
    assert_eq!(test.encryption, Encryption::Tls { version: TlsVersion::Tls13, verified: Verified::Full });
}

/// To another machine (an address that is not loopback, without a tunnel), a TLS connection
/// whose server was not verified never carries the password itself: the login is refused
/// before anything about it is sent, unless the server was verified or the profile trusts it.
#[tokio::test(flavor = "multi_thread")]
async fn the_password_goes_over_tls_only_to_a_server_that_was_checked() {
    let Some(env) = tls_env("the_password_goes_over_tls_only_to_a_server_that_was_checked") else { return };
    let _serial = SERIAL.lock().await;
    // 0.0.0.0 reaches this machine, and is not a loopback address: as another machine.
    let remote = |mode: &str, ca: Option<&str>| {
        let mut c = profile(&env.url, mode, &env.certs, ca);
        c.host = "0.0.0.0".into();
        c
    };
    forget_logins(&env).await;
    assert_eq!(connect(&remote("REQUIRED", None), None).await, Err(DbError::Tls(TlsProblem::PasswordUnverified)));
    forget_logins(&env).await;
    assert_eq!(connect(&remote("", None), None).await, Err(DbError::Tls(TlsProblem::PasswordUnverified)));
    // Checked: the password goes.
    forget_logins(&env).await;
    assert_eq!(connect(&remote("VERIFY_CA", Some("ca.crt")), None).await, tls(Verified::Chain));
    // The profile trusts whoever answers (as it trusts the key such a server sends).
    forget_logins(&env).await;
    let mut trusting = remote("REQUIRED", None);
    trusting.allow_public_key_retrieval = true;
    assert_eq!(connect(&trusting, None).await, tls(Verified::No));
    // A login the server knows already (its cache) never needs the password itself.
    assert_eq!(connect(&remote("REQUIRED", None), None).await, tls(Verified::No));
}

/// A stream that keeps what was written to it.
struct Recorded {
    inner: tokio::net::TcpStream,
    written: Arc<Mutex<Vec<u8>>>,
}

impl AsyncRead for Recorded {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Recorded {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        let n = std::task::ready!(Pin::new(&mut self.inner).poll_write(cx, buf))?;
        self.written.lock().unwrap().extend_from_slice(&buf[..n]);
        Poll::Ready(Ok(n))
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// A dialer as a tunnel is one: whatever host and port it is asked for, it reaches the TLS
/// server; each stream's writes are kept.
struct Tunnel {
    to: (String, u16),
    streams: Mutex<Vec<Arc<Mutex<Vec<u8>>>>>,
}

impl Dialer for Tunnel {
    fn dial(&self, _host: &str, _port: u16) -> BoxFuture<'static, Result<BoxedStream, DialError>> {
        let written = Arc::new(Mutex::new(Vec::new()));
        self.streams.lock().unwrap().push(written.clone());
        let to = self.to.clone();
        Box::pin(async move {
            match tokio::net::TcpStream::connect((to.0.as_str(), to.1)).await {
                Ok(inner) => Ok(Box::new(Recorded { inner, written }) as BoxedStream),
                Err(e) => Err(DialError::Failed(Fault::io(&e))),
            }
        })
    }
}

/// Through a tunnel the certificate is checked against the profile's host, and the cancel's
/// connection (`KILL QUERY`) is encrypted like the session's.
#[tokio::test(flavor = "multi_thread")]
async fn through_a_tunnel_the_profiles_host_is_checked_and_the_kill_is_encrypted() {
    let Some(env) = tls_env("through_a_tunnel_the_profiles_host_is_checked_and_the_kill_is_encrypted") else { return };
    let _serial = SERIAL.lock().await;
    let d = dsn::parse(&env.url).unwrap();
    let tunnel = Arc::new(Tunnel { to: ("127.0.0.1".into(), d.port.unwrap()), streams: Mutex::default() });
    let dialer = Some(DialerRef(tunnel.clone() as Arc<dyn Dialer>));
    let mut cfg = profile(&env.url, "VERIFY_IDENTITY", &env.certs, Some("ca.crt"));
    cfg.host = "mysql-tls.test".into();
    cfg.port = 3306;
    forget_logins(&env).await;
    assert_eq!(connect(&cfg, dialer.clone()).await, tls(Verified::Full));
    let mut other = cfg.clone();
    other.host = "db.internal".into();
    assert_eq!(
        connect(&other, dialer.clone()).await,
        Err(DbError::Tls(TlsProblem::NameMismatch { host: "db.internal".into() }))
    );
    tunnel.streams.lock().unwrap().clear();
    let (session, mut rx) = start(&cfg, SessionRole::Query, dialer);
    loop {
        match next(&mut rx).await {
            DbEvent::Connected => break,
            DbEvent::ConnectFailed { error, .. } => panic!("{error:?}"),
            _ => {}
        }
    }
    session.send(DbCommand::Execute {
        id: 1,
        statements: vec!["SELECT SLEEP(30)".into()],
        paging: PagingMode::Hold,
        tx: TxRule::Auto,
    });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let t0 = Instant::now();
    session.cancel();
    loop {
        match next(&mut rx).await {
            // A killed SLEEP answers 1 at once.
            DbEvent::Page { rows, .. } => {
                assert_eq!(rows[0][0].as_deref(), Some("1"), "interrupted");
                break;
            }
            DbEvent::Failed { error, .. } => panic!("{error:?}"),
            _ => {}
        }
    }
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
    let streams = tunnel.streams.lock().unwrap();
    assert!(streams.len() >= 2, "the session's connection and the kill's: {}", streams.len());
    for s in streams.iter() {
        let w = s.lock().unwrap();
        // The SSLRequest (a 32-byte packet, sequence 1), then a TLS handshake record: the login
        // and everything after it is encrypted.
        assert_eq!(&w[..4], &[32, 0, 0, 1], "the SSLRequest first");
        assert_eq!(w.get(36), Some(&0x16), "a TLS handshake record follows");
    }
}
