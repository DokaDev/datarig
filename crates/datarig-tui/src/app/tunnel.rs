//! The SSH tunnels of the profiles.
//!
//! A profile whose tunnel is on connects through it: once its database password is known
//! ([`App::start_connect`]) the tunnel opens (unless one of this profile's is open with the same
//! settings), and every session of the profile, its cancels and its aux sessions dial through
//! it. A session of such a profile never connects directly: while no tunnel is open it dials a
//! closed one, which fails at once. Opening always starts here, from the UI; nothing reopens a
//! tunnel by itself. After it is lost, the next use of the profile connects again.
//!
//! A profile's tunnel is its own (`[connections.ssh]`) or a tunnel preset it names
//! ([`route`]). The profiles using a preset at the same time share one SSH connection
//! ([`SharedTunnel`]): the first one's attempt opens it (one login, one host key question, one
//! secret prompt, whatever number of profiles wait for it), the others take it as it is, and it
//! closes when the last of them lets it go (disconnect, delete, a failed attempt). Its loss ends
//! every profile on it. A preset changed while in use opens a new connection for the profiles
//! that connect from then on; the ones on the old connection keep it until they connect again.
//! A preset that is not there, or both a preset and the profile's own tunnel, is an error of the
//! profile ([`RouteError`]): it never connects directly.
//!
//! Opening runs on a task: the tunnel's secret is read there (the keychain never on the UI
//! thread), and every question for the user comes back as an [`AppEvent::Tunnel`] with a
//! channel for the answer: a host key (a confirmation, Cancel the default), a password, a key's
//! passphrase or a keyboard-interactive answer (the password prompt). An answer is bound to
//! the attempt that asked (profile and generation); a stale one is dropped, which cancels.
//! The attempt's own connect timeout does not run while the tunnel opens (each step of the
//! tunnel has its own, which stops while a question waits).

use super::password::{EnvLookup, Secret};
use super::*;
use datarig_core::fault::ErrorLog;
use datarig_core::profile::ssh::{SshAuth, SshSettings};
use datarig_core::profile::tunnel::{Route, RouteError, TunnelId, TunnelPreset, route};
use datarig_core::secret::Lookup;
use datarig_core::secret::source;
use datarig_core::transport::{BoxedStream, DialError, Dialer, DialerRef};
use datarig_ssh::known_hosts::KnownHosts;
use datarig_ssh::tunnel::{
    Asker, Auth, ErrorKind, Hop, HostKeyQuestion, Loss, Method, Opened, Options, Prompts, Secret as SshSecret,
    SshError, Stage,
};
use futures::future::BoxFuture;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use tokio::sync::oneshot;

mod shared;

/// An open tunnel as the app holds it.
pub trait OpenTunnel: Dialer {
    fn is_open(&self) -> bool;
    /// Resolves when it ends, with why.
    fn lost(&self) -> BoxFuture<'static, Loss>;
    fn close(&self) -> BoxFuture<'static, ()>;
    /// The host key it took and the login that worked (`None`: not known, as for a fake).
    fn opened(&self) -> Option<Opened> {
        None
    }
}

/// What opening a tunnel needs; its secret is read on the opening task.
pub struct TunnelRequest {
    pub settings: SshSettings,
    /// Typed earlier in this session (kept like a prompted database password).
    pub session_secret: Option<String>,
    pub account: String,
    pub stores: Stores,
    pub env: EnvLookup,
    pub known_hosts: KnownHosts,
    pub home: Option<PathBuf>,
    pub agent: Option<PathBuf>,
    pub asker: Arc<dyn Asker>,
}

/// Why a tunnel did not open: the hop said no, or its secret's source failed first.
#[derive(Debug)]
pub enum TunnelFailure {
    Ssh(SshError),
    Source(SourceError),
    /// The key file setting is missing (a profile the loader or form should have refused).
    NoKeyFile,
}

/// Opens tunnels: the binary's opens SSH connections ([`SshTunnels`]); tests fake them.
pub trait Tunnels: Send + Sync {
    fn open(&self, request: TunnelRequest) -> BoxFuture<'static, Result<Arc<dyn OpenTunnel>, TunnelFailure>>;
}

/// A question of the tunnel for the user, with where the answer goes.
pub enum TunnelAsk {
    HostKey(HostKeyQuestion, oneshot::Sender<bool>),
    Secret(SecretAsk, oneshot::Sender<Option<Secret>>),
}

/// Which secret the tunnel asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretAsk {
    /// The hop's password (`wrong`: the last one was refused).
    Password { wrong: bool },
    /// The key file's passphrase.
    Passphrase { path: PathBuf, wrong: bool },
    /// One keyboard-interactive question (`echo`: the answer may show as typed).
    Answer { prompt: String, echo: bool },
}

/// What happens to a profile's tunnel attempt.
pub enum TunnelEvent {
    Stage(Stage),
    Ask(TunnelAsk),
    Opened(Arc<dyn OpenTunnel>),
    /// A test connection's tunnel opened after this long, and how (its handle stays with the
    /// test).
    Through(Duration, Option<Opened>),
    Failed(TunnelFailure),
    /// The open tunnel ended.
    Lost(Loss),
}

impl std::fmt::Debug for TunnelEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TunnelEvent::Stage(s) => write!(f, "Stage({s:?})"),
            TunnelEvent::Ask(TunnelAsk::HostKey(q, _)) => write!(f, "Ask(HostKey({q:?}))"),
            TunnelEvent::Ask(TunnelAsk::Secret(s, _)) => write!(f, "Ask(Secret({s:?}))"),
            TunnelEvent::Opened(_) => write!(f, "Opened"),
            TunnelEvent::Through(d, how) => write!(f, "Through({d:?}, {how:?})"),
            TunnelEvent::Failed(e) => write!(f, "Failed({e:?})"),
            TunnelEvent::Lost(l) => write!(f, "Lost({l:?})"),
        }
    }
}

/// A dialer of a profile whose tunnel is not open: every dial fails at once (a session of a
/// tunnelled profile never connects directly).
pub struct ClosedTunnel;

impl Dialer for ClosedTunnel {
    fn dial(&self, _: &str, _: u16) -> BoxFuture<'static, Result<BoxedStream, DialError>> {
        Box::pin(async { Err(DialError::NotOpen) })
    }
}

