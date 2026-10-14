//! The one-time offer to stop the macOS keychain prompts after updates. The keychain is a fake
//! that takes as long as the test says by the app's fake clock; the platform, the running
//! binary and the environment are the test's too. No test touches the real keychain or the
//! `security` tool.

mod common;

use common::*;
use datarig_core::config::{self, Config, KeychainAccess};
use datarig_core::driver::DbEvent;
use datarig_core::i18n::Lang;
use datarig_core::paths::Paths;
use datarig_core::secret::offer::{self, Answer, BinaryId, Host, OfferState};
use datarig_core::secret::{MemoryStore, PasswordSource, SecretStore, Stores, Unavailable};
use datarig_tui::app::Startup;
use datarig_tui::app::overlay::ConfirmAction;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A keychain whose calls take `delay` by the app's clock.
struct Slow {
    inner: MemoryStore,
    clock: FakeClock,
    delay: Mutex<Duration>,
}

impl Slow {
    fn wait(&self) {
        self.clock.advance(*self.delay.lock().unwrap());
    }
}

impl SecretStore for Slow {
    fn get(&self, account: &str) -> Result<Option<String>, Unavailable> {
        self.wait();
        self.inner.get(account)
    }
    fn set(&self, account: &str, secret: &str) -> Result<(), Unavailable> {
        self.wait();
        self.inner.set(account, secret)
    }
    fn delete(&self, account: &str) -> Result<bool, Unavailable> {
        self.wait();
        self.inner.delete(account)
    }
}

fn binary(version: &str) -> BinaryId {
    BinaryId { version: version.into(), exe: "/opt/datarig/bin/datarig".into(), size: 4_000_000, mtime_ns: 7 }
}

const PROMPTED: Duration = Duration::from_secs(3);
const FAST: Duration = Duration::from_millis(20);
const SIZES: [(u16, u16); 2] = [(80, 24), (120, 40)];
const SSH: &[(&str, &str)] = &[("SSH_CLIENT", "10.0.0.2 52100 22")];

/// What a test changes from a Mac running a new binary with the OS keychain in `direct` mode,
/// the last direct access made by the version before.
struct Setup {
    delay: Duration,
    macos: bool,
    os_keychain: bool,
    access: KeychainAccess,
    env: &'static [(&'static str, &'static str)],
    /// The state file before the app starts.
    state: Before,
    /// The config file cannot be written (its folder is a file).
    config_blocked: bool,
}

enum Before {
    Nothing,
    State(OfferState),
    Text(&'static str),
}

impl Default for Setup {
    fn default() -> Self {
        Setup {
            delay: PROMPTED,
            macos: true,
            os_keychain: true,
            access: KeychainAccess::Direct,
            env: &[],
            state: Before::State(OfferState { answer: None, binary: Some(binary("0.18.0")) }),
            config_blocked: false,
        }
    }
}

struct Offer {
    h: Harness,
    dir: PathBuf,
    store: Arc<Slow>,
}

impl Drop for Offer {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Offer {
    fn state_file(&self) -> PathBuf {
        self.dir.join("state").join(offer::STATE_FILE)
    }

    fn state(&self) -> OfferState {
        offer::load(&self.state_file()).unwrap()
    }

    fn config_text(&self) -> String {
        std::fs::read_to_string(self.dir.join("config.toml")).unwrap_or_default()
    }

    fn offered(&self) -> bool {
        self.h.app.overlays.confirm().is_some_and(|c| c.action == ConfirmAction::KeychainOffer)
    }

    /// Connect to `local-pg` (its password read from the keychain), and the server says yes.
    fn connect(&mut self) {
        self.h.key(KeyCode::Enter);
        assert!(self.h.app.conns.attempt().is_some(), "connecting");
        self.h.db(DbEvent::Connected);
    }

    /// One keychain call outside a connect, then an event (the app looks after each).
    fn keychain_call(&mut self) {
        let _ = self.h.app.secrets.stores().keychain.get("profile:other");
        self.h.mouse(MouseEventKind::Moved, 0, 0);
    }

