//! Shared headless harness for the snapshot and app-flow tests.
//!
//! Every harness uses a [`MemoryStore`] for passwords, whatever `DATARIG_SECRET_STORE`
//! says: tests never read or write the OS keychain (only the gated keychain round trip in
//! `datarig-core` does).
#![allow(dead_code)]

use datarig_core::config::Config;
use datarig_core::driver::structure::TableStructure;
use datarig_core::driver::{
    Canceller, Capabilities, ColumnMeta, ColumnOrigin, ConnectOptions, DbCommand, DbEvent, Driver, KeyCatalog,
    PingError, PingInfo, Session, SessionRole,
};
use datarig_core::i18n::Lang;
use datarig_core::profile::ConnectionConfig;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_core::sql::complete::{Catalog, ColumnInfo, Relation};
use datarig_tui::app::explorer::RowKind;
use datarig_tui::app::overlay::{Overlay, OverlayKind};
use datarig_tui::app::profiles::ProfileForm;
use datarig_tui::app::{App, AppEvent, CommandLine, EventTarget, Focus, PasswordPrompt, Popup, Startup, Viewer};
use datarig_tui::screens;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Records a cancel request on the session's own flag and on the harness-wide one.
pub struct FakeCancel {
    pub own: Arc<AtomicBool>,
    pub any: Arc<AtomicBool>,
}
impl Canceller for FakeCancel {
    fn cancel(&self) {
        self.own.store(true, Ordering::SeqCst);
        self.any.store(true, Ordering::SeqCst);
    }
}

/// A session the fake driver opened: its role, options and command channel.
pub struct FakeSession {
    pub role: SessionRole,
    pub opts: ConnectOptions,
    pub cmds: UnboundedReceiver<DbCommand>,
    pub cancelled: Arc<AtomicBool>,
    /// Commands already drained from `cmds`.
    pub seen: Vec<DbCommand>,
}

impl FakeSession {
    /// The app closed the session (its command channel is closed).
    pub fn closed(&mut self) -> bool {
        loop {
            match self.cmds.try_recv() {
                Ok(c) => self.seen.push(c),
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => return true,
            }
        }
    }
}

/// A driver whose sessions are plain channels, one per `connect`, kept for the test to read.
#[derive(Clone, Default)]
pub struct FakeDriver {
    pub sessions: Arc<Mutex<Vec<FakeSession>>>,
    pub any_cancel: Arc<AtomicBool>,
    /// The password each `connect` was given, in order.
    pub passwords: Arc<Mutex<Vec<String>>>,
    /// Without `Capabilities::structure` (as a driver that cannot read a table's structure):
    /// an open table shows its columns from the completion catalog.
    pub no_structure: Arc<AtomicBool>,
}

impl Driver for FakeDriver {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            server_paging: true,
            cancel: true,
            introspection: true,
            key_metadata: true,
            contexts: true,
            structure: !self.no_structure.load(Ordering::SeqCst),
        }
    }

    fn connect(
        &self,
        cfg: &ConnectionConfig,
        role: SessionRole,
        opts: ConnectOptions,
        _events: UnboundedSender<DbEvent>,
    ) -> Session {
        self.passwords.lock().unwrap().push(cfg.password.clone());
        let (tx, cmds) = unbounded_channel();
        let own = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(FakeCancel { own: own.clone(), any: self.any_cancel.clone() });
        self.sessions.lock().unwrap().push(FakeSession { role, opts, cmds, cancelled: own, seen: Vec::new() });
        Session::new(self.capabilities(), role, tx, cancel)
    }

    fn ping(
        &self,
        _cfg: &ConnectionConfig,
        _timeout: Duration,
        _dialer: Option<datarig_core::transport::DialerRef>,
    ) -> futures::future::BoxFuture<'static, Result<PingInfo, PingError>> {
        Box::pin(async { Err(PingError::Failed("fake driver".into())) })
    }
}

/// A keychain that is slow on purpose: `set` waits until [`GatedStore::open`] (the OS asking the
/// user for permission). `forgetful` makes it give back another password than the one stored.
///
/// The password move waits in `set` on a blocking thread, and a test runtime waits for its
/// blocking threads when it shuts down. So a test that holds the gate shut keeps an
/// [`OpenOnDrop`] ([`GatedStore::open_on_drop`]): a failed assertion opens the gate while it
/// unwinds and the test fails instead of hanging. As a last resort `set` gives up after
/// [`GATE_LIMIT`] with an error.
#[derive(Default)]
pub struct GatedStore {
    pub inner: MemoryStore,
    gate: std::sync::Mutex<Gate>,
    wake: std::sync::Condvar,
    pub forgetful: bool,
}

#[derive(Default)]
struct Gate {
    open: bool,
    /// `set` calls waiting at the gate right now.
    held: usize,
}

/// How long a `set` waits at a shut gate before it fails.
pub const GATE_LIMIT: Duration = Duration::from_secs(30);

impl GatedStore {
    /// Gives back another password than the one it was given (once something is stored).
    pub fn forgetful() -> Self {
        Self { forgetful: true, ..Self::default() }
    }

    pub fn open(&self) {
        self.gate.lock().unwrap().open = true;
        self.wake.notify_all();
    }

    /// Opens the gate when dropped (see the type's docs).
    pub fn open_on_drop(self: &Arc<Self>) -> OpenOnDrop {
        OpenOnDrop(self.clone())
    }