/// A profile's open tunnel and the settings it was opened with.
pub struct ProfileTunnel {
    pub handle: Arc<dyn OpenTunnel>,
    pub settings: SshSettings,
    /// The shared connection of a preset it is (its serial and the preset); `None`: the
    /// profile's own.
    pub shared: Option<(u64, TunnelId)>,
}

/// What a tunnel was opened for: a connection attempt of a profile, a test connection, or a
/// preset's shared connection (by its serial).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Attempt { profile: ProfileId, generation: u64 },
    Test(u64),
    Shared(u64),
}

/// The SSH connection of a tunnel preset, shared by the profiles that use the preset at the same
/// time.
pub struct SharedTunnel {
    /// Its own number: its events carry it, and a number no entry has any more is stale.
    pub serial: u64,
    pub tunnel: TunnelId,
    /// The settings it opens (or opened) with.
    pub settings: SshSettings,
    pub state: SharedState,
    /// The profiles on it (their sessions dial through it).
    pub users: BTreeSet<ProfileId>,
    /// The attempts waiting for it to open: (profile, generation).
    pub waiting: Vec<(ProfileId, u64)>,
}

pub enum SharedState {
    /// Opening, at this stage.
    Opening(Stage),
    Open(Arc<dyn OpenTunnel>),
}

impl SharedTunnel {
    /// Its handle while it is open.
    pub fn open(&self) -> Option<&Arc<dyn OpenTunnel>> {
        match &self.state {
            SharedState::Open(h) if h.is_open() => Some(h),
            _ => None,
        }
    }
}

/// The shared connections of the presets.
#[derive(Default)]
pub struct SharedTunnels {
    pub list: Vec<SharedTunnel>,
    seq: u64,
    /// Why the last connection of a preset ended (its explorer row says so until it opens
    /// again).
    pub lost: HashMap<TunnelId, Notice>,
}

impl SharedTunnels {
    pub fn get(&self, serial: u64) -> Option<&SharedTunnel> {
        self.list.iter().find(|e| e.serial == serial)
    }

    fn get_mut(&mut self, serial: u64) -> Option<&mut SharedTunnel> {
        self.list.iter_mut().find(|e| e.serial == serial)
    }

    fn remove(&mut self, serial: u64) -> Option<SharedTunnel> {
        let i = self.list.iter().position(|e| e.serial == serial)?;
        Some(self.list.remove(i))
    }
}

/// Where a preset's connection is (its explorer row).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresetState {
    Closed,
    /// An attempt waits for it to open.
    Opening,
    /// Open, with this many profiles on it.
    Open(usize),
    /// Its last connection was lost (why: [`SharedTunnels::lost`]).
    Dead,
}

/// What a test of a preset reached through it: a database address of a profile using it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub host: String,
    pub port: u16,
    /// A channel to it opened after this long, or why not.
    pub result: Result<Duration, DialError>,
}

/// The asker of a tunnel: its questions go to the app as events of its owner.
struct AppAsker {
    tx: UnboundedSender<AppEvent>,
    owner: Owner,
}

impl AppAsker {
    fn send(&self, ev: TunnelEvent) {
        let _ = self.tx.send(match self.owner {
            Owner::Attempt { profile, generation } => AppEvent::Tunnel { profile, generation, ev },
            Owner::Test(seq) => AppEvent::TestTunnel { seq, ev },
            Owner::Shared(serial) => AppEvent::SharedTunnel { serial, ev },
        });
    }

    fn secret(&self, ask: SecretAsk) -> BoxFuture<'static, Option<SshSecret>> {
        let (tx, rx) = oneshot::channel();
        self.send(TunnelEvent::Ask(TunnelAsk::Secret(ask, tx)));
        Box::pin(async move { rx.await.ok().flatten().map(|Secret(s)| SshSecret::new(s)) })
    }
}

impl Asker for AppAsker {
    fn stage(&self, stage: Stage) {
        self.send(TunnelEvent::Stage(stage));
    }

    fn host_key(&self, question: HostKeyQuestion) -> BoxFuture<'static, bool> {
        let (tx, rx) = oneshot::channel();
        self.send(TunnelEvent::Ask(TunnelAsk::HostKey(question, tx)));
        Box::pin(async move { rx.await.unwrap_or(false) })
    }

    fn passphrase(&self, path: PathBuf, wrong: bool) -> BoxFuture<'static, Option<SshSecret>> {
        self.secret(SecretAsk::Passphrase { path, wrong })
    }

    fn password(&self, wrong: bool) -> BoxFuture<'static, Option<SshSecret>> {
        self.secret(SecretAsk::Password { wrong })
    }

    fn answers(&self, prompts: Prompts) -> BoxFuture<'static, Option<Vec<SshSecret>>> {
        // One question at a time, in order.
        let asks: Vec<_> =
            prompts.prompts.into_iter().map(|(prompt, echo)| self.secret(SecretAsk::Answer { prompt, echo })).collect();
        Box::pin(async move {
            let mut out = Vec::new();
            for a in asks {
                out.push(a.await?);
            }
            Some(out)
        })
    }
}

/// Real SSH tunnels (`datarig-ssh`).
pub struct SshTunnels;

/// An open `datarig_ssh::Tunnel`.
struct SshTunnel(Arc<datarig_ssh::Tunnel>);

impl Dialer for SshTunnel {
    fn dial(&self, host: &str, port: u16) -> BoxFuture<'static, Result<BoxedStream, DialError>> {
        self.0.dial(host, port)
    }
}

impl OpenTunnel for SshTunnel {
    fn is_open(&self) -> bool {
        self.0.is_open()
    }

    fn lost(&self) -> BoxFuture<'static, Loss> {
        Box::pin(self.0.lost())
    }

    fn close(&self) -> BoxFuture<'static, ()> {
        let t = self.0.clone();
        Box::pin(async move { t.close().await })
    }

    fn opened(&self) -> Option<Opened> {
        Some(self.0.opened().clone())
    }
}

