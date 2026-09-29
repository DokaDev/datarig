//! One SSH connection to a bastion (a hop) and its `direct-tcpip` channels.
//!
//! [`Tunnel::open`] connects (TCP, key exchange), checks the host key ([`crate::known_hosts`],
//! asking the user through [`Asker`] about a key it does not know or that changed), and logs
//! in with the hop's method. The tunnel is then a [`Dialer`]: each dial opens a channel to
//! `host:port` as the bastion sees them. Nothing else is ever opened or accepted: no shell,
//! no agent forwarding, no channel the server tries to open.
//!
//! Every step reports its stage ([`Asker::stage`]) and every failure names the hop and why
//! ([`SshError`]). Each step has the hop's timeout, which stops running while a question waits
//! for the user. Keepalives notice a dead connection; [`Tunnel::lost`] says when and why it
//! ended. The tunnel never reopens itself.

use crate::keys::{self, KeyError};
use crate::known_hosts::{self, FileError, KnownHosts, Stored, Verdict};
use datarig_core::fault::{Fault, FaultKind};
use datarig_core::transport::{BoxedStream, DialError, Dialer, Refusal};
use futures::future::BoxFuture;
use russh::client::{self, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{Algorithm, HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{ChannelOpenFailure, MethodKind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use zeroize::Zeroizing;

/// A secret the user typed or a source gave (a password, a passphrase, an answer).
pub type Secret = Zeroizing<String>;

/// Where a hop is and how to log in there.
#[derive(Clone, Debug)]
pub struct Hop {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: Auth,
}

/// How to log in. `Debug` never shows a secret.
#[derive(Clone)]
pub enum Auth {
    /// The keys of the ssh-agent ([`Env::agent`]).
    Agent,
    /// A private key file (and its `-cert.pub`), with its passphrase when it has one and it is
    /// known already; otherwise [`Asker::passphrase`] asks.
    Key { path: PathBuf, passphrase: Option<Secret> },
    /// A password (also given to a keyboard-interactive login that asks for one thing only).
    /// `None`, or one the server refuses: [`Asker::password`] asks, at most three times.
    Password(Option<Secret>),
    /// The server's own questions (a one-time code), answered through [`Asker::answers`].
    KeyboardInteractive,
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Auth::Agent => write!(f, "Agent"),
            Auth::Key { path, passphrase } => {
                write!(f, "Key({path:?}, passphrase {})", if passphrase.is_some() { "<redacted>" } else { "<none>" })
            }
            Auth::Password(_) => write!(f, "Password(<redacted>)"),
            Auth::KeyboardInteractive => write!(f, "KeyboardInteractive"),
        }
    }
}

/// Settings of a tunnel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Each step's limit (connect, key exchange, login), not counting time a question waits.
    pub timeout: Duration,
    /// Send a keepalive after this long without traffic; three unanswered end the tunnel.
    pub keepalive: Option<Duration>,
}

impl Default for Options {
    fn default() -> Self {
        Options { timeout: Duration::from_secs(10), keepalive: Some(Duration::from_secs(15)) }
    }
}

/// What the tunnel uses of the machine: host key files and the agent.
#[derive(Clone, Debug)]
pub struct Env {
    pub known_hosts: KnownHosts,
    /// The agent's socket (Unix) or pipe (Windows); `None`: there is no agent. The app takes
    /// it from `SSH_AUTH_SOCK` (tests pass their own), never from here.
    pub agent: Option<PathBuf>,
}

/// Where opening a tunnel is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Connecting,
    Handshake,
    HostKey,
    Authenticating,
}

/// A host key to decide on. `Changed`: the files have other keys for the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKeyQuestion {
    pub host: String,
    pub port: u16,
    /// `ssh-ed25519`, … of the presented key.
    pub algorithm: String,
    pub fingerprint: String,
    /// The keys stored for the host (empty: it is unknown).
    pub stored: Vec<Stored>,
}

