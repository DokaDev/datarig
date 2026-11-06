//! The request against an in-process HTTPS server whose certificates are made here (rcgen), and
//! a CONNECT proxy in front of it. Nothing leaves the machine.

use super::*;
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

/// A certificate authority and a server configuration with a leaf for `names` it signed.
struct Tls {
    ca: Vec<u8>,
    config: Arc<rustls::ServerConfig>,
}

fn tls(names: &[&str]) -> Tls {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap();
    let key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
        .unwrap()
        .signed_by(&key, &ca)
        .unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(leaf.der().to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        )
        .unwrap();
    Tls { ca: ca.der().to_vec(), config: Arc::new(config) }
}

/// What the server does once it has read a request.
#[derive(Clone)]
enum Reply {
    /// These bytes, then the connection closes.
    Bytes(Vec<u8>),
    /// Nothing, ever (until the test ends).
    Hang,
}

/// The request's head, up to its blank line.
fn read_head(r: &mut impl BufRead) -> String {
    let mut head = String::new();
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            return head;
        }
        head.push_str(&line);
    }
}

/// An HTTPS server for one request on 127.0.0.1: its port, and the request's head when it came.
fn serve(tls: &Tls, reply: Reply) -> (u16, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = tls.config.clone();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let Ok((sock, _)) = listener.accept() else { return };
        let conn = rustls::ServerConnection::new(config).unwrap();
        let mut stream = BufReader::new(rustls::StreamOwned::new(conn, sock));
        let head = read_head(&mut stream);
        let _ = tx.send(head);
        match reply {
            Reply::Bytes(b) => {
                let s = stream.get_mut();
                let _ = s.write_all(&b);
                let _ = s.flush();
                s.conn.send_close_notify();
                let _ = s.flush();
            }
            Reply::Hang => std::thread::sleep(Duration::from_secs(30)),
        }
    });
    (port, rx)
}

/// A CONNECT proxy for one tunnel: its port, and the CONNECT request's head. With `refuse` it
/// answers 407.
fn proxy(refuse: bool) -> (u16, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let Ok((client, _)) = listener.accept() else { return };
        let mut reader = BufReader::new(client.try_clone().unwrap());
        let head = read_head(&mut reader);
        let target = head.split_whitespace().nth(1).unwrap_or_default().to_string();
        let _ = tx.send(head);
        let mut client = client;
        if refuse {
            let _ = client.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n");
            return;
        }
        let port = target.rsplit_once(':').map(|(_, p)| p.to_string()).unwrap_or_default();
        let upstream = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
        client.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").unwrap();
        let (mut up_r, mut up_w) = (upstream.try_clone().unwrap(), upstream);
        let mut down_w = client;
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut reader, &mut up_w);
        });
        let _ = std::io::copy(&mut up_r, &mut down_w);
    });
    (port, rx)
}

/// An answer with `status`, `headers` and `body` as `Content-Length` says.
fn answer(status: &str, headers: &str, body: &str) -> Reply {
    Reply::Bytes(format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes())
}

const RELEASE: &str = r#"{"tag_name":"v0.39.1","draft":false,"prerelease":false,"published_at":"2026-11-10T09:00:00Z","body":"The summary.\r\n\r\n### Added\r\n\r\n- x"}"#;

fn release() -> Release {
    Release {
        version: datarig_core::update::Version::parse("0.39.1").unwrap(),
        date: Some("2026-11-10".into()),
        summary: Some("The summary.".into()),
    }
}

fn fast() -> Timeouts {
    Timeouts { connect: Duration::from_millis(500), total: Duration::from_millis(800) }
}

/// The request to `port` (as `localhost`) trusting `ca`, through `proxy`.
fn client(port: u16, ca: &[u8], proxy: &Proxy, timeouts: Timeouts) -> Https {
    let url = format!("https://localhost:{port}/repos/DokaDev/datarig/releases/latest");
    Https::with(&url, proxy, Https::roots(&[ca.to_vec()]), timeouts).unwrap()
}