impl Tunnels for SshTunnels {
    fn open(&self, r: TunnelRequest) -> BoxFuture<'static, Result<Arc<dyn OpenTunnel>, TunnelFailure>> {
        Box::pin(async move {
            let s = &r.settings;
            // The stored secret, when the method has one. A keychain that does not answer is
            // not "none stored": the tunnel asks for it (the prompt says why).
            let stored = match (s.auth.has_secret(), r.session_secret.clone()) {
                (false, _) => None,
                (true, Some(p)) => Some(p),
                (true, None) => {
                    let (source, account, stores, env) =
                        (s.source(), r.account.clone(), r.stores.clone(), r.env.clone());
                    let lookup = tokio::task::spawn_blocking(move || {
                        source::lookup(&source, &account, &stores, &*env, datarig_core::secret::command::TIMEOUT)
                    })
                    .await;
                    match lookup {
                        Ok(Ok(Lookup::Found(p))) => Some(p),
                        Ok(Ok(_)) | Ok(Err(SourceError::Keychain(_))) | Err(_) => None,
                        Ok(Err(e)) => return Err(TunnelFailure::Source(e)),
                    }
                }
            };
            let secret = stored.map(SshSecret::new);
            let auth = match s.auth {
                SshAuth::Key => match s.key_path(r.home.as_deref()) {
                    Some(path) => Auth::Key { path, passphrase: secret },
                    None => return Err(TunnelFailure::NoKeyFile),
                },
                SshAuth::Password => Auth::Password(secret),
                SshAuth::Agent => Auth::Agent,
                SshAuth::KeyboardInteractive => Auth::KeyboardInteractive,
            };
            let hop = Hop { host: s.host.trim().to_string(), port: s.port, user: s.user.trim().to_string(), auth };
            let options = Options { timeout: s.timeout(), keepalive: s.keepalive() };
            let env = datarig_ssh::Env { known_hosts: r.known_hosts, agent: r.agent };
            let tunnel = datarig_ssh::Tunnel::open(hop, options, env, r.asker).await.map_err(TunnelFailure::Ssh)?;
            Ok(Arc::new(SshTunnel(Arc::new(tunnel))) as Arc<dyn OpenTunnel>)
        })
    }
}

/// What saving the profile form does to the tunnel's secret: the settings before and after,
/// and a secret typed in the form.
pub(super) struct FormSecret {
    pub id: ProfileId,
    pub old: Option<SshSettings>,
    pub new: Option<SshSettings>,
    pub typed: Option<String>,
}

/// The store a tunnel's secret is kept in, if it is kept (its method has one, its source stores).
fn secret_store(s: Option<&SshSettings>) -> Option<SourceKind> {
    s.filter(|s| s.auth.has_secret()).map(|s| s.source().kind()).filter(|k| k.stores_secret())
}

/// A host key question waiting for the user (one confirmation shows at a time).
pub(super) struct HostKeyWait {
    pub owner: Owner,
    pub question: HostKeyQuestion,
    pub answer: oneshot::Sender<bool>,
}

/// A secret the tunnel waits for (one prompt shows at a time).
pub(super) struct SecretWait {
    pub owner: Owner,
    /// The profile's name and tunnel settings (a test's: the form's).
    pub name: String,
    pub settings: SshSettings,
    pub ask: SecretAsk,
    pub answer: oneshot::Sender<Option<Secret>>,
}

impl App {
    /// Open SSH tunnels with `tunnels` (the binary: [`SshTunnels`]; tests: a fake).
    pub fn set_tunnels(&mut self, tunnels: Arc<dyn Tunnels>) {
        self.tunnels = Some(tunnels);
    }

    /// How profile `c` reaches its database (see [`route`]).
    pub(crate) fn route_of<'a>(&'a self, c: &'a ConnectionConfig) -> Result<Route<'a>, RouteError> {
        route(c, &self.presets)
    }

    /// What profile `name`'s node says when its tunnel setting is wrong.
    pub(super) fn route_error_notice(&self, name: &str, e: &RouteError) -> Notice {
        let name = name.to_string();
        let msg = match e {
            RouteError::NotFound(tunnel) => Msg::TunnelNotFound { name, tunnel: tunnel.clone() },
            RouteError::Both { tunnel } => Msg::TunnelBoth { name, tunnel: tunnel.clone() },
        };
        Notice::new(msg, Level::Error)
    }

    /// The dialer sessions of profile `id` use: its open tunnel, a closed one while its tunnel
    /// is on but not open (or its tunnel setting is wrong), `None` without a tunnel.
    pub(super) fn dialer_of(&self, cfg: &ConnectionConfig) -> Option<DialerRef> {
        if matches!(self.route_of(cfg), Ok(Route::Direct)) {
            return None;
        }
        let open = self.conns.get(cfg.id).and_then(|c| c.tunnel.as_ref()).filter(|t| t.handle.is_open());
        Some(match open {
            Some(t) if self.route_of(cfg).is_ok() => DialerRef(t.handle.clone() as Arc<dyn Dialer>),
            _ => DialerRef(Arc::new(ClosedTunnel)),
        })
    }

    /// Profile `id`'s tunnel is open with `settings`, and is the shared one of `preset` (`None`:
    /// its own).
    pub(super) fn tunnel_ready(&self, id: ProfileId, settings: &SshSettings, preset: Option<TunnelId>) -> bool {
        self.conns
            .get(id)
            .and_then(|c| c.tunnel.as_ref())
            .is_some_and(|t| t.handle.is_open() && t.settings == *settings && t.shared.map(|(_, p)| p) == preset)
    }

    /// Let go of profile `id`'s tunnel: its own closes (its sessions' channels end with it); a
    /// preset's shared one closes when no other profile is on it.
    pub(super) fn close_tunnel(&mut self, id: ProfileId) {
        let Some(t) = self.conns.get_mut(id).and_then(|c| c.tunnel.take()) else { return };
        match t.shared {
            Some((serial, _)) => self.release_shared(serial, id),
            None => self.close_handle(t.handle),
        }
    }

    /// Close an open tunnel (on a task when there is an event loop).
    fn close_handle(&self, handle: Arc<dyn OpenTunnel>) {
        let close = handle.close();
        match self.tx {
            Some(_) => {
                tokio::spawn(close);
            }
            None => drop(close),
        }
    }

    /// Where datarig keeps the host keys it trusts, and the user's OpenSSH file (read only).
    fn known_hosts(&self) -> KnownHosts {
        let home = self.home();
        // The data directory (next to the saved queries); without one the state directory;
        // without either, nowhere (trusting a key then fails, and says why).
        let dir = self.paths.data.as_ref().or(self.paths.state.as_ref());
        KnownHosts {
            user: home.map(|h| h.join(".ssh").join("known_hosts")),
            app: dir.map(|d| d.join("known_hosts")).unwrap_or_default(),
        }
    }

