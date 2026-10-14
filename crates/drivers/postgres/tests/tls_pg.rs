//! TLS against a real PostgreSQL with TLS on (`dev/tls/up.sh`: certificates of
//! `dev/tls/make-certs.sh`, every login over TLS, the user `zz_cert` by its client certificate)
//! and the dev PostgreSQL (no TLS).
//!
//! `DATARIG_TEST_PG_TLS_URL` (`postgres://datarig:datarig@localhost:55433/datarig`, by the name
//! the certificate has) and `DATARIG_TEST_TLS_CERTS` (the certificates' directory);
//! `DATARIG_TEST_PG_URL` for the server without TLS. Unset: each test prints `SKIPPED` and
//! returns; with `DATARIG_REQUIRE_TLS=1` (CI's integration job) it fails instead.

use datarig_core::driver::{ConnectOptions, DbCommand, DbError, DbEvent, Driver, PagingMode, SessionRole};
use datarig_core::fault::Fault;
use datarig_core::profile::ConnectionConfig;
use datarig_core::profile::dsn;
use datarig_core::tls::{Encryption, FileProblem, TlsFile, TlsProblem, TlsVersion, Verified};
use datarig_core::transport::{BoxedStream, DialError, Dialer, DialerRef};
use datarig_driver_postgres::PgDriver;
use futures::future::BoxFuture;
use std::io::Write;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// The TLS server's URL and the certificates' directory.
fn tls_env(test: &str) -> Option<(String, String)> {
    match (std::env::var("DATARIG_TEST_PG_TLS_URL"), std::env::var("DATARIG_TEST_TLS_CERTS")) {
        (Ok(u), Ok(c)) if !u.is_empty() && !c.is_empty() => Some((u, c)),
        _ => {
            if std::env::var("DATARIG_REQUIRE_TLS").is_ok_and(|v| v == "1") {
                panic!("DATARIG_TEST_PG_TLS_URL and DATARIG_TEST_TLS_CERTS must be set when DATARIG_REQUIRE_TLS=1");
            }
            let _ = writeln!(
                std::io::stderr(),
                "SKIPPED {test}: set DATARIG_TEST_PG_TLS_URL=postgres://datarig:datarig@localhost:55433/datarig \
                 and DATARIG_TEST_TLS_CERTS=<dir of dev/tls/make-certs.sh>"
            );
            None
        }
    }
}

/// The profile of `url` with a TLS mode and files of `certs` (`None`: none).
fn profile(url: &str, mode: &str, certs: &str, files: [Option<&str>; 3]) -> ConnectionConfig {
    let d = dsn::parse(url).expect("a URL");
    let file = |f: Option<&str>| f.map(|f| format!("{certs}/{f}"));
    ConnectionConfig {
        name: "tls".into(),
        host: d.host,
        port: d.port.unwrap_or(5432),
        user: d.user,
        password: d.password.unwrap_or_default(),
        database: d.database,
        sslmode: mode.into(),
        sslrootcert: file(files[0]),
        sslcert: file(files[1]),
        sslkey: file(files[2]),
        ..ConnectionConfig::default()
    }
}

/// A session of `cfg` (`role`), through `dialer` when there is one, and its events.
fn start(
    cfg: &ConnectionConfig,
    role: SessionRole,
    dialer: Option<DialerRef>,
) -> (datarig_core::driver::Session, UnboundedReceiver<DbEvent>) {
    let (tx, rx) = unbounded_channel();
    let opts = ConnectOptions::new(100, role, "datarig-tls-test").dialer(dialer);
    (PgDriver.connect(cfg, role, opts, tx), rx)
}

