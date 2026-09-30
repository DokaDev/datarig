//! The OS keychain never blocks the UI: over SSH the login keychain is often
//! locked and macOS may hold a call while it asks on a screen the remote user never sees. The
//! keychain here is a fake that never answers, answers late, or fails; no test touches the real
//! one. Connecting, testing, the profile form and the launch-time move read and write it on a
//! worker, each call ends within the keychain's time limit, and a keychain that did not answer
//! is unknown, never "no password stored": the password is asked for instead.

mod common;

use common::*;
use datarig_core::config::{self, Config};
use datarig_core::driver::DbEvent;
use datarig_core::fault::{Fault, FaultKind, KeychainFault};
use datarig_core::i18n::Lang;
use datarig_core::secret::{DeletionLog, Logged, MemoryStore, SecretStore, SourceKind, Unavailable};
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::{AppEvent, NodeState, PromptPurpose, Startup, TabKind, TestState};
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::KeyCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// A keychain that holds each call (a dialog nobody answers) until it is dropped or `release`d,
/// or that takes `delay` first. It counts the writes and removals that reached it.
#[derive(Default)]
struct Held {
    inner: MemoryStore,
    open: Mutex<bool>,
    wake: Condvar,
    delay: Duration,
    /// Only writes take this long (the other calls answer at once).
    set_delay: Duration,
    sets: AtomicUsize,
    deletes: AtomicUsize,
}

impl Held {
    fn never() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn slow(delay: Duration) -> Arc<Self> {
        Arc::new(Self { delay, ..Self::default() })
    }

    fn slow_writes(delay: Duration) -> Arc<Self> {
        Arc::new(Self { set_delay: delay, ..Self::default() })
    }

    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.wake.notify_all();
    }

    fn hold(&self) {
        // Only writes are slow: the rest answers at once.
        if !self.set_delay.is_zero() {
            return;
        }
        if !self.delay.is_zero() {
            return std::thread::sleep(self.delay);
        }
        // A last resort so a failed test cannot leave the thread behind for long.
        let open = self.open.lock().unwrap();
        drop(self.wake.wait_timeout_while(open, Duration::from_secs(30), |o| !*o).unwrap());
    }
}

/// Opens the gate when the test ends, however it ends.
struct Release(Arc<Held>);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

impl SecretStore for Held {
    fn get(&self, account: &str) -> Result<Option<String>, Unavailable> {
        self.hold();
        self.inner.get(account)
    }
    fn set(&self, account: &str, secret: &str) -> Result<(), Unavailable> {
        self.hold();
        std::thread::sleep(self.set_delay);
        self.sets.fetch_add(1, Ordering::SeqCst);
        self.inner.set(account, secret)
    }
    fn delete(&self, account: &str) -> Result<bool, Unavailable> {
        self.hold();
        self.deletes.fetch_add(1, Ordering::SeqCst);
        self.inner.delete(account)
    }
}

const LIMIT: Duration = Duration::from_millis(300);

/// The binary's startup (`App::start` with an event channel) on the sample profiles, the
/// keychain `store` behind a `limit`, the environment `env` (SSH or not) and the fake driver.
fn started(
    cfg: &Config,
    store: Arc<dyn SecretStore>,
    limit: Duration,
    env: &'static [(&'static str, &'static str)],
) -> (Harness, UnboundedReceiver<AppEvent>) {
    let (mut app, clock) = new_app_with_clock(cfg, Lang::En);
    app.set_keychain_timeout(limit);
    app.set_secret_store(store);
    app.set_env_lookup(Arc::new(move |k| env.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())));
    let (tx, rx) = unbounded_channel();
    app.start(tx, Startup::Normal);
    let driver = FakeDriver::default();
    let h = Harness { app, cancelled: driver.any_cancel.clone(), driver, store: Arc::new(MemoryStore::new()), clock };
    (h.with_fake_driver(), rx)
}

const SSH: &[(&str, &str)] = &[("SSH_CONNECTION", "10.0.0.2 52100 10.0.0.1 22")];

/// Feed background events to the app until `done` holds (at most five seconds).
async fn pump(h: &mut Harness, rx: &mut UnboundedReceiver<AppEvent>, done: impl Fn(&Harness) -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !done(h) {
        let left = until.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(ev)) => h.app.on_app_event(ev),
            _ => panic!("not done within 5 s"),
        }
    }
}

