//! Profiles that connect through an SSH tunnel, headless: the tunnel attempts
//! are recorded and the test answers them as the tunnel task would (stages, questions, the
//! open tunnel or its failure, its loss).

mod common;

use common::*;
use datarig_core::driver::{DbEvent, SessionRole};
use datarig_core::i18n::Lang;
use datarig_core::profile::ProfileId;
use datarig_core::profile::ssh::{SshAuth, SshSettings};
use datarig_core::secret::MemoryStore;
use datarig_core::transport::{BoxedStream, DialError, Dialer};
use datarig_ssh::known_hosts::Stored;
use datarig_ssh::tunnel::{ErrorKind, HostKeyQuestion, Loss, Method, Opened, SshError, Stage};
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::tunnel::{OpenTunnel, SecretAsk, TunnelAsk, TunnelEvent, TunnelFailure, TunnelRequest, Tunnels};
use datarig_tui::app::{AppEvent, NodeState, PromptPurpose, Startup};
use futures::future::BoxFuture;
use ratatui::crossterm::event::KeyCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;

/// Headless attempts never reach it: they are recorded in `App::tunnel_requests`.
struct Recorded;

impl Tunnels for Recorded {
    fn open(&self, _: TunnelRequest) -> BoxFuture<'static, Result<Arc<dyn OpenTunnel>, TunnelFailure>> {
        unreachable!("headless attempts are recorded, not made")
    }
}

/// A tunnel the test holds: open until closed.
#[derive(Default)]
struct FakeTunnel {
    closed: AtomicBool,
}

impl Dialer for FakeTunnel {
    fn dial(&self, _: &str, _: u16) -> BoxFuture<'static, Result<BoxedStream, DialError>> {
        Box::pin(async { Err(DialError::NotOpen) })
    }
}

impl OpenTunnel for FakeTunnel {
    fn is_open(&self) -> bool {
        !self.closed.load(Ordering::SeqCst)
    }

    fn lost(&self) -> BoxFuture<'static, Loss> {
        Box::pin(futures::future::pending())
    }

    fn close(&self) -> BoxFuture<'static, ()> {
        self.closed.store(true, Ordering::SeqCst);
        Box::pin(async {})
    }
}

fn tunnel_settings() -> SshSettings {
    SshSettings {
        enabled: true,
        host: "bastion.example.com".into(),
        user: "ec2-user".into(),
        auth: SshAuth::Password,
        ..SshSettings::default()
    }
}

/// The sample profiles, `local-pg` through a tunnel; connecting headless with the fake driver.
fn harness() -> (Harness, ProfileId) {
    let mut cfg = sample_config(None);
    cfg.connections[0].ssh = Some(tunnel_settings());
    let mut h = Harness::launched(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal).with_fake_driver();
    h.app.set_tunnels(Arc::new(Recorded));
    let id = h.app.profiles[0].id;
    (h, id)
}

/// Connect `local-pg`: its tunnel is asked for (and nothing else yet). The attempt's generation.
fn connect(h: &mut Harness, id: ProfileId) -> u64 {
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    let generation = h.app.conns.get(id).unwrap().generation;
    assert_eq!(h.app.tunnel_requests.last(), Some(&(id, generation)));
    assert!(h.driver.sessions.lock().unwrap().is_empty(), "no session before the tunnel is open");
    generation
}

fn tunnel(h: &mut Harness, id: ProfileId, generation: u64, ev: TunnelEvent) {
    h.app.on_app_event(AppEvent::Tunnel { profile: id, generation, ev });
}

fn question(stored: Vec<Stored>) -> HostKeyQuestion {
    HostKeyQuestion {
        host: "bastion.example.com".into(),
        port: 22,
        algorithm: "ssh-ed25519".into(),
        fingerprint: "SHA256:presentedPresentedPresentedPresented00000".into(),
        stored,
    }
}

