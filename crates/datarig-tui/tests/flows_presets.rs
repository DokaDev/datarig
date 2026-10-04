//! Tunnel presets, headless: profiles that name the same preset share one SSH
//! connection (the test answers its attempts as the tunnel task would), a preset that is not
//! there is an error of the profile (never a direct connection), and the "Tunnels" section of
//! the explorer makes, edits, deletes and tests presets, with their secrets under their own
//! account.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbEvent, SessionRole};
use datarig_core::i18n::Lang;
use datarig_core::profile::ProfileId;
use datarig_core::profile::ssh::{SshAuth, SshSettings};
use datarig_core::profile::tunnel::{TunnelId, TunnelPreset};
use datarig_core::secret::{MemoryStore, SecretStore, SourceKind};
use datarig_core::transport::{BoxedStream, DialError, Dialer};
use datarig_ssh::tunnel::{Loss, Stage};
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::tunnel::{
    OpenTunnel, PresetState, Probe, SecretAsk, TunnelAsk, TunnelEvent, TunnelFailure, TunnelRequest, Tunnels,
};
use datarig_tui::app::{AppEvent, NodeState, Startup};
use futures::future::BoxFuture;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;

/// Headless attempts never reach it: they are recorded in the app.
struct Recorded;

impl Tunnels for Recorded {
    fn open(&self, _: TunnelRequest) -> BoxFuture<'static, Result<Arc<dyn OpenTunnel>, TunnelFailure>> {
        unreachable!("headless attempts are recorded, not made")
    }
}

/// A connection the test holds: open until closed.
#[derive(Default)]
struct FakeTunnel {
    closed: AtomicBool,
}

impl FakeTunnel {
    fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

impl Dialer for FakeTunnel {
    fn dial(&self, _: &str, _: u16) -> BoxFuture<'static, Result<BoxedStream, DialError>> {
        Box::pin(async { Err(DialError::NotOpen) })
    }
}

impl OpenTunnel for FakeTunnel {
    fn is_open(&self) -> bool {
        !self.closed()
    }

    fn lost(&self) -> BoxFuture<'static, Loss> {
        Box::pin(futures::future::pending())
    }

    fn close(&self) -> BoxFuture<'static, ()> {
        self.closed.store(true, Ordering::SeqCst);
        Box::pin(async {})
    }
}

fn bastion() -> SshSettings {
    SshSettings {
        enabled: true,
        host: "bastion.example.com".into(),
        user: "ec2-user".into(),
        auth: SshAuth::Password,
        ..SshSettings::default()
    }
}

/// A scratch config file, removed with its directory when dropped.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(tag: &str) -> Scratch {
    let d = std::env::temp_dir().join(format!("datarig-presets-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Scratch(d)
}

/// The sample profiles, `local-pg` and `v6` naming the preset `office`; the config is written
/// to `dir` (when given).
fn config(dir: Option<&Scratch>) -> Config {
    let mut cfg = sample_config(None);
    cfg.tunnels = vec![TunnelPreset::new("office", bastion())];
    cfg.connections[0].tunnel = Some("office".into());
    cfg.connections[2].tunnel = Some("office".into());
    if let Some(d) = dir {
        cfg.path = Some(d.0.join("config.toml"));
    }
    cfg
}

fn harness_with(cfg: &Config) -> Harness {
    let mut h = Harness::launched(cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal).with_fake_driver();
    h.app.set_tunnels(Arc::new(Recorded));
    h
}

fn harness() -> Harness {
    harness_with(&config(None))
}

fn id_of(h: &Harness, name: &str) -> ProfileId {
    h.app.profiles.iter().find(|p| p.name == name).expect("profile").id
}

fn office(h: &Harness) -> TunnelId {
    h.app.presets.iter().find(|p| p.name == "office").expect("preset").id
}

/// Connect profile `name` from the explorer.
fn connect(h: &mut Harness, name: &str) {
    h.explore(name);
    h.key(KeyCode::Enter);
}

fn shared(h: &mut Harness, serial: u64, ev: TunnelEvent) {
    h.app.on_app_event(AppEvent::SharedTunnel { serial, ev });
}

/// The dialer of the session `i` the fake driver opened.
fn dialer_of(h: &Harness, i: usize) -> Arc<dyn Dialer> {
    h.driver.sessions.lock().unwrap()[i].opts.dialer.clone().expect("through a dialer").0
}

fn same(a: &Arc<dyn Dialer>, b: &Arc<FakeTunnel>) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b))
}