    /// A new item written to the keychain (looked at after the next event).
    fn keychain_write(&mut self) {
        self.h.app.secrets.stores().keychain.set("profile:new", "pw").unwrap();
    }

    fn set_delay(&self, d: Duration) {
        *self.store.delay.lock().unwrap() = d;
    }

    /// The offer drawn, then past the time it ignores keys.
    fn arm(&mut self) {
        self.h.draw(80, 24);
        self.h.advance(Duration::from_millis(500));
    }
}

/// The sample profiles at launch (`local-pg` on the keychain, a password stored there), set up
/// as `s` says.
fn launched(tag: &str, s: Setup) -> Offer {
    let dir = std::env::temp_dir().join(format!("datarig-offer-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("state")).unwrap();
    let file = dir.join("state").join(offer::STATE_FILE);
    match &s.state {
        Before::Nothing => {}
        Before::State(state) => offer::save(&file, state).unwrap(),
        Before::Text(text) => std::fs::write(&file, text).unwrap(),
    }
    let config_dir = if s.config_blocked {
        std::fs::write(dir.join("blocked"), "").unwrap();
        dir.join("blocked")
    } else {
        dir.clone()
    };
    let mut cfg: Config = sample_config(None);
    cfg.connections[0].set_source(PasswordSource::Keychain);
    cfg.version = config::CONFIG_VERSION;
    cfg.path = Some(config_dir.join("config.toml"));
    cfg.prefs.keychain_access = s.access;
    let (mut app, clock) = new_app_with_clock(&cfg, Lang::En);
    app.set_paths(Paths { data: Some(dir.join("data")), state: Some(dir.join("state")) });
    let env = s.env;
    app.set_env_lookup(Arc::new(move |k| env.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())));
    app.set_keychain_host(Host { macos: s.macos, binary: Some(binary("0.19.0")) });
    let store = Arc::new(Slow { inner: MemoryStore::new(), clock: clock.clone(), delay: Mutex::new(s.delay) });
    store.inner.set(&cfg.connections[0].id.account(), "stored-pw").unwrap();
    app.set_secret_stores(Stores { os_keychain: s.os_keychain, ..Stores::with_keychain(store.clone()) });
    app.launch(Startup::Normal);
    let driver = FakeDriver::default();
    let h = Harness { app, cancelled: driver.any_cancel.clone(), driver, store: Arc::new(MemoryStore::new()), clock };
    Offer { h, dir, store }
}

/// A connect whose keychain read waited for the user, by a binary that is not the one recorded:
/// once it has connected, the offer asks (never before: the connect goes on meanwhile). Keys
/// typed as it comes up are not taken; `y` switches as the setting does, saves it, records the
/// answer and says what macOS asks next.
#[test]
fn a_prompted_connect_after_an_update_offers_the_security_tool() {
    let mut o = launched("yes", Setup::default());
    o.h.key(KeyCode::Enter);
    assert!(o.h.app.conns.attempt().is_some(), "the connect goes on");
    assert!(!o.offered(), "not while connecting");
    assert_eq!(o.state().binary, Some(binary("0.19.0")), "the running binary is recorded");
    o.h.db(DbEvent::Connected);
    assert!(o.offered(), "asked once connected");
    for (w, hh) in SIZES {
        insta::assert_snapshot!(format!("keychain_offer_en_{w}x{hh}"), o.h.draw(w, hh).backend());
    }
    // Typed as it came up: not an answer.
    o.h.keys("y");
    assert!(o.offered());
    o.arm();
    o.h.keys("y");
    assert!(!o.offered());
    assert_eq!(o.h.app.prefs.keychain_access, KeychainAccess::SecurityTool);
    assert_eq!(o.h.app.secrets.stores().access.get(), KeychainAccess::SecurityTool, "from the next call on");
    assert!(o.config_text().contains("macos_keychain_access = \"security_tool\""), "{}", o.config_text());
    assert_eq!(o.state().answer, Some(Answer::Accepted));
    let status = o.h.status(200, 24);
    let said = "Switched to the security tool. macOS will ask once for each saved password: choose \"Always Allow\".";
    assert!(status.contains(said), "{status}");
    // Never again: the answer is recorded (even were the mode direct again).
    o.h.command("set secrets.macos_keychain_access=direct");
    offer::save(&o.state_file(), &OfferState { answer: Some(Answer::Accepted), binary: Some(binary("0.18.5")) })
        .unwrap();
    o.keychain_call();
    assert!(!o.offered());
}

/// `n` keeps `direct` and is never asked again; `Esc` and `Enter` keep it too but record nothing,
/// so the next prompt after another update asks again.
#[test]
fn n_declines_for_good_and_esc_or_enter_decide_later() {
    let mut o = launched("no", Setup::default());
    o.connect();
    o.arm();
    o.h.key(KeyCode::Esc);
    assert!(!o.offered());
    assert_eq!(o.state(), OfferState { answer: None, binary: Some(binary("0.19.0")) }, "nothing recorded");
    assert_eq!(o.h.app.prefs.keychain_access, KeychainAccess::Direct);
    // The same binary: a slow call is not an update prompt.
    o.keychain_call();
    assert!(!o.offered(), "slow + same binary");
    // Another update, another prompt: asked again; `Enter` keeps.
    offer::save(&o.state_file(), &OfferState { answer: None, binary: Some(binary("0.18.5")) }).unwrap();
    o.keychain_call();
    assert!(o.offered());
    o.arm();
    o.h.key(KeyCode::Enter);
    assert!(!o.offered());
    assert_eq!(o.state().answer, None);
    assert_eq!(o.h.app.prefs.keychain_access, KeychainAccess::Direct);
    offer::save(&o.state_file(), &OfferState { answer: None, binary: Some(binary("0.18.5")) }).unwrap();
    o.keychain_call();
    o.arm();
    o.h.keys("n");
    assert!(!o.offered());
    assert_eq!(o.state().answer, Some(Answer::Declined));
    assert_eq!(o.h.app.prefs.keychain_access, KeychainAccess::Direct);
    assert!(!o.config_text().contains("macos_keychain_access"), "{}", o.config_text());
    offer::save(&o.state_file(), &OfferState { answer: Some(Answer::Declined), binary: Some(binary("0.18.5")) })
        .unwrap();
    o.keychain_call();
    assert!(!o.offered(), "declined: never again");
}

/// The buttons answer as the keys do: "No, keep asking" declines, "Yes, switch" switches.
#[test]
fn the_offer_buttons_answer_with_the_mouse() {
    let find = |o: &mut Offer, text: &str| -> (u16, u16) {
        let screen = o.h.screen(80, 24);
        screen
            .lines()
            .enumerate()
            .find_map(|(y, row)| row.find(text).map(|i| (datarig_tui::text::width(&row[..i]) as u16 + 1, y as u16)))
            .unwrap_or_else(|| panic!("no {text:?}:\n{screen}"))
    };
    let click = |o: &mut Offer, (x, y): (u16, u16)| {
        o.h.advance(Duration::from_millis(500));
        o.h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        o.h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    };
    let mut o = launched("mouse-no", Setup::default());
    o.connect();
    let at = find(&mut o, "No, keep asking");
    click(&mut o, at);
    assert!(!o.offered());
    assert_eq!(o.state().answer, Some(Answer::Declined));

    let mut o = launched("mouse-yes", Setup::default());
    o.connect();
    let at = find(&mut o, "Yes, switch");
    click(&mut o, at);
    assert!(!o.offered());
    assert_eq!(o.state().answer, Some(Answer::Accepted));
    assert_eq!(o.h.app.prefs.keychain_access, KeychainAccess::SecurityTool);
}

/// A dialog open when the offer is due defers it until it closes.
#[test]
fn an_open_dialog_defers_the_offer() {
    let mut o = launched("deferred", Setup::default());
    o.h.key(KeyCode::Enter);
    o.h.ctrl('k');
    assert!(o.h.cmdline().is_some());
    o.h.db(DbEvent::Connected);
    assert!(!o.offered(), "not over the command line");
    o.h.key(KeyCode::Esc);
    assert!(o.offered(), "once it closed");
}

/// The config cannot be written: the error is shown and nothing is recorded.
#[test]
fn a_config_that_cannot_be_written_records_nothing() {
    let mut o = launched("readonly", Setup { config_blocked: true, ..Setup::default() });
    o.connect();
    o.arm();
    o.h.keys("y");
    assert!(!o.offered());
    let status = o.h.status(200, 24);
    assert!(status.contains("Could not save config"), "{status}");
    assert_eq!(o.state().answer, None, "not recorded");
    // As with the setting: the mode applies for this session, unsaved.
    assert_eq!(o.h.app.prefs.keychain_access, KeychainAccess::SecurityTool);
    assert_eq!(o.h.app.secrets.stores().access.get(), KeychainAccess::SecurityTool);
}

/// Never offered: a fast call, over SSH, in `security_tool` mode, with the in-memory store,
/// outside macOS, or with a state file that cannot be read (which is not written over either).
#[test]
fn never_offered_when_it_does_not_apply() {
    let cases: Vec<(&str, Setup)> = vec![
        ("fast", Setup { delay: FAST, ..Setup::default() }),
        ("ssh", Setup { env: SSH, ..Setup::default() }),
        ("tool", Setup { access: KeychainAccess::SecurityTool, ..Setup::default() }),
        ("memory", Setup { os_keychain: false, ..Setup::default() }),
        ("linux", Setup { macos: false, ..Setup::default() }),
        ("answered", Setup { state: Before::Text("answer = \"declined\"\n"), ..Setup::default() }),
        ("unreadable", Setup { state: Before::Text("answer = ["), ..Setup::default() }),
    ];
    for (tag, s) in cases {
        let mut o = launched(tag, s);
        o.connect();
        assert!(!o.offered(), "{tag}");
        o.keychain_call();
        assert!(!o.offered(), "{tag}");
    }
    let mut o = launched("unreadable-kept", Setup { state: Before::Text("answer = ["), ..Setup::default() });
    o.connect();
    assert_eq!(std::fs::read_to_string(o.state_file()).unwrap(), "answer = [", "not written over");
    // No file yet: the first prompt is enough (timing alone decides), a fast call never is.
    let mut o = launched("first-fast", Setup { state: Before::Nothing, delay: FAST, ..Setup::default() });
    o.connect();
    assert!(!o.offered());
    assert_eq!(o.state().binary, Some(binary("0.19.0")), "recorded");
    let mut o = launched("first-slow", Setup { state: Before::Nothing, ..Setup::default() });
    o.connect();
    assert!(o.offered());
}

/// Only a read that returned a saved password proves that macOS trusts the new binary: a new
/// item written first (which never asks) does not hide the prompt of the read after it, while a
/// read that answered at once does.
#[test]
fn only_a_read_that_returned_a_password_records_the_binary() {
    let mut o = launched("write-first", Setup::default());
    // Fast, before the connect's slow read; both are looked at in order after the connect key.
    o.set_delay(FAST);
    o.keychain_write();
    o.set_delay(PROMPTED);
    o.connect();
    assert!(o.offered(), "the slow read after the write still counts");

    let mut o = launched("read-first", Setup { delay: FAST, ..Setup::default() });
    o.connect();
    assert_eq!(o.state().binary, Some(binary("0.19.0")), "the read proved it");
    o.set_delay(PROMPTED);
    o.keychain_write();
    o.keychain_call();
    assert!(!o.offered(), "slow afterwards: not an update prompt");
    assert_eq!(o.state().binary, Some(binary("0.19.0")));
}