/// Feed whatever arrives within `d`.
async fn settle(h: &mut Harness, rx: &mut UnboundedReceiver<AppEvent>, d: Duration) {
    let until = Instant::now() + d;
    while let Ok(Some(ev)) = tokio::time::timeout(until.saturating_duration_since(Instant::now()), rx.recv()).await {
        h.app.on_app_event(ev);
    }
}

fn sent_passwords(h: &Harness) -> Vec<String> {
    h.driver.passwords.lock().unwrap().clone()
}

/// The status bar's message of the moment.
fn flashed(h: &Harness) -> String {
    h.app.transient.as_ref().map(|(m, _)| m.render(&h.app.i18n).to_string()).unwrap_or_default()
}

fn prompt_open(h: &Harness) -> bool {
    h.app.overlays.prompt().is_some()
}

/// The keychain account of the sample config's first profile (`local-pg`).
fn first_account(cfg: &Config) -> String {
    cfg.connections[0].id.account()
}

/// A keychain that never answers: the connect key returns at once, the UI keeps working, and
/// nothing is sent to the server (unknown is not "no password"). At the limit the password is
/// asked for, with what to do over SSH; saving it to the keychain is offered, unchecked. The
/// typed password connects and is not written anywhere.
#[tokio::test(flavor = "multi_thread")]
async fn a_keychain_that_does_not_answer_falls_back_to_the_prompt() {
    let cfg = sample_config(None);
    let store = Held::never();
    let _release = Release(store.clone());
    store.inner.set(&first_account(&cfg), "stored-pw").unwrap();
    let (mut h, mut rx) = started(&cfg, store.clone(), LIMIT, SSH);
    let t = Instant::now();
    h.key(KeyCode::Enter);
    let status = h.status(160, 45);
    assert!(status.contains("Reading the password of local-pg from the keychain"), "{status}");
    // The UI answers meanwhile.
    h.keys("j");
    assert_eq!(h.selected(), 2, "the explorer moved");
    h.keys("k");
    h.ctrl('k');
    assert!(h.cmdline().is_some());
    h.key(KeyCode::Esc);
    assert!(t.elapsed() < LIMIT, "the UI waited {:?}", t.elapsed());
    pump(&mut h, &mut rx, prompt_open).await;
    assert!(sent_passwords(&h).is_empty(), "no empty password was sent: unknown is not absent");
    let screen = h.screen(100, 30);
    if cfg!(target_os = "macos") {
        assert!(screen.contains("remote session?") && screen.contains("security unlock-keychain"), "{screen}");
    } else {
        assert!(screen.contains("The keychain did not answer"), "{screen}");
    }
    let p = h.prompt().unwrap();
    assert_eq!((p.save, p.save_to), (false, Some(SourceKind::Keychain)), "offered, never checked");
    assert!(screen.contains("[ ] Save to OS keychain (Space; it failed just now)"), "{screen}");
    h.type_text("typed-pw");
    h.key(KeyCode::Enter);
    assert_eq!(sent_passwords(&h), ["typed-pw"]);
    h.meta_db("local-pg", DbEvent::Connected);
    settle(&mut h, &mut rx, LIMIT * 2).await;
    assert_eq!(store.sets.load(Ordering::SeqCst), 0, "not saved implicitly");
    assert_eq!(h.app.secrets.session(&first_account(&cfg)), Some("typed-pw"), "kept for this session");
}