    pub(crate) fn home(&self) -> Option<PathBuf> {
        (self.env)("HOME").or_else(|| (self.env)("USERPROFILE")).filter(|h| !h.is_empty()).map(PathBuf::from)
    }

    /// What opening the tunnel `settings` (its secret under `account`) for `owner` needs
    /// (`typed`: a secret typed in a form, used before the stored one).
    pub(super) fn tunnel_request(
        &self,
        account: String,
        settings: SshSettings,
        tx: UnboundedSender<AppEvent>,
        owner: Owner,
        typed: Option<String>,
    ) -> TunnelRequest {
        TunnelRequest {
            session_secret: typed.or_else(|| self.secrets.session(&account).map(str::to_string)),
            account,
            settings,
            stores: self.secrets.stores().clone(),
            env: self.env.clone(),
            known_hosts: self.known_hosts(),
            home: self.home(),
            agent: (self.env)("SSH_AUTH_SOCK").filter(|s| !s.is_empty()).map(PathBuf::from),
            asker: Arc::new(AppAsker { tx, owner }),
        }
    }

    /// Test `cfg` through a throwaway tunnel of its settings: the
    /// tunnel's stages show while it opens, then the database is asked through it, and the
    /// tunnel closes. `command`: the database password comes from this command, run in the
    /// test's task. Its questions are asked like a connect's; nothing typed is saved.
    pub(super) fn start_tunnel_test(&mut self, cfg: ConnectionConfig, command: Option<String>) {
        use datarig_core::secret::command as cmd;
        // The preset's settings and secret, or the profile's own.
        let (settings, account, own) = match self.route_of(&cfg) {
            Ok(Route::Preset(p)) => (p.settings.clone(), p.id.account(), false),
            Ok(Route::Inline(s)) => (s.clone(), SshSettings::account(cfg.id), true),
            _ => return,
        };
        self.clear_test();
        self.test_seq += 1;
        let seq = self.test_seq;
        let tunnel = Some(TunnelTest {
            host: settings.host.trim().to_string(),
            name: cfg.name.clone(),
            settings: settings.clone(),
            stage: None,
            opened: None,
            how: None,
            failure: None,
        });
        // A secret typed in the form is the profile's own tunnel's.
        let typed = self
            .overlays
            .form()
            .filter(|f| own && f.profile_id() == cfg.id)
            .and_then(super::profiles::ProfileForm::typed_bastion_secret);
        let mut state = TestState::Running;
        let abort = match (self.driver(&cfg.driver), self.tunnels.clone(), self.tx.clone()) {
            (None, _, _) => {
                state = TestState::Failed(
                    self.i18n.msg(&Msg::ConnUnknownDriver { driver: cfg.driver.clone() }).to_string(),
                );
                None
            }
            (_, None, _) => {
                state = TestState::Failed(self.i18n.label(Label::SshUnavailable).to_string());
                None
            }
            (Some(d), Some(tunnels), Some(tx)) => {
                let request = self.tunnel_request(account, settings, tx.clone(), Owner::Test(seq), typed);
                let open = tunnels.open(request);
                let task = tokio::spawn(async move {
                    let start = Instant::now();
                    let t = match open.await {
                        Ok(t) => t,
                        Err(e) => {
                            let _ = tx.send(AppEvent::TestTunnel { seq, ev: TunnelEvent::Failed(e) });
                            return;
                        }
                    };
                    let ev = TunnelEvent::Through(start.elapsed(), t.opened());
                    let _ = tx.send(AppEvent::TestTunnel { seq, ev });
                    let mut cfg = cfg;
                    if let Some(c) = command {
                        match tokio::task::spawn_blocking(move || cmd::run(&c, cmd::TIMEOUT)).await {
                            Ok(Ok(pw)) => cfg.password = pw,
                            Ok(Err(e)) => {
                                t.close().await;
                                let _ = tx.send(AppEvent::TestSource { seq, error: SourceError::Command(e) });
                                return;
                            }
                            Err(_) => {}
                        }
                    }
                    let dialer = Some(DialerRef(t.clone() as Arc<dyn Dialer>));
                    let result = d.ping(&cfg, TEST_TIMEOUT, dialer).await;
                    t.close().await;
                    let _ = tx.send(AppEvent::Ping { seq, result });
                });
                Some(task.abort_handle())
            }
            // Headless: recorded; the test feeds the events.
            (Some(_), Some(_), None) => {
                self.test_tunnel_requests.push(seq);
                None
            }
        };
        self.conn_test = Some(ConnTest { seq, started: Instant::now(), state, abort, tunnel, probe: false });
        self.report_test();
    }

    /// An event of test connection `seq`'s tunnel.
    pub(super) fn on_test_tunnel_event(&mut self, seq: u64, ev: TunnelEvent) {
        let Some(t) = self.conn_test.as_mut().filter(|t| t.seq == seq && t.state == TestState::Running) else {
            return;
        };
        let Some(tt) = t.tunnel.as_mut() else { return };
        match ev {
            TunnelEvent::Stage(stage) => tt.stage = Some(stage),
            TunnelEvent::Through(d, how) => (tt.opened, tt.how) = (Some(d), how),
            TunnelEvent::Ask(ask) => {
                let (name, settings) = (tt.name.clone(), tt.settings.clone());
                return self.queue_ask(Owner::Test(seq), name, settings, ask);
            }
            TunnelEvent::Failed(e) => {
                t.abort = None;
                let text = match &e {
                    TunnelFailure::Ssh(e) => self.ssh_error_text(e),
                    TunnelFailure::Source(e) => self.i18n.msg(&self.source_error(e)).to_string(),
                    TunnelFailure::NoKeyFile => self.i18n.label(Label::FormFieldSshKeyFile).to_string(),
                };
                let text = self.i18n.msg(&Msg::SshFailed { error: text }).to_string();
                if let Some(t) = self.conn_test.as_mut() {
                    if let Some(tt) = t.tunnel.as_mut() {
                        tt.failure = Some(text.clone());
                    }
                    t.state = TestState::Failed(text);
                }
            }
            TunnelEvent::Opened(_) | TunnelEvent::Lost(_) => {}
        }
        self.report_test();
    }