#[test]
fn two_profiles_on_one_preset_share_one_ssh_connection_until_the_last_one_leaves() {
    let mut h = harness();
    let (a, b) = (id_of(&h, "local-pg"), id_of(&h, "v6"));
    connect(&mut h, "local-pg");
    assert_eq!(h.app.shared_tunnel_requests, [1], "one connection asked for");
    assert!(h.driver.sessions.lock().unwrap().is_empty(), "no session before it is open");
    assert_eq!(h.app.preset_state(office(&h)), PresetState::Opening);
    // The second profile, while it opens: it waits for the same one.
    connect(&mut h, "v6");
    assert_eq!(h.app.shared_tunnel_requests, [1], "still one");
    shared(&mut h, 1, TunnelEvent::Stage(Stage::Authenticating));
    assert_eq!(
        h.app.conns.get(b).and_then(|c| c.connecting.as_ref()).and_then(|c| c.tunnel),
        Some(Stage::Authenticating)
    );
    let fake = Arc::new(FakeTunnel::default());
    shared(&mut h, 1, TunnelEvent::Opened(fake.clone()));
    // Both go on through it.
    let sessions = h.driver.sessions.lock().unwrap().iter().map(|s| s.role).collect::<Vec<_>>();
    assert_eq!(sessions, [SessionRole::Meta, SessionRole::Meta]);
    assert!(same(&dialer_of(&h, 0), &fake) && same(&dialer_of(&h, 1), &fake));
    assert_eq!(h.app.preset_state(office(&h)), PresetState::Open(2));
    // One leaves: it stays open for the other.
    h.explore("local-pg");
    h.keys("x");
    assert!(!fake.closed(), "still used by v6");
    assert_eq!(h.app.preset_state(office(&h)), PresetState::Open(1));
    // Connecting again takes the open one: nothing new is asked for.
    connect(&mut h, "local-pg");
    assert_eq!(h.app.shared_tunnel_requests, [1]);
    assert!(same(&dialer_of(&h, 2), &fake));
    // The last ones leave: closed.
    h.explore("local-pg");
    h.keys("x");
    h.explore("v6");
    h.keys("x");
    assert!(fake.closed());
    assert_eq!(h.app.preset_state(office(&h)), PresetState::Closed);
    let _ = a;
}

#[test]
fn a_lost_connection_ends_every_profile_on_it_and_the_next_use_opens_a_new_one() {
    let mut h = harness();
    let (a, b) = (id_of(&h, "local-pg"), id_of(&h, "v6"));
    connect(&mut h, "local-pg");
    connect(&mut h, "v6");
    shared(&mut h, 1, TunnelEvent::Opened(Arc::new(FakeTunnel::default())));
    h.app.on_app_event(AppEvent::Db {
        target: datarig_tui::app::EventTarget::Meta(a),
        generation: h.app.conns.get(a).unwrap().generation,
        ev: DbEvent::Connected,
    });
    shared(&mut h, 1, TunnelEvent::Lost(Loss::Keepalive));
    for (p, name) in [(a, "local-pg"), (b, "v6")] {
        assert_ne!(h.app.conns.state(p), NodeState::Connected);
        assert_eq!(
            h.node_error(name).as_deref(),
            Some("SSH: the connection to bastion.example.com was lost (no reply to keepalives)"),
            "{name}"
        );
        assert!(h.app.conns.get(p).unwrap().tunnel.is_none());
    }
    assert_eq!(h.app.preset_state(office(&h)), PresetState::Dead);
    assert!(h.rows().contains(&"    !".to_string()), "the preset says why: {:?}", h.rows());
    connect(&mut h, "local-pg");
    assert_eq!(h.app.shared_tunnel_requests, [1, 2], "a new connection for the next use");
    assert_eq!(h.app.preset_state(office(&h)), PresetState::Opening);
}

#[test]
fn one_prompt_for_every_profile_waiting_and_the_secret_goes_to_the_presets_account() {
    let mut h = harness();
    connect(&mut h, "local-pg");
    connect(&mut h, "v6");
    let (tx, mut rx) = oneshot::channel();
    shared(&mut h, 1, TunnelEvent::Ask(TunnelAsk::Secret(SecretAsk::Password { wrong: false }, tx)));
    let screen = h.screen(110, 30);
    assert!(screen.contains("The tunnel “office” asks for this (for local-pg, v6)."), "{screen}");
    let p = h.prompt().expect("prompted");
    assert_eq!(p.save_to, Some(SourceKind::Keychain));
    h.type_text("s3cret");
    h.key(KeyCode::Enter);
    assert_eq!(rx.try_recv().unwrap().map(|s| s.0), Some("s3cret".to_string()));
    let account = office(&h).account();
    assert_eq!(h.store.get(&account).unwrap(), None, "not before it worked");
    shared(&mut h, 1, TunnelEvent::Opened(Arc::new(FakeTunnel::default())));
    let keychain = h.app.secrets.stores().keychain.clone();
    assert_eq!(keychain.get(&account).unwrap().as_deref(), Some("s3cret"));
    for name in ["local-pg", "v6"] {
        let id = id_of(&h, name);
        assert_eq!(keychain.get(&SshSettings::account(id)).unwrap(), None, "not a profile's");
    }
}