/// `Esc` in the explorer and `Ctrl+C` in a tab of the profile end the wait; the answer that
/// comes later is dropped (nothing connects, no prompt opens).
#[tokio::test(flavor = "multi_thread")]
async fn the_keychain_wait_is_cancelled_with_esc_or_ctrl_c() {
    let cfg = sample_config(None);
    let store = Held::slow(Duration::from_millis(300));
    store.inner.set(&first_account(&cfg), "stored-pw").unwrap();
    let (mut h, mut rx) = started(&cfg, store, Duration::from_secs(5), &[]);
    let id = h.app.profiles[0].id;
    h.key(KeyCode::Enter);
    assert_eq!(h.app.conns.state(id), NodeState::Connecting);
    assert!(h.status(160, 45).contains("(Esc cancels)"), "{}", h.status(160, 45));
    h.key(KeyCode::Esc);
    assert_ne!(h.app.conns.state(id), NodeState::Connecting);
    assert!(h.status(160, 45).contains("cancelled"), "{}", h.status(160, 45));
    settle(&mut h, &mut rx, Duration::from_millis(800)).await;
    assert!(sent_passwords(&h).is_empty() && !prompt_open(&h), "the late answer is dropped");
    // A statement run in a tab of the profile waits for the keychain; Ctrl+C ends both.
    h.app.tabs.open(TabKind::Console, Some(id), Editor::new("SELECT 1"));
    h.app.focus = datarig_tui::app::Focus::Editor;
    h.ctrl('e');
    assert_eq!(h.app.conns.state(id), NodeState::Connecting);
    h.app.transient = None; // the earlier "cancelled" flash
    assert!(h.status(160, 45).contains("(Ctrl+C cancels)"), "{}", h.status(160, 45));
    h.ctrl('c');
    assert_ne!(h.app.conns.state(id), NodeState::Connecting);
    assert!(h.app.conns.get(id).is_none_or(|c| c.pending.is_empty()), "the statement went with it");
    settle(&mut h, &mut rx, Duration::from_millis(800)).await;
    assert!(sent_passwords(&h).is_empty() && !prompt_open(&h));
}

/// A slow keychain that answers within the limit connects with its password.
#[tokio::test(flavor = "multi_thread")]
async fn a_slow_keychain_that_answers_in_time_connects() {
    let cfg = sample_config(None);
    let store = Held::slow(Duration::from_millis(150));
    store.inner.set(&first_account(&cfg), "stored-pw").unwrap();
    let (mut h, mut rx) = started(&cfg, store, Duration::from_secs(5), &[]);
    h.key(KeyCode::Enter);
    pump(&mut h, &mut rx, |h| !sent_passwords(h).is_empty()).await;
    assert_eq!(sent_passwords(&h), ["stored-pw"]);
    assert!(!prompt_open(&h));
}

/// A locked keychain (it fails at once): the prompt says why, offers to save there unchecked,
/// and a save the user asks for runs on the worker and says it failed; the password is kept
/// for the session. (Headless: the worker's part runs inline.)
#[test]
fn a_locked_keychain_falls_back_to_the_prompt() {
    let locked = Fault::new(FaultKind::Keychain(KeychainFault::Locked), "User interaction is not allowed.");
    let store = Arc::new(MemoryStore::failing(locked));
    let mut h = Harness::launched(&sample_config(None), Lang::En, store, Startup::Normal).with_fake_driver();
    h.key(KeyCode::Enter);
    let screen = h.screen(100, 30);
    assert!(screen.contains("The keychain could not be read: the keychain is locked"), "{screen}");
    assert!(h.driver.passwords.lock().unwrap().is_empty(), "nothing sent");
    assert_eq!(h.prompt().map(|p| p.save), Some(false));
    h.type_text("typed-pw");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Char(' '));
    assert_eq!(h.prompt().map(|p| p.save), Some(true), "the user asks for it");
    h.key(KeyCode::Enter);
    h.meta_db("local-pg", DbEvent::Connected);
    let status = h.status(160, 45);
    assert!(status.contains("OS keychain: the keychain is locked"), "{status}");
    let account = h.account("local-pg");
    assert_eq!(h.app.secrets.session(&account), Some("typed-pw"));
}

/// The profile form opens at once on a keychain profile; its password field is filled when
/// the keychain answers (left alone meanwhile, it is marked unread).
#[tokio::test(flavor = "multi_thread")]
async fn the_form_opens_at_once_and_is_filled_when_the_keychain_answers() {
    let cfg = sample_config(None);
    let store = Held::slow(Duration::from_millis(200));
    store.inner.set(&first_account(&cfg), "stored-pw").unwrap();
    let (mut h, mut rx) = started(&cfg, store, Duration::from_secs(5), &[]);
    let t = Instant::now();
    h.command("conn.edit");
    assert!(t.elapsed() < Duration::from_millis(150), "the form waited {:?}", t.elapsed());
    assert!(h.form_open());
    assert_eq!((h.form().password.text(), h.form().password_unread), ("", true));
    pump(&mut h, &mut rx, |h| h.form().reading.is_none()).await;
    assert_eq!((h.form().password.text(), h.form().password_unread), ("stored-pw", false));
}