    /// Open profile `conn`'s tunnel for the attempt in progress; `cfg` (the profile with its
    /// resolved database password) waits for it.
    pub(super) fn open_tunnel(&mut self, conn: &ConnectionConfig, cfg: ConnectionConfig) {
        let settings = match self.route_of(conn) {
            Ok(Route::Preset(p)) => {
                let p = p.clone();
                return self.open_shared(conn, p, cfg);
            }
            Ok(Route::Inline(s)) => s.clone(),
            _ => return,
        };
        let id = conn.id;
        self.close_tunnel(id);
        let generation = self.conns.entry(id).generation;
        {
            let c = self.conns.entry(id);
            c.tunnel_wait = Some(cfg);
            if let Some(k) = c.connecting.as_mut() {
                k.tunnel = Some(Stage::Connecting);
            }
        }
        let Some(tunnels) = self.tunnels.clone() else {
            // No tunnels in this build (headless tests without a fake): never directly.
            let m = Notice::new(Label::SshUnavailable, Level::Error);
            return self.attempt_failed(id, m);
        };
        let Some(tx) = self.tx.clone() else {
            // Headless: the test feeds the attempt's events; the request is recorded.
            self.tunnel_requests.push((id, generation));
            return;
        };
        let owner = Owner::Attempt { profile: id, generation };
        let request = self.tunnel_request(SshSettings::account(id), settings, tx.clone(), owner, None);
        let open = tunnels.open(request);
        tokio::spawn(async move {
            let ev = match open.await {
                Ok(t) => TunnelEvent::Opened(t),
                Err(e) => TunnelEvent::Failed(e),
            };
            let _ = tx.send(AppEvent::Tunnel { profile: id, generation, ev });
        });
    }

    /// An event of profile `id`'s tunnel attempt `generation`.
    pub(super) fn on_tunnel_event(&mut self, id: ProfileId, generation: u64, ev: TunnelEvent) {
        let current = self.conns.is_current(id, generation);
        match ev {
            // A late tunnel of an attempt that is over: closed at once.
            TunnelEvent::Opened(t) if !current || self.conns.get(id).is_none_or(|c| c.tunnel_wait.is_none()) => {
                if self.tx.is_some() {
                    tokio::spawn(t.close());
                }
            }
            // The watcher of an open tunnel says it ended with the generation that opened it,
            // which a later attempt that reused the tunnel moved past: it still counts for the
            // tunnel the profile holds when that one is no longer open (a newer, open tunnel
            // is not the one lost).
            TunnelEvent::Lost(loss) if !current => {
                if self.conns.get(id).and_then(|c| c.tunnel.as_ref()).is_some_and(|t| !t.handle.is_open()) {
                    self.tunnel_lost(id, loss);
                }
            }
            // Questions of an attempt that is over are dropped (which cancels them).
            _ if !current => {}
            TunnelEvent::Stage(stage) => {
                if let Some(k) = self.conns.get_mut(id).and_then(|c| c.connecting.as_mut()) {
                    k.tunnel = Some(stage);
                }
                if let Some(p) = self.profile(id) {
                    let host = p.ssh.as_ref().map(|s| s.host.trim().to_string()).unwrap_or_default();
                    let stage = self.i18n.label(stage_label(stage)).to_string();
                    let m = Notice::new(Msg::SshConnecting { name: p.name.clone(), host, stage }, Level::Info);
                    self.show_status(m);
                }
            }
            TunnelEvent::Ask(ask) => {
                let Some(p) = self.profile(id).cloned() else { return };
                let settings = p.ssh.clone().unwrap_or_default();
                self.queue_ask(Owner::Attempt { profile: id, generation }, p.name, settings, ask);
            }
            TunnelEvent::Opened(handle) => self.tunnel_opened(id, generation, handle),
            TunnelEvent::Through(..) => {}
            TunnelEvent::Failed(e) => self.tunnel_failed(id, e),
            TunnelEvent::Lost(loss) => self.tunnel_lost(id, loss),
        }
    }

    fn tunnel_opened(&mut self, id: ProfileId, generation: u64, handle: Arc<dyn OpenTunnel>) {
        let Some(p) = self.profile(id).cloned() else { return self.close_handle(handle) };
        // Its own tunnel was turned off (or a preset picked) while it opened: not this one.
        let Some(settings) = p.inline_ssh().cloned() else {
            self.close_handle(handle);
            if let Some(c) = self.conns.get_mut(id) {
                c.tunnel_wait = None;
            }
            let m = Notice::new(Msg::ConnCancelled { name: p.name.clone() }, Level::Warning);
            return self.attempt_failed(id, m);
        };
        let now = self.now();
        let c = self.conns.entry(id);
        let cfg = c.tunnel_wait.take();
        c.tunnel = Some(ProfileTunnel { handle: handle.clone(), settings, shared: None });
        c.tunnel_lost = None;
        if let Some(k) = c.connecting.as_mut() {
            // The database's own connect timeout starts now.
            k.tunnel = None;
            k.started = now;
        }
        if let Some(tx) = self.tx.clone() {
            let lost = handle.lost();
            tokio::spawn(async move {
                let loss = lost.await;
                let _ = tx.send(AppEvent::Tunnel { profile: id, generation, ev: TunnelEvent::Lost(loss) });
            });
        }
        self.save_tunnel_secret(id);
        if let Some(cfg) = cfg {
            let pw = cfg.password.clone();
            self.start_connect(cfg, pw);
        }
    }

    fn tunnel_failed(&mut self, id: ProfileId, e: TunnelFailure) {
        self.tunnel_saves.remove(&id);
        if let Some(c) = self.conns.get_mut(id) {
            c.tunnel_wait = None;
        }
        let name = self.profile(id).map(|p| p.name.clone()).unwrap_or_default();
        let m = self.tunnel_failure_notice(&e, &name);
        self.attempt_failed(id, m.clone());
        self.status = Some(m);
    }

    /// A tunnel that did not open, in words (`name`: whose settings miss a key file).
    fn tunnel_failure_notice(&self, e: &TunnelFailure, name: &str) -> Notice {
        let text = match e {
            TunnelFailure::Ssh(e) => self.ssh_error_text(e),
            TunnelFailure::Source(e) => self.i18n.msg(&self.source_error(e)).to_string(),
            TunnelFailure::NoKeyFile => self
                .i18n
                .msg(&Msg::ConfigErrorSshMissing { key: "key_file".into(), profile: name.to_string() })
                .to_string(),
        };
        Notice::new(Msg::SshFailed { error: text }, Level::Error)
    }