#[test]
fn an_opening_nobody_waits_for_any_more_is_given_up_with_its_questions() {
    let mut h = harness();
    connect(&mut h, "local-pg");
    connect(&mut h, "v6");
    // One of them is cancelled: the connection is still wanted by the other, whose question
    // shows.
    h.explore("local-pg");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.preset_state(office(&h)), PresetState::Opening);
    let (tx, mut rx) = oneshot::channel();
    shared(&mut h, 1, TunnelEvent::Ask(TunnelAsk::HostKey(question(), tx)));
    assert!(h.app.overlays.confirm().is_some_and(|c| c.action == ConfirmAction::TrustHostKey), "v6 still waits");
    assert!(rx.try_recv().is_err(), "not answered");
    // The other one too (`Esc` on its node; the dialog put aside first): the question goes,
    // unanswered (no), and a late connection is closed at once.
    let v6 = id_of(&h, "v6");
    h.app.overlays.close(OverlayKind::Confirm);
    h.app.explorer.select_kind(datarig_tui::app::explorer::RowKind::Profile(v6));
    h.app.focus = datarig_tui::app::Focus::Tree;
    h.app.dispatch(datarig_tui::app::action::Action::Explorer(datarig_tui::app::action::ExplorerAction::Back));
    assert_eq!(rx.try_recv(), Err(oneshot::error::TryRecvError::Closed), "dropped: the tunnel takes it for no");
    assert_eq!(h.app.preset_state(office(&h)), PresetState::Closed);
    let late = Arc::new(FakeTunnel::default());
    shared(&mut h, 1, TunnelEvent::Opened(late.clone()));
    assert!(late.closed());
    assert!(h.driver.sessions.lock().unwrap().is_empty());
    // A question of an opening nobody waits for is answered no as it arrives.
    let (tx, mut rx) = oneshot::channel();
    shared(&mut h, 1, TunnelEvent::Ask(TunnelAsk::HostKey(question(), tx)));
    assert_eq!(rx.try_recv(), Err(oneshot::error::TryRecvError::Closed));
}

fn question() -> datarig_ssh::tunnel::HostKeyQuestion {
    datarig_ssh::tunnel::HostKeyQuestion {
        host: "bastion.example.com".into(),
        port: 22,
        algorithm: "ssh-ed25519".into(),
        fingerprint: "SHA256:presented".into(),
        stored: Vec::new(),
    }
}

#[test]
fn a_failed_opening_fails_every_attempt_waiting_for_it() {
    let mut h = harness();
    connect(&mut h, "local-pg");
    connect(&mut h, "v6");
    let e = datarig_ssh::tunnel::SshError {
        host: "bastion.example.com".into(),
        port: 22,
        kind: datarig_ssh::tunnel::ErrorKind::HostKeyNotTrusted { fingerprint: "SHA256:x".into() },
    };
    shared(&mut h, 1, TunnelEvent::Failed(TunnelFailure::Ssh(e)));
    for name in ["local-pg", "v6"] {
        assert_eq!(h.app.conns.state(id_of(&h, name)), NodeState::Failed);
        assert_eq!(
            h.node_error(name).as_deref(),
            Some("SSH: the host key of bastion.example.com:22 was not trusted"),
            "{name}"
        );
    }
    assert_eq!(h.app.preset_state(office(&h)), PresetState::Closed);
}

#[test]
fn a_preset_that_is_not_there_or_two_tunnels_never_connect_directly() {
    let mut cfg = config(None);
    cfg.connections[0].tunnel = Some("gone".into());
    cfg.connections[1].tunnel = Some("office".into());
    cfg.connections[1].ssh = Some(bastion());
    let mut h = harness_with(&cfg);
    connect(&mut h, "local-pg");
    assert!(h.driver.sessions.lock().unwrap().is_empty() && h.app.shared_tunnel_requests.is_empty());
    assert_eq!(
        h.node_error("local-pg").as_deref(),
        Some(
            "local-pg names the tunnel “gone”, which does not exist: pick one in its form (it never connects directly instead)"
        )
    );
    connect(&mut h, "分析-replica");
    assert!(h.driver.sessions.lock().unwrap().is_empty() && h.app.tunnel_requests.is_empty());
    assert!(h.node_error("分析-replica").is_some_and(|e| e.contains("has its own SSH tunnel on")));
    // A test of either fails the same way, without trying anything.
    h.explore("local-pg");
    h.keys("t");
    assert!(h.app.test_tunnel_requests.is_empty());
    assert!(h.app.transient.as_ref().is_some_and(|(n, _)| n.render(&h.app.i18n).contains("“gone”")));
}

#[test]
fn a_preset_changed_while_in_use_keeps_the_old_connection_for_the_profiles_on_it() {
    let mut h = harness();
    connect(&mut h, "local-pg");
    let old = Arc::new(FakeTunnel::default());
    shared(&mut h, 1, TunnelEvent::Opened(old.clone()));
    // Changed (as the tunnel form saves it).
    h.app.presets[0].settings.port = 2222;
    connect(&mut h, "v6");
    assert_eq!(h.app.shared_tunnel_requests, [1, 2], "the new settings: a new connection");
    let new = Arc::new(FakeTunnel::default());
    shared(&mut h, 2, TunnelEvent::Opened(new.clone()));
    assert!(same(&dialer_of(&h, 1), &new));
    assert!(!old.closed(), "local-pg keeps the old one");
    h.explore("local-pg");
    h.keys("x");
    assert!(old.closed(), "its last profile left");
    assert!(!new.closed());
}