/// A keychain that never answers while the form is open: saved at once without touching the
/// stored password (unread is not empty); a typed password is saved to the profile at once,
/// its keychain write says later that it did not answer, and it is kept for the session.
#[tokio::test(flavor = "multi_thread")]
async fn saving_the_form_never_waits_for_the_keychain() {
    let cfg = sample_config(None);
    let store = Held::never();
    let _release = Release(store.clone());
    store.inner.set(&first_account(&cfg), "stored-pw").unwrap();
    let (mut h, mut rx) = started(&cfg, store.clone(), LIMIT, &[]);
    h.command("conn.edit");
    h.ctrl('s');
    assert!(!h.form_open(), "saved at once");
    settle(&mut h, &mut rx, LIMIT * 3).await;
    assert_eq!(store.deletes.load(Ordering::SeqCst), 0, "the unread password is not removed");
    assert_eq!(store.inner.get(&first_account(&cfg)).unwrap().as_deref(), Some("stored-pw"));
    // The first read still hangs: the write fails at once.
    h.command("conn.edit");
    while h.form().focus != datarig_tui::app::profiles::Field::Password {
        h.key(KeyCode::Tab);
    }
    h.type_text("new-pw");
    let t = Instant::now();
    h.ctrl('s');
    assert!(t.elapsed() < Duration::from_millis(150) && !h.form_open(), "saved at once");
    pump(&mut h, &mut rx, |h| flashed(h).contains("did not answer in time")).await;
    assert_eq!(h.app.secrets.session(&first_account(&cfg)), Some("new-pw"), "kept for this session");
    assert_eq!(store.inner.get(&first_account(&cfg)).unwrap().as_deref(), Some("stored-pw"));
}

/// The launch-time move of plaintext passwords ends within the limit when the keychain does
/// not answer: the busy notice goes, the password stays in the file.
#[tokio::test(flavor = "multi_thread")]
async fn the_launch_move_ends_when_the_keychain_does_not_answer() {
    let dir = std::env::temp_dir().join(format!("datarig-keychain-move-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, "[[connections]]\nname = \"local-pg\"\npassword = \"datarig\"\n").unwrap();
    let (cfg, _) = config::load(Some(path.clone()));
    let store = Held::never();
    let _release = Release(store.clone());
    let (mut h, mut rx) = started(&cfg, store, LIMIT, &[]);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Busy));
    pump(&mut h, &mut rx, |h| h.overlay_kind() != Some(OverlayKind::Busy)).await;
    assert!(std::fs::read_to_string(&path).unwrap().contains("password = \"datarig\""));
    assert_eq!(h.app.profiles[0].password, "datarig", "still usable");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Testing a keychain profile reads its password on a worker too; a keychain that does not
/// answer asks for the password to test with.
#[tokio::test(flavor = "multi_thread")]
async fn testing_a_keychain_profile_never_waits_for_the_keychain() {
    let cfg = sample_config(None);
    let store = Held::never();
    let _release = Release(store.clone());
    let (mut h, mut rx) = started(&cfg, store, LIMIT, &[]);
    let t = Instant::now();
    h.command("conn.test");
    assert!(t.elapsed() < LIMIT, "the UI waited {:?}", t.elapsed());
    assert_eq!(h.app.conn_test.as_ref().map(|t| t.state.clone()), Some(TestState::Running));
    pump(&mut h, &mut rx, prompt_open).await;
    assert!(matches!(h.prompt().map(|p| &p.purpose), Some(PromptPurpose::Test(_))));
}

fn to_source(h: &mut Harness, kind: SourceKind) {
    while h.form().focus != datarig_tui::app::profiles::Field::Source {
        h.key(KeyCode::Tab);
    }
    while h.form().source != kind {
        h.key(KeyCode::Right);
    }
}