#[test]
fn sessions_of_a_tunnelled_profile_dial_through_its_tunnel() {
    let (mut h, id) = harness();
    let generation = connect(&mut h, id);
    assert_eq!(h.app.conns.state(id), NodeState::Connecting);
    tunnel(&mut h, id, generation, TunnelEvent::Stage(Stage::Authenticating));
    assert!(
        h.status(160, 24).contains("Connecting to local-pg through bastion.example.com (login)"),
        "{}",
        h.status(160, 24)
    );
    // The attempt's own timeout does not run while the tunnel opens.
    h.clock.advance(Duration::from_secs(60));
    h.app.on_tick(h.clock.now());
    assert_eq!(h.app.conns.state(id), NodeState::Connecting);
    let fake = Arc::new(FakeTunnel::default());
    tunnel(&mut h, id, generation, TunnelEvent::Opened(fake.clone()));
    let sessions =
        h.driver.sessions.lock().unwrap().iter().map(|s| (s.role, s.opts.dialer.clone())).collect::<Vec<_>>();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].0, SessionRole::Meta);
    let through = sessions[0].1.clone().expect("through a dialer");
    // The tunnel's own handle.
    assert!(std::ptr::addr_eq(Arc::as_ptr(&through.0), Arc::as_ptr(&fake)));
    h.db(DbEvent::Connected);
    assert_eq!(h.app.conns.state(id), NodeState::Connected);
    // The console's query session goes through it too.
    h.app.focus = datarig_tui::app::Focus::Editor;
    h.keys("i");
    h.type_text("select 1");
    h.ctrl('e');
    let q =
        h.driver.sessions.lock().unwrap().iter().find(|s| s.role == SessionRole::Query).map(|s| s.opts.dialer.clone());
    assert!(q.flatten().is_some_and(|d| std::ptr::addr_eq(Arc::as_ptr(&d.0), Arc::as_ptr(&fake))));
    // Disconnecting closes it.
    h.explore("local-pg");
    h.keys("x");
    // The statement still runs: asked first.
    assert!(h.app.overlays.confirm().is_some());
    h.key(KeyCode::Char('y'));
    assert!(fake.closed.load(Ordering::SeqCst));
}

#[test]
fn a_host_key_question_is_a_confirmation_that_cancel_answers() {
    let (mut h, id) = harness();
    let generation = connect(&mut h, id);
    let (tx, mut rx) = oneshot::channel();
    tunnel(&mut h, id, generation, TunnelEvent::Ask(TunnelAsk::HostKey(question(Vec::new()), tx)));
    let c = h.app.overlays.confirm().expect("asked");
    assert_eq!(c.action, ConfirmAction::TrustHostKey);
    let screen = h.screen(100, 30);
    assert!(screen.contains("Unknown SSH host") && screen.contains("SHA256:presented"), "{screen}");
    // Enter keeps the host untrusted (Cancel is the default).
    h.key(KeyCode::Enter);
    assert_eq!(rx.try_recv(), Ok(false));
    assert!(h.app.overlays.confirm().is_none());
    // A changed key: the stored one is shown with its file, and `y` trusts the new one.
    let stored = Stored {
        file: "/home/me/.ssh/known_hosts".into(),
        line: 7,
        algorithm: "ssh-ed25519".into(),
        fingerprint: "SHA256:storedStoredStoredStoredStoredStored0000".into(),
    };
    let (tx, mut rx) = oneshot::channel();
    tunnel(&mut h, id, generation, TunnelEvent::Ask(TunnelAsk::HostKey(question(vec![stored]), tx)));
    let screen = h.screen(100, 30);
    assert!(screen.contains("SSH host key changed"), "{screen}");
    assert!(screen.contains("SHA256:stored") && screen.contains("line 7"), "{screen}");
    h.key(KeyCode::Char('y'));
    assert_eq!(rx.try_recv(), Ok(true));
}

#[test]
fn a_tunnel_password_is_prompted_and_saved_once_the_tunnel_opened() {
    let (mut h, id) = harness();
    let generation = connect(&mut h, id);
    let (tx, mut rx) = oneshot::channel();
    tunnel(&mut h, id, generation, TunnelEvent::Ask(TunnelAsk::Secret(SecretAsk::Password { wrong: false }, tx)));
    let p = h.prompt().expect("prompted");
    assert!(matches!(p.purpose, PromptPurpose::Tunnel));
    assert!(h.screen(100, 30).contains("SSH password for ec2-user@bastion.example.com"));
    h.type_text("s3cret");
    h.key(KeyCode::Enter);
    assert_eq!(rx.try_recv().unwrap().map(|s| s.0), Some("s3cret".to_string()));
    let account = SshSettings::account(id);
    assert_eq!(h.app.secrets.stores().keychain.get(&account).unwrap(), None, "not before it worked");
    tunnel(&mut h, id, generation, TunnelEvent::Opened(Arc::new(FakeTunnel::default())));
    assert_eq!(h.app.secrets.stores().keychain.get(&account).unwrap().as_deref(), Some("s3cret"));
    assert_eq!(h.app.secrets.stores().keychain.get(&id.account()).unwrap(), None, "not the database's");
}

