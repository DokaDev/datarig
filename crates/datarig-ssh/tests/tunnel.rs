//! The tunnel against an SSH server in the test process: host keys, each login method, channels
//! and their refusals, timeouts and loss. No Docker; runs on every OS.

mod common;

use common::*;
use datarig_core::transport::{DialError, Dialer, Refusal};
use datarig_ssh::known_hosts::Verdict;
use datarig_ssh::tunnel::{ErrorKind, Loss, Method, Opened, Secret, Stage};
use datarig_ssh::{Auth, Tunnel};
use russh::keys::{Algorithm, EcdsaCurve};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn trusting() -> Arc<TestAsker> {
    TestAsker::new(Answers { trust: true, ..Answers::default() })
}

/// Bytes through a channel to the echo server come back.
async fn round_trip(tunnel: &Tunnel, target: std::net::SocketAddr) {
    let mut s = tunnel.dial(&target.ip().to_string(), target.port()).await.expect("dial");
    s.write_all(b"through the tunnel").await.unwrap();
    let mut back = [0u8; 18];
    s.read_exact(&mut back).await.unwrap();
    assert_eq!(&back, b"through the tunnel");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_host_is_asked_about_once_and_trusted_in_datarigs_file_only() {
    let d = scratch("tofu");
    let (path, public) = key_file(&d, "id", Algorithm::Ed25519);
    let server = TestServer::start(Accounts { keys: vec![public], ..Accounts::default() }).await;
    let auth = || Auth::Key { path: path.clone(), passphrase: None };
    let asker = trusting();
    let tunnel = Tunnel::open(hop(&server, auth()), options(), env(&d), asker.clone()).await.expect("open");
    {
        let seen = asker.seen.lock().unwrap();
        assert_eq!(seen.stages, [Stage::Connecting, Stage::Handshake, Stage::HostKey, Stage::Authenticating]);
        assert_eq!(seen.host_keys.len(), 1);
        let q = &seen.host_keys[0];
        assert!(q.stored.is_empty(), "unknown");
        assert_eq!((q.host.as_str(), q.port, q.algorithm.as_str()), ("127.0.0.1", server.addr.port(), "ssh-ed25519"));
        assert_eq!(q.fingerprint, datarig_ssh::known_hosts::fingerprint(&server.host_key));
    }
    // How it opened (the staged test's SSH line): the key trusted now, the key file's login.
    let opened = Opened {
        host_key: "ssh-ed25519".into(),
        trusted_now: true,
        method: Method::Key,
        key: Some("ssh-ed25519".into()),
    };
    assert_eq!(tunnel.opened(), &opened);
    round_trip(&tunnel, echo().await).await;
    let kh = known(&d);
    assert_eq!(kh.check("127.0.0.1", server.addr.port(), &server.host_key).unwrap(), Verdict::Known);
    assert!(!kh.user.as_ref().unwrap().exists(), "OpenSSH's file is never written");
    // Known now: not asked again.
    let again = trusting();
    let tunnel = Tunnel::open(hop(&server, auth()), options(), env(&d), again.clone()).await.expect("open again");
    assert!(again.seen.lock().unwrap().host_keys.is_empty());
    assert_eq!(tunnel.opened(), &Opened { trusted_now: false, ..opened });
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_not_trusted_is_not_logged_in_to_and_nothing_is_written() {
    let d = scratch("untrusted");
    let (path, public) = key_file(&d, "id", Algorithm::Ed25519);
    let server = TestServer::start(Accounts { keys: vec![public], ..Accounts::default() }).await;
    let asker = TestAsker::new(Answers::default());
    let e = Tunnel::open(hop(&server, Auth::Key { path, passphrase: None }), options(), env(&d), asker.clone())
        .await
        .err()
        .expect("refused");
    assert_eq!((e.host.as_str(), e.port), ("127.0.0.1", server.addr.port()));
    assert!(matches!(e.kind, ErrorKind::HostKeyNotTrusted { .. }), "{e:?}");
    assert!(!asker.seen.lock().unwrap().stages.contains(&Stage::Authenticating));
    assert!(!known(&d).app.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_key_warns_with_both_fingerprints_and_is_replaced_only_when_accepted() {
    let d = scratch("changed");
    let (path, public) = key_file(&d, "id", Algorithm::Ed25519);
    let server = TestServer::start(Accounts { keys: vec![public], ..Accounts::default() }).await;
    let old = russh::keys::PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap().public_key().clone();
    // OpenSSH's file knows another key for the host.
    let user_text = line(&server, &old);
    std::fs::write(known(&d).user.unwrap(), &user_text).unwrap();
    let auth = || Auth::Key { path: path.clone(), passphrase: None };
    let decline = TestAsker::new(Answers::default());
    let e = Tunnel::open(hop(&server, auth()), options(), env(&d), decline.clone()).await.err().expect("refused");
    match &e.kind {
        ErrorKind::HostKeyChanged { fingerprint, stored } => {
            assert_eq!(fingerprint, &datarig_ssh::known_hosts::fingerprint(&server.host_key));
            assert_eq!(stored[0].fingerprint, datarig_ssh::known_hosts::fingerprint(&old));
            assert_eq!(stored[0].file, known(&d).user.unwrap());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(decline.seen.lock().unwrap().host_keys[0].stored.len(), 1, "asked with the stored key");
    // Accepted: written to datarig's file, which decides from now on; OpenSSH's is untouched.
    Tunnel::open(hop(&server, auth()), options(), env(&d), trusting()).await.expect("replaced");
    assert_eq!(std::fs::read_to_string(known(&d).user.unwrap()).unwrap(), user_text);
    let again = trusting();
    Tunnel::open(hop(&server, auth()), options(), env(&d), again.clone()).await.expect("known");
    assert!(again.seen.lock().unwrap().host_keys.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_the_user_trusts_in_openssh_is_known() {
    let d = scratch("openssh");
    let (path, public) = key_file(&d, "id", Algorithm::Ed25519);
    let server = TestServer::start(Accounts { keys: vec![public], ..Accounts::default() }).await;
    std::fs::write(known(&d).user.unwrap(), line(&server, &server.host_key)).unwrap();
    let asker = trusting();
    Tunnel::open(hop(&server, Auth::Key { path, passphrase: None }), options(), env(&d), asker.clone()).await.unwrap();
    assert!(asker.seen.lock().unwrap().host_keys.is_empty());
    assert!(!known(&d).app.exists(), "nothing to write");
}

#[tokio::test(flavor = "multi_thread")]
async fn key_files_of_every_kind_log_in() {
    let d = scratch("keys");
    let mut accepted = Vec::new();
    let mut files = Vec::new();
    for (name, algo) in [
        ("ed", Algorithm::Ed25519),
        ("ecdsa", Algorithm::Ecdsa { curve: EcdsaCurve::NistP256 }),
        ("rsa", Algorithm::Rsa { hash: None }),
    ] {
        let (path, public) = key_file(&d, name, algo);
        accepted.push(public);
        files.push(path);
    }
    // An AWS-style PEM (PKCS#1 RSA), made the way users make it.
    let pem = d.join("aws.pem");
    if std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "rsa", "-b", "2048", "-m", "PEM", "-N", "", "-f", pem.to_str().unwrap()])
        .status()
        .is_ok_and(|s| s.success())
    {
        private(&pem);
        let text = std::fs::read_to_string(&pem).unwrap();
        assert!(text.starts_with("-----BEGIN RSA PRIVATE KEY-----"), "{text:.40}");
        accepted.push(datarig_ssh::keys::load(&pem, None).unwrap().key.public_key().clone());
        files.push(pem);
    } else {
        assert!(std::env::var_os("CI").is_none(), "ssh-keygen is needed on CI");
    }
    let server = TestServer::start(Accounts { keys: accepted, ..Accounts::default() }).await;
    let signed = ["ssh-ed25519", "ecdsa-sha2-nistp256", "rsa-sha2-512", "rsa-sha2-512"];
    for (path, signed) in files.into_iter().zip(signed) {
        let t = Tunnel::open(
            hop(&server, Auth::Key { path: path.clone(), passphrase: None }),
            options(),
            env(&d),
            trusting(),
        )
        .await
        .unwrap_or_else(|e| panic!("{path:?}: {e:?}"));
        assert_eq!((t.opened().method, t.opened().key.as_deref()), (Method::Key, Some(signed)), "{path:?}");
        round_trip(&t, echo().await).await;
    }
    // A key the server does not take.
    let (other, _) = key_file(&d, "other", Algorithm::Ed25519);
    let e = Tunnel::open(hop(&server, Auth::Key { path: other, passphrase: None }), options(), env(&d), trusting())
        .await
        .err()
        .unwrap();
    assert!(matches!(e.kind, ErrorKind::Rejected { method: Method::Key, .. }), "{e:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_encrypted_key_asks_for_its_passphrase_until_it_opens() {
    let d = scratch("passphrase");
    let key = russh::keys::PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    let public = key.public_key().clone();
    let path = d.join("enc");
    let enc = key.encrypt(&mut rand::rng(), "right-one").unwrap();
    std::fs::write(&path, enc.to_openssh(russh::keys::ssh_key::LineEnding::LF).unwrap().as_bytes()).unwrap();
    private(&path);
    let server = TestServer::start(Accounts { keys: vec![public], ..Accounts::default() }).await;
    let asker = TestAsker::new(Answers {
        trust: true,
        passphrases: vec!["wrong".into(), "right-one".into()],
        ..Answers::default()
    });
    Tunnel::open(hop(&server, Auth::Key { path: path.clone(), passphrase: None }), options(), env(&d), asker.clone())
        .await
        .expect("opened with the second passphrase");
    assert_eq!(asker.seen.lock().unwrap().passphrase_asks, [(path.clone(), false), (path.clone(), true)]);
    // A stored passphrase is used without asking; a cancelled question cancels.
    let quiet = trusting();
    let stored = Auth::Key { path: path.clone(), passphrase: Some(Secret::new("right-one".into())) };
    Tunnel::open(hop(&server, stored), options(), env(&d), quiet.clone()).await.unwrap();
    assert!(quiet.seen.lock().unwrap().passphrase_asks.is_empty());
    let e =
        Tunnel::open(hop(&server, Auth::Key { path: path.clone(), passphrase: None }), options(), env(&d), trusting())
            .await
            .err()
            .unwrap();
    assert_eq!(e.kind, ErrorKind::Cancelled);
    // A stored passphrase that is wrong, and nothing better given: that error.
    let wrong = Auth::Key { path: path.clone(), passphrase: Some(Secret::new("stale".into())) };
    let e = Tunnel::open(hop(&server, wrong), options(), env(&d), trusting()).await.err().unwrap();
    assert_eq!(e.kind, ErrorKind::Key { path, error: datarig_ssh::keys::KeyError::WrongPassphrase });
}

#[tokio::test(flavor = "multi_thread")]
async fn password_and_keyboard_interactive_logins() {
    let d = scratch("password");
    let server = TestServer::start(Accounts { password: true, ..Accounts::default() }).await;
    let pw = |p: &str| Auth::Password(Some(Secret::new(p.into())));
    let t = Tunnel::open(hop(&server, pw(PASSWORD)), options(), env(&d), trusting()).await.expect("password");
    assert_eq!((t.opened().method, t.opened().key.as_deref()), (Method::Password, None));
    // A refused one: asked again (up to three in all); not answered, the refusal stands.
    let refused = trusting();
    let e = Tunnel::open(hop(&server, pw("nope")), options(), env(&d), refused.clone()).await.err().unwrap();
    assert!(matches!(e.kind, ErrorKind::Rejected { method: Method::Password, .. }), "{e:?}");
    assert_eq!(refused.seen.lock().unwrap().password_asks, [true]);
    let retry = TestAsker::new(Answers {
        trust: true,
        passwords: vec!["again-wrong".into(), PASSWORD.into()],
        ..Answers::default()
    });
    Tunnel::open(hop(&server, pw("nope")), options(), env(&d), retry.clone()).await.expect("third time");
    assert_eq!(retry.seen.lock().unwrap().password_asks, [true, true]);
    // None stored: asked; the question cancelled cancels.
    let ask = TestAsker::new(Answers { trust: true, passwords: vec![PASSWORD.into()], ..Answers::default() });
    Tunnel::open(hop(&server, Auth::Password(None)), options(), env(&d), ask.clone()).await.expect("asked");
    assert_eq!(ask.seen.lock().unwrap().password_asks, [false]);
    let e = Tunnel::open(hop(&server, Auth::Password(None)), options(), env(&d), trusting()).await.err().unwrap();
    assert_eq!(e.kind, ErrorKind::Cancelled);

    // A server that takes the password only through keyboard-interactive.
    let kbd = TestServer::start(Accounts { password_by_kbd: true, ..Accounts::default() }).await;
    let t = Tunnel::open(hop(&kbd, pw(PASSWORD)), options(), env(&d), trusting()).await.expect("password by kbd");
    assert_eq!(t.opened().method, Method::Password);

    // A one-time code, answered by the user.
    let code = TestServer::start(Accounts { code: true, ..Accounts::default() }).await;
    let asker = TestAsker::new(Answers { trust: true, answers: Some(vec![CODE.into()]), ..Answers::default() });
    let t = Tunnel::open(hop(&code, Auth::KeyboardInteractive), options(), env(&d), asker.clone()).await.expect("code");
    assert_eq!((t.opened().method, t.opened().key.as_deref()), (Method::KeyboardInteractive, None));
    let asked = asker.seen.lock().unwrap().prompts[0].prompts.clone();
    assert_eq!(asked, [("Verification code: ".to_string(), true)]);
    let cancel = trusting();
    let e = Tunnel::open(hop(&code, Auth::KeyboardInteractive), options(), env(&d), cancel).await.err().unwrap();
    assert_eq!(e.kind, ErrorKind::Cancelled);
}

/// Nothing the server opens toward datarig is accepted: sessions, X11, direct-tcpip,
/// direct-streamlocal, forwarded ports and sockets, agent forwarding. The tunnel still works.
#[tokio::test(flavor = "multi_thread")]
async fn every_channel_the_server_opens_is_refused() {
    let d = scratch("server-channels");
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let server = TestServer::start(Accounts { password: true, probe: Some(seen.clone()), ..Accounts::default() }).await;
    let pw = Auth::Password(Some(Secret::new(PASSWORD.into())));
    let tunnel = Tunnel::open(hop(&server, pw), options(), env(&d), trusting()).await.expect("login");
    let t0 = Instant::now();
    while seen.lock().unwrap().len() < SERVER_CHANNELS.len() {
        assert!(t0.elapsed() < Duration::from_secs(20), "the probe did not finish: {:?}", seen.lock().unwrap());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let seen = seen.lock().unwrap().clone();
    let kinds: Vec<&str> = seen.iter().map(|(k, _)| *k).collect();
    assert_eq!(kinds, SERVER_CHANNELS, "every kind was tried");
    let accepted: Vec<&str> = seen.iter().filter(|(_, a)| *a).map(|(k, _)| *k).collect();
    assert!(accepted.is_empty(), "accepted: {accepted:?}");
    round_trip(&tunnel, echo().await).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn channels_the_bastion_refuses_say_why_and_a_closed_tunnel_dials_nothing() {
    let d = scratch("channels");
    let (path, public) = key_file(&d, "id", Algorithm::Ed25519);
    let server = TestServer::start(Accounts { keys: vec![public], ..Accounts::default() }).await;
    let tunnel =
        Tunnel::open(hop(&server, Auth::Key { path, passphrase: None }), options(), env(&d), trusting()).await.unwrap();
    match tunnel.dial("denied.invalid", 5432).await {
        Err(DialError::Refused { host, port, reason: Refusal::Prohibited, .. }) => {
            assert_eq!((host.as_str(), port), ("denied.invalid", 5432))
        }
        other => panic!("{:?}", other.err()),
    }
    let closed_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    assert!(matches!(
        tunnel.dial("127.0.0.1", closed_port).await,
        Err(DialError::Refused { reason: Refusal::Unreachable, .. })
    ));
    // Many channels at once over the one connection.
    let target = echo().await;
    let mut all = Vec::new();
    for _ in 0..8 {
        all.push(round_trip(&tunnel, target));
    }
    futures::future::join_all(all).await;
    let lost = tunnel.lost();
    tunnel.close().await;
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), lost).await.unwrap(), Loss::ClosedByApp);
    assert!(!tunnel.is_open());
    assert!(matches!(tunnel.dial("127.0.0.1", target.port()).await, Err(DialError::NotOpen)));
}

#[tokio::test(flavor = "multi_thread")]
async fn connecting_fails_by_stage() {
    let d = scratch("stages");
    let (path, _) = key_file(&d, "id", Algorithm::Ed25519);
    let auth = || Auth::Key { path: path.clone(), passphrase: None };
    let at = |host: &str, port| datarig_ssh::Hop { host: host.into(), port, user: USER.into(), auth: auth() };
    let e = Tunnel::open(at("datarig-no-such-host.invalid", 22), options(), env(&d), trusting()).await.err().unwrap();
    assert!(matches!(e.kind, ErrorKind::HostNotFound(_)), "{e:?}");
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let e = Tunnel::open(at("127.0.0.1", closed), options(), env(&d), trusting()).await.err().unwrap();
    assert!(matches!(e.kind, ErrorKind::Connect(_)), "{e:?}");
    // A server that accepts and never speaks: the handshake's time runs out.
    let quiet = mute().await;
    let short = datarig_ssh::Options { timeout: Duration::from_millis(400), keepalive: None };
    let t0 = Instant::now();
    let e = Tunnel::open(at("127.0.0.1", quiet.port()), short, env(&d), trusting()).await.err().unwrap();
    assert_eq!(e.kind, ErrorKind::Timeout(Stage::Handshake, Duration::from_millis(400)));
    assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_clock_stops_while_the_user_decides() {
    let d = scratch("pause");
    let (path, public) = key_file(&d, "id", Algorithm::Ed25519);
    let server = TestServer::start(Accounts { keys: vec![public], ..Accounts::default() }).await;
    let slow = TestAsker::new(Answers { trust: true, think: Duration::from_millis(1200), ..Answers::default() });
    let short = datarig_ssh::Options { timeout: Duration::from_millis(500), keepalive: None };
    Tunnel::open(hop(&server, Auth::Key { path, passphrase: None }), short, env(&d), slow)
        .await
        .expect("not timed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dead_path_is_noticed_by_keepalives() {
    let d = scratch("keepalive");
    let (path, public) = key_file(&d, "id", Algorithm::Ed25519);
    let server = TestServer::start(Accounts { keys: vec![public], ..Accounts::default() }).await;
    let p = proxy(server.addr).await;
    let h = datarig_ssh::Hop {
        host: "127.0.0.1".into(),
        port: p.addr.port(),
        user: USER.into(),
        auth: Auth::Key { path, passphrase: None },
    };
    let opts = datarig_ssh::Options { timeout: Duration::from_secs(5), keepalive: Some(Duration::from_millis(200)) };
    let tunnel = Tunnel::open(h, opts, env(&d), trusting()).await.unwrap();
    round_trip(&tunnel, echo().await).await;
    let lost = tunnel.lost();
    p.frozen.store(true, std::sync::atomic::Ordering::SeqCst);
    let t0 = Instant::now();
    assert_eq!(tokio::time::timeout(Duration::from_secs(10), lost).await.expect("noticed"), Loss::Keepalive);
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
    assert!(matches!(tunnel.dial("127.0.0.1", 1).await, Err(DialError::NotOpen)));
}

/// The ssh-agent: a scratch agent of the test's own (`ssh-agent -a <scratch socket>`), never
/// the user's. Its keys log in; an agent without keys and no agent at all say so.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn the_agents_keys_log_in() {
    use std::process::{Command, Stdio};
    let d = scratch("agent");
    let (path, public) = key_file(&d, "id", Algorithm::Ed25519);
    let server = TestServer::start(Accounts { keys: vec![public], ..Accounts::default() }).await;
    // A Unix socket's path has a limit (104 bytes on macOS, 108 on Linux): under a long
    // `TMPDIR` the agent could not bind, so its socket goes to a short directory of its own.
    let sock_dir = SockDir::new(&d);
    let sock = sock_dir.0.join("agent.sock");
    let agent = match Command::new("ssh-agent").arg("-D").arg("-a").arg(&sock).stdout(Stdio::null()).spawn() {
        Ok(a) => a,
        Err(e) => {
            assert!(std::env::var_os("CI").is_none(), "ssh-agent is needed on CI: {e}");
            eprintln!("SKIPPED the_agents_keys_log_in: no ssh-agent ({e})");
            return;
        }
    };
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _agent = Kill(agent);
    let t0 = Instant::now();
    while !sock.exists() {
        assert!(t0.elapsed() < Duration::from_secs(5), "the agent's socket never appeared");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let with_agent = |env: &mut datarig_ssh::Env| env.agent = Some(sock.clone());
    // No keys yet.
    let mut e1 = env(&d);
    with_agent(&mut e1);
    let e = Tunnel::open(hop(&server, Auth::Agent), options(), e1.clone(), trusting()).await.err().unwrap();
    assert_eq!(e.kind, ErrorKind::AgentEmpty);
    let added = Command::new("ssh-add").arg(&path).env("SSH_AUTH_SOCK", &sock).stderr(Stdio::null()).status().unwrap();
    assert!(added.success());
    let tunnel = Tunnel::open(hop(&server, Auth::Agent), options(), e1, trusting()).await.expect("agent login");
    assert_eq!((tunnel.opened().method, tunnel.opened().key.as_deref()), (Method::Agent, Some("ssh-ed25519")));
    round_trip(&tunnel, echo().await).await;
    // No agent.
    let e = Tunnel::open(hop(&server, Auth::Agent), options(), env(&d), trusting()).await.err().unwrap();
    assert!(matches!(e.kind, ErrorKind::NoAgent(_)), "{e:?}");
    let mut gone = env(&d);
    gone.agent = Some(d.join("no-such.sock"));
    let e = Tunnel::open(hop(&server, Auth::Agent), options(), gone, trusting()).await.err().unwrap();
    assert!(matches!(e.kind, ErrorKind::NoAgent(_)), "{e:?}");
}

/// The directory of a test's agent socket: `dir` when the socket's path fits the limit of a
/// Unix socket, else a short one under `/tmp`, removed when dropped.
#[cfg(unix)]
struct SockDir(std::path::PathBuf, bool);

#[cfg(unix)]
impl SockDir {
    fn new(dir: &std::path::Path) -> Self {
        // 104 bytes on macOS, with room for the name and the terminating NUL.
        if dir.join("agent.sock").as_os_str().len() < 100 {
            return Self(dir.to_path_buf(), false);
        }
        let short = std::path::PathBuf::from(format!("/tmp/lzo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&short);
        std::fs::create_dir_all(&short).unwrap();
        Self(short, true)
    }
}

#[cfg(unix)]
impl Drop for SockDir {
    fn drop(&mut self) {
        if self.1 {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// A legacy PEM key with a cipher datarig cannot open is said so at once: its passphrase is
/// not asked for (asking three times and calling it wrong would mislead).
#[tokio::test(flavor = "multi_thread")]
async fn a_key_with_a_cipher_datarig_cannot_open_is_not_asked_for() {
    let d = scratch("legacy-cipher");
    let server = TestServer::start(Accounts::default()).await;
    let path = d.join("aws.pem");
    std::fs::write(
        &path,
        "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-256-CBC,00112233445566778899AABBCCDDEEFF\n\nAAAA\n-----END RSA PRIVATE KEY-----\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let asker = TestAsker::new(Answers { trust: true, passphrases: vec!["x".into(); 3], ..Answers::default() });
    let auth = Auth::Key { path: path.clone(), passphrase: None };
    let e = Tunnel::open(hop(&server, auth), options(), env(&d), asker.clone()).await.err().unwrap();
    assert_eq!(
        e.kind,
        ErrorKind::Key { path, error: datarig_ssh::keys::KeyError::UnsupportedEncryption("AES-256-CBC".into()) }
    );
    assert!(asker.seen.lock().unwrap().passphrase_asks.is_empty(), "no passphrase asked for");
}

/// The private keys a test generates never outlive it: its scratch directory goes with it.
#[test]
fn generated_keys_are_removed_with_the_test() {
    let d = scratch("removed");
    let (path, _) = key_file(&d, "id", Algorithm::Ed25519);
    let dir = d.to_path_buf();
    assert!(path.exists());
    drop(d);
    assert!(!path.exists() && !dir.exists(), "{dir:?} is left behind");
}
