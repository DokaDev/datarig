//! An SSH server in the test process (russh's server side), a TCP echo target, a proxy that can
//! freeze, and an [`Asker`] that answers as the test says and records what it was asked.

#![allow(dead_code)]

use datarig_ssh::known_hosts::KnownHosts;
use datarig_ssh::tunnel::{HostKeyQuestion, Prompts, Secret, Stage};
use datarig_ssh::{Asker, Auth, Env, Hop, Options};
use futures::future::BoxFuture;
use russh::keys::{Algorithm, PrivateKey, PublicKey};
use russh::server::{self, Auth as ServerAuth, Msg, Session};
use russh::{Channel, ChannelOpenFailure, MethodKind, MethodSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub const USER: &str = "tunnel";
pub const PASSWORD: &str = "datarig-pw";
pub const CODE: &str = "123456";

/// Who may log in, and how.
#[derive(Clone, Default)]
pub struct Accounts {
    /// Public keys accepted for `USER`.
    pub keys: Vec<PublicKey>,
    /// `USER`'s password (the `password` method).
    pub password: bool,
    /// `USER` answers `CODE` to a keyboard-interactive question.
    pub code: bool,
    /// The password goes through keyboard-interactive only (one hidden prompt).
    pub password_by_kbd: bool,
    /// Once a user logged in, the server tries to open every kind of channel toward the
    /// client and records each kind and whether it was accepted.
    pub probe: Option<Probed>,
}

/// What the probe tried: each kind of channel and whether the client accepted it.
pub type Probed = Arc<Mutex<Vec<(&'static str, bool)>>>;

/// The kinds of channel a server may open toward its client.
pub const SERVER_CHANNELS: [&str; 7] =
    ["session", "x11", "direct-tcpip", "direct-streamlocal", "forwarded-tcpip", "forwarded-streamlocal", "agent"];

/// Try to open every kind of channel toward the client; an accepted one also gets data.
async fn probe(handle: server::Handle, seen: Probed) {
    for kind in SERVER_CHANNELS {
        let opened = tokio::time::timeout(Duration::from_secs(5), async {
            match kind {
                "session" => handle.channel_open_session().await,
                "x11" => handle.channel_open_x11("127.0.0.1", 6010).await,
                "direct-tcpip" => handle.channel_open_direct_tcpip("127.0.0.1", 22, "127.0.0.1", 40000).await,
                "direct-streamlocal" => handle.channel_open_direct_streamlocal("/tmp/datarig-probe.sock").await,
                "forwarded-tcpip" => handle.channel_open_forwarded_tcpip("127.0.0.1", 8080, "127.0.0.1", 40001).await,
                "forwarded-streamlocal" => handle.channel_open_forwarded_streamlocal("/tmp/datarig-probe.sock").await,
                _ => handle.channel_open_agent().await,
            }
        })
        .await;
        let accepted = match opened {
            Ok(Ok(channel)) => {
                let _ = channel.data(&b"probe"[..]).await;
                true
            }
            _ => false,
        };
        seen.lock().unwrap().push((kind, accepted));
    }
}

pub struct TestServer {
    pub addr: SocketAddr,
    pub host_key: PublicKey,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    pub async fn start(accounts: Accounts) -> Self {
        Self::start_with(accounts, PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()).await
    }

    pub async fn start_with(accounts: Accounts, host: PrivateKey) -> Self {
        let host_key = host.public_key().clone();
        let config = Arc::new(server::Config {
            keys: vec![host],
            auth_rejection_time: Duration::from_millis(1),
            auth_rejection_time_initial: Some(Duration::ZERO),
            methods: MethodSet::from(
                &[MethodKind::PublicKey, MethodKind::Password, MethodKind::KeyboardInteractive][..],
            ),
            ..Default::default()
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let handler = Handler { accounts: accounts.clone() };
                let config = config.clone();
                tokio::spawn(async move {
                    if let Ok(session) = server::run_stream(config, socket, handler).await {
                        let _ = session.await;
                    }
                });
            }
        });
        TestServer { addr, host_key, task }
    }

    pub fn stop(&self) {
        self.task.abort();
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Handler {
    accounts: Accounts,
}

impl server::Handler for Handler {
    type Error = russh::Error;

    async fn auth_succeeded(&mut self, session: &mut Session) -> Result<(), Self::Error> {
        if let Some(seen) = self.accounts.probe.clone() {
            tokio::spawn(probe(session.handle(), seen));
        }
        Ok(())
    }

    async fn auth_publickey_offered(&mut self, _: &str, key: &PublicKey) -> Result<ServerAuth, Self::Error> {
        Ok(if self.accounts.keys.contains(key) { ServerAuth::Accept } else { ServerAuth::reject() })
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<ServerAuth, Self::Error> {
        Ok(if user == USER && self.accounts.keys.contains(key) { ServerAuth::Accept } else { ServerAuth::reject() })
    }

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<ServerAuth, Self::Error> {
        if self.accounts.password_by_kbd {
            return Ok(ServerAuth::Reject {
                proceed_with_methods: Some(MethodSet::from(&[MethodKind::KeyboardInteractive][..])),
                partial_success: false,
            });
        }
        Ok(if self.accounts.password && user == USER && password == PASSWORD {
            ServerAuth::Accept
        } else {
            ServerAuth::reject()
        })
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _: &str,
        response: Option<server::Response<'a>>,
    ) -> Result<ServerAuth, Self::Error> {
        let (question, expected, echo) = match (self.accounts.code, self.accounts.password_by_kbd) {
            (true, _) => ("Verification code: ", CODE, true),
            (_, true) => ("Password: ", PASSWORD, false),
            _ => return Ok(ServerAuth::reject()),
        };
        match response {
            None => Ok(ServerAuth::Partial {
                name: "".into(),
                instructions: "".into(),
                prompts: vec![(question.into(), echo)].into(),
            }),
            Some(mut r) => {
                let answer = r.next();
                let ok = user == USER && answer.as_deref() == Some(expected.as_bytes());
                Ok(if ok { ServerAuth::Accept } else { ServerAuth::reject() })
            }
        }
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: russh::server::ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        if host == "denied.invalid" {
            reply.reject(ChannelOpenFailure::AdministrativelyProhibited).await;
            return Ok(());
        }
        match TcpStream::connect((host, port as u16)).await {
            Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
            Ok(mut target) => {
                reply.accept().await;
                tokio::spawn(async move {
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut stream, &mut target).await;
                });
            }
        }
        Ok(())
    }
}

/// A TCP server that echoes what it gets.
pub async fn echo() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                while let Ok(n) = s.read(&mut buf).await {
                    if n == 0 || s.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

/// A listener that accepts connections and never says anything.
pub async fn mute() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = listener.accept().await {
            held.push(s);
        }
    });
    addr
}

/// A TCP proxy to `target` that stops passing anything on (without closing) once frozen.
pub struct Proxy {
    pub addr: SocketAddr,
    pub frozen: Arc<AtomicBool>,
}

pub async fn proxy(target: SocketAddr) -> Proxy {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let frozen = Arc::new(AtomicBool::new(false));
    let f = frozen.clone();
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let Ok(server) = TcpStream::connect(target).await else { continue };
            let (mut cr, mut cw) = client.into_split();
            let (mut sr, mut sw) = server.into_split();
            let (f1, f2) = (f.clone(), f.clone());
            tokio::spawn(async move { pump(&mut cr, &mut sw, f1).await });
            tokio::spawn(async move { pump(&mut sr, &mut cw, f2).await });
        }
    });
    Proxy { addr, frozen }
}