/// Moving a password out of a keychain that does not answer: the form waits (it says so, its
/// keys wait), then says nothing changed; nothing was removed and the profile keeps its source.
/// A keychain that answers: the password moves and the old copy goes, in that order.
#[tokio::test(flavor = "multi_thread")]
async fn a_source_change_waits_for_the_keychain_off_the_ui_thread() {
    let cfg = sample_config(None);
    let store = Held::never();
    let _release = Release(store.clone());
    store.inner.set(&first_account(&cfg), "stored-pw").unwrap();
    let (mut h, mut rx) = started(&cfg, store.clone(), LIMIT, &[]);
    h.command("conn.edit");
    to_source(&mut h, SourceKind::File);
    h.ctrl('s');
    let t = Instant::now();
    h.key(KeyCode::Char('y'));
    assert!(t.elapsed() < LIMIT, "the UI waited {:?}", t.elapsed());
    assert!(h.form().saving);
    assert!(h.screen(120, 40).contains("Saving… waiting for the keychain"));
    h.key(KeyCode::Left);
    assert_eq!(h.form().source, SourceKind::File, "the form's keys wait");
    pump(&mut h, &mut rx, |h| !h.form().saving).await;
    assert!(h.form_open(), "nothing changed: the form stays");
    assert!(flashed(&h).contains("Nothing changed") && flashed(&h).contains("did not answer"), "{}", flashed(&h));
    assert_eq!(h.app.profiles[0].source().kind(), SourceKind::Keychain);
    assert_eq!(store.deletes.load(Ordering::SeqCst), 0);

    let store = Held::slow(Duration::from_millis(100));
    store.inner.set(&first_account(&cfg), "stored-pw").unwrap();
    let (mut h, mut rx) = started(&cfg, store.clone(), Duration::from_secs(5), &[]);
    h.command("conn.edit");
    pump(&mut h, &mut rx, |h| h.form().reading.is_none()).await;
    to_source(&mut h, SourceKind::File);
    h.ctrl('s');
    h.key(KeyCode::Char('y'));
    pump(&mut h, &mut rx, |h| flashed(h).contains("Moved the password")).await;
    assert!(!h.form_open());
    assert_eq!(h.app.profiles[0].source().kind(), SourceKind::File);
    assert_eq!(h.app.secrets.stores().file.get(&first_account(&cfg)).unwrap().as_deref(), Some("stored-pw"));
    assert_eq!(store.inner.get(&first_account(&cfg)).unwrap(), None, "the old copy went, after");
}

/// A profile deleted while its new password is being written to the keychain. The
/// removal waits for the write (the calls of an account run in the order asked), so no
/// password is left behind in the keychain.
#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_profile_while_its_password_is_written_leaves_nothing() {
    let cfg = sample_config(None);
    let account = first_account(&cfg);
    let store = Held::slow_writes(Duration::from_millis(400));
    store.inner.set(&account, "stored-pw").unwrap();
    let (mut h, mut rx) = started(&cfg, store.clone(), Duration::from_secs(5), &[]);
    h.command("conn.edit");
    pump(&mut h, &mut rx, |h| h.form().reading.is_none()).await;
    while h.form().focus != datarig_tui::app::profiles::Field::Password {
        h.key(KeyCode::Tab);
    }
    h.type_text("new-pw");
    h.ctrl('s');
    assert!(!h.form_open(), "saved at once; the write is on its way");
    // Deleted before the write ends.
    h.explore("local-pg");
    h.command("explorer.delete");
    h.key(KeyCode::Char('y'));
    assert!(h.app.profiles.iter().all(|p| p.id.account() != account), "deleted");
    settle(&mut h, &mut rx, Duration::from_millis(1200)).await;
    assert_eq!(store.sets.load(Ordering::SeqCst), 1, "the write ran");
    assert!(store.deletes.load(Ordering::SeqCst) >= 1, "the removal ran");
    assert_eq!(store.inner.get(&account).unwrap(), None, "no password left in the keychain");
    assert_eq!(h.app.secrets.session(&account), None);
}