fn fetch(tls: &Tls, reply: Reply, etag: Option<&str>) -> (Answer, String) {
    let (port, head) = serve(tls, reply);
    let got = client(port, &tls.ca, &Proxy::Direct, Timeouts::default()).fetch(etag);
    (got, head.recv_timeout(Duration::from_secs(5)).unwrap_or_default())
}

#[test]
fn the_request_says_nothing_but_what_it_asks() {
    let t = tls(&["localhost"]);
    let (got, head) = fetch(&t, answer("200 OK", "ETag: W/\"abc\"\r\n", RELEASE), None);
    assert_eq!(got, Answer::Release(release(), Some("W/\"abc\"".into())));
    let mut lines: Vec<String> = head.lines().map(|l| l.to_ascii_lowercase()).collect();
    assert_eq!(lines.remove(0), "get /repos/dokadev/datarig/releases/latest http/1.1");
    lines.sort();
    let port = lines.iter().find(|l| l.starts_with("host:")).cloned().unwrap();
    assert!(port.starts_with("host: localhost:"), "{port}");
    let want = ["accept: application/vnd.github+json", "user-agent: datarig", "x-github-api-version: 2022-11-28"];
    let rest: Vec<&str> = lines.iter().map(String::as_str).filter(|l| !l.starts_with("host:")).collect();
    assert_eq!(rest, want, "exactly these headers: {head}");
    // With the last answer's tag.
    let (got, head) = fetch(&t, answer("304 Not Modified", "", ""), Some("W/\"abc\""));
    assert_eq!(got, Answer::NotModified);
    assert!(head.to_ascii_lowercase().contains("if-none-match: w/\"abc\"\r\n"), "{head}");
}

#[test]
fn chunked_and_close_delimited_answers_read() {
    let t = tls(&["localhost"]);
    let (a, b) = RELEASE.split_at(40);
    let chunked = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n",
        a.len(),
        b.len()
    );
    assert_eq!(fetch(&t, Reply::Bytes(chunked.into_bytes()), None).0, Answer::Release(release(), None));
    let close = format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{RELEASE}");
    assert_eq!(fetch(&t, Reply::Bytes(close.into_bytes()), None).0, Answer::Release(release(), None));
}

#[test]
fn statuses_say_why_nothing_was_learnt() {
    let t = tls(&["localhost"]);
    for (status, want) in [
        ("403 Forbidden", Outcome::RateLimited),
        ("429 Too Many Requests", Outcome::RateLimited),
        ("500 Internal Server Error", Outcome::Unreachable),
        ("503 Service Unavailable", Outcome::Unreachable),
        ("404 Not Found", Outcome::BadAnswer),
        ("301 Moved Permanently", Outcome::BadAnswer),
    ] {
        let headers = if status.starts_with("301") { "Location: https://example.org/\r\n" } else { "" };
        assert_eq!(fetch(&t, answer(status, headers, "{}"), None).0, Answer::Failed(want), "{status}");
    }
}