#[test]
fn the_tunnels_section_lists_presets_with_their_state_and_profiles() {
    let mut h = harness();
    let rows = h.rows();
    let at = rows.iter().position(|r| r == "[tunnels]").expect("the section");
    assert_eq!(rows[at + 1], "  ~office");
    h.explore("local-pg");
    connect(&mut h, "local-pg");
    shared(&mut h, 1, TunnelEvent::Opened(Arc::new(FakeTunnel::default())));
    // Opened: the preset's profiles under it.
    h.app.explorer.select_kind(datarig_tui::app::explorer::RowKind::Tunnel(office(&h)));
    h.key(KeyCode::Char('l'));
    let rows = h.rows();
    let at = rows.iter().position(|r| r == "  ~office").unwrap();
    assert_eq!(&rows[at + 1..at + 3], ["    @local-pg", "    @v6"]);
    let line = h.explorer_line();
    assert!(line.contains("office  ec2-user@bastion.example.com · open · 1 connection on it"), "{line}");
    // Enter on a profile under it goes to that profile.
    h.key(KeyCode::Char('j'));
    h.key(KeyCode::Enter);
    assert_eq!(h.app.selected_profile(), Some(id_of(&h, "local-pg")));
    let screen = h.screen(80, 24);
    assert!(screen.contains("Tunnels 1"), "{screen}");
}

#[test]
fn deleting_a_preset_in_use_lists_its_profiles_enter_keeps_y_deletes_and_its_secret_goes() {
    let dir = scratch("delete");
    let mut h = harness_with(&config(Some(&dir)));
    let account = office(&h).account();
    h.app.secrets.stores().keychain.set(&account, "pw").unwrap();
    h.app.explorer.select_kind(datarig_tui::app::explorer::RowKind::Tunnel(office(&h)));
    h.app.focus = datarig_tui::app::Focus::Tree;
    h.keys("d");
    let c = h.app.overlays.confirm().expect("asked");
    assert_eq!(c.action, ConfirmAction::DeleteTunnel(office(&h)));
    let screen = h.screen(120, 30);
    assert!(screen.contains("Delete the SSH tunnel “office”?"), "{screen}");
    assert!(screen.contains("2 profiles use it (local-pg, v6)"), "{screen}");
    assert!(screen.contains("OS keychain is removed too"), "{screen}");
    // Enter keeps it.
    h.key(KeyCode::Enter);
    assert_eq!(h.app.presets.len(), 1);
    assert_eq!(h.app.secrets.stores().keychain.get(&account).unwrap().as_deref(), Some("pw"));
    h.keys("d");
    h.key(KeyCode::Char('y'));
    assert!(h.app.presets.is_empty());
    assert_eq!(h.app.secrets.stores().keychain.get(&account).unwrap(), None);
    let written = std::fs::read_to_string(dir.0.join("config.toml")).unwrap();
    assert!(!written.contains("[tunnels"), "{written}");
    // The profiles keep the name: they do not connect, never directly.
    assert!(written.matches("tunnel = \"office\"").count() == 2, "{written}");
    connect(&mut h, "local-pg");
    assert!(h.driver.sessions.lock().unwrap().is_empty());
    assert!(h.node_error("local-pg").is_some_and(|e| e.contains("“office”, which does not exist")));
}