/// The server's keyboard-interactive questions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompts {
    pub name: String,
    pub instructions: String,
    /// Each question and whether its answer may be shown as typed.
    pub prompts: Vec<(String, bool)>,
}

/// What the tunnel asks of the app. Answers may take as long as the user needs; `None` or
/// `false` cancels the attempt.
pub trait Asker: Send + Sync {
    fn stage(&self, stage: Stage);
    /// Trust this key for the host (written to datarig's known_hosts only)?
    fn host_key(&self, question: HostKeyQuestion) -> BoxFuture<'static, bool>;
    /// The passphrase of the key file at `path` (`wrong`: the last one did not open it).
    /// `None` after a wrong one gives up with that error; before, it cancels.
    fn passphrase(&self, path: PathBuf, wrong: bool) -> BoxFuture<'static, Option<Secret>>;
    /// The password of the hop (`wrong`: the server refused the last one). `None` after a
    /// wrong one gives up with the refusal; before, it cancels.
    fn password(&self, wrong: bool) -> BoxFuture<'static, Option<Secret>>;
    /// Answers to the server's questions, one per prompt.
    fn answers(&self, prompts: Prompts) -> BoxFuture<'static, Option<Vec<Secret>>>;
}

/// How a hop was logged in to (for the error that says which one was refused).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Agent,
    Key,
    Password,
    KeyboardInteractive,
}

/// How an open tunnel got there: the host key it took and the login that worked (the staged
/// test connection shows them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    /// The host key's type (`ssh-ed25519`, …).
    pub host_key: String,
    /// The user trusted the key during this attempt (it was unknown, or it changed).
    pub trusted_now: bool,
    pub method: Method,
    /// What signed the login of a key file or the agent (`ssh-ed25519`, `rsa-sha2-512`, …).
    pub key: Option<String>,
}

/// Why opening a tunnel failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The host name does not resolve.
    HostNotFound(Fault),
    /// The TCP connection failed (refused, unreachable, …).
    Connect(Fault),
    /// A step took longer than the timeout.
    Timeout(Stage, Duration),
    /// The SSH handshake failed (no common algorithm, not an SSH server, …): the detail.
    Handshake(String),
    /// The user did not trust the host's key (it was unknown).
    HostKeyNotTrusted { fingerprint: String },
    /// The host's key changed and the user did not replace it.
    HostKeyChanged { fingerprint: String, stored: Vec<Stored> },
    /// A `@revoked` line names the host's key.
    HostKeyRevoked { fingerprint: String, stored: Stored },
    /// A known_hosts file could not be read or written.
    KnownHosts(FileError),
    /// The key file cannot be used.
    Key { path: PathBuf, error: KeyError },
    /// No agent to ask ([`Env::agent`] is `None`, or its socket does not answer).
    NoAgent(String),
    /// The agent has no keys.
    AgentEmpty,
    /// The server refused the login; the methods it would still take.
    Rejected { method: Method, remaining: Vec<String> },
    /// The server takes RSA signatures only with SHA-1 (`ssh-rsa`), which datarig does not
    /// make (OpenSSH 8.8 and later do not either).
    RsaSha1Only,
    /// The user cancelled a question.
    Cancelled,
    /// Anything else the SSH library reported.
    Protocol(String),
}

/// A failure of a hop: which one, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshError {
    pub host: String,
    pub port: u16,
    pub kind: ErrorKind,
}

/// Why an open tunnel ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loss {
    /// Keepalives went unanswered (a dead network or bastion).
    Keepalive,
    /// The server closed it (its message).
    Closed(String),
    /// The connection failed (the detail).
    Failed(String),
    /// The app closed it.
    ClosedByApp,
}

/// The time a step may take, not counting time while a question waits for the user.
#[derive(Clone)]
struct Deadline {
    paused: Arc<AtomicUsize>,
}