async fn pump(
    from: &mut tokio::net::tcp::OwnedReadHalf,
    to: &mut tokio::net::tcp::OwnedWriteHalf,
    frozen: Arc<AtomicBool>,
) {
    let mut buf = [0u8; 16384];
    loop {
        let n = match from.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if frozen.load(Ordering::SeqCst) {
            // Swallowed: the other side hears nothing, and nothing is closed.
            continue;
        }
        if to.write_all(&buf[..n]).await.is_err() {
            return;
        }
    }
}

/// What the test's asker answers.
#[derive(Clone, Default)]
pub struct Answers {
    pub trust: bool,
    /// Passphrases, in the order asked.
    pub passphrases: Vec<String>,
    /// Passwords, in the order asked.
    pub passwords: Vec<String>,
    /// Keyboard-interactive answers (`None`: cancel).
    pub answers: Option<Vec<String>>,
    /// How long a host key question takes to answer.
    pub think: Duration,
}

/// Records every stage and question.
#[derive(Default)]
pub struct Recorded {
    pub stages: Vec<Stage>,
    pub host_keys: Vec<HostKeyQuestion>,
    pub passphrase_asks: Vec<(PathBuf, bool)>,
    pub password_asks: Vec<bool>,
    pub prompts: Vec<Prompts>,
}