#[test]
fn the_tunnel_form_makes_renames_and_tests_a_preset() {
    let dir = scratch("form");
    let mut cfg = config(Some(&dir));
    cfg.tunnels.clear();
    cfg.connections[0].tunnel = None;
    cfg.connections[2].tunnel = None;
    let mut h = harness_with(&cfg);
    // `n` in the section: a new preset.
    h.app.focus = datarig_tui::app::Focus::Tree;
    h.app.explorer.select_kind(datarig_tui::app::explorer::RowKind::TunnelsEmpty);
    h.keys("n");
    assert!(h.form().is_tunnel());
    h.type_text("hq");
    h.key(KeyCode::Tab);
    h.type_text("bastion.example.com");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.type_text("ec2-user");
    // Log in with the agent: nothing else needed.
    h.key(KeyCode::Tab);
    h.key(KeyCode::Right);
    h.key(KeyCode::Right);
    let screen = h.screen(100, 30);
    assert!(screen.contains("New SSH tunnel"), "{screen}");
    h.ctrl('s');
    assert!(!h.form_open(), "saved");
    let p = h.app.presets.first().expect("made").clone();
    assert_eq!(
        (p.name.as_str(), p.settings.auth, p.settings.host.as_str()),
        ("hq", SshAuth::Agent, "bastion.example.com")
    );
    let written = std::fs::read_to_string(dir.0.join("config.toml")).unwrap();
    assert!(written.contains("[tunnels.hq]") && written.contains(&format!("id = \"{}\"", p.id)), "{written}");
    // A profile names it; the preset is renamed: the profile follows, the id (and its secret)
    // stays.
    h.app.profiles[0].tunnel = Some("hq".into());
    h.app.secrets.stores().keychain.set(&p.id.account(), "kept").unwrap();
    h.app.explorer.select_kind(datarig_tui::app::explorer::RowKind::Tunnel(p.id));
    h.keys("e");
    for _ in 0..2 {
        h.key(KeyCode::Backspace);
    }
    h.type_text("office");
    h.ctrl('s');
    assert_eq!(h.app.presets[0].name, "office");
    assert_eq!(h.app.presets[0].id, p.id);
    assert_eq!(h.app.profiles[0].tunnel.as_deref(), Some("office"));
    let written = std::fs::read_to_string(dir.0.join("config.toml")).unwrap();
    assert!(written.contains("[tunnels.office]") && written.contains("tunnel = \"office\""), "{written}");
    assert!(!written.contains("hq"), "{written}");
    assert_eq!(h.app.secrets.stores().keychain.get(&p.id.account()).unwrap().as_deref(), Some("kept"));
    // A second preset cannot take its name (ignoring case).
    h.app.explorer.select_kind(datarig_tui::app::explorer::RowKind::Tunnel(p.id));
    h.keys("c");
    for _ in 0.."office-copy".len() {
        h.key(KeyCode::Backspace);
    }
    h.type_text("Office");
    h.ctrl('s');
    assert!(h.form_open(), "refused");
    assert!(h.screen(100, 30).contains("name already used"));
    h.key(KeyCode::Esc);
    // Its test: the login, then a channel to each database address of its profiles.
    h.app.explorer.select_kind(datarig_tui::app::explorer::RowKind::Tunnel(p.id));
    h.keys("t");
    let seq = *h.app.test_tunnel_requests.last().expect("a test tunnel");
    h.app.on_app_event(AppEvent::TestTunnel { seq, ev: TunnelEvent::Through(Duration::from_millis(40), None) });
    let probes = vec![Probe { host: "127.0.0.1".into(), port: 55432, result: Ok(Duration::from_millis(3)) }];
    h.app.on_app_event(AppEvent::TestProbe { seq, probes });
    let status = h.status(160, 24);
    assert!(status.contains("SSH bastion.example.com OK (40ms) · forwarding to 127.0.0.1:55432 OK"), "{status}");
    let lines: Vec<String> = h.app.test_lines().unwrap().into_iter().map(|(t, _)| t.to_string()).collect();
    assert_eq!(lines[1], "Forwarding: 127.0.0.1:55432 OK (a channel opened and closed; nothing sent)");
    // A channel refused: said, with the address.
    h.keys("t");
    let seq = *h.app.test_tunnel_requests.last().unwrap();
    h.app.on_app_event(AppEvent::TestTunnel { seq, ev: TunnelEvent::Through(Duration::from_millis(40), None) });
    let refused = DialError::Refused {
        host: "127.0.0.1".into(),
        port: 55432,
        reason: datarig_core::transport::Refusal::Prohibited,
        detail: String::new(),
    };
    let probes = vec![Probe { host: "127.0.0.1".into(), port: 55432, result: Err(refused) }];
    h.app.on_app_event(AppEvent::TestProbe { seq, probes });
    let status = h.status(200, 24);
    assert!(status.contains("forwarding failed: 127.0.0.1:55432:"), "{status}");
}

/// "Save as tunnel preset": the profile's own tunnel becomes a preset when the form is saved,
/// and its secret moves to the preset's account (copied, read back, then removed).
#[test]
fn save_as_tunnel_preset_moves_the_settings_and_the_secret() {
    let dir = scratch("save-as");
    let mut cfg = config(Some(&dir));
    cfg.tunnels.clear();
    cfg.connections[0].tunnel = None;
    cfg.connections[2].tunnel = None;
    cfg.connections[0].ssh = Some(bastion());
    let mut h = harness_with(&cfg);
    let id = id_of(&h, "local-pg");
    let keychain = h.app.secrets.stores().keychain.clone();
    keychain.set(&SshSettings::account(id), "bastion-pw").unwrap();
    h.explore("local-pg");
    h.keys("e");
    h.ctrl('n');
    assert!(h.screen(110, 30).contains("save as tunnel preset Ctrl+B"));
    h.key_mod(KeyCode::Char('b'), KeyModifiers::CONTROL);
    let n = h.app.overlays.name_input().expect("asks the name");
    assert_eq!(n.input.text(), "bastion");
    h.key(KeyCode::Enter);
    assert!(h.form().new_preset_picked());
    assert!(h.screen(110, 30).contains("bastion (new)"));
    // Nothing is written before the form is saved.
    assert!(h.app.presets.is_empty());
    h.ctrl('s');
    assert!(!h.form_open());
    let p = h.app.presets.first().expect("made").clone();
    assert_eq!((p.name.as_str(), &p.settings.host), ("bastion", &bastion().host));
    let c = h.app.profile(id).unwrap();
    assert_eq!((c.tunnel.as_deref(), c.ssh.as_ref()), (Some("bastion"), None), "moved, not copied");
    assert_eq!(keychain.get(&p.id.account()).unwrap().as_deref(), Some("bastion-pw"));
    assert_eq!(keychain.get(&SshSettings::account(id)).unwrap(), None, "the old copy is gone");
    let written = std::fs::read_to_string(dir.0.join("config.toml")).unwrap();
    assert!(written.contains("[tunnels.bastion]") && written.contains("tunnel = \"bastion\""), "{written}");
    assert!(!written.contains("[connections.ssh]"), "{written}");
}