    /// Profile `id`'s tunnel ended: its sessions end with it (each says so); the profile shows
    /// why, and connects again on its next use: its resolved connection is forgotten, so the
    /// next statement (or table) of one of its tabs waits for a connect of the profile, which
    /// opens a new tunnel the usual way (its questions and prompts included) and then runs it.
    /// Nothing that ran before runs again.
    fn tunnel_lost(&mut self, id: ProfileId, loss: Loss) {
        let Some(c) = self.conns.get_mut(id) else { return };
        let Some(t) = c.tunnel.take() else { return };
        if loss == Loss::ClosedByApp {
            return;
        }
        let m = self.loss_notice(&t.settings.host, &loss);
        self.lost_through(id, m);
    }

    /// A tunnel to `host` ended for `loss`, in words (the details go to the error log).
    fn loss_notice(&self, host: &str, loss: &Loss) -> Notice {
        let host = host.to_string();
        let msg = match loss {
            Loss::Keepalive => Msg::SshLostKeepalive { host },
            Loss::Closed(_) => Msg::SshLostClosed { host },
            Loss::Failed(_) | Loss::ClosedByApp => Msg::SshLostFailed { host },
        };
        if let Loss::Closed(detail) | Loss::Failed(detail) = loss {
            ErrorLog::new(self.paths.errors_log())
                .record("ssh.lost", &datarig_core::fault::Fault::other(detail.clone()));
        }
        Notice::new(msg, Level::Error)
    }

    /// Profile `id`'s tunnel ended (`m` says why; the tunnel is already let go): see
    /// [`App::tunnel_lost`].
    fn lost_through(&mut self, id: ProfileId, m: Notice) {
        // An attempt that was still connecting through it ends with it.
        if self.conns.get(id).is_some_and(|c| c.connecting.is_some()) {
            self.abort_connect(id, m.clone());
        }
        let c = self.conns.entry(id);
        c.tunnel_lost = Some(m.clone());
        c.connected = false;
        c.resolved = None;
        c.error = Some(m.clone());
        if let Some(s) = c.meta.take() {
            s.close();
        }
        self.status = Some(m);
    }

    /// A question of the tunnel of `owner` (the profile `name`, its `settings`) for the user.
    pub(super) fn queue_ask(&mut self, owner: Owner, name: String, settings: SshSettings, ask: TunnelAsk) {
        match ask {
            TunnelAsk::HostKey(question, answer) => {
                self.host_keys.push_back(HostKeyWait { owner, question, answer });
                self.next_host_key();
            }
            TunnelAsk::Secret(ask, answer) => {
                self.secret_waits.push_back(SecretWait { owner, name, settings, ask, answer });
                self.next_secret_wait();
            }
        }
    }

    /// The tunnel of `owner` is still wanted: its attempt is the current one, its test runs, or
    /// a current attempt waits for the shared connection.
    fn owner_current(&self, owner: Owner) -> bool {
        match owner {
            Owner::Attempt { profile, generation } => self.conns.is_current(profile, generation),
            Owner::Test(seq) => self.conn_test.as_ref().is_some_and(|t| t.seq == seq && t.state == TestState::Running),
            Owner::Shared(serial) => self.shared.get(serial).is_some_and(|e| {
                matches!(e.state, SharedState::Opening(_)) && e.waiting.iter().any(|w| self.waits_for_tunnel(*w))
            }),
        }
    }

    /// The attempt `(profile, generation)` is the current one and waits for its tunnel.
    fn waits_for_tunnel(&self, (profile, generation): (ProfileId, u64)) -> bool {
        self.conns.is_current(profile, generation) && self.conns.get(profile).is_some_and(|c| c.tunnel_wait.is_some())
    }

    /// The next host key question, when no other shows.
    fn next_host_key(&mut self) {
        if self.overlays.confirm().is_some_and(|c| c.action == ConfirmAction::TrustHostKey) {
            return;
        }
        while let Some(w) = self.host_keys.front() {
            if self.owner_current(w.owner) {
                break;
            }
            self.host_keys.pop_front();
        }
        let Some(w) = self.host_keys.front() else { return };
        let q = &w.question;
        let host = format!("{}:{}", q.host, q.port);
        let (title, text) = if q.stored.is_empty() {
            (
                Label::SshHostKeyNewTitle,
                Msg::SshHostKeyNew { host, algorithm: q.algorithm.clone(), fingerprint: q.fingerprint.clone() },
            )
        } else {
            (
                Label::SshHostKeyChangedTitle,
                Msg::SshHostKeyChanged { host, algorithm: q.algorithm.clone(), fingerprint: q.fingerprint.clone() },
            )
        };
        let mut details: Vec<Msg> = q
            .stored
            .iter()
            .map(|s| Msg::SshHostKeyStored {
                fingerprint: s.fingerprint.clone(),
                file: s.file.display().to_string(),
                line: s.line.to_string(),
            })
            .collect();
        if !q.stored.is_empty() {
            details.push(Label::SshHostKeyChangedWarning.into());
        }
        details.push(Label::SshHostKeyWhere.into());
        self.overlays.push(Overlay::Confirm(Confirm {
            title,
            text,
            details,
            keys: Label::SshHostKeyKeys,
            action: ConfirmAction::TrustHostKey,
            folder: None,
            path: None,
        }));
    }

    /// Profile `id`'s attempt ended: the questions its tunnel asked are answered "no" and
    /// their dialogs close; it no longer waits for a preset's shared connection (whose
    /// questions go too when no other attempt waits for it).
    pub(super) fn drop_tunnel_asks(&mut self, id: ProfileId) {
        self.drop_asks(|o| matches!(o, Owner::Attempt { profile, .. } if profile == id));
        self.tunnel_saves.remove(&id);
        self.leave_shared_waits(id);
    }

    /// The questions of the tunnels `of` selects are answered "no" and their dialogs close.
    pub(super) fn drop_asks(&mut self, of: impl Fn(Owner) -> bool) {
        let showing_key = self.host_keys.front().is_some_and(|w| of(w.owner))
            && self.overlays.confirm().is_some_and(|c| c.action == ConfirmAction::TrustHostKey);
        self.host_keys.retain(|w| !of(w.owner));
        if showing_key {
            self.overlays.close(OverlayKind::Confirm);
            self.next_host_key();
        }
        let showing_secret = self.secret_waits.front().is_some_and(|w| of(w.owner))
            && self.overlays.prompt().is_some_and(|p| matches!(p.purpose, PromptPurpose::Tunnel));
        self.secret_waits.retain(|w| !of(w.owner));
        if showing_secret {
            self.overlays.close(OverlayKind::Password);
            self.next_secret_wait();
        }
    }