#[test]
fn esc_during_the_tunnel_cancels_it_and_what_it_asked() {
    let (mut h, id) = harness();
    let generation = connect(&mut h, id);
    let (tx, mut rx) = oneshot::channel();
    tunnel(&mut h, id, generation, TunnelEvent::Ask(TunnelAsk::Secret(SecretAsk::Password { wrong: false }, tx)));
    assert!(h.prompt().is_some());
    h.key(KeyCode::Esc);
    assert!(h.prompt().is_none());
    assert!(matches!(rx.try_recv(), Ok(None)), "cancelled");
    // The attempt goes on until cancelled; Esc on its node cancels it.
    h.explore("local-pg");
    h.key(KeyCode::Esc);
    assert_ne!(h.app.conns.state(id), NodeState::Connecting);
    // Its tunnel, opening late, is closed at once and nothing connects.
    let late = Arc::new(FakeTunnel::default());
    tunnel(&mut h, id, generation, TunnelEvent::Opened(late.clone()));
    assert!(h.driver.sessions.lock().unwrap().is_empty());
    assert!(h.app.conns.get(id).is_none_or(|c| c.tunnel.is_none()));
}

#[test]
fn a_tunnel_that_fails_or_is_lost_says_why_on_the_node() {
    let (mut h, id) = harness();
    let generation = connect(&mut h, id);
    let e = SshError {
        host: "bastion.example.com".into(),
        port: 22,
        kind: ErrorKind::HostKeyChanged { fingerprint: "SHA256:x".into(), stored: Vec::new() },
    };
    tunnel(&mut h, id, generation, TunnelEvent::Failed(TunnelFailure::Ssh(e)));
    assert_eq!(h.app.conns.state(id), NodeState::Failed);
    assert_eq!(
        h.node_error("local-pg").as_deref(),
        Some("SSH: the host key of bastion.example.com:22 changed; not connected")
    );
    // Connected, then lost: the node says so and the next use connects again (a new tunnel).
    let generation = connect(&mut h, id);
    tunnel(&mut h, id, generation, TunnelEvent::Opened(Arc::new(FakeTunnel::default())));
    h.db(DbEvent::Connected);
    tunnel(&mut h, id, generation, TunnelEvent::Lost(Loss::Keepalive));
    assert_eq!(
        h.node_error("local-pg").as_deref(),
        Some("SSH: the connection to bastion.example.com was lost (no reply to keepalives)")
    );
    assert_ne!(h.app.conns.state(id), NodeState::Connected);
    let before = h.app.tunnel_requests.len();
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tunnel_requests.len(), before + 1, "a new tunnel for the new attempt");
}

#[test]
fn without_tunnels_a_tunnelled_profile_never_connects_directly() {
    let mut cfg = sample_config(None);
    cfg.connections[0].ssh = Some(tunnel_settings());
    let mut h = Harness::launched(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal).with_fake_driver();
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    assert!(h.driver.sessions.lock().unwrap().is_empty());
    assert_eq!(h.node_error("local-pg").as_deref(), Some("SSH tunnels are not available in this build"));
    let _ = OverlayKind::Confirm;
}

fn test_tunnel(h: &mut Harness, seq: u64, ev: TunnelEvent) {
    h.app.on_app_event(AppEvent::TestTunnel { seq, ev });
}

fn test_line(h: &Harness) -> String {
    h.app.test_status().map(|(t, _)| t.to_string()).unwrap_or_default()
}

/// The form's test lines through the tunnel (one per stage).
fn test_lines(h: &Harness) -> Vec<String> {
    h.app.test_lines().unwrap_or_default().into_iter().map(|(t, _)| t.to_string()).collect()
}