/// A profile deleted while its new password is being written to a keychain that holds the
/// write past the limit: the removal is refused while the write hangs. When the write lands
/// after all, the password it wrote is removed (and the removal logged), not left behind.
#[tokio::test(flavor = "multi_thread")]
async fn a_password_written_after_the_limit_for_a_deleted_profile_is_removed() {
    let cfg = sample_config(None);
    let account = first_account(&cfg);
    let gated = Arc::new(GatedStore::default());
    let _open = gated.open_on_drop();
    gated.inner.set(&account, "stored-pw").unwrap();
    let dir = std::env::temp_dir().join(format!("datarig-late-write-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let log = Arc::new(DeletionLog::new(Some(dir.join("secrets.log"))));
    let store = Arc::new(Logged::new(gated.clone(), "keychain", log));
    let (mut h, mut rx) = started(&cfg, store, LIMIT, &[]);
    h.command("conn.edit");
    pump(&mut h, &mut rx, |h| h.form().reading.is_none()).await;
    while h.form().focus != datarig_tui::app::profiles::Field::Password {
        h.key(KeyCode::Tab);
    }
    h.type_text("new-pw");
    h.ctrl('s');
    gated.wait_until_held();
    pump(&mut h, &mut rx, |h| flashed(h).contains("did not answer in time")).await;
    // Deleted while the write still hangs: its removal is refused at once.
    h.explore("local-pg");
    h.command("explorer.delete");
    h.key(KeyCode::Char('y'));
    assert!(h.app.profiles.iter().all(|p| p.id.account() != account), "deleted");
    settle(&mut h, &mut rx, LIMIT).await;
    assert_eq!(gated.inner.get(&account).unwrap().as_deref(), Some("stored-pw"), "refused while the write hangs");
    // The write lands after all.
    gated.open();
    pump(&mut h, &mut rx, |_| gated.inner.get(&account).unwrap().is_none()).await;
    settle(&mut h, &mut rx, LIMIT).await;
    assert_eq!(gated.inner.get(&account).unwrap(), None, "no password left in the keychain");
    let logged = std::fs::read_to_string(dir.join("secrets.log")).unwrap_or_default();
    assert!(logged.contains(&format!("deleted source=keychain account={account}")), "{logged}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// After `Esc` ends the wait for the keychain, the status bar does not go back to
/// "Reading the password … (Esc cancels)" once the "cancelled" flash is gone.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_keychain_wait_leaves_no_reading_status() {
    let cfg = sample_config(None);
    let store = Held::slow(Duration::from_millis(300));
    store.inner.set(&first_account(&cfg), "stored-pw").unwrap();
    let (mut h, mut rx) = started(&cfg, store, Duration::from_secs(5), &[]);
    h.key(KeyCode::Enter);
    assert!(h.status(160, 45).contains("Reading the password of local-pg"), "{}", h.status(160, 45));
    h.key(KeyCode::Esc);
    settle(&mut h, &mut rx, Duration::from_millis(600)).await;
    // The flash expires.
    h.app.transient = None;
    let status = h.status(160, 45);
    assert!(!status.contains("Reading the password"), "{status}");
    assert!(status.contains("Connection to local-pg cancelled"), "{status}");
}

/// While the launch-time move waits for a keychain that does not answer (up to its
/// limit), `q` and `Ctrl+C` quit at once (as `Ctrl+Q` does everywhere) and the busy notice
/// says so. The move is abandoned;
/// the config file is whole (the app writes it only after the move's result) and keeps the
/// plaintext password, to move on the next launch.
#[tokio::test(flavor = "multi_thread")]
async fn quitting_while_the_launch_move_waits() {
    for quit in ["q", "ctrl+c", "ctrl+q"] {
        // Declared before the store's `_release`, so it is dropped after the store answers.
        let scratch = MovingConfig::new(
            &format!("keychain-quit-{quit}"),
            "[[connections]]\nname = \"local-pg\"\npassword = \"datarig\"\n",
        );
        let path = scratch.path.clone();
        let (cfg, _) = config::load(Some(path.clone()));
        let store = Held::never();
        let _release = Release(store.clone());
        let (mut h, mut rx) = started(&cfg, store, Duration::from_secs(10), &[]);
        assert_eq!(h.overlay_kind(), Some(OverlayKind::Busy));
        assert!(h.screen(100, 30).contains("q/Ctrl+C quit"), "{}", h.screen(100, 30));
        let t = Instant::now();
        match quit {
            "q" => h.keys("q"),
            "ctrl+c" => h.ctrl('c'),
            _ => h.ctrl('q'),
        }
        assert!(h.app.quit, "{quit} quits while the move waits");
        assert!(t.elapsed() < Duration::from_millis(150), "at once: {:?}", t.elapsed());
        settle(&mut h, &mut rx, Duration::from_millis(100)).await;
        let (again, error) = config::load(Some(path.clone()));
        assert!(error.is_none(), "{error:?}");
        assert_eq!(again.connections[0].password, "datarig", "the password is still in the file");
    }
}