    /// The user answered the host key question shown.
    pub(super) fn host_key_answered(&mut self, trust: bool) {
        if let Some(w) = self.host_keys.pop_front() {
            let _ = w.answer.send(trust);
        }
        self.next_host_key();
    }

    /// The next secret the tunnel waits for, when no prompt shows.
    pub(super) fn next_secret_wait(&mut self) {
        if self.overlays.prompt().is_some() {
            return;
        }
        while let Some(w) = self.secret_waits.front() {
            if self.owner_current(w.owner) {
                break;
            }
            self.secret_waits.pop_front();
        }
        let Some(w) = self.secret_waits.front() else { return };
        let ssh = w.settings.clone();
        // A test never saves what was typed for it.
        let (testing, id) = match w.owner {
            Owner::Attempt { profile, .. } => (false, profile),
            Owner::Test(_) => (true, ProfileId::new()),
            Owner::Shared(_) => (false, ProfileId::new()),
        };
        let target = format!("{}@{}", ssh.user.trim(), ssh.host.trim());
        let (title, field, echo, storable) = match &w.ask {
            SecretAsk::Password { .. } => {
                (Msg::SshPromptPassword { target }, Label::FormFieldPassword.into(), false, true)
            }
            SecretAsk::Passphrase { path, .. } => (
                Msg::SshPromptPassphrase { path: short_path(path, self.home().as_deref(), PROMPT_PATH) },
                Label::SshFieldPassphrase.into(),
                false,
                true,
            ),
            SecretAsk::Answer { prompt, echo } => (
                Msg::SshPromptAnswer { target },
                crate::app::PromptField::Text(prompt.trim().to_string()),
                *echo,
                false,
            ),
        };
        let wrong = matches!(w.ask, SecretAsk::Password { wrong: true } | SecretAsk::Passphrase { wrong: true, .. });
        let kind = ssh.source().kind();
        let (save_to, note, save) = match kind {
            _ if !storable || testing => (None, Label::PromptPasswordNotSaved, false),
            SourceKind::Keychain if self.keychain_ok() => (Some(SourceKind::Keychain), Label::PromptPasswordSave, true),
            SourceKind::Keychain if self.keychain_unsure() => {
                (Some(SourceKind::Keychain), Label::PromptPasswordSaveUnsure, false)
            }
            SourceKind::Keychain => (None, Label::PromptPasswordSaveUnavailable, false),
            SourceKind::File => (Some(SourceKind::File), Label::PromptPasswordSaveFile, true),
            _ => (None, Label::PromptPasswordNotSaved, false),
        };
        let error = match (wrong, &w.ask) {
            (true, SecretAsk::Passphrase { .. }) => Notice::new(Label::SshPassphraseWrong, Level::Warning),
            (true, _) => Notice::new(Label::SshPasswordWrong, Level::Warning),
            _ => match w.owner {
                // One prompt for every profile waiting for the preset's connection.
                Owner::Shared(serial) => {
                    let names = self.shared_waiters(serial).join(", ");
                    Notice::new(Msg::SshPromptWhyPreset { tunnel: w.name.clone(), names }, Level::Info)
                }
                _ => Notice::new(Msg::SshPromptWhy { name: w.name.clone() }, Level::Info),
            },
        };
        self.overlays.push(Overlay::Password(PasswordPrompt {
            id,
            profile: w.name.clone(),
            input: TextInput::default(),
            error,
            save,
            save_focus: false,
            save_to,
            note,
            purpose: PromptPurpose::Tunnel,
            title: Some(title),
            field,
            echo,
        }));
    }

    /// `Enter` (`answer`) or `Esc` (`None`) in a tunnel's prompt.
    pub(super) fn tunnel_prompt_answered(&mut self, answer: Option<(String, bool, Option<SourceKind>)>) {
        let Some(w) = self.secret_waits.pop_front() else { return };
        match answer {
            Some((text, save, save_to)) => {
                let storable = matches!(w.ask, SecretAsk::Password { .. } | SecretAsk::Passphrase { .. });
                if let (true, Owner::Shared(serial)) = (storable, w.owner)
                    && let Some(tunnel) = self.shared.get(serial).map(|e| e.tunnel)
                {
                    // The preset's: kept for this session, written to its store once it opened.
                    self.secrets.remember(&tunnel.account(), &text);
                    match (save, save_to) {
                        (true, Some(kind)) => {
                            self.shared_saves.insert(serial, (kind, text.clone()));
                        }
                        _ => {
                            self.shared_saves.remove(&serial);
                        }
                    }
                }
                if let (true, Owner::Attempt { profile, generation }) = (storable, w.owner) {
                    // Kept for this session (a reconnect after a loss does not ask again), and
                    // written to the tunnel's store once it opened, when asked to.
                    let account = SshSettings::account(profile);
                    self.secrets.remember(&account, &text);
                    match (save, save_to) {
                        (true, Some(kind)) => {
                            self.tunnel_saves.insert(profile, (generation, kind, text.clone()));
                        }
                        _ => {
                            self.tunnel_saves.remove(&profile);
                        }
                    }
                }
                let _ = w.answer.send(Some(Secret(text)));
            }
            None => {
                let _ = w.answer.send(None);
            }
        }
    }

    /// The profile form was saved: a secret typed for the tunnel goes to its store; a store the
    /// tunnel's secret no longer uses loses it (the keychain on a worker).
    pub(super) fn apply_tunnel_secret(&mut self, f: FormSecret) {
        let account = SshSettings::account(f.id);
        let (old, new) = (secret_store(f.old.as_ref()), secret_store(f.new.as_ref()));
        if let Some(kind) = old.filter(|k| Some(*k) != new) {
            if kind == SourceKind::Keychain {
                self.remove_from_keychain(vec![account.clone()]);
            } else {
                let _ = self.secrets.remove_in(kind, &account);
            }
            self.secrets.forget(&account);
        }
        let (Some(kind), Some(text)) = (new, f.typed) else { return };
        self.secrets.forget(&account);
        let Some(p) = self.profile(f.id).cloned() else { return };
        if kind == SourceKind::Keychain {
            self.save_account_to_keychain(&p, account, text, keychain::AfterSave::Form { plaintext: false });
        } else if let Err(e) = self.secrets.save_in(kind, &account, &text) {
            let m = Notice::new(self.source_error(&e), Level::Warning);
            self.flash(m);
        }
    }