/// "Test" of a tunnelled profile: the tunnel's stage while it opens, then its time
/// next to the database's result; a tunnel that fails says so as the SSH part; what the
/// tunnel asks during a test is never saved.
#[test]
fn a_test_connection_goes_through_a_throwaway_tunnel_stage_by_stage() {
    let (mut h, _) = harness();
    h.explore("local-pg");
    h.keys("e");
    h.ctrl('t');
    let seq = *h.app.test_tunnel_requests.last().expect("a tunnel for the test");
    test_tunnel(&mut h, seq, TunnelEvent::Stage(Stage::Authenticating));
    assert!(test_line(&h).contains("SSH bastion.example.com: login"), "{}", test_line(&h));
    let lines = test_lines(&h);
    assert!(lines[0].starts_with("SSH bastion.example.com: login"), "{lines:?}");
    assert_eq!(lines[1], "Database: waits for the tunnel");
    let (tx, mut rx) = oneshot::channel();
    test_tunnel(&mut h, seq, TunnelEvent::Ask(TunnelAsk::Secret(SecretAsk::Password { wrong: false }, tx)));
    let p = h.prompt().expect("asked");
    assert_eq!(p.save_to, None, "a test saves nothing");
    h.type_text("pw");
    h.key(KeyCode::Enter);
    assert_eq!(rx.try_recv().unwrap().map(|s| s.0), Some("pw".to_string()));
    let opened = Opened { host_key: "ssh-ed25519".into(), trusted_now: true, method: Method::Password, key: None };
    test_tunnel(&mut h, seq, TunnelEvent::Through(Duration::from_millis(84), Some(opened)));
    assert!(test_line(&h).contains("SSH bastion.example.com OK (84ms) · testing the database"), "{}", test_line(&h));
    h.app.on_app_event(AppEvent::Ping {
        seq,
        result: Ok(datarig_core::driver::PingInfo {
            server_version: "17.2".into(),
            latency: Duration::from_millis(12),
        }),
    });
    let line = test_line(&h);
    assert!(line.contains("SSH bastion.example.com OK (84ms) · database OK · server 17.2"), "{line}");
    assert_eq!(
        test_lines(&h),
        [
            "SSH bastion.example.com: OK 84ms · host key ED25519 trusted now · password",
            "Database: OK · server 17.2 · 12ms"
        ]
    );
    // A tunnel that fails: the SSH part says why.
    h.ctrl('t');
    let seq = *h.app.test_tunnel_requests.last().unwrap();
    let e = SshError {
        host: "bastion.example.com".into(),
        port: 22,
        kind: ErrorKind::Rejected { method: Method::Password, remaining: vec!["publickey".into()] },
    };
    test_tunnel(&mut h, seq, TunnelEvent::Failed(TunnelFailure::Ssh(e)));
    let line = test_line(&h);
    assert!(line.contains("SSH: bastion.example.com:22 refused the password (it accepts: publickey)"), "{line}");
    let lines = test_lines(&h);
    assert_eq!(lines[0], "SSH: bastion.example.com:22 refused the password (it accepts: publickey)");
    assert_eq!(lines[1], "Database: not tested (the tunnel did not open)");
    // After the tunnel, the database's failure is the database part.
    h.ctrl('t');
    let seq = *h.app.test_tunnel_requests.last().unwrap();
    let opened = Opened {
        host_key: "ecdsa-sha2-nistp256".into(),
        trusted_now: false,
        method: Method::Key,
        key: Some("rsa-sha2-512".into()),
    };
    h.app.conn_test.as_mut().unwrap().tunnel.as_mut().unwrap().settings.key_file = Some("~/.ssh/work.pem".into());
    test_tunnel(&mut h, seq, TunnelEvent::Through(Duration::from_millis(90), Some(opened)));
    h.app.on_app_event(AppEvent::Ping {
        seq,
        result: Err(datarig_core::driver::PingError::Failed("FATAL: password authentication failed".into())),
    });
    let line = test_line(&h);
    assert!(line.contains("SSH bastion.example.com OK (90ms) · database failed: FATAL: password"), "{line}");
    let lines = test_lines(&h);
    let ssh = "SSH bastion.example.com: OK 90ms · host key ECDSA known · key work.pem (rsa-sha2-512)";
    assert_eq!(lines[0], ssh);
    assert!(lines[1].starts_with("Database: failed: FATAL: password"), "{lines:?}");
    // In the form: two lines.
    let screen = h.screen(120, 40);
    assert!(screen.contains(ssh) && screen.contains("Database: failed: FATAL: password"), "{screen}");
    // Narrow: each line is cut once, with the stage and its outcome first.
    let screen = h.screen(80, 24);
    assert!(screen.contains("SSH bastion.example.com: OK 90ms · host key ECDSA known"), "{screen}");
    assert!(screen.contains("Database: failed: FATAL: password") && !screen.contains("……"), "{screen}");
    // A tunnel that opened without saying how (not the SSH library's): its time only.
    h.ctrl('t');
    let seq = *h.app.test_tunnel_requests.last().unwrap();
    test_tunnel(&mut h, seq, TunnelEvent::Through(Duration::from_millis(70), None));
    assert_eq!(test_lines(&h), ["SSH bastion.example.com: OK 70ms", "Database: testing…"]);
}