impl Deadline {
    fn new() -> Self {
        Deadline { paused: Arc::new(AtomicUsize::new(0)) }
    }

    /// Stop the clock while the returned guard lives.
    fn pause(&self) -> Paused {
        self.paused.fetch_add(1, Ordering::SeqCst);
        Paused(self.paused.clone())
    }

    /// Run `fut`, failing with `Err(())` once `limit` of unpaused time has passed.
    async fn run<T>(&self, limit: Duration, fut: impl std::future::Future<Output = T>) -> Result<T, ()> {
        let tick = Duration::from_millis(50);
        let clock = async {
            let mut left = limit;
            loop {
                tokio::time::sleep(tick.min(left)).await;
                if self.paused.load(Ordering::SeqCst) == 0 {
                    left = left.saturating_sub(tick);
                    if left.is_zero() {
                        return;
                    }
                }
            }
        };
        tokio::select! {
            out = fut => Ok(out),
            () = clock => Err(()),
        }
    }
}

struct Paused(Arc<AtomicUsize>);

impl Drop for Paused {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The russh client's side of the session: the host key check, and nothing the server offers.
struct Client {
    host: String,
    port: u16,
    known_hosts: KnownHosts,
    asker: Arc<dyn Asker>,
    deadline: Deadline,
    /// Why the host key was not accepted (the connect error says only "unknown key").
    refused: Arc<Mutex<Option<ErrorKind>>>,
    /// The host key taken: its type, and whether the user trusted it just now.
    accepted: Arc<Mutex<Option<(String, bool)>>>,
    lost: watch::Sender<Option<Loss>>,
}

impl Client {
    fn refuse(&self, kind: ErrorKind) -> bool {
        if let Ok(mut r) = self.refused.lock() {
            *r = Some(kind);
        }
        false
    }

    fn accept(&self, key: &PublicKey, now: bool) -> bool {
        if let Ok(mut a) = self.accepted.lock() {
            *a = Some((key.algorithm().to_string(), now));
        }
        true
    }