pub struct TestAsker {
    pub answers: Mutex<Answers>,
    pub seen: Mutex<Recorded>,
}

impl TestAsker {
    pub fn new(answers: Answers) -> Arc<Self> {
        Arc::new(TestAsker { answers: Mutex::new(answers), seen: Mutex::new(Recorded::default()) })
    }
}

impl Asker for TestAsker {
    fn stage(&self, stage: Stage) {
        self.seen.lock().unwrap().stages.push(stage);
    }

    fn host_key(&self, question: HostKeyQuestion) -> BoxFuture<'static, bool> {
        self.seen.lock().unwrap().host_keys.push(question);
        let a = self.answers.lock().unwrap().clone();
        Box::pin(async move {
            tokio::time::sleep(a.think).await;
            a.trust
        })
    }

    fn passphrase(&self, path: PathBuf, wrong: bool) -> BoxFuture<'static, Option<Secret>> {
        self.seen.lock().unwrap().passphrase_asks.push((path, wrong));
        let mut a = self.answers.lock().unwrap();
        let next = (!a.passphrases.is_empty()).then(|| a.passphrases.remove(0));
        Box::pin(async move { next.map(Secret::new) })
    }

    fn password(&self, wrong: bool) -> BoxFuture<'static, Option<Secret>> {
        self.seen.lock().unwrap().password_asks.push(wrong);
        let mut a = self.answers.lock().unwrap();
        let next = (!a.passwords.is_empty()).then(|| a.passwords.remove(0));
        Box::pin(async move { next.map(Secret::new) })
    }

    fn answers(&self, prompts: Prompts) -> BoxFuture<'static, Option<Vec<Secret>>> {
        self.seen.lock().unwrap().prompts.push(prompts);
        let a = self.answers.lock().unwrap().answers.clone();
        Box::pin(async move { a.map(|v| v.into_iter().map(Secret::new).collect()) })
    }
}

/// A scratch directory of the test, removed with everything in it (generated private keys
/// too) when it goes out of scope, also when the test fails.
pub struct Scratch(PathBuf);

impl std::ops::Deref for Scratch {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A scratch directory of the test (see [`Scratch`]).
pub fn scratch(tag: &str) -> Scratch {
    let d = std::env::temp_dir().join(format!("datarig-ssh-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Scratch(d)
}

/// Host key files in `dir`: datarig's (`app`) and a stand-in for OpenSSH's (`user`).
pub fn known(dir: &std::path::Path) -> KnownHosts {
    KnownHosts { user: Some(dir.join("user_known_hosts")), app: dir.join("data").join("known_hosts") }
}

pub fn env(dir: &std::path::Path) -> Env {
    Env { known_hosts: known(dir), agent: None }
}

pub fn hop(server: &TestServer, auth: Auth) -> Hop {
    Hop { host: "127.0.0.1".into(), port: server.addr.port(), user: USER.into(), auth }
}

pub fn options() -> Options {
    Options { timeout: Duration::from_secs(5), keepalive: None }
}

/// A key file of `algorithm` in `dir`, private (OpenSSH format).
pub fn key_file(dir: &std::path::Path, name: &str, algorithm: Algorithm) -> (PathBuf, PublicKey) {
    let key = PrivateKey::random(&mut rand::rng(), algorithm).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, key.to_openssh(russh::keys::ssh_key::LineEnding::LF).unwrap().as_bytes()).unwrap();
    private(&path);
    (path, key.public_key().clone())
}

pub fn private(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let _ = path;
}

/// A known_hosts line for `key` at the test server.
pub fn line(server: &TestServer, key: &PublicKey) -> String {
    let o = key.to_openssh().unwrap();
    let mut w = o.split_whitespace();
    format!("[127.0.0.1]:{} {} {}\n", server.addr.port(), w.next().unwrap(), w.next().unwrap())
}