/// Saving the form: a passphrase typed in the SSH section goes to the tunnel's store (not the
/// database password's account); moving the tunnel to a source that stores nothing removes it.
#[test]
fn the_form_saves_the_tunnels_secret_in_its_own_account() {
    use datarig_tui::app::profiles::Field;
    let mut cfg = sample_config(None);
    cfg.connections[0].ssh =
        Some(SshSettings { auth: SshAuth::Key, key_file: Some("~/.ssh/k.pem".into()), ..tunnel_settings() });
    let mut h = Harness::launched(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal).with_fake_driver();
    let id = h.app.profiles[0].id;
    let account = SshSettings::account(id);
    h.explore("local-pg");
    h.keys("e");
    h.app.overlays.form_mut().unwrap().focus_field(Field::SshSecret);
    h.type_text("key-passphrase");
    h.ctrl('s');
    assert!(h.app.overlays.form().is_none(), "saved");
    assert_eq!(h.app.secrets.stores().keychain.get(&account).unwrap().as_deref(), Some("key-passphrase"));
    assert_eq!(h.app.secrets.stores().keychain.get(&id.account()).unwrap(), None);
    // To `prompt`: nothing kept any more.
    h.keys("e");
    h.app.overlays.form_mut().unwrap().focus_field(Field::SshSource);
    for _ in 0..4 {
        h.key(KeyCode::Right);
    }
    h.ctrl('s');
    assert!(h.app.overlays.form().is_none(), "saved");
    assert_eq!(h.app.secrets.stores().keychain.get(&account).unwrap(), None);
    assert_eq!(h.app.profiles[0].ssh.as_ref().unwrap().source().kind(), datarig_core::secret::SourceKind::Prompt);
}

/// The statements `Execute`d since the last look.
fn executed(h: &mut Harness) -> Vec<Vec<String>> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            datarig_core::driver::DbCommand::Execute { statements, .. } => Some(statements),
            _ => None,
        })
        .collect()
}

/// After the tunnel is lost, the next statement of a tab connects the profile again (a new
/// tunnel, the usual way) and then runs; what ran before does not run again, and the user's
/// transaction that was open is said to be gone.
#[test]
fn the_next_statement_after_a_lost_tunnel_connects_again() {
    let (mut h, id) = harness();
    let generation = connect(&mut h, id);
    tunnel(&mut h, id, generation, TunnelEvent::Opened(Arc::new(FakeTunnel::default())));
    h.db(DbEvent::Connected);
    h.app.focus = datarig_tui::app::Focus::Editor;
    h.keys("i");
    h.type_text("begin");
    h.ctrl('e');
    assert_eq!(executed(&mut h), [["begin"]]);
    let qid = h.app.tab().exec.query_id;
    h.tab_db(
        0,
        DbEvent::Done {
            id: qid,
            outcome: datarig_core::driver::Outcome::Command("BEGIN".into()),
            elapsed: Duration::ZERO,
        },
    );
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    // The tunnel ends; the sessions through it end with it.
    tunnel(&mut h, id, generation, TunnelEvent::Lost(Loss::Keepalive));
    h.tab_db(0, DbEvent::Lost { error: "server closed the connection".into() });
    let before = h.app.tunnel_requests.len();
    // The next statement: a new tunnel is asked for; nothing runs until it is open.
    for _ in 0.."begin".len() {
        h.key(KeyCode::Backspace);
    }
    h.type_text("select 1");
    h.ctrl('e');
    assert_eq!(h.app.tunnel_requests.len(), before + 1, "the profile connects again");
    assert!(executed(&mut h).is_empty(), "nothing is sent before the tunnel is open");
    let &(_, generation) = h.app.tunnel_requests.last().unwrap();
    tunnel(&mut h, id, generation, TunnelEvent::Opened(Arc::new(FakeTunnel::default())));
    h.db(DbEvent::Connected);
    assert_eq!(executed(&mut h), [["select 1"]], "only the new statement, once");
    let status = h.status(160, 24);
    assert!(status.contains("the transaction that was open is gone"), "{status}");
}