    /// Write a secret typed for profile `id`'s tunnel to its store, now that it opened.
    fn save_tunnel_secret(&mut self, id: ProfileId) {
        let Some((generation, kind, text)) = self.tunnel_saves.remove(&id) else { return };
        if !self.conns.is_current(id, generation) {
            return;
        }
        let Some(p) = self.profile(id).cloned() else { return };
        let account = SshSettings::account(id);
        if kind == SourceKind::Keychain {
            self.save_account_to_keychain(&p, account, text, keychain::AfterSave::Tunnel);
        } else {
            let m = match self.secrets.save_in(kind, &account, &text) {
                Ok(()) => Notice::new(Msg::SshSecretSavedFile { name: p.name.clone() }, Level::Success),
                Err(e) => Notice::new(self.source_error(&e), Level::Warning),
            };
            self.flash(m);
        }
    }

    /// A tunnel failure in words: the hop, and what went wrong at which stage.
    pub(super) fn ssh_error_text(&self, e: &SshError) -> String {
        let host = format!("{}:{}", e.host, e.port);
        let log = |detail: &str| {
            ErrorLog::new(self.paths.errors_log())
                .record("ssh.failed", &datarig_core::fault::Fault::other(format!("{host}: {detail}")))
        };
        let msg = match &e.kind {
            ErrorKind::HostNotFound(f) => {
                log(&f.detail);
                Msg::SshHostNotFound { host: e.host.clone() }
            }
            ErrorKind::Connect(f) => {
                log(&f.detail);
                let reason = self.i18n.label(super::execution::network_reason(&f.kind)).to_string();
                Msg::SshConnectFailed { host, reason }
            }
            ErrorKind::Timeout(stage, d) => {
                let stage = self.i18n.label(stage_label(*stage)).to_string();
                Msg::SshTimeout { host, stage, elapsed: *d }
            }
            ErrorKind::Handshake(detail) => {
                log(detail);
                Msg::SshHandshake { host }
            }
            ErrorKind::HostKeyNotTrusted { .. } => Msg::SshHostKeyNotTrusted { host },
            ErrorKind::HostKeyChanged { .. } => Msg::SshHostKeyChangedRefused { host },
            ErrorKind::HostKeyRevoked { .. } => Msg::SshHostKeyRevoked { host },
            ErrorKind::KnownHosts(f) => {
                log(&f.fault.detail);
                Msg::SshKnownHosts { file: f.file.display().to_string() }
            }
            ErrorKind::Key { path, error } => {
                let path = path.display().to_string();
                match error {
                    datarig_ssh::keys::KeyError::Unreadable(f) => {
                        log(&f.detail);
                        Msg::SshKeyUnreadable { path }
                    }
                    datarig_ssh::keys::KeyError::OpenToOthers { mode } => {
                        Msg::SshKeyOpen { path, mode: format!("{mode:03o}") }
                    }
                    datarig_ssh::keys::KeyError::Putty => Msg::SshKeyPutty { path },
                    datarig_ssh::keys::KeyError::Encrypted | datarig_ssh::keys::KeyError::WrongPassphrase => {
                        Msg::SshKeyPassphrase { path }
                    }
                    datarig_ssh::keys::KeyError::UnsupportedEncryption(cipher) => {
                        Msg::SshKeyCipher { path, cipher: cipher.clone() }
                    }
                    datarig_ssh::keys::KeyError::Unsupported(detail) => {
                        log(detail);
                        Msg::SshKeyUnsupported { path }
                    }
                }
            }
            ErrorKind::NoAgent(detail) => {
                log(detail);
                Msg::SshNoAgent { host }
            }
            ErrorKind::AgentEmpty => Msg::SshAgentEmpty { host },
            ErrorKind::Rejected { method, remaining } => {
                let accepts = remaining.join(", ");
                match method {
                    Method::Key => Msg::SshRejectedKey { host, accepts },
                    Method::Password => Msg::SshRejectedPassword { host, accepts },
                    Method::Agent => Msg::SshRejectedAgent { host, accepts },
                    Method::KeyboardInteractive => Msg::SshRejectedAnswers { host, accepts },
                }
            }
            ErrorKind::RsaSha1Only => Msg::SshRsaSha1 { host },
            ErrorKind::Cancelled => Msg::SshCancelled { host },
            ErrorKind::Protocol(detail) => {
                log(detail);
                Msg::SshProtocol { host }
            }
        };
        self.i18n.msg(&msg).to_string()
    }
}

/// A stage of opening a tunnel, as the spinner and messages name it.
/// The most columns a key file's path takes in the passphrase prompt's title (64 wide at
/// most), so the words around it stay.
const PROMPT_PATH: usize = 36;

/// `path` in at most `max` columns: the home directory as `~`, then, when still too long,
/// `…/` and the end of the path (the file's name is kept whole). The path's own separators are
/// kept: on Windows both `\` and `/` separate (a path typed with `/`, or `~/` before the rest).
pub fn short_path(path: &Path, home: Option<&Path>, max: usize) -> String {
    let full = match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    };
    if crate::text::width(&full) <= max {
        return full;
    }
    // Where each separator is; the tail starts after one of them (the last: the file's name).
    let seps: Vec<usize> = full.match_indices(std::path::is_separator).map(|(at, _)| at).collect();
    let mut start = seps.last().map_or(0, |at| at + 1);
    for &at in seps.iter().rev().skip(1) {
        if crate::text::width(&full[at + 1..]) + 2 > max {
            break;
        }
        start = at + 1;
    }
    let sep = full[..start].chars().next_back().unwrap_or(std::path::MAIN_SEPARATOR);
    format!("…{sep}{}", &full[start..])
}

pub fn stage_label(stage: Stage) -> Label {
    match stage {
        Stage::Connecting => Label::SshStageConnecting,
        Stage::Handshake => Label::SshStageHandshake,
        Stage::HostKey => Label::SshStageHostKey,
        Stage::Authenticating => Label::SshStageAuthenticating,
    }
}