    /// Wait until a `set` is held at the shut gate (at most [`GATE_LIMIT`]).
    pub fn wait_until_held(&self) {
        let gate = self.gate.lock().unwrap();
        let (gate, _) = self.wake.wait_timeout_while(gate, GATE_LIMIT, |g| g.held == 0).unwrap();
        assert!(gate.held > 0, "nothing reached the keychain within {GATE_LIMIT:?}");
    }
}

/// Opens its [`GatedStore`]'s gate when dropped.
pub struct OpenOnDrop(Arc<GatedStore>);

impl Drop for OpenOnDrop {
    fn drop(&mut self) {
        self.0.open();
    }
}

/// A scratch directory `<tmp>/datarig-<tag>-<pid>` with a `config.toml` of `body` that still
/// holds a plaintext password, removed on drop. The launch-time move of that password to the
/// keychain rewrites the file from its own thread once the store answers, even after the test
/// is done with the app (a real quit ends the process first). Drop this after the store is
/// released: it waits (up to [`MOVE_LIMIT`]) for that rewrite before removing the directory,
/// which would otherwise be written again and left behind.
pub struct MovingConfig {
    pub dir: std::path::PathBuf,
    pub path: std::path::PathBuf,
}

/// How long [`MovingConfig`] waits for the move to rewrite the file.
pub const MOVE_LIMIT: Duration = Duration::from_secs(5);