/// A keychain that cannot be read keeps the profile's secret where it was: unknown is not absent.
#[test]
fn save_as_tunnel_preset_keeps_a_secret_it_could_not_read() {
    let dir = scratch("save-as-unread");
    let mut cfg = config(Some(&dir));
    cfg.tunnels.clear();
    cfg.connections[0].tunnel = None;
    cfg.connections[2].tunnel = None;
    cfg.connections[0].ssh = Some(bastion());
    let (mut app, clock) = new_app_with_clock(&cfg, Lang::En);
    let store = Arc::new(MemoryStore::new());
    let id = cfg.connections[0].id;
    store.set(&SshSettings::account(id), "bastion-pw").unwrap();
    // Reads fail; the entry is there all along.
    struct NoReads(Arc<MemoryStore>);
    impl SecretStore for NoReads {
        fn get(&self, _: &str) -> Result<Option<String>, datarig_core::secret::Unavailable> {
            Err(datarig_core::secret::Unavailable(datarig_core::fault::Fault::new(
                datarig_core::fault::FaultKind::Keychain(datarig_core::fault::KeychainFault::Locked),
                "locked",
            )))
        }
        fn set(&self, a: &str, s: &str) -> Result<(), datarig_core::secret::Unavailable> {
            self.0.set(a, s)
        }
        fn delete(&self, a: &str) -> Result<bool, datarig_core::secret::Unavailable> {
            self.0.delete(a)
        }
    }
    app.set_secret_store(Arc::new(NoReads(store.clone())));
    app.set_tunnels(Arc::new(Recorded));
    app.launch(Startup::Normal);
    let mut h = Harness { app, driver: Default::default(), cancelled: Default::default(), store: store.clone(), clock };
    h.explore("local-pg");
    h.keys("e");
    h.ctrl('n');
    h.key_mod(KeyCode::Char('b'), KeyModifiers::CONTROL);
    h.key(KeyCode::Enter);
    h.ctrl('s');
    let p = h.app.presets.first().expect("made").clone();
    assert_eq!(store.get(&SshSettings::account(id)).unwrap().as_deref(), Some("bastion-pw"), "kept");
    assert_eq!(store.get(&p.id.account()).unwrap(), None);
    let said = h.app.transient.as_ref().map(|(n, _)| n.render(&h.app.i18n).to_string()).unwrap_or_default();
    assert!(said.contains("was not moved"), "{said}");
}

fn lang_tag(l: Lang) -> &'static str {
    if l == Lang::En { "en" } else { "ko" }
}

/// The "Tunnels" section (an open preset with its profiles under it, one lost), the tunnel form,
/// the delete question and a profile form that picked a preset, in English (pinned) and Korean
/// (checked against the catalog).
#[test]
fn tunnel_preset_screens() {
    for lang in [Lang::En, Lang::Ko] {
        let tag = lang_tag(lang);
        let mut cfg = config(None);
        let lab = SshSettings { host: "lab-bastion.internal".into(), port: 2222, auth: SshAuth::Agent, ..bastion() };
        cfg.tunnels.push(TunnelPreset::new("lab", lab));
        cfg.connections[1].tunnel = Some("lab".into());
        let mut h = Harness::launched(&cfg, lang, Arc::new(MemoryStore::new()), Startup::Normal).with_fake_driver();
        h.app.set_tunnels(Arc::new(Recorded));
        connect(&mut h, "local-pg");
        shared(&mut h, 1, TunnelEvent::Opened(Arc::new(FakeTunnel::default())));
        connect(&mut h, "分析-replica");
        shared(&mut h, 2, TunnelEvent::Opened(Arc::new(FakeTunnel::default())));
        shared(&mut h, 2, TunnelEvent::Lost(Loss::Keepalive));
        let office = office(&h);
        h.app.focus = datarig_tui::app::Focus::Tree;
        h.app.explorer.select_kind(datarig_tui::app::explorer::RowKind::Tunnel(office));
        h.key(KeyCode::Char('l'));
        assert_screen!(format!("tunnels_section_{tag}_80x24"), lang, h.draw(80, 24));
        h.keys("e");
        assert_screen!(format!("tunnel_form_{tag}_100x30"), lang, h.draw(100, 30));
        h.key(KeyCode::Esc);
        h.keys("d");
        assert_screen!(format!("tunnel_delete_{tag}_100x30"), lang, h.draw(100, 30));
        h.key(KeyCode::Enter);
        h.explore("v6");
        h.keys("e");
        h.ctrl('n');
        assert_screen!(format!("profile_form_preset_{tag}_100x30"), lang, h.draw(100, 30));
    }
}

/// A right click on a preset: its own actions, worded for a tunnel; on the section's head, a
/// new one.
#[test]
fn the_context_menu_of_a_preset() {
    let mut h = harness();
    h.right_click_row("~office");
    assert_eq!(
        h.menu_labels(),
        [
            "Explorer: open or toggle",
            "New SSH tunnel",
            "Edit SSH tunnel",
            "Duplicate SSH tunnel",
            "Delete SSH tunnel",
            "Test SSH tunnel"
        ]
    );
    h.key(KeyCode::Esc);
    h.right_click_row("[tunnels]");
    assert_eq!(h.menu_labels(), ["Explorer: open or toggle", "New SSH tunnel"]);
}