/// A tunnel reused by a later attempt (a newer generation) is still watched: its loss is
/// said on the node, not dropped as stale.
#[test]
fn a_tunnel_reused_by_a_later_attempt_still_reports_its_loss() {
    let (mut h, id) = harness();
    let generation = connect(&mut h, id);
    let fake = Arc::new(FakeTunnel::default());
    tunnel(&mut h, id, generation, TunnelEvent::Opened(fake.clone()));
    h.db(DbEvent::Connected);
    // The metadata session is lost and reopened (a new generation) through the same tunnel.
    h.db(DbEvent::Lost { error: "terminated".into() });
    h.app.focus = datarig_tui::app::Focus::Editor;
    h.keys("i");
    h.type_text("select 1");
    h.ctrl('e');
    assert!(!h.app.conns.is_current(id, generation), "a newer generation");
    // The tunnel ends; its watcher speaks with the generation that opened it.
    fake.closed.store(true, Ordering::SeqCst);
    tunnel(&mut h, id, generation, TunnelEvent::Lost(Loss::Keepalive));
    assert_eq!(
        h.node_error("local-pg").as_deref(),
        Some("SSH: the connection to bastion.example.com was lost (no reply to keepalives)")
    );
    // A late loss of an older tunnel does not end a newer, open one.
    let before = h.app.tunnel_requests.len();
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    let &(_, g2) = h.app.tunnel_requests.last().unwrap();
    assert_eq!(h.app.tunnel_requests.len(), before + 1);
    let newer = Arc::new(FakeTunnel::default());
    tunnel(&mut h, id, g2, TunnelEvent::Opened(newer));
    h.db(DbEvent::Connected);
    tunnel(&mut h, id, generation, TunnelEvent::Lost(Loss::Keepalive));
    assert_eq!(h.app.conns.state(id), NodeState::Connected);
}

/// A bastion that refuses the TCP connection is said in network words (never a file system
/// error), on the node and in a test connection; a refused channel's log line says what it was
/// even when the bastion gave no message.
#[test]
fn network_failures_are_said_in_network_words() {
    use datarig_core::fault::{Fault, FaultKind};
    let (mut h, id) = harness();
    let refused = || SshError {
        host: "bastion.example.com".into(),
        port: 22,
        kind: ErrorKind::Connect(Fault::new(
            FaultKind::Io(std::io::ErrorKind::ConnectionRefused),
            "Connection refused (os error 61)",
        )),
    };
    let generation = connect(&mut h, id);
    tunnel(&mut h, id, generation, TunnelEvent::Failed(TunnelFailure::Ssh(refused())));
    let e = h.node_error("local-pg").unwrap();
    assert!(e.contains("cannot connect to bastion.example.com:22: the connection was refused"), "{e}");
    assert!(!e.contains("file system"), "{e}");
    let other = SshError { kind: ErrorKind::Connect(Fault::other("no route")), ..refused() };
    let generation = connect(&mut h, id);
    tunnel(&mut h, id, generation, TunnelEvent::Failed(TunnelFailure::Ssh(other)));
    let e = h.node_error("local-pg").unwrap();
    assert!(!e.contains("file system"), "{e}");
    // The test connection says it the same way.
    h.explore("local-pg");
    h.keys("e");
    h.ctrl('t');
    let seq = *h.app.test_tunnel_requests.last().expect("a tunnel for the test");
    test_tunnel(&mut h, seq, TunnelEvent::Failed(TunnelFailure::Ssh(refused())));
    let line = test_line(&h);
    assert!(line.contains("the connection was refused") && !line.contains("file system"), "{line}");
}