    async fn check(&self, key: &PublicKey) -> bool {
        self.asker.stage(Stage::HostKey);
        let fingerprint = known_hosts::fingerprint(key);
        let stored = match self.known_hosts.check(&self.host, self.port, key) {
            Err(e) => return self.refuse(ErrorKind::KnownHosts(e)),
            Ok(Verdict::Known) => return self.accept(key, false),
            Ok(Verdict::Revoked(stored)) => return self.refuse(ErrorKind::HostKeyRevoked { fingerprint, stored }),
            Ok(Verdict::Unknown) => Vec::new(),
            Ok(Verdict::Changed(stored)) => stored,
        };
        let question = HostKeyQuestion {
            host: self.host.clone(),
            port: self.port,
            algorithm: key.algorithm().to_string(),
            fingerprint: fingerprint.clone(),
            stored: stored.clone(),
        };
        let trusted = {
            let _paused = self.deadline.pause();
            self.asker.host_key(question).await
        };
        if !trusted {
            return self.refuse(match stored.is_empty() {
                true => ErrorKind::HostKeyNotTrusted { fingerprint },
                false => ErrorKind::HostKeyChanged { fingerprint, stored },
            });
        }
        match self.known_hosts.trust(&self.host, self.port, key) {
            Ok(()) => self.accept(key, true),
            Err(e) => self.refuse(ErrorKind::KnownHosts(e)),
        }
    }
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => Ok(self.check(key).await),
            // Certificates are not asked for (`host_key_certificates` is empty).
            _ => Ok(self.refuse(ErrorKind::Protocol("the server presented a host certificate".into()))),
        }
    }

    // Nothing the server opens is accepted: every kind of channel it may open is refused
    // explicitly (russh's defaults accept sessions, X11, direct-tcpip and direct-streamlocal
    // channels), and a kind russh does not know is never accepted.
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        _: russh::Channel<client::Msg>,
        _: &str,
        _: u32,
        _: &str,
        _: u32,
        reply: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Self::Error> {
        refuse(reply).await
    }

    async fn server_channel_open_forwarded_streamlocal(
        &mut self,
        _: russh::Channel<client::Msg>,
        _: &str,
        reply: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Self::Error> {
        refuse(reply).await
    }

    async fn server_channel_open_agent_forward(
        &mut self,
        _: russh::Channel<client::Msg>,
        reply: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Self::Error> {
        refuse(reply).await
    }

    async fn should_accept_unknown_server_channel(&mut self, _: russh::ChannelId, _: &str) -> bool {
        false
    }

    async fn server_channel_open_unknown(
        &mut self,
        _: russh::Channel<client::Msg>,
        reply: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Self::Error> {
        refuse(reply).await
    }

    async fn server_channel_open_session(
        &mut self,
        _: russh::Channel<client::Msg>,
        reply: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Self::Error> {
        refuse(reply).await
    }

    async fn server_channel_open_direct_tcpip(
        &mut self,
        _: russh::Channel<client::Msg>,
        _: &str,
        _: u32,
        _: &str,
        _: u32,
        reply: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Self::Error> {
        refuse(reply).await
    }

    async fn server_channel_open_direct_streamlocal(
        &mut self,
        _: russh::Channel<client::Msg>,
        _: &str,
        reply: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Self::Error> {
        refuse(reply).await
    }

    async fn server_channel_open_x11(
        &mut self,
        _: russh::Channel<client::Msg>,
        _: &str,
        _: u32,
        reply: client::ChannelOpenHandle,
        _: &mut client::Session,
    ) -> Result<(), Self::Error> {
        refuse(reply).await
    }

    async fn disconnected(&mut self, reason: client::DisconnectReason<Self::Error>) -> Result<(), Self::Error> {
        let loss = match &reason {
            client::DisconnectReason::ReceivedDisconnect(info) => Loss::Closed(info.message.clone()),
            client::DisconnectReason::Error(russh::Error::KeepaliveTimeout) => Loss::Keepalive,
            client::DisconnectReason::Error(e) => Loss::Failed(e.to_string()),
        };
        self.lost.send_if_modified(|l| {
            let first = l.is_none();
            if first {
                *l = Some(loss);
            }
            first
        });
        match reason {
            client::DisconnectReason::ReceivedDisconnect(_) => Ok(()),
            client::DisconnectReason::Error(e) => Err(e),
        }
    }
}

/// Refuse a channel the server opened.
async fn refuse(reply: client::ChannelOpenHandle) -> Result<(), russh::Error> {
    reply.reject(russh::ChannelOpenFailure::AdministrativelyProhibited).await;
    Ok(())
}

/// An open tunnel: a [`Dialer`] to what the bastion can reach.
pub struct Tunnel {
    handle: Arc<Handle<Client>>,
    lost: watch::Receiver<Option<Loss>>,
    lost_tx: watch::Sender<Option<Loss>>,
    host: String,
    port: u16,
    opened: Opened,
}

impl Tunnel {
    /// Connect to `hop` and log in (see the module documentation).
    pub async fn open(hop: Hop, options: Options, env: Env, asker: Arc<dyn Asker>) -> Result<Tunnel, SshError> {
        let fail = |kind| SshError { host: hop.host.clone(), port: hop.port, kind };
        let deadline = Deadline::new();
        let limit = options.timeout;
        asker.stage(Stage::Connecting);
        let addrs = match deadline.run(limit, tokio::net::lookup_host((hop.host.as_str(), hop.port))).await {
            Err(()) => return Err(fail(ErrorKind::Timeout(Stage::Connecting, limit))),
            Ok(Err(e)) => {
                return Err(fail(ErrorKind::HostNotFound(Fault::new(FaultKind::HostNotFound, e.to_string()))));
            }
            Ok(Ok(a)) => a.collect::<Vec<_>>(),
        };
        let socket = match deadline.run(limit, tokio::net::TcpStream::connect(&addrs[..])).await {
            Err(()) => return Err(fail(ErrorKind::Timeout(Stage::Connecting, limit))),
            Ok(Err(e)) => return Err(fail(ErrorKind::Connect(Fault::io(&e)))),
            Ok(Ok(s)) => s,
        };
        let _ = socket.set_nodelay(true);
        asker.stage(Stage::Handshake);
        let (lost_tx, lost) = watch::channel(None);
        let refused = Arc::new(Mutex::new(None));
        let accepted = Arc::new(Mutex::new(None));
        let client = Client {
            host: hop.host.clone(),
            port: hop.port,
            known_hosts: env.known_hosts.clone(),
            asker: asker.clone(),
            deadline: deadline.clone(),
            refused: refused.clone(),
            accepted: accepted.clone(),
            lost: lost_tx.clone(),
        };
        let config = Arc::new(config(&options, &env.known_hosts.algorithms(&hop.host, hop.port)));
        let mut handle = match deadline.run(limit, client::connect_stream(config, socket, client)).await {
            Err(()) => {
                let stage = if refused.lock().is_ok_and(|r| r.is_some()) { Stage::HostKey } else { Stage::Handshake };
                return Err(fail(ErrorKind::Timeout(stage, limit)));
            }
            Ok(Err(e)) => {
                let why = refused.lock().ok().and_then(|mut r| r.take());
                return Err(fail(why.unwrap_or_else(|| ErrorKind::Handshake(e.to_string()))));
            }
            Ok(Ok(h)) => h,
        };
        asker.stage(Stage::Authenticating);
        let login = login(&mut handle, &hop, &env, asker.as_ref(), &deadline);
        let (method, key) = match deadline.run(limit, login).await {
            Err(()) => return Err(fail(ErrorKind::Timeout(Stage::Authenticating, limit))),
            Ok(Err(kind)) => return Err(fail(kind)),
            Ok(Ok(used)) => used,
        };
        let (host_key, trusted_now) = accepted.lock().ok().and_then(|a| a.clone()).unwrap_or_default();
        let opened = Opened { host_key, trusted_now, method, key };
        Ok(Tunnel { handle: Arc::new(handle), lost, lost_tx, host: hop.host, port: hop.port, opened })
    }

    /// How it opened.
    pub fn opened(&self) -> &Opened {
        &self.opened
    }

    /// The bastion this tunnel goes through.
    pub fn hop(&self) -> (&str, u16) {
        (&self.host, self.port)
    }

    /// Whether it is still open.
    pub fn is_open(&self) -> bool {
        !self.handle.is_closed() && self.lost.borrow().is_none()
    }

    /// Resolves when the tunnel ends, with why.
    pub fn lost(&self) -> impl std::future::Future<Output = Loss> + Send + 'static {
        let mut lost = self.lost.clone();
        async move {
            match lost.wait_for(Option::is_some).await {
                Ok(l) => l.clone().unwrap_or(Loss::ClosedByApp),
                Err(_) => Loss::ClosedByApp,
            }
        }
    }

    /// Close it (its channels end). What waits on [`Tunnel::lost`] hears `ClosedByApp`.
    pub async fn close(&self) {
        self.lost_tx.send_if_modified(|l| {
            let first = l.is_none();
            if first {
                *l = Some(Loss::ClosedByApp);
            }
            first
        });
        let _ = self.handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
    }
}

impl Dialer for Tunnel {
    fn dial(&self, host: &str, port: u16) -> BoxFuture<'static, Result<BoxedStream, DialError>> {
        if !self.is_open() {
            return Box::pin(async { Err(DialError::NotOpen) });
        }
        let (host, handle) = (host.to_string(), self.handle.clone());
        let fut = async move {
            match handle.channel_open_direct_tcpip(host.clone(), u32::from(port), "127.0.0.1", 0).await {
                Ok(channel) => Ok(Box::new(channel.into_stream()) as BoxedStream),
                Err(russh::Error::ChannelOpenFailure(reason)) => {
                    let (reason, detail) = match reason {
                        ChannelOpenFailure::AdministrativelyProhibited => (Refusal::Prohibited, String::new()),
                        ChannelOpenFailure::ConnectFailed => (Refusal::Unreachable, String::new()),
                        // A refusal without words: its reason code.
                        ChannelOpenFailure::Other { code, reason } if reason.trim().is_empty() => {
                            (Refusal::Other, format!("reason code {code}"))
                        }
                        ChannelOpenFailure::Other { reason, .. } => (Refusal::Other, reason),
                        other => (Refusal::Other, format!("{other:?}")),
                    };
                    Err(DialError::Refused { host, port, reason, detail })
                }
                Err(russh::Error::SendError | russh::Error::Disconnect) => Err(DialError::NotOpen),
                Err(e) => Err(DialError::Failed(Fault::other(e.to_string()))),
            }
        };
        Box::pin(fut)
    }
}

/// The client configuration: keepalives, no compression, and host key types without SHA-1
/// signatures (`ssh-rsa`), the ones the files know for the host first.
fn config(options: &Options, known: &[Algorithm]) -> client::Config {
    let safe: Vec<Algorithm> =
        russh::Preferred::DEFAULT.key.iter().filter(|a| !matches!(a, Algorithm::Rsa { hash: None })).cloned().collect();
    let mut keys: Vec<Algorithm> = Vec::new();
    for a in known {
        // A stored RSA key is verified with SHA-2 signatures.
        let wanted: Vec<Algorithm> = match a {
            Algorithm::Rsa { .. } => safe.iter().filter(|s| matches!(s, Algorithm::Rsa { .. })).cloned().collect(),
            other => vec![other.clone()],
        };
        for w in wanted {
            if safe.contains(&w) && !keys.contains(&w) {
                keys.push(w);
            }
        }
    }
    for a in safe {
        if !keys.contains(&a) {
            keys.push(a);
        }
    }
    client::Config {
        keepalive_interval: options.keepalive,
        keepalive_max: 3,
        inactivity_timeout: None,
        nodelay: true,
        preferred: russh::Preferred { key: keys.into(), ..russh::Preferred::DEFAULT },
        ..Default::default()
    }
}

/// The methods a refused login leaves, by name.
fn names(methods: &russh::MethodSet) -> Vec<String> {
    methods.iter().map(String::from).collect()
}

/// The name of what a key signs with: its type, and for RSA the hash (`rsa-sha2-512`).
fn signing(algorithm: Algorithm, hash: Option<HashAlg>) -> String {
    match algorithm {
        Algorithm::Rsa { .. } => Algorithm::Rsa { hash }.to_string(),
        other => other.to_string(),
    }
}

/// Log in with the hop's method: the method that worked and what signed for it.
async fn login(
    handle: &mut Handle<Client>,
    hop: &Hop,
    env: &Env,
    asker: &dyn Asker,
    deadline: &Deadline,
) -> Result<(Method, Option<String>), ErrorKind> {
    let proto = |e: russh::Error| ErrorKind::Protocol(e.to_string());
    match &hop.auth {
        Auth::Key { path, passphrase } => {
            let mut pass = passphrase.clone();
            let mut asked = 0;
            let loaded = loop {
                match keys::load(path, pass.as_deref().map(String::as_str)) {
                    Ok(k) => break k,
                    // Asked for (again), at most three times (as OpenSSH).
                    Err(e @ (KeyError::Encrypted | KeyError::WrongPassphrase)) if asked < 3 => {
                        let _paused = deadline.pause();
                        let wrong = e == KeyError::WrongPassphrase;
                        match asker.passphrase(path.clone(), wrong).await {
                            Some(p) => pass = Some(p),
                            None if wrong => return Err(ErrorKind::Key { path: path.clone(), error: e }),
                            None => return Err(ErrorKind::Cancelled),
                        }
                        asked += 1;
                    }
                    Err(error) => return Err(ErrorKind::Key { path: path.clone(), error }),
                }
            };
            if let Some(cert) = loaded.cert.clone() {
                let algorithm = cert.algorithm().to_certificate_type().to_string();
                if handle.authenticate_openssh_cert(&hop.user, loaded.key.clone(), cert).await.map_err(proto)?.success()
                {
                    return Ok((Method::Key, Some(algorithm)));
                }
            }
            let hash = rsa_hash(handle, loaded.key.algorithm()).await?;
            let signed = signing(loaded.key.algorithm(), hash);
            let key = PrivateKeyWithHashAlg::new(loaded.key.clone(), hash);
            match handle.authenticate_publickey(&hop.user, key).await.map_err(proto)? {
                russh::client::AuthResult::Success => Ok((Method::Key, Some(signed))),
                russh::client::AuthResult::Failure { remaining_methods, .. } => {
                    Err(ErrorKind::Rejected { method: Method::Key, remaining: names(&remaining_methods) })
                }
            }
        }
        Auth::Password(given) => {
            let mut next = given.clone();
            let mut wrong = false;
            let mut remaining = Vec::new();
            for _ in 0..3 {
                let password = match next.take() {
                    Some(p) => p,
                    None => {
                        let _paused = deadline.pause();
                        match asker.password(wrong).await {
                            Some(p) => p,
                            None if wrong => break,
                            None => return Err(ErrorKind::Cancelled),
                        }
                    }
                };
                match handle.authenticate_password(&hop.user, password.as_str()).await.map_err(proto)? {
                    russh::client::AuthResult::Success => return Ok((Method::Password, None)),
                    russh::client::AuthResult::Failure { remaining_methods, .. }
                        if remaining_methods.contains(&MethodKind::KeyboardInteractive) =>
                    {
                        // A server that asks for the password through keyboard-interactive.
                        let answer =
                            |p: &Prompts| (p.prompts.len() == 1 && !p.prompts[0].1).then(|| vec![password.clone()]);
                        let by_kbd = interactive(handle, &hop.user, Method::Password, |p| {
                            Box::pin(futures::future::ready(answer(&p)))
                        });
                        match by_kbd.await {
                            Ok(()) => return Ok((Method::Password, None)),
                            Err(ErrorKind::Rejected { remaining: r, .. }) => remaining = r,
                            Err(e) => return Err(e),
                        }
                    }
                    russh::client::AuthResult::Failure { remaining_methods, .. } => {
                        remaining = names(&remaining_methods);
                    }
                }
                wrong = true;
            }
            Err(ErrorKind::Rejected { method: Method::Password, remaining })
        }
        Auth::KeyboardInteractive => interactive(handle, &hop.user, Method::KeyboardInteractive, |p| {
            let _paused = deadline.pause();
            let answers = asker.answers(p);
            Box::pin(async move {
                let _paused = _paused;
                answers.await
            })
        })
        .await
        .map(|()| (Method::KeyboardInteractive, None)),
        Auth::Agent => agent(handle, &hop.user, env).await.map(|signed| (Method::Agent, Some(signed))),
    }
}

/// The hash of an RSA signature: SHA-512 or SHA-256 as the server says it takes them
/// (`server-sig-algs`), SHA-256 when it does not say; never SHA-1. `None` for other keys.
async fn rsa_hash(handle: &Handle<Client>, algorithm: Algorithm) -> Result<Option<HashAlg>, ErrorKind> {
    if !algorithm.is_rsa() {
        return Ok(None);
    }
    match tokio::time::timeout(Duration::from_secs(2), handle.best_supported_rsa_hash()).await {
        Ok(Ok(Some(Some(hash)))) => Ok(Some(hash)),
        Ok(Ok(Some(None))) => Err(ErrorKind::RsaSha1Only),
        _ => Ok(Some(HashAlg::Sha256)),
    }
}

/// A keyboard-interactive login, each round of questions answered by `answer`.
async fn interactive(
    handle: &mut Handle<Client>,
    user: &str,
    method: Method,
    answer: impl Fn(Prompts) -> BoxFuture<'static, Option<Vec<Secret>>>,
) -> Result<(), ErrorKind> {
    let proto = |e: russh::Error| ErrorKind::Protocol(e.to_string());
    let mut reply = handle.authenticate_keyboard_interactive_start(user, None::<String>).await.map_err(proto)?;
    // A server that keeps asking forever is refused after a few rounds.
    for _ in 0..8 {
        match reply {
            KeyboardInteractiveAuthResponse::Success => return Ok(()),
            KeyboardInteractiveAuthResponse::Failure { remaining_methods, .. } => {
                return Err(ErrorKind::Rejected { method, remaining: names(&remaining_methods) });
            }
            KeyboardInteractiveAuthResponse::InfoRequest { name, instructions, prompts } => {
                let prompts =
                    Prompts { name, instructions, prompts: prompts.into_iter().map(|p| (p.prompt, p.echo)).collect() };
                let expected = prompts.prompts.len();
                let answers = match answer(prompts).await {
                    Some(a) if a.len() == expected => a,
                    Some(_) => return Err(ErrorKind::Rejected { method, remaining: Vec::new() }),
                    None => return Err(ErrorKind::Cancelled),
                };
                let answers: Vec<String> = answers.iter().map(|a| a.to_string()).collect();
                reply = handle.authenticate_keyboard_interactive_respond(answers).await.map_err(proto)?;
            }
        }
    }
    Err(ErrorKind::Rejected { method, remaining: Vec::new() })
}

/// Log in with each of the agent's keys in turn; what signed for the one that worked.
async fn agent(handle: &mut Handle<Client>, user: &str, env: &Env) -> Result<String, ErrorKind> {
    let Some(path) = env.agent.clone() else { return Err(ErrorKind::NoAgent(String::new())) };
    #[cfg(unix)]
    let client = russh::keys::agent::client::AgentClient::connect_uds(&path).await;
    #[cfg(windows)]
    let client = russh::keys::agent::client::AgentClient::connect_named_pipe(&path).await;
    let mut client = client.map_err(|e| ErrorKind::NoAgent(e.to_string()))?;
    let identities = client.request_identities().await.map_err(|e| ErrorKind::NoAgent(e.to_string()))?;
    if identities.is_empty() {
        return Err(ErrorKind::AgentEmpty);
    }
    let mut remaining = Vec::new();
    for identity in identities {
        let result = match identity {
            russh::keys::agent::AgentIdentity::PublicKey { key, .. } => {
                let hash = rsa_hash(handle, key.algorithm()).await?;
                let signed = signing(key.algorithm(), hash);
                (handle.authenticate_publickey_with(user, key, hash, &mut client).await, signed)
            }
            russh::keys::agent::AgentIdentity::Certificate { certificate, .. } => {
                let hash = rsa_hash(handle, certificate.algorithm()).await?;
                let signed = certificate.algorithm().to_certificate_type().to_string();
                (handle.authenticate_certificate_with(user, certificate, hash, &mut client).await, signed)
            }
        };
        match result {
            (Ok(russh::client::AuthResult::Success), signed) => return Ok(signed),
            (Ok(russh::client::AuthResult::Failure { remaining_methods, .. }), _) => {
                remaining = names(&remaining_methods)
            }
            (Err(e), _) => return Err(ErrorKind::Protocol(format!("{e:?}"))),
        }
    }
    Err(ErrorKind::Rejected { method: Method::Agent, remaining })
}