/// A profile with its own tunnel, its secret in the secrets file; a scratch config file.
fn own_tunnel_config(dir: &Scratch) -> Config {
    let mut cfg = config(Some(dir));
    cfg.tunnels.clear();
    cfg.connections[0].tunnel = None;
    cfg.connections[2].tunnel = None;
    let mut own = bastion();
    own.set_source(datarig_core::secret::PasswordSource::File);
    cfg.connections[0].ssh = Some(own);
    cfg
}

/// Save as tunnel preset while the form also moves the secret from the secrets file to the
/// keychain: the secret is read where it is (the file), written where the preset keeps it.
#[test]
fn save_as_tunnel_preset_reads_the_secret_where_it_was_saved() {
    let dir = scratch("save-as-store");
    let mut h = harness_with(&own_tunnel_config(&dir));
    let id = id_of(&h, "local-pg");
    let stores = h.app.secrets.stores().clone();
    stores.file.set(&SshSettings::account(id), "in-file").unwrap();
    h.explore("local-pg");
    h.keys("e");
    h.ctrl('n');
    // The secret's storage: the keychain (from the file).
    while h.form().focus != datarig_tui::app::profiles::Field::SshSource {
        h.key(KeyCode::Tab);
    }
    h.key(KeyCode::Left);
    assert_eq!(h.form().ssh_source, SourceKind::Keychain);
    h.key_mod(KeyCode::Char('b'), KeyModifiers::CONTROL);
    h.key(KeyCode::Enter);
    h.ctrl('s');
    let p = h.app.presets.first().expect("made").clone();
    assert_eq!(p.settings.source().kind(), SourceKind::Keychain);
    assert_eq!(stores.keychain.get(&p.id.account()).unwrap().as_deref(), Some("in-file"));
    assert_eq!(stores.file.get(&SshSettings::account(id)).unwrap(), None, "moved out of the file");
}

/// A secret typed in the form is the one the preset uses from now on (not an older one this
/// session remembered).
#[test]
fn save_as_tunnel_preset_takes_the_typed_secret_over_the_sessions() {
    let dir = scratch("save-as-typed");
    let mut h = harness_with(&own_tunnel_config(&dir));
    let id = id_of(&h, "local-pg");
    h.app.secrets.remember(&SshSettings::account(id), "old-session");
    h.explore("local-pg");
    h.keys("e");
    h.ctrl('n');
    while h.form().focus != datarig_tui::app::profiles::Field::SshSecret {
        h.key(KeyCode::Tab);
    }
    h.type_text("typed-new");
    h.key_mod(KeyCode::Char('b'), KeyModifiers::CONTROL);
    h.key(KeyCode::Enter);
    h.ctrl('s');
    let p = h.app.presets.first().expect("made").clone();
    assert_eq!(h.app.secrets.session(&p.id.account()), Some("typed-new"));
    assert_eq!(h.app.secrets.session(&SshSettings::account(id)), None);
    assert_eq!(h.app.secrets.stores().file.get(&p.id.account()).unwrap().as_deref(), Some("typed-new"));
}

/// The config cannot be written: nothing of "save as tunnel preset" happens (no preset in
/// memory either, so a later save cannot write one), the form stays open and the profile's own
/// secret stays where it is.
#[test]
fn save_as_tunnel_preset_that_cannot_be_written_changes_nothing() {
    let dir = scratch("save-as-fail");
    let mut cfg = own_tunnel_config(&dir);
    // A folder where the file should be: every write fails.
    let path = dir.0.join("config.toml");
    std::fs::create_dir_all(&path).unwrap();
    cfg.path = Some(path);
    let mut h = harness_with(&cfg);
    let id = id_of(&h, "local-pg");
    let stores = h.app.secrets.stores().clone();
    stores.file.set(&SshSettings::account(id), "in-file").unwrap();
    h.explore("local-pg");
    h.keys("e");
    h.ctrl('n');
    h.key_mod(KeyCode::Char('b'), KeyModifiers::CONTROL);
    h.key(KeyCode::Enter);
    h.ctrl('s');
    assert!(h.form_open(), "the form stays");
    assert!(h.app.presets.is_empty());
    let c = h.app.profile(id).unwrap();
    assert_eq!((c.tunnel.as_deref(), c.ssh.as_ref().map(|s| s.enabled)), (None, Some(true)));
    assert_eq!(stores.file.get(&SshSettings::account(id)).unwrap().as_deref(), Some("in-file"));
}

/// The database password's storage and "save as tunnel preset" in one save: one at a time.
#[test]
fn save_as_tunnel_preset_waits_for_a_password_storage_change() {
    let dir = scratch("save-as-source");
    let mut h = harness_with(&own_tunnel_config(&dir));
    h.explore("local-pg");
    h.keys("e");
    while h.form().focus != datarig_tui::app::profiles::Field::Source {
        h.key(KeyCode::Tab);
    }
    h.key(KeyCode::Right);
    h.ctrl('n');
    h.key_mod(KeyCode::Char('b'), KeyModifiers::CONTROL);
    h.key(KeyCode::Enter);
    h.ctrl('s');
    assert!(h.form_open());
    assert!(h.app.presets.is_empty());
    let said = h.app.transient.as_ref().map(|(n, _)| n.render(&h.app.i18n).to_string()).unwrap_or_default();
    assert!(said.contains("password storage first"), "{said}");
}