/// How a session of `cfg` connected: what it said about its traffic, or why it failed.
async fn connect(cfg: &ConnectionConfig, dialer: Option<DialerRef>) -> Result<Encryption, DbError> {
    let (_session, mut rx) = start(cfg, SessionRole::Meta, dialer);
    let mut encryption = None;
    loop {
        match tokio::time::timeout(Duration::from_secs(20), rx.recv()).await.expect("an event in time") {
            Some(DbEvent::Encryption(e)) => encryption = Some(e),
            Some(DbEvent::Connected) => return Ok(encryption.expect("Encryption before Connected")),
            Some(DbEvent::ConnectFailed { error, .. }) => return Err(error),
            Some(_) => {}
            None => panic!("the session ended without a word"),
        }
    }
}

const TLS13: TlsVersion = TlsVersion::Tls13;
fn tls(verified: Verified) -> Result<Encryption, DbError> {
    Ok(Encryption::Tls { version: TLS13, verified })
}

#[tokio::test(flavor = "multi_thread")]
async fn every_mode_against_a_trusted_an_untrusted_and_a_misnamed_certificate() {
    let Some((url, certs)) = tls_env("every_mode_against_a_trusted_an_untrusted_and_a_misnamed_certificate") else {
        return;
    };
    let at = |mode: &str, ca: Option<&str>| profile(&url, mode, &certs, [ca, None, None]);
    // By its name (the certificate's), by an address it does not name.
    let by_ip = |mut c: ConnectionConfig| {
        c.host = "127.0.0.1".into();
        c
    };
    let untrusted = || Err(DbError::Tls(TlsProblem::Untrusted));
    let mismatch = || Err(DbError::Tls(TlsProblem::NameMismatch { host: "127.0.0.1".into() }));
    let cases: Vec<(ConnectionConfig, Result<Encryption, DbError>)> = vec![
        // Not checked: any certificate, any name.
        (at("prefer", None), tls(Verified::No)),
        (at("require", None), tls(Verified::No)),
        (by_ip(at("require", None)), tls(Verified::No)),
        // `require` with a CA file checks the chain (libpq).
        (at("require", Some("ca.crt")), tls(Verified::Chain)),
        (at("require", Some("other-ca.crt")), untrusted()),
        // The chain, not the name.
        (at("verify-ca", Some("ca.crt")), tls(Verified::Chain)),
        (by_ip(at("verify-ca", Some("ca.crt"))), tls(Verified::Chain)),
        (at("verify-ca", Some("other-ca.crt")), untrusted()),
        // The chain and the name.
        (at("verify-full", Some("ca.crt")), tls(Verified::Full)),
        (by_ip(at("verify-full", Some("ca.crt"))), mismatch()),
        (at("verify-full", Some("other-ca.crt")), untrusted()),
        // No CA file: the system's trusted certificates, which do not include the test CA.
        (at("verify-full", None), untrusted()),
        // TLS off: the server takes TLS logins only.
        (at("disable", None), Err(DbError::Server(String::new()))),
    ];
    for (cfg, want) in cases {
        let got = connect(&cfg, None).await;
        let label =
            format!("{} {:?} {:?}", cfg.sslmode, cfg.sslrootcert.as_deref().map(|p| &p[certs.len()..]), cfg.host);
        match (&got, &want) {
            // The server's own words ("no pg_hba.conf entry … no encryption").
            (Err(DbError::Server(m)), Err(DbError::Server(_))) => assert!(m.contains("no encryption"), "{label}: {m}"),
            _ => assert_eq!(got, want, "{label}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn without_tls_on_the_server_prefer_is_plain_and_require_says_so() {
    let Some((_, certs)) = tls_env("without_tls_on_the_server_prefer_is_plain_and_require_says_so") else { return };
    let Some(plain_url) = std::env::var("DATARIG_TEST_PG_URL").ok().filter(|u| !u.is_empty()) else {
        if std::env::var("DATARIG_REQUIRE_TLS").is_ok_and(|v| v == "1") {
            panic!("DATARIG_TEST_PG_URL (a server without TLS) must be set when DATARIG_REQUIRE_TLS=1");
        }
        let _ = writeln!(std::io::stderr(), "SKIPPED without_tls_on_the_server_…: set DATARIG_TEST_PG_URL");
        return;
    };
    // The server answers the SSLRequest with `N`: `prefer` goes on unencrypted, and says so.
    assert_eq!(connect(&profile(&plain_url, "prefer", &certs, [None; 3]), None).await, Ok(Encryption::Plain));
    for mode in ["require", "verify-full"] {
        let got = connect(&profile(&plain_url, mode, &certs, [Some("ca.crt"), None, None]), None).await;
        assert_eq!(got, Err(DbError::Tls(TlsProblem::Unsupported)), "{mode}");
    }
    let test = PgDriver.ping(&profile(&plain_url, "prefer", &certs, [None; 3]), Duration::from_secs(10), None).await;
    assert_eq!(test.map(|t| t.encryption), Ok(Encryption::Plain));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_certificate_logs_in_and_a_wrong_one_is_refused() {
    let Some((url, certs)) = tls_env("a_client_certificate_logs_in_and_a_wrong_one_is_refused") else { return };
    let user = |files: [Option<&str>; 3]| {
        let mut c = profile(&url, "verify-full", &certs, files);
        c.user = "zz_cert".into();
        c.password = String::new();
        c
    };
    let ok = connect(&user([Some("ca.crt"), Some("client.crt"), Some("client.key")]), None).await;
    assert_eq!(ok, tls(Verified::Full));
    // Without one: the server's own refusal (its rule needs a certificate).
    let none = connect(&user([Some("ca.crt"), None, None]), None).await;
    assert!(matches!(&none, Err(DbError::Server(m)) if m.contains("certificate")), "{none:?}");
    // One of a CA the server does not take: refused in the TLS layer.
    let stranger = connect(&user([Some("ca.crt"), Some("stranger.crt"), Some("stranger.key")]), None).await;
    assert_eq!(stranger, Err(DbError::Tls(TlsProblem::ClientCertRejected)));
    // A key file that is not there: said before anything is sent.
    let missing = connect(&user([Some("ca.crt"), Some("client.crt"), Some("nope.key")]), None).await;
    assert!(
        matches!(
            &missing,
            Err(DbError::Tls(TlsProblem::File { file: TlsFile::Key, problem: FileProblem::Unreadable(_), .. }))
        ),
        "{missing:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_sees_the_session_encrypted_and_the_test_says_how() {
    let Some((url, certs)) = tls_env("the_server_sees_the_session_encrypted_and_the_test_says_how") else { return };
    let cfg = profile(&url, "verify-full", &certs, [Some("ca.crt"), None, None]);
    let (session, mut rx) = start(&cfg, SessionRole::Query, None);
    session.send(DbCommand::Execute {
        id: 1,
        statements: vec!["SELECT ssl::text || ' ' || version FROM pg_stat_ssl WHERE pid = pg_backend_pid()".into()],
        paging: PagingMode::Hold,
    });
    let row = loop {
        match tokio::time::timeout(Duration::from_secs(20), rx.recv()).await.unwrap().unwrap() {
            DbEvent::Page { rows, .. } => break rows[0][0].clone(),
            DbEvent::ConnectFailed { error, .. } | DbEvent::Failed { error, .. } => panic!("{error:?}"),
            _ => {}
        }
    };
    assert_eq!(row.as_deref(), Some("true TLSv1.3"));
    // SCRAM with channel binding (SCRAM-SHA-256-PLUS): required, the login still works, so the
    // binding data of the server's certificate is right.
    let bound = ConnectionConfig {
        dsn: Some(format!(
            "{url}?channel_binding=require&sslmode=verify-full&sslrootcert={certs}/ca.crt",
            url = url.trim_end_matches('/')
        )),
        ..cfg.clone()
    };
    assert_eq!(connect(&bound, None).await, tls(Verified::Full));
    let test = PgDriver.ping(&cfg, Duration::from_secs(10), None).await.expect("the test connects");
    assert_eq!(test.encryption, Encryption::Tls { version: TLS13, verified: Verified::Full });
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

/// A dialer as a tunnel is one: whatever host and port it is asked for (the far end resolves
/// them), it reaches the TLS server; each stream's writes are kept.
struct Tunnel {
    to: (String, u16),
    asked: Mutex<Vec<(String, u16)>>,
    streams: Mutex<Vec<Arc<Mutex<Vec<u8>>>>>,
}

impl Dialer for Tunnel {
    fn dial(&self, host: &str, port: u16) -> BoxFuture<'static, Result<BoxedStream, DialError>> {
        self.asked.lock().unwrap().push((host.to_string(), port));
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

/// The SSLRequest packet: length 8, code 80877103.
const SSL_REQUEST: [u8; 8] = [0, 0, 0, 8, 4, 210, 22, 47];

/// Through a tunnel the certificate is checked against the profile's host, not the tunnel's
/// end, and a cancel request goes over TLS like the session.
#[tokio::test(flavor = "multi_thread")]
async fn through_a_tunnel_the_profiles_host_is_checked_and_the_cancel_is_encrypted() {
    let Some((url, certs)) = tls_env("through_a_tunnel_the_profiles_host_is_checked_and_the_cancel_is_encrypted")
    else {
        return;
    };
    let d = dsn::parse(&url).unwrap();
    let tunnel = Arc::new(Tunnel {
        to: ("127.0.0.1".into(), d.port.unwrap()),
        asked: Mutex::default(),
        streams: Mutex::default(),
    });
    let dialer = Some(DialerRef(tunnel.clone() as Arc<dyn Dialer>));
    let mut cfg = profile(&url, "verify-full", &certs, [Some("ca.crt"), None, None]);
    cfg.host = "pg-tls.test".into();
    cfg.port = 5432;
    assert_eq!(connect(&cfg, dialer.clone()).await, tls(Verified::Full));
    assert_eq!(tunnel.asked.lock().unwrap()[0], ("pg-tls.test".to_string(), 5432));
    // A name the certificate does not have fails, though the tunnel reaches the same server.
    let mut other = cfg.clone();
    other.host = "db.internal".into();
    assert_eq!(
        connect(&other, dialer.clone()).await,
        Err(DbError::Tls(TlsProblem::NameMismatch { host: "db.internal".into() }))
    );
    // The cancel of a running statement: its own connection, encrypted.
    tunnel.streams.lock().unwrap().clear();
    let (session, mut rx) = start(&cfg, SessionRole::Query, dialer);
    session.send(DbCommand::Execute {
        id: 1,
        statements: vec!["SELECT pg_sleep(30)".into()],
        paging: PagingMode::Hold,
    });
    loop {
        match tokio::time::timeout(Duration::from_secs(20), rx.recv()).await.unwrap().unwrap() {
            DbEvent::Connected => break,
            DbEvent::ConnectFailed { error, .. } => panic!("{error:?}"),
            _ => {}
        }
    }
    // Long enough for the statement to reach the server (a cancel before it is lost).
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let t0 = Instant::now();
    session.cancel();
    loop {
        match tokio::time::timeout(Duration::from_secs(20), rx.recv()).await.unwrap().unwrap() {
            DbEvent::Failed { cancelled: true, .. } => break,
            DbEvent::Failed { error, .. } => panic!("{error:?}"),
            _ => {}
        }
    }
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
    let streams = tunnel.streams.lock().unwrap();
    assert_eq!(streams.len(), 2, "the session's connection and the cancel's");
    for s in streams.iter() {
        let w = s.lock().unwrap();
        // The SSLRequest, then a TLS handshake record: nothing in the clear.
        assert_eq!(&w[..8], &SSL_REQUEST);
        assert_eq!(w.get(8), Some(&0x16), "a TLS handshake record follows");
    }
}