impl MovingConfig {
    pub fn new(tag: &str, body: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("datarig-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, body).unwrap();
        Self { dir, path }
    }
}

impl Drop for MovingConfig {
    fn drop(&mut self) {
        let t = Instant::now();
        let moving = |p: &std::path::Path| std::fs::read_to_string(p).is_ok_and(|s| s.contains("password ="));
        while moving(&self.path) && t.elapsed() < MOVE_LIMIT {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl SecretStore for GatedStore {
    fn get(&self, account: &str) -> Result<Option<String>, datarig_core::secret::Unavailable> {
        if self.forgetful && self.inner.get(account)?.is_some() {
            return Ok(Some("not what was stored".into()));
        }
        self.inner.get(account)
    }
    fn set(&self, account: &str, secret: &str) -> Result<(), datarig_core::secret::Unavailable> {
        let mut gate = self.gate.lock().unwrap();
        gate.held += 1;
        self.wake.notify_all();
        let (mut gate, _) = self.wake.wait_timeout_while(gate, GATE_LIMIT, |g| !g.open).unwrap();
        gate.held -= 1;
        if !gate.open {
            return Err(datarig_core::secret::Unavailable("the test never opened the keychain gate".into()));
        }
        drop(gate);
        self.inner.set(account, secret)
    }
    fn delete(&self, account: &str) -> Result<bool, datarig_core::secret::Unavailable> {
        self.inner.delete(account)
    }
}

/// A system clipboard that records what it was given (`broken`: it cannot be opened).
#[derive(Clone, Default)]
pub struct FakeClipboard {
    pub texts: Arc<Mutex<Vec<String>>>,
    pub broken: bool,
}

impl datarig_tui::clipboard::SystemClipboard for FakeClipboard {
    fn set_text(&mut self, text: &str) -> Result<(), String> {
        self.texts.lock().unwrap().push(text.to_string());
        Ok(())
    }
}

impl FakeClipboard {
    /// Use it as `h`'s system clipboard, with an environment that has only `env` (no SSH, no
    /// tmux unless given).
    pub fn attach(h: &mut Harness, broken: bool, env: &'static [(&'static str, &'static str)]) -> FakeClipboard {
        let c = FakeClipboard { broken, ..FakeClipboard::default() };
        let open = c.clone();
        h.app.set_clipboard(Arc::new(move || {
            if open.broken {
                Err("no display".to_string())
            } else {
                Ok(Box::new(open.clone()) as Box<dyn datarig_tui::clipboard::SystemClipboard>)
            }
        }));
        h.app.set_env_lookup(Arc::new(move |k: &str| env.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())));
        c
    }

    /// The last text copied to it.
    pub fn last(&self) -> Option<String> {
        self.texts.lock().unwrap().last().cloned()
    }
}

/// A clock that only moves when a test says so (idle paging countdowns in snapshots and
/// flows never depend on how fast the tests run).
#[derive(Clone)]
pub struct FakeClock {
    base: Instant,
    offset: Arc<Mutex<Duration>>,
}

impl FakeClock {
    pub fn new() -> Self {
        Self { base: Instant::now(), offset: Arc::new(Mutex::new(Duration::ZERO)) }
    }

    pub fn now(&self) -> Instant {
        self.base + *self.offset.lock().unwrap()
    }

    pub fn advance(&self, d: Duration) {
        *self.offset.lock().unwrap() += d;
    }
}

/// `App::new` as the binary makes it, minus the terminal: the icons question is never asked
/// (`icons = auto` draws text marks), and the clock is a [`FakeClock`].
pub fn new_app(cfg: &Config, lang: Lang) -> App {
    new_app_with_clock(cfg, lang).0
}

pub fn new_app_with_clock(cfg: &Config, lang: Lang) -> (App, FakeClock) {
    let mut app = App::new(cfg, None, lang);
    let clock = FakeClock::new();
    let c = clock.clone();
    app.set_clock(Arc::new(move || c.now()));
    (app, clock)
}

/// Pin a screen rendered in `lang` (`$term`: a drawn terminal, or a reference to one). An
/// English screen is an insta snapshot named `$name`. A screen in another language is checked
/// against its catalog instead ([`check_localized`]): the repository keeps no UI text in another
/// language outside `locales/`.
#[macro_export]
macro_rules! assert_screen {
    ($name:expr, $lang:expr, $term:expr) => {{
        let name: String = $name.to_string();
        let term = $term;
        let backend = term.backend();
        if $lang == datarig_core::i18n::Lang::En {
            $crate::common::remember_english(&name, backend.buffer());
            insta::assert_snapshot!(name, backend);
        } else {
            $crate::common::check_localized(&name, $lang, backend.buffer());
        }
    }};
}

thread_local! {
    /// The English screens [`assert_screen!`] pinned in this test so far, by snapshot name.
    static ENGLISH_SCREENS: std::cell::RefCell<std::collections::HashMap<String, String>> =
        Default::default();
}

/// Every row of `buf` as text, one line each.
pub fn buffer_text(buf: &Buffer) -> String {
    (0..buf.area.height).map(|y| row_text(buf, y)).collect::<Vec<_>>().join("\n")
}

pub fn remember_english(name: &str, buf: &Buffer) {
    ENGLISH_SCREENS.with(|m| m.borrow_mut().insert(name.to_string(), buffer_text(buf)));
}

/// A screen in `lang` (snapshot name `name`, with the language's code where the English one has
/// `en`), checked against the catalogs:
/// * no catalog text the language translates is left in English on it;
/// * every catalog text of two words or more the English screen of the same name shows, this one
///   shows in `lang`: whole, cut short with `…`, or wrapped over several lines;
/// * something of the language is on it.
pub fn check_localized(name: &str, lang: Lang, buf: &Buffer) {
    use datarig_core::i18n::Label;
    let screen = buffer_text(buf);
    let translated = |l: &Label| l.text(Lang::En) != l.text(lang);
    for l in Label::ALL.iter().filter(|l| translated(l)) {
        let en = l.text(Lang::En);
        assert!(
            en.split_whitespace().count() < 2 || !has_words(&screen, en),
            "{name}: “{en}” ({}) is in English on a {lang:?} screen:\n{screen}",
            l.key()
        );
    }
    let tag = format!("_{lang:?}").to_lowercase();
    let english = match name.rfind(&format!("{tag}_")) {
        Some(i) => format!("{}_en{}", &name[..i], &name[i + tag.len()..]),
        None => name.strip_suffix(&tag).map(|n| format!("{n}_en")).unwrap_or_default(),
    };
    let mut expected = 0;
    if let Some(en_screen) = ENGLISH_SCREENS.with(|m| m.borrow().get(&english).cloned()) {
        for l in Label::ALL.iter().filter(|l| translated(l)) {
            let (en, text) = (l.text(Lang::En), l.text(lang));
            if en.split_whitespace().count() < 2 || !has_words(&en_screen, en) {
                continue;
            }
            expected += 1;
            assert!(
                shown(&screen, text),
                "{name}: “{text}” ({}; “{en}” on {english}) is not on the screen:\n{screen}",
                l.key()
            );
        }
    }
    assert!(
        expected > 0 || Label::ALL.iter().any(|l| translated(l) && screen.contains(l.text(lang))),
        "{name}: nothing of the {lang:?} catalog on the screen:\n{screen}"
    );
}

/// `text` is on `screen` with no letter or digit right before or after it.
fn has_words(screen: &str, text: &str) -> bool {
    screen.match_indices(text).any(|(i, _)| {
        let before = screen[..i].chars().next_back();
        let after = screen[i + text.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// `text` is on `screen`: whole, cut to fit (two characters or more, then `…`), or wrapped
/// (each of its words is there).
fn shown(screen: &str, text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    screen.contains(text)
        || (2..chars.len()).any(|n| screen.contains(&format!("{}…", chars[..n].iter().collect::<String>().trim_end())))
        || (text.contains(' ') && text.split_whitespace().all(|w| screen.contains(w)))
}

/// A Korean UI text, from the Korean catalog (`locales/ko.toml`): the tests hold no Korean
/// text of their own.
pub fn ko(l: datarig_core::i18n::Label) -> &'static str {
    l.text(Lang::Ko)
}

/// A Korean message with its values filled in, from the Korean catalog.
pub fn ko_msg(m: &datarig_core::i18n::Msg) -> String {
    datarig_core::i18n::I18n::new(Lang::Ko).msg(m).to_string()
}

/// The example statements the tests' first console holds (the harness's console on the test
/// DB profile starts with them, the cursor on the first `SELECT`).
pub const SAMPLE_SQL: &str = "-- datarig 🐘  Ctrl+E: run the statement under the cursor
SELECT * FROM shop.users WHERE id <= 8;

SELECT u.name, o.status, o.total_amount
FROM shop.orders o
JOIN shop.users u ON u.id = o.user_id
WHERE o.status = 'paid';

SELECT * FROM analytics.events;

SELECT * FROM analytics.events ORDER BY created_at DESC;

SELECT analytics.slow(30);

SELECT $$a body; with a semicolon$$ AS dollar, 'it''s; fine' AS quoted;
";

/// A config with the one test DB profile (`local-pg`); there is no built-in default any more.
pub fn test_db_config() -> Config {
    Config { connections: vec![datarig_core::profile::ConnectionConfig::test_db()], ..Config::default() }
}

pub struct Harness {
    pub app: App,
    /// Sessions the app opened through the fake driver (`with_config` harnesses only).
    pub driver: FakeDriver,
    /// Some session was asked to cancel.
    pub cancelled: Arc<AtomicBool>,
    pub store: Arc<MemoryStore>,
    /// The app's clock (`clock.advance` + `tick()` run its timers).
    pub clock: FakeClock,
}

impl Harness {
    /// Connecting to the test DB profile (`local-pg`) in the workspace: its node open, the
    /// first tab on it, the editor focused (feed `Connected` next).
    pub fn new(lang: Lang) -> Self {
        Self::with_config(&test_db_config(), lang)
    }

    pub fn with_config(cfg: &Config, lang: Lang) -> Self {
        let (mut app, clock) = new_app_with_clock(cfg, lang);
        let store = Arc::new(MemoryStore::new());
        app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
        let driver = FakeDriver::default();
        let fake = driver.clone();
        app.set_drivers(Arc::new(move |name: &str| {
            matches!(name, "postgres" | "postgresql" | "pg").then(|| Arc::new(fake.clone()) as Arc<dyn Driver>)
        }));
        app.attach_workspace(SAMPLE_SQL);
        // The first statement.
        if app.tabs.len() == 1 {
            app.tab_mut().editor.row = 1;
        }
        let cancelled = driver.any_cancel.clone();
        Harness { app, driver, cancelled, store, clock }
    }

    /// The startup flow (`App::launch`) with `store` as the keychain. No session is attached:
    /// connection attempts are recorded (`connecting()`) but not made, and tests answer them with
    /// `db(DbEvent::Connected | ConnectFailed)`.
    pub fn launched(cfg: &Config, lang: Lang, store: Arc<MemoryStore>, startup: Startup) -> Self {
        let (mut app, clock) = new_app_with_clock(cfg, lang);
        app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
        app.launch(startup);
        let driver = FakeDriver::default();
        Harness { app, cancelled: driver.any_cancel.clone(), driver, store, clock }
    }

    /// Sessions open through the fake driver from now on (headless: the test feeds their
    /// events).
    pub fn with_fake_driver(mut self) -> Self {
        let fake = self.driver.clone();
        self.app.set_drivers(Arc::new(move |name: &str| {
            matches!(name, "postgres" | "postgresql" | "pg").then(|| Arc::new(fake.clone()) as Arc<dyn Driver>)
        }));
        self
    }

    /// The binary's startup (`App::start` with an event channel), to be called inside a Tokio
    /// runtime: the launch-time password move runs in the background with `store` as the
    /// keychain. Background events arrive on the returned receiver; feed them to
    /// `app.on_app_event`. `store` here is a fresh `MemoryStore` the app does not use.
    pub fn started(
        cfg: &Config,
        lang: Lang,
        store: Arc<dyn SecretStore>,
        startup: Startup,
    ) -> (Self, UnboundedReceiver<AppEvent>) {
        let (mut app, clock) = new_app_with_clock(cfg, lang);
        app.set_secret_store(store);
        let (tx, rx) = unbounded_channel();
        app.start(tx, startup);
        let driver = FakeDriver::default();
        let h =
            Harness { app, cancelled: driver.any_cancel.clone(), driver, store: Arc::new(MemoryStore::new()), clock };
        (h, rx)
    }

    /// Name of the `last_used` profile.
    pub fn last_used_name(&self) -> Option<String> {
        let id = self.app.last_used?;
        self.app.profiles.iter().find(|p| p.id == id).map(|p| p.name.clone())
    }

    /// Keychain account of the profile named `name`.
    pub fn account(&self, name: &str) -> String {
        self.app.profiles.iter().find(|p| p.name == name).expect("profile").id.account()
    }

    /// Whole screen as text, one line per row.
    pub fn screen(&mut self, w: u16, h: u16) -> String {
        let t = self.draw(w, h);
        (0..h).map(|y| row_text(t.backend().buffer(), y)).collect::<Vec<_>>().join("\n")
    }

    /// Connected, schemas listed, catalog loaded (as the driver would report it).
    pub fn connected(lang: Lang) -> Self {
        let mut h = Self::new(lang);
        h.db(DbEvent::Connected);
        h.db(DbEvent::Schemas(Ok(vec!["analytics".into(), "public".into(), "shop".into()])));
        h.db(DbEvent::Catalog(Ok(catalog())));
        // The explorer's node is open, so it asked for the server's databases:
        // the answer is the profile's own only, and the request is not the test's.
        if h.sent().iter().any(|c| matches!(c, DbCommand::LoadDatabases)) {
            h.db(DbEvent::Databases(Ok(vec!["datarig".into()])));
        }
        h
    }

    pub fn db(&mut self, ev: DbEvent) {
        self.app.on_db_event(ev);
    }

    /// A driver event of the metadata session of profile `name` (its current attempt).
    pub fn meta_db(&mut self, name: &str, ev: DbEvent) {
        let id = self.app.profiles.iter().find(|p| p.name == name).expect("profile").id;
        let generation = self.app.conns.get(id).map_or(0, |c| c.generation);
        self.app.on_app_event(AppEvent::Db { target: EventTarget::Meta(id), generation, ev });
    }

    pub fn key_mod(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let k = KeyEvent { code, modifiers, kind: KeyEventKind::Press, state: KeyEventState::NONE };
        self.app.handle_event(Event::Key(k));
    }

    /// A mouse event at (x, y), no modifiers.
    pub fn mouse(&mut self, kind: ratatui::crossterm::event::MouseEventKind, x: u16, y: u16) {
        use ratatui::crossterm::event::MouseEvent;
        let m = MouseEvent { kind, column: x, row: y, modifiers: KeyModifiers::NONE };
        self.app.handle_event(Event::Mouse(m));
    }

    /// Press the left button at `from`, drag through `path`, release at the last point.
    pub fn drag(&mut self, from: (u16, u16), path: &[(u16, u16)]) {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        self.mouse(MouseEventKind::Down(MouseButton::Left), from.0, from.1);
        for &(x, y) in path {
            self.mouse(MouseEventKind::Drag(MouseButton::Left), x, y);
        }
        let last = path.last().copied().unwrap_or(from);
        self.mouse(MouseEventKind::Up(MouseButton::Left), last.0, last.1);
    }

    pub fn key(&mut self, code: KeyCode) {
        self.key_mod(code, KeyModifiers::NONE);
    }

    pub fn key_kind(&mut self, code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) {
        self.app.handle_event(Event::Key(KeyEvent { code, modifiers, kind, state: KeyEventState::NONE }));
    }

    /// Move the app's clock by `d` and run its timers at the new time.
    pub fn advance(&mut self, d: Duration) {
        self.clock.advance(d);
        self.app.on_tick(self.clock.now());
    }

    /// Let debounce timers (auto-completion) fire.
    pub fn settle(&mut self) {
        self.app.on_tick(Instant::now() + Duration::from_secs(1));
    }

    /// Type text without Shift handling tricks (for non-ASCII input).
    pub fn type_text(&mut self, s: &str) {
        for c in s.chars() {
            self.key_mod(KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    // ── state accessors (tests look at dialogs only through these) ──────────

    /// Kind of the dialog that has the keyboard, if any.
    pub fn overlay_kind(&self) -> Option<OverlayKind> {
        self.app.overlays.top().map(Overlay::kind)
    }

    /// The `:` command line (`command(text)` runs something through it).
    pub fn cmdline(&self) -> Option<&CommandLine> {
        self.app.overlays.command_line()
    }

    /// The profile form being edited.
    pub fn form(&self) -> &ProfileForm {
        self.app.overlays.form().expect("profile form open")
    }

    /// The profile form is open.
    pub fn form_open(&self) -> bool {
        self.app.overlays.form().is_some()
    }

    pub fn prompt(&self) -> Option<&PasswordPrompt> {
        self.app.overlays.prompt()
    }

    pub fn viewer(&self) -> Option<&Viewer> {
        self.app.overlays.viewer()
    }

    /// Index of the explorer's cursor (row 0 is "＋ New connection").
    pub fn selected(&self) -> usize {
        self.app.explorer_selected()
    }

    /// The explorer's rows as text: folders `name/`, profiles by name, error lines `!`,
    /// databases `db:name` (the profile's own `db:name*`), notes `(note)`, schema nodes by their
    /// label, each indented by its depth.
    pub fn rows(&self) -> Vec<String> {
        self.app
            .explorer_rows()
            .into_iter()
            .map(|r| {
                let text = match &r.kind {
                    RowKind::NewConnection => "+".to_string(),
                    RowKind::Folder(f) => format!("{}/", f.name()),
                    RowKind::Profile(id) => self.app.profile(*id).map(|p| p.name.clone()).unwrap_or_default(),
                    RowKind::ProfileError(_) => "!".to_string(),
                    RowKind::Node(id, n) => {
                        self.app.conns.get(*id).map(|c| c.tree.label(*n, &self.app.i18n).0).unwrap_or_default()
                    }
                    // Databases: `db:name`, the profile's own `db:name*`.
                    RowKind::Database(id, None) => format!("db:{}*", self.app.own_database(*id)),
                    RowKind::Database(_, Some(db)) => format!("db:{db}"),
                    RowKind::DatabasesNote(_) | RowKind::DatabaseNote(..) => "(note)".to_string(),
                    RowKind::AuxNode(id, db, n) => self
                        .app
                        .conns
                        .aux(*id, db)
                        .map(|a| a.tree.label(*n, &self.app.i18n).0)
                        .unwrap_or_else(|| "Loading…".to_string()),
                    RowKind::ScriptsHeader => "[saved queries]".to_string(),
                    RowKind::ScriptFolder(p) => format!("{}/", datarig_core::scripts::display_name(p, true)),
                    RowKind::Script(p) => datarig_core::scripts::display_name(p, false).to_string(),
                    RowKind::ScriptsEmpty => "(none)".to_string(),
                };
                format!("{}{text}", "  ".repeat(r.depth))
            })
            .collect()
    }

    /// The explorer's line under the cursor, whole (indentation, arrow, icons, marks and
    /// details; the explorer cuts it at its width).
    pub fn explorer_line(&mut self) -> String {
        let rows = self.app.explorer_rows();
        rows.get(self.selected()).map(|r| self.app.explorer_line_text(r)).unwrap_or_default()
    }

    /// Move the explorer's cursor down (`j`) until its line contains `text`.
    pub fn goto(&mut self, text: &str) {
        for _ in 0..300 {
            if self.explorer_line().contains(text) {
                return;
            }
            let before = self.selected();
            self.keys("j");
            if self.selected() == before {
                break;
            }
        }
        let rows = self.app.explorer_rows();
        let all: Vec<String> = rows.iter().map(|r| self.app.explorer_line_text(r)).collect();
        panic!("no explorer line with {text:?} below the cursor:\n{}", all.join("\n"));
    }

    /// Focus the explorer and put its cursor on the profile named `name`.
    pub fn explore(&mut self, name: &str) {
        self.app.focus = Focus::Tree;
        let rows = self.app.explorer_rows();
        let i = rows
            .iter()
            .position(
                |r| matches!(r.kind, RowKind::Profile(id) if self.app.profile(id).is_some_and(|p| p.name == name)),
            )
            .unwrap_or_else(|| panic!("no profile {name} in {:?}", self.rows()));
        self.app.explorer.select(&rows, i);
    }

    /// The latest connection attempt in progress: the profile's name and whether a password
    /// was sent.
    pub fn connecting(&self) -> Option<(String, bool)> {
        self.app.conns.attempt().map(|c| (c.name.clone(), c.had_password))
    }

    /// The error line of profile `name`'s node, if it failed.
    pub fn node_error(&self, name: &str) -> Option<String> {
        let id = self.app.profiles.iter().find(|p| p.name == name)?.id;
        self.app.conns.get(id)?.error.as_ref().map(|e| e.render(&self.app.i18n).to_string())
    }

    /// Completion popup of the editor.
    pub fn popup(&self) -> Option<&Popup> {
        self.app.tab().popup.as_ref()
    }

    /// Ctrl+K, type `text` (a command or an action search), Enter.
    pub fn command(&mut self, text: &str) {
        self.ctrl('k');
        assert!(self.cmdline().is_some(), "command line opened");
        self.type_text(text);
        self.key(KeyCode::Enter);
    }

    pub fn ping(&mut self, result: Result<datarig_core::driver::PingInfo, datarig_core::driver::PingError>) {
        let seq = self.app.conn_test.as_ref().expect("a test is running").seq;
        self.app.on_app_event(AppEvent::Ping { seq, result });
    }

    pub fn ctrl(&mut self, c: char) {
        self.key_mod(KeyCode::Char(c), KeyModifiers::CONTROL);
    }

    pub fn keys(&mut self, s: &str) {
        for c in s.chars() {
            let m = if c.is_ascii_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
            self.key_mod(KeyCode::Char(c), m);
        }
    }

    /// Commands sent to any session since the last call, session by session in the order they
    /// were opened.
    pub fn sent(&mut self) -> Vec<DbCommand> {
        let mut sessions = self.driver.sessions.lock().unwrap();
        let mut v = Vec::new();
        for s in sessions.iter_mut() {
            s.closed();
            v.append(&mut s.seen);
        }
        v
    }

    /// Commands sent to session `i` (in opening order) since the last call.
    pub fn sent_to(&mut self, i: usize) -> Vec<DbCommand> {
        let mut sessions = self.driver.sessions.lock().unwrap();
        let s = &mut sessions[i];
        s.closed();
        std::mem::take(&mut s.seen)
    }

    /// Roles of the sessions opened so far, in order.
    pub fn roles(&self) -> Vec<SessionRole> {
        self.driver.sessions.lock().unwrap().iter().map(|s| s.role).collect()
    }

    /// Session `i` was closed by the app.
    pub fn session_closed(&mut self, i: usize) -> bool {
        self.driver.sessions.lock().unwrap()[i].closed()
    }

    /// Session `i` was asked to cancel.
    pub fn session_cancelled(&self, i: usize) -> bool {
        self.driver.sessions.lock().unwrap()[i].cancelled.load(Ordering::SeqCst)
    }

    /// A driver event of the query session of tab `index` (its current generation).
    pub fn tab_db(&mut self, index: usize, ev: DbEvent) {
        let t = self.app.tabs.iter().nth(index).expect("tab");
        let (target, generation) = (EventTarget::Tab(t.id), t.exec.generation);
        self.app.on_app_event(AppEvent::Db { target, generation, ev });
    }

    pub fn draw(&mut self, w: u16, h: u16) -> Terminal<TestBackend> {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| screens::draw(f, &mut self.app)).unwrap();
        t
    }

    pub fn status(&mut self, w: u16, h: u16) -> String {
        let t = self.draw(w, h);
        row_text(t.backend().buffer(), h - 1)
    }
}

/// Text of a screen row as the terminal shows it (the filler cell after a wide grapheme is
/// skipped).
pub fn row_text(buf: &Buffer, y: u16) -> String {
    let mut s = String::new();
    let mut x = 0;
    while x < buf.area.width {
        let sym = buf[(x, y)].symbol();
        s.push_str(sym);
        x += datarig_tui::text::width(sym).max(1) as u16;
    }
    s
}

/// Three profiles: the test DB, a CJK-named replica and an IPv6 host.
pub fn sample_profiles() -> Vec<datarig_core::profile::ConnectionConfig> {
    use datarig_core::profile::ConnectionConfig;
    // Each profile has its own id (`..Default::default()` makes a new one).
    vec![
        ConnectionConfig { password: String::new(), ..ConnectionConfig::test_db() },
        ConnectionConfig {
            name: "分析-replica".into(),
            host: "analytics.internal".into(),
            port: 6432,
            user: "report".into(),
            database: "warehouse".into(),
            sslmode: "require".into(),
            ..ConnectionConfig::default()
        },
        ConnectionConfig {
            name: "v6".into(),
            host: "::1".into(),
            port: 5432,
            user: "postgres".into(),
            database: "postgres".into(),
            ..ConnectionConfig::default()
        },
    ]
}

/// The sample profiles; `last_used` by name.
pub fn sample_config(last_used: Option<&str>) -> Config {
    let connections = sample_profiles();
    let last_used = last_used.and_then(|n| connections.iter().find(|c| c.name == n)).map(|c| c.id);
    Config { connections, last_used, ..Config::default() }
}

pub fn catalog() -> Catalog {
    let col = |n: &str, t: &str| ColumnInfo { name: n.into(), type_name: t.into() };
    let rel = |s: &str, n: &str, v: bool, cols: Vec<ColumnInfo>| Relation {
        schema: s.into(),
        name: n.into(),
        is_view: v,
        columns: cols,
    };
    Catalog {
        schemas: vec!["analytics".into(), "public".into(), "shop".into()],
        relations: vec![
            rel("analytics", "events", false, vec![col("id", "bigint"), col("event_type", "text")]),
            rel("shop", "orders", false, vec![col("id", "bigint"), col("user_id", "bigint"), col("status", "text")]),
            rel(
                "shop",
                "users",
                false,
                vec![
                    col("id", "bigint"),
                    col("email", "text"),
                    col("name", "text"),
                    col("nickname", "text"),
                    col("profile", "jsonb"),
                ],
            ),
        ],
    }
}

pub fn meta(name: &str, ty: &str, numeric: bool, json: bool) -> ColumnMeta {
    ColumnMeta { name: name.into(), type_name: ty.into(), numeric, json, origin: None }
}

/// A result column that comes from column `column` of table `table` (see [`shop_keys`]).
pub fn meta_from(name: &str, ty: &str, numeric: bool, table: u32, column: i16) -> ColumnMeta {
    ColumnMeta { origin: Some(ColumnOrigin { table, column }), ..meta(name, ty, numeric, false) }
}

/// Ids of the tables of [`shop_keys`].
pub const USERS: u32 = 16_401;
pub const ORDERS: u32 = 16_402;
pub const ORDER_ITEMS: u32 = 16_403;

/// The keys of the test database's `shop` tables as the driver reports them: users (id PK,
/// email UQ), orders (id PK, user_id FK), order_items (order_id + line_no composite PK,
/// order_id and product_id FK).
pub fn shop_keys() -> KeyCatalog {
    use datarig_core::driver::keys::KeyKind;
    let cols =
        |names: &[&str]| names.iter().enumerate().map(|(i, n)| (i as i16 + 1, n.to_string())).collect::<Vec<_>>();
    let mut k = KeyCatalog::default();
    k.add_table(USERS, "shop", "users", cols(&["id", "email", "name", "nickname", "profile"]));
    k.add_table(ORDERS, "shop", "orders", cols(&["id", "user_id", "status", "total_amount"]));
    k.add_table(ORDER_ITEMS, "shop", "order_items", cols(&["order_id", "line_no", "product_id", "quantity"]));
    k.mark(USERS, &[1], KeyKind::Primary);
    k.mark(USERS, &[2], KeyKind::Unique);
    k.mark(ORDERS, &[1], KeyKind::Primary);
    k.mark(ORDERS, &[2], KeyKind::Foreign);
    k.mark(ORDER_ITEMS, &[1, 2], KeyKind::Primary);
    k.mark(ORDER_ITEMS, &[1], KeyKind::Foreign);
    k.mark(ORDER_ITEMS, &[3], KeyKind::Foreign);
    k
}

/// The structure of `shop.users` as the driver reports it: about 11,000 rows in 4.2 MB, an
/// identity-free serial key, a unique email, indexes (one partial), no checks, two triggers (one
/// disabled), no foreign keys.
pub fn users_structure() -> TableStructure {
    use datarig_core::driver::structure::*;
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let col = |name: &str, ty: &str, not_null: bool, default: Option<&str>| StructureColumn {
        name: name.into(),
        type_name: ty.into(),
        not_null,
        default: default.map(str::to_string),
        fill: ColumnFill::Default,
    };
    let index =
        |name: &str, cols: &[&str], unique: bool, predicate: Option<&str>, primary: bool, constraint: bool| Index {
            name: name.into(),
            columns: s(cols),
            options: Vec::new(),
            include: Vec::new(),
            unique,
            method: "btree".into(),
            predicate: predicate.map(str::to_string),
            primary,
            constraint,
            definition: String::new(),
        };
    let mut t = TableStructure::new(RelationKind::Table);
    t.estimated_rows = Some(11_000);
    t.total_bytes = Some(4_404_019);
    t.columns = vec![
        col("id", "bigint", true, Some("nextval('shop.users_id_seq'::regclass)")),
        col("email", "text", true, None),
        col("name", "text", true, None),
        col("nickname", "text", false, None),
        col("profile", "jsonb", false, Some("'{}'::jsonb")),
    ];
    t.primary_key = Some(KeyConstraint { name: "users_pkey".into(), columns: s(&["id"]), definition: String::new() });
    t.unique_constraints =
        vec![KeyConstraint { name: "users_email_key".into(), columns: s(&["email"]), definition: String::new() }];
    t.indexes = vec![
        index("users_email_key", &["email"], true, None, false, true),
        index("users_nickname_idx", &["nickname"], false, Some("nickname IS NOT NULL"), false, false),
        index("users_pkey", &["id"], true, None, true, true),
    ];
    t.indexes[1].options = s(&["DESC"]);
    t.triggers = vec![
        Trigger {
            name: "users_audit".into(),
            timing: TriggerTiming::After,
            events: vec![TriggerEvent::Insert, TriggerEvent::Delete],
            for_each_row: false,
            function: "shop.audit".into(),
            enabled: false,
            update_columns: Vec::new(),
            condition: None,
            definition: String::new(),
        },
        Trigger {
            name: "users_touch".into(),
            timing: TriggerTiming::Before,
            events: vec![TriggerEvent::Update],
            for_each_row: true,
            function: "shop.touch".into(),
            enabled: true,
            update_columns: s(&["name"]),
            condition: Some("old.name IS DISTINCT FROM new.name".into()),
            definition: String::new(),
        },
    ];
    t
}

/// The structure of `shop.orders`: never analyzed (no row estimate), a foreign key to
/// `shop.users` and one to `analytics.events` (`ON DELETE CASCADE ON UPDATE SET NULL`), a check.
pub fn orders_structure() -> TableStructure {
    use datarig_core::driver::structure::*;
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let col = |name: &str, ty: &str| StructureColumn {
        name: name.into(),
        type_name: ty.into(),
        not_null: true,
        default: None,
        fill: ColumnFill::Default,
    };
    let mut t = TableStructure::new(RelationKind::Table);
    t.total_bytes = Some(8192);
    t.columns = vec![col("id", "bigint"), col("user_id", "bigint"), col("status", "text")];
    t.columns[0].fill = ColumnFill::IdentityAlways;
    t.primary_key = Some(KeyConstraint { name: "orders_pkey".into(), columns: s(&["id"]), definition: String::new() });
    let fk = |name: &str, cols: &[&str], schema: &str, table: &str, on_delete, on_update| ForeignKey {
        name: name.into(),
        columns: s(cols),
        ref_schema: schema.into(),
        ref_table: table.into(),
        ref_columns: s(&["id"]),
        on_delete,
        on_update,
        definition: String::new(),
    };
    t.foreign_keys = vec![
        fk("orders_event_fkey", &["id"], "analytics", "events", FkAction::Cascade, FkAction::SetNull),
        fk("orders_user_id_fkey", &["user_id"], "shop", "users", FkAction::NoAction, FkAction::NoAction),
    ];
    t.checks = vec![CheckConstraint {
        name: "orders_status_check".into(),
        expression: "status = ANY (ARRAY['pending'::text, 'paid'::text])".into(),
        definition: String::new(),
    }];
    t
}

/// The eight hand-crafted edge-case users from `dev/init/02_seed.sql`.
pub fn edge_rows() -> (Vec<ColumnMeta>, Vec<Vec<Option<String>>>) {
    let cols = vec![
        meta("id", "int8", true, false),
        meta("name", "text", false, false),
        meta("nickname", "text", false, false),
        meta("address", "text", false, false),
        meta("profile", "jsonb", false, true),
    ];
    let r = |id: &str, name: &str, nick: Option<&str>, addr: Option<&str>, prof: &str| {
        vec![Some(id.into()), Some(name.into()), nick.map(Into::into), addr.map(Into::into), Some(prof.into())]
    };
    let rows = vec![
        r(
            "1",
            "陳大文",
            Some("大文🐘"),
            Some("臺北市信義區信義路五段7號, 台北101大樓 10樓"),
            r#"{"lang": "ko", "tags": ["dev", "db"]}"#,
        ),
        r("2", "林美玲", None, Some("高雄市前鎮區成功二路 55號"), "{}"),
        r("3", "張志明", Some("👨‍👩‍👧‍👦 family"), None, r#"{"vip": true, "level": 7}"#),
        r("4", "Emoji テスト", Some("🔥🚀✨🎉"), Some("沖縄県那覇市おもろまち 242 🏝️"), r#"{"flags": ["🇰🇷", "🇺🇸"]}"#),
        r("5", "山田太郎", Some("やまだ"), Some("東京都渋谷区道玄坂1-2-3"), r#"{"lang": "ja"}"#),
        r("6", "王小明", None, Some("北京市海淀区中关村大街1号"), r#"{"lang": "zh"}"#),
        r(
            "7",
            "Plain ASCII User",
            Some("ascii_only"),
            Some("1600 Amphitheatre Pkwy, Mountain View, CA"),
            r#"{"lang": "en"}"#,
        ),
        r(
            "8",
            "全角 ＡＢＣ 半角 ｱｲｳ",
            Some("e\u{301} combining"),
            Some("タブ\t入り住所 (tab inside)"),
            r#"{"multiline": true}"#,
        ),
    ];
    (cols, rows)
}

/// Run the statement under the cursor via Ctrl+E and answer with the edge-case rows.
pub fn with_edge_results(lang: Lang) -> Harness {
    let mut h = Harness::connected(lang);
    h.ctrl('e');
    let sent = h.sent();
    assert!(
        matches!(&sent[..], [DbCommand::Execute { id: 1, statements }] if statements == &["SELECT * FROM shop.users WHERE id <= 8"]),
        "{sent:?}"
    );
    let (cols, rows) = edge_rows();
    h.db(DbEvent::Page { id: 1, columns: Some(cols), rows, more: false, elapsed: Duration::from_millis(12) });
    h
}