/// A profile edited to another tunnel while its preset's connection opened: it does not take
/// that connection (nobody else needs it, so it closes).
#[test]
fn a_profile_edited_while_its_preset_opened_does_not_take_the_connection() {
    let mut h = harness();
    let a = id_of(&h, "local-pg");
    connect(&mut h, "local-pg");
    // Saved meanwhile without the preset (as its form does).
    h.app.profiles.iter_mut().find(|p| p.id == a).unwrap().tunnel = None;
    let fake = Arc::new(FakeTunnel::default());
    shared(&mut h, 1, TunnelEvent::Opened(fake.clone()));
    assert!(h.driver.sessions.lock().unwrap().is_empty(), "nothing through it");
    assert!(fake.closed());
    assert_ne!(h.app.conns.state(a), NodeState::Connecting);
    assert!(h.app.conns.get(a).unwrap().tunnel.is_none());
}

/// A preset deleted while its connection opens: the attempts waiting for it end (the preset is
/// gone), its questions go, and a late connection is closed.
#[test]
fn deleting_a_preset_while_it_opens_ends_its_attempts() {
    let dir = scratch("delete-opening");
    let mut h = harness_with(&config(Some(&dir)));
    connect(&mut h, "local-pg");
    let (tx, mut rx) = oneshot::channel();
    shared(&mut h, 1, TunnelEvent::Ask(TunnelAsk::Secret(SecretAsk::Password { wrong: false }, tx)));
    assert!(h.prompt().is_some());
    h.app.overlays.close(OverlayKind::Password);
    h.app.focus = datarig_tui::app::Focus::Tree;
    h.app.explorer.select_kind(datarig_tui::app::explorer::RowKind::Tunnel(office(&h)));
    h.keys("d");
    h.key(KeyCode::Char('y'));
    assert!(h.app.presets.is_empty());
    assert!(rx.try_recv().is_err(), "the question was dropped");
    assert_eq!(h.app.conns.state(id_of(&h, "local-pg")), NodeState::Failed);
    assert!(h.node_error("local-pg").is_some_and(|e| e.contains("“office”, which does not exist")));
    let late = Arc::new(FakeTunnel::default());
    shared(&mut h, 1, TunnelEvent::Opened(late.clone()));
    assert!(late.closed());
}

/// A profile listed under a preset only leads to that profile: `d` there deletes nothing, and
/// its menu has nothing else.
#[test]
fn a_profile_under_a_preset_only_leads_to_it() {
    let mut h = harness();
    let office = office(&h);
    h.app.tunnels_open.insert(office);
    let local = id_of(&h, "local-pg");
    h.app.focus = datarig_tui::app::Focus::Tree;
    h.app.explorer.select_kind(datarig_tui::app::explorer::RowKind::TunnelUser(office, local));
    h.keys("d");
    assert!(h.app.overlays.confirm().is_none());
    h.keys("e");
    assert!(!h.form_open());
    h.right_click_row("@local-pg");
    assert_eq!(h.menu_labels(), ["Explorer: open or toggle"]);
}

/// A profile that named a preset and had its own tunnel on: its form shows the preset picked
/// and its own tunnel off, and says so; saving keeps that (no longer both).
#[test]
fn the_form_of_a_profile_with_both_tunnels_keeps_the_preset_when_saved() {
    let dir = scratch("both");
    let mut cfg = config(Some(&dir));
    cfg.connections[0].ssh = Some(bastion());
    let mut h = harness_with(&cfg);
    let id = id_of(&h, "local-pg");
    h.explore("local-pg");
    h.keys("e");
    h.ctrl('n');
    assert_eq!(h.form().ssh_choice(), datarig_tui::app::profiles::SshChoice::Preset("office".into()));
    assert!(!h.form().ssh_enabled);
    let screen = h.screen(110, 30);
    assert!(screen.contains("had its own SSH tunnel on"), "{screen}");
    h.ctrl('s');
    let c = h.app.profile(id).unwrap();
    assert_eq!(c.tunnel.as_deref(), Some("office"));
    assert_eq!(c.ssh.as_ref().map(|s| s.enabled), Some(false), "kept, off");
    assert!(h.app.route_of(c).is_ok());
}

/// Presets whose names differ only in case (a hand-written file): each can still be saved
/// under its own name; no new one may take such a name.
#[test]
fn names_that_differ_in_case_only_can_stay() {
    let dir = scratch("case");
    let mut cfg = config(Some(&dir));
    cfg.tunnels.push(TunnelPreset::new("Office", bastion()));
    let mut h = harness_with(&cfg);
    let upper = h.app.presets.iter().find(|p| p.name == "Office").unwrap().id;
    h.app.focus = datarig_tui::app::Focus::Tree;
    h.app.explorer.select_kind(datarig_tui::app::explorer::RowKind::Tunnel(upper));
    h.keys("e");
    h.ctrl('s');
    assert!(!h.form_open(), "saved under its own name");
    h.app.explorer.select_kind(datarig_tui::app::explorer::RowKind::Tunnel(upper));
    h.keys("c");
    for _ in 0.."Office-copy".len() {
        h.key(KeyCode::Backspace);
    }
    h.type_text("OFFICE");
    h.ctrl('s');
    assert!(h.form_open(), "a new one cannot");
}