#[test]
fn an_answer_that_is_not_a_release_or_too_long_is_bad() {
    let t = tls(&["localhost"]);
    for body in ["<html>", "{}", r#"{"tag_name":"v0.40.0-rc.1"}"#] {
        assert_eq!(fetch(&t, answer("200 OK", "", body), None).0, Answer::Failed(Outcome::BadAnswer), "{body}");
    }
    let long = format!(r#"{{"tag_name":"v0.39.1","body":"{}"}}"#, "a".repeat(MAX_ANSWER));
    assert_eq!(fetch(&t, answer("200 OK", "", &long), None).0, Answer::Failed(Outcome::BadAnswer));
    // A body longer than it says it is, without a length: the cap still holds.
    let endless = format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{}", "a".repeat(MAX_ANSWER + 10));
    assert_eq!(fetch(&t, Reply::Bytes(endless.into_bytes()), None).0, Answer::Failed(Outcome::BadAnswer));
}

#[test]
fn a_server_that_never_answers_times_out_in_time() {
    let t = tls(&["localhost"]);
    let (port, _head) = serve(&t, Reply::Hang);
    let started = Instant::now();
    let got = client(port, &t.ca, &Proxy::Direct, fast()).fetch(None);
    assert_eq!(got, Answer::Failed(Outcome::TimedOut));
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
}

#[test]
fn a_certificate_the_client_does_not_trust_is_a_tls_failure() {
    // Another name.
    let t = tls(&["other.test"]);
    assert_eq!(fetch(&t, answer("200 OK", "", RELEASE), None).0, Answer::Failed(Outcome::Tls));
    // Another CA.
    let t = tls(&["localhost"]);
    let stranger = tls(&["localhost"]);
    let (port, _head) = serve(&t, answer("200 OK", "", RELEASE));
    assert_eq!(client(port, &stranger.ca, &Proxy::Direct, fast()).fetch(None), Answer::Failed(Outcome::Tls));
}

#[test]
fn nobody_listening_is_unreachable() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let t = tls(&["localhost"]);
    let url = format!("https://127.0.0.1:{port}/");
    // The connect limit of a real check: Windows retries a refused connection for about two
    // seconds before it says so.
    let roots = Https::roots(std::slice::from_ref(&t.ca));
    let https = Https::with(&url, &Proxy::Direct, roots, Timeouts::default()).unwrap();
    assert_eq!(https.fetch(None), Answer::Failed(Outcome::Unreachable));
}

#[test]
fn through_an_http_proxy_with_its_credentials() {
    let t = tls(&["localhost"]);
    let (port, head) = serve(&t, answer("200 OK", "", RELEASE));
    let (pport, connect) = proxy(false);
    let p = Proxy::Http(format!("http://u:p@127.0.0.1:{pport}"));
    assert_eq!(client(port, &t.ca, &p, Timeouts::default()).fetch(None), Answer::Release(release(), None));
    let connect = connect.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(connect.starts_with(&format!("CONNECT localhost:{port} HTTP/1.1\r\n")), "{connect}");
    // `u:p` in Basic authentication.
    assert!(connect.to_ascii_lowercase().contains("proxy-authorization: basic dtpw\r\n"), "{connect}");
    // The request went through the tunnel as it would have gone direct.
    assert!(head.recv_timeout(Duration::from_secs(5)).unwrap().contains("user-agent: datarig"));
}

#[test]
fn a_proxy_that_refuses_is_a_proxy_failure() {
    let t = tls(&["localhost"]);
    let (port, _head) = serve(&t, answer("200 OK", "", RELEASE));
    let (pport, _connect) = proxy(true);
    let p = Proxy::Http(format!("http://127.0.0.1:{pport}"));
    assert_eq!(client(port, &t.ca, &p, fast()).fetch(None), Answer::Failed(Outcome::Proxy));
}

#[test]
fn an_unusable_proxy_makes_no_client() {
    assert!(Https::new(&Proxy::Unsupported).is_none());
    assert!(Https::new(&Proxy::Direct).is_some());
    assert!(Https::new(&Proxy::Http("http://proxy.corp:3128".into())).is_some());
    assert_eq!(Https::new(&Proxy::Direct).unwrap().url, URL);
}

#[test]
fn a_plain_http_address_is_never_asked() {
    let t = tls(&["localhost"]);
    let https = Https::with("http://127.0.0.1:9/", &Proxy::Direct, Https::roots(&[t.ca]), fast()).unwrap();
    assert!(matches!(https.fetch(None), Answer::Failed(_)));
}

/// Bytes that are not HTTP never make the client panic: it ends in a failure.
#[test]
fn any_bytes_from_the_server_end_in_an_answer() {
    let t = tls(&["localhost"]);
    let mut seed: u32 = 7;
    for _ in 0..12 {
        let bytes: Vec<u8> = (0..200)
            .map(|_| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (seed >> 16) as u8
            })
            .collect();
        let (got, _) = fetch(&t, Reply::Bytes(bytes), None);
        assert!(matches!(got, Answer::Failed(_)), "{got:?}");
    }
}