#[test]
fn a_refused_channel_is_logged_with_its_target() {
    use datarig_core::driver::DbError;
    use datarig_core::transport::{DialError, Refusal};
    let (mut h, id) = harness();
    let dir = std::env::temp_dir().join(format!("datarig-refused-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    h.app.set_paths(datarig_core::paths::Paths { data: None, state: Some(dir.clone()) });
    let generation = connect(&mut h, id);
    tunnel(&mut h, id, generation, TunnelEvent::Opened(Arc::new(FakeTunnel::default())));
    let refusal =
        DialError::Refused { host: "db.internal".into(), port: 5432, reason: Refusal::Other, detail: String::new() };
    h.db(DbEvent::ConnectFailed { error: DbError::Transport(refusal), auth: false });
    let log = std::fs::read_to_string(dir.join("errors.log")).expect("logged");
    assert!(log.contains("db.transport") && log.contains("db.internal:5432 refused (Other)"), "{log}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Testing from the form checks the fields a connection needs as Save does: an empty database
/// host is marked and nothing is tried (no tunnel is opened for it).
#[test]
fn a_test_with_an_empty_database_host_is_not_run() {
    use datarig_tui::app::profiles::Field;
    let (mut h, _) = harness();
    h.explore("local-pg");
    h.keys("e");
    h.app.overlays.form_mut().unwrap().host.set("");
    h.app.overlays.form_mut().unwrap().focus_field(Field::SshHost);
    let before = h.app.test_tunnel_requests.len();
    h.ctrl('t');
    assert_eq!(h.app.test_tunnel_requests.len(), before, "nothing tried");
    let f = h.app.overlays.form().unwrap();
    assert_eq!(f.focus, Field::Host, "the empty host has the focus");
    assert!(h.app.conn_test.is_none(), "no test runs");
    let screen = h.screen(120, 40);
    assert!(screen.contains("Not tested: fill in the fields marked in red"), "{screen}");
    // A profile without a tunnel: no test either.
    h.key(KeyCode::Esc);
    h.explore("v6");
    h.keys("e");
    h.app.overlays.form_mut().unwrap().host.set("");
    h.ctrl('t');
    assert!(h.app.conn_test.is_none(), "no test runs");
    assert_eq!(h.app.overlays.form().unwrap().focus, Field::Host);
}

/// The explorer marks a profile that connects through its tunnel: the SSH glyph
/// with icons on, `SSH` with them off; a profile without one, or with it turned off, has none.
#[test]
fn the_explorer_marks_a_tunnelled_profile() {
    use datarig_core::config::IconsSetting;
    let (mut h, id) = harness();
    let row = |h: &mut Harness, name: &str| {
        let screen = h.screen(100, 30);
        screen.lines().find(|l| l.contains(name)).unwrap_or_else(|| panic!("{screen}")).to_string()
    };
    h.app.icons = IconsSetting::Off;
    assert!(row(&mut h, "local-pg").contains("local-pg SSH"), "{}", row(&mut h, "local-pg"));
    assert!(!row(&mut h, "v6").contains("SSH"), "{}", row(&mut h, "v6"));
    h.app.icons = IconsSetting::On;
    assert!(row(&mut h, "local-pg").contains("local-pg \u{f08c0}"), "{}", row(&mut h, "local-pg"));
    assert!(!row(&mut h, "local-pg").contains("SSH"));
    assert!(!row(&mut h, "v6").contains('\u{f08c0}'));
    // The tunnel turned off in the profile: no mark.
    h.app.profiles.iter_mut().find(|p| p.id == id).unwrap().ssh.as_mut().unwrap().enabled = false;
    assert!(!row(&mut h, "local-pg").contains('\u{f08c0}'));
}

/// The passphrase prompt's title keeps its words in every language (the Korean title ended
/// with the word and lost it to a long path): the path is shortened from the front, the
/// file's name kept.
#[test]
fn the_passphrase_prompt_keeps_its_words_with_a_long_key_path() {
    use datarig_core::i18n::Label;
    use datarig_tui::app::tunnel::short_path;
    use std::path::Path;
    let home = Path::new("/home/me");
    assert_eq!(short_path(Path::new("/home/me/.ssh/work.pem"), Some(home), 36), "~/.ssh/work.pem");
    assert_eq!(short_path(Path::new("/etc/keys/a.pem"), Some(home), 36), "/etc/keys/a.pem");
    let long = Path::new("/home/me/projects/company/infrastructure/secrets/bastion-prod.pem");
    assert_eq!(short_path(long, Some(home), 36), "…/secrets/bastion-prod.pem");
    let huge = Path::new("/k/a-very-long-directory-name-indeed/an-even-longer-key-file-name-here.pem");
    assert_eq!(short_path(huge, None, 36), "…/an-even-longer-key-file-name-here.pem");
    for lang in [Lang::En, Lang::Ko] {
        let (mut h, id) = harness();
        h.app.i18n = datarig_core::i18n::I18n::new(lang);
        let generation = connect(&mut h, id);
        let (tx, _rx) = oneshot::channel();
        let path = Path::new("/opt/company/infrastructure/secrets/bastion-prod.pem").to_path_buf();
        let ask = SecretAsk::Passphrase { path, wrong: false };
        tunnel(&mut h, id, generation, TunnelEvent::Ask(TunnelAsk::Secret(ask, tx)));
        let screen = h.screen(80, 24);
        let word = h.app.i18n.label(Label::SshFieldPassphrase).to_string();
        let title = screen.lines().find(|l| l.contains("bastion-prod.pem")).unwrap_or_else(|| panic!("{screen}"));
        assert!(title.contains(&word) && title.contains("…/secrets/bastion-prod.pem"), "{lang:?}: {title}");
    }
}

/// The host key question by mouse: a click on Cancel keeps the host untrusted, one on Trust
/// trusts it; the pointer on Trust does not move the default (Enter still cancels).
#[test]
fn a_host_key_question_takes_clicks() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let (mut h, id) = harness();
    let generation = connect(&mut h, id);
    let buttons = |h: &mut Harness| {
        h.draw(100, 30);
        let c = h.app.overlays.confirm().expect("asked");
        assert_eq!(c.action, ConfirmAction::TrustHostKey);
        (c.buttons.rects[0], c.buttons.rects[1])
    };
    let (tx, mut rx) = oneshot::channel();
    tunnel(&mut h, id, generation, TunnelEvent::Ask(TunnelAsk::HostKey(question(Vec::new()), tx)));
    let (_, trust) = buttons(&mut h);
    assert!(h.screen(100, 30).contains("[ Cancel ]     Trust"));
    h.mouse(MouseEventKind::Moved, trust.x + 1, trust.y);
    h.key(KeyCode::Enter);
    assert_eq!(rx.try_recv(), Ok(false), "Enter cancels after the pointer was on Trust");
    let (tx, mut rx) = oneshot::channel();
    tunnel(&mut h, id, generation, TunnelEvent::Ask(TunnelAsk::HostKey(question(Vec::new()), tx)));
    let (cancel, _) = buttons(&mut h);
    h.advance(Duration::from_millis(500));
    h.mouse(MouseEventKind::Down(MouseButton::Left), cancel.x + 1, cancel.y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), cancel.x + 1, cancel.y);
    assert_eq!(rx.try_recv(), Ok(false));
    assert!(h.app.overlays.confirm().is_none());
    let (tx, mut rx) = oneshot::channel();
    tunnel(&mut h, id, generation, TunnelEvent::Ask(TunnelAsk::HostKey(question(Vec::new()), tx)));
    let (_, trust) = buttons(&mut h);
    // A press at once on the question that just appeared is not taken.
    h.mouse(MouseEventKind::Down(MouseButton::Left), trust.x + 1, trust.y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), trust.x + 1, trust.y);
    assert!(rx.try_recv().is_err(), "not answered yet");
    h.advance(Duration::from_millis(500));
    h.mouse(MouseEventKind::Down(MouseButton::Left), trust.x + 1, trust.y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), trust.x + 1, trust.y);
    assert_eq!(rx.try_recv(), Ok(true));
}
