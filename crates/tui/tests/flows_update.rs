//! The update check with a fake request that counts its calls: when it asks (after the first
//! frame, once a day), each switch that turns it off (zero calls), what is kept in
//! `update.toml`, the notice and its answers, `:update`'s words, and the notice's look.
mod common;
use common::*;
use datarig_core::config::{Config, UpdateCheck};
use datarig_core::i18n::{Label, Lang, Msg};
use datarig_core::paths::Paths;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_core::update::install::Install;
use datarig_core::update::{self, Answer, Outcome, Release, Version};
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::{AppEvent, Startup};
use datarig_update::Fetch;
use ratatui::crossterm::event::KeyCode;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;

/// A request that answers `answer` (after `delay`) and counts its calls and the tags it got.
struct FakeFetch {
    calls: AtomicUsize,
    etags: Mutex<Vec<Option<String>>>,
    answer: Answer,
    delay: Duration,
}

impl FakeFetch {
    fn new(answer: Answer) -> Arc<Self> {
        Arc::new(FakeFetch { calls: AtomicUsize::new(0), etags: Mutex::default(), answer, delay: Duration::ZERO })
    }
    fn slow(answer: Answer, delay: Duration) -> Arc<Self> {
        Arc::new(FakeFetch { calls: AtomicUsize::new(0), etags: Mutex::default(), answer, delay })
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Fetch for FakeFetch {
    fn fetch(&self, etag: Option<&str>) -> Answer {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.etags.lock().unwrap().push(etag.map(str::to_string));
        std::thread::sleep(self.delay);
        self.answer.clone()
    }
}

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

fn newer() -> Answer {
    let r = Release { version: v("99.0.0"), date: Some("2099-01-01".into()), summary: Some("A summary.".into()) };
    Answer::Release(r, Some("W/\"t\"".into()))
}

/// A state directory (and a config file) of its own, removed at the end.
struct State(PathBuf);

impl State {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("datarig-update-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        State(dir)
    }
    fn file(&self) -> PathBuf {
        self.0.join(update::FILE)
    }
    fn state(&self) -> update::State {
        update::load(&self.file()).unwrap()
    }
    fn config(&self) -> Config {
        // A profile, so launch does not open the new-connection form under the notice.
        Config { path: Some(self.0.join("config.toml")), connections: sample_profiles(), ..Default::default() }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The runtime the app's background work runs on, as the binary's.
static RT: std::sync::LazyLock<tokio::runtime::Runtime> =
    std::sync::LazyLock::new(|| tokio::runtime::Runtime::new().unwrap());

/// Launched at `state` (or without one) with `env` and `fetch`; the first frame not drawn yet.
fn launch(
    cfg: &Config,
    state: Option<&State>,
    env: &'static [(&'static str, &'static str)],
    fetch: Arc<FakeFetch>,
) -> (Harness, UnboundedReceiver<AppEvent>) {
    let (mut app, clock) = new_app_with_clock(cfg, Lang::En);
    app.set_secret_store(Arc::new(MemoryStore::new()) as Arc<dyn SecretStore>);
    app.set_env_lookup(Arc::new(move |k: &str| env.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())));
    app.set_paths(Paths { data: None, state: state.map(|s| s.0.clone()) });
    app.updates.fetch = Some(fetch as Arc<dyn Fetch>);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let _rt = RT.enter();
    app.start(tx, Startup::Normal);
    let driver = FakeDriver::default();
    let h = Harness { app, cancelled: driver.any_cancel.clone(), driver, store: Arc::new(MemoryStore::new()), clock };
    (h, rx)
}

/// The check's answer, fed to the app (at most 5 seconds).
fn answered(h: &mut Harness, rx: &mut UnboundedReceiver<AppEvent>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match rx.try_recv() {
            Ok(ev @ AppEvent::UpdateChecked(_)) => return h.app.on_app_event(ev),
            Ok(ev) => h.app.on_app_event(ev),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => panic!("no answer: {e:?}"),
        }
    }
}

/// No answer comes (the request was not made).
fn quiet(rx: &mut UnboundedReceiver<AppEvent>) {
    std::thread::sleep(Duration::from_millis(100));
    while let Ok(ev) = rx.try_recv() {
        assert!(!matches!(ev, AppEvent::UpdateChecked(_)), "an answer came");
    }
}

#[test]
fn the_check_waits_for_the_first_frame_keeps_its_attempt_first_and_runs_once_a_day() {
    let s = State::new("once");
    let fetch = FakeFetch::slow(newer(), Duration::from_millis(300));
    let (mut h, mut rx) = launch(&s.config(), Some(&s), &[], fetch.clone());
    assert_eq!(fetch.calls(), 0, "nothing before the first frame");
    assert!(!s.file().exists() || s.state().last_attempt.is_none());
    h.app.first_frame_drawn();
    // The attempt is kept before the answer comes.
    let st = s.state();
    assert!(st.last_attempt.is_some() && st.result.is_none(), "{st:?}");
    answered(&mut h, &mut rx);
    assert_eq!(fetch.calls(), 1);
    assert_eq!(fetch.etags.lock().unwrap()[0], None);
    let st = s.state();
    assert_eq!((st.result, st.latest, st.etag.as_deref()), (Some(Outcome::Ok), Some(v("99.0.0")), Some("W/\"t\"")));
    // Found during the session: a status line, no dialog.
    assert_eq!(h.overlay_kind(), None);
    assert!(h.status(120, 24).contains("datarig 99.0.0 is out. :update for how to update"), "{}", h.status(120, 24));
    drop(h);
    // The next launch the same day: no request, and the notice from what is kept.
    let fetch2 = FakeFetch::new(newer());
    let (mut h, mut rx) = launch(&s.config(), Some(&s), &[], fetch2.clone());
    assert_eq!(h.overlay_kind(), Some(OverlayKind::UpdateNotice));
    h.app.first_frame_drawn();
    quiet(&mut rx);
    assert_eq!(fetch2.calls(), 0);
}

#[test]
fn a_day_later_it_asks_again_with_the_tag() {
    let s = State::new("tag");
    std::fs::write(s.file(), "last_attempt = 2000-01-01T00:00:00Z\nresult = \"ok\"\netag = \"W/\\\"old\\\"\"\n")
        .unwrap();
    let fetch = FakeFetch::new(Answer::NotModified);
    let (mut h, mut rx) = launch(&s.config(), Some(&s), &[], fetch.clone());
    h.app.first_frame_drawn();
    answered(&mut h, &mut rx);
    assert_eq!(fetch.etags.lock().unwrap().as_slice(), [Some("W/\"old\"".to_string())]);
    assert_eq!(s.state().result, Some(Outcome::NotModified));
}

#[test]
fn each_switch_that_turns_it_off_makes_no_request() {
    let mut off_config = State::new("off-config").config();
    off_config.prefs.update_check = UpdateCheck::Off;
    /// What turns it off, the config, whether there is a state directory, the environment.
    type Case = (&'static str, Config, bool, &'static [(&'static str, &'static str)]);
    let cases: [Case; 5] = [
        ("config", off_config, true, &[]),
        ("env", Config::default(), true, &[("DATARIG_NO_UPDATE_CHECK", "1")]),
        ("ci", Config::default(), true, &[("CI", "true")]),
        ("no state dir", Config::default(), false, &[]),
        ("socks proxy", Config::default(), true, &[("HTTPS_PROXY", "socks5://proxy:1080")]),
    ];
    for (what, cfg, with_state, env) in cases {
        let s = State::new(&format!("off-{}", what.replace(' ', "-")));
        let fetch = FakeFetch::new(newer());
        let (mut h, mut rx) = launch(&cfg, with_state.then_some(&s), env, fetch.clone());
        h.app.first_frame_drawn();
        quiet(&mut rx);
        assert_eq!(fetch.calls(), 0, "{what}");
        if what == "socks proxy" {
            // Recorded as the proxy's, for `:update`; never a direct connection.
            assert_eq!(s.state().result, Some(Outcome::Proxy), "{what}");
        } else if with_state {
            assert!(s.state().last_attempt.is_none(), "{what}: nothing written");
        }
    }
    // An update.toml that cannot be read: no request, and it is left as it is.
    let s = State::new("off-unreadable");
    std::fs::write(s.file(), "latest = 9\n").unwrap();
    let fetch = FakeFetch::new(newer());
    let (mut h, mut rx) = launch(&s.config(), Some(&s), &[], fetch.clone());
    h.app.first_frame_drawn();
    quiet(&mut rx);
    assert_eq!(fetch.calls(), 0);
    assert_eq!(std::fs::read_to_string(s.file()).unwrap(), "latest = 9\n");
    // A second instance reads and tells, never asks nor writes.
    let s = State::new("off-second");
    std::fs::write(s.file(), "latest = \"99.0.0\"\n").unwrap();
    let _lock = datarig_core::workspace::acquire(&s.0).unwrap();
    let fetch = FakeFetch::new(newer());
    let (mut h, mut rx) = launch(&s.config(), Some(&s), &[], fetch.clone());
    assert!(h.app.read_only);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::UpdateNotice));
    h.key(KeyCode::Char('s'));
    // Not kept here, and said so (the other instance keeps the file).
    let t = h.status(200, 24);
    assert!(t.contains("Not skipped: another datarig") && !t.contains("Not told about"), "{t}");
    h.app.first_frame_drawn();
    quiet(&mut rx);
    assert_eq!(fetch.calls(), 0);
    assert_eq!(std::fs::read_to_string(s.file()).unwrap(), "latest = \"99.0.0\"\n");
}

#[test]
fn a_debug_build_never_makes_the_real_request() {
    // Without a fetcher of a test's own, this (debug) build decides not to check at all.
    let s = State::new("debug");
    let (mut app, _clock) = new_app_with_clock(&s.config(), Lang::En);
    app.set_secret_store(Arc::new(MemoryStore::new()) as Arc<dyn SecretStore>);
    app.set_paths(Paths { data: None, state: Some(s.0.clone()) });
    // Not the process's environment: CI services set `CI`.
    app.set_env_lookup(Arc::new(|_: &str| None));
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let _rt = RT.enter();
    app.start(tx, Startup::Normal);
    app.first_frame_drawn();
    if cfg!(debug_assertions) {
        assert_eq!(app.updates.decision, Some(update::Decision::Off(update::Off::DebugBuild)));
        assert!(!app.updates.running);
        assert!(!s.file().exists() || s.state().last_attempt.is_none());
    }
}

/// Launched at `s` with `latest` already known: the notice is up.
fn with_notice(s: &State, latest: &str) -> Harness {
    std::fs::write(s.file(), format!("latest = \"{latest}\"\nsummary = \"The summary line.\"\n")).unwrap();
    let (h, _rx) = launch(&s.config(), Some(s), &[("DATARIG_NO_UPDATE_CHECK", "")], FakeFetch::new(newer()));
    assert_eq!(
        h.overlay_kind(),
        Some(OverlayKind::UpdateNotice),
        "{:?} {:?} {}",
        h.app.updates.decision,
        h.app.updates.switches,
        std::fs::read_to_string(s.file()).unwrap_or_default()
    );
    h
}

#[test]
fn the_notice_closes_skips_or_turns_the_checks_off() {
    // Close: told again after a week.
    let s = State::new("close");
    let mut h = with_notice(&s, "99.0.0");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), None);
    assert_eq!(s.state().reminded.map(|r| r.0), Some(v("99.0.0")));
    drop(h);
    let (h, _rx) = launch(&s.config(), Some(&s), &[], FakeFetch::new(newer()));
    assert_eq!(h.overlay_kind(), None, "not again within a week");
    // Skip this version.
    let s = State::new("skip");
    let mut h = with_notice(&s, "99.0.0");
    h.key(KeyCode::Char('s'));
    assert_eq!(s.state().skipped, Some(v("99.0.0")));
    assert!(h.status(120, 24).contains("Not told about datarig 99.0.0 again"), "{}", h.status(120, 24));
    // Turn the checks off: saved in the config file, and said.
    let s = State::new("turn-off");
    let mut h = with_notice(&s, "99.0.0");
    h.key(KeyCode::Char('o'));
    assert_eq!(h.app.prefs.update_check, UpdateCheck::Off);
    let text = std::fs::read_to_string(s.0.join("config.toml")).unwrap();
    assert!(text.contains("[updates]\ncheck = \"off\""), "{text}");
    assert!(h.status(120, 24).contains("Update checks are off. :set updates.check=on turns them on"));
    // The focus moves; Enter presses the focused button.
    let s = State::new("focus");
    let mut h = with_notice(&s, "99.0.0");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Enter);
    assert_eq!(s.state().skipped, Some(v("99.0.0")));
}

#[test]
fn update_says_what_is_known_and_never_up_to_date_after_a_failure() {
    let say = |state: Option<&str>, env: &'static [(&'static str, &'static str)], off: bool| {
        let s = State::new("say");
        if let Some(t) = state {
            std::fs::write(s.file(), t).unwrap();
        }
        let mut cfg = s.config();
        if off {
            cfg.prefs.update_check = UpdateCheck::Off;
        }
        let (mut h, _rx) = launch(&cfg, Some(&s), env, FakeFetch::new(newer()));
        h.command("update");
        let shown = h.overlay_kind();
        (shown, h.status(160, 24))
    };
    let skip = &[("DATARIG_NO_UPDATE_CHECK", "1")];
    let (_, t) = say(None, &[], true);
    assert!(t.contains("Update checks are off (updates.check)"), "{t}");
    let (_, t) = say(None, skip, false);
    assert!(t.contains("Update checks are off (DATARIG_NO_UPDATE_CHECK is set)"), "{t}");
    let (_, t) = say(None, &[("CI", "1")], false);
    assert!(t.contains("Update checks are off on CI"), "{t}");
    // The fake's clock: 2025-10-09. An answer from the day before.
    let at = "last_attempt = 2025-10-08T08:00:00Z\n";
    let (_, t) = say(Some(&format!("{at}result = \"ok\"\nlatest = \"0.1.0\"\n")), &[], false);
    assert!(t.contains(&format!("datarig {} is up to date (checked ", Version::running())), "{t}");
    for (result, why) in [
        ("unreachable", "could not reach api.github.com"),
        ("timed_out", "did not answer in time"),
        ("tls", "was not trusted"),
        ("proxy", "the proxy could not be used"),
        ("rate_limited", "GitHub limits requests"),
        ("bad_answer", "could not be read"),
    ] {
        let (_, t) = say(Some(&format!("{at}result = \"{result}\"\nlatest = \"0.1.0\"\n")), &[], false);
        assert!(t.contains("Not checked: ") && t.contains(why) && !t.contains("up to date"), "{result}: {t}");
    }
    let (_, t) = say(Some(at), &[], false);
    assert!(t.contains("the last check did not finish"), "{t}");
    let (_, t) = say(Some("latest = 9\n"), &[], false);
    assert!(t.contains("Not checked: update.toml could not be read"), "{t}");
    // A newer release known: the notice, even skipped or closed.
    let (shown, _) = say(Some("latest = \"99.0.0\"\nskipped = \"99.0.0\"\n"), skip, false);
    assert_eq!(shown, Some(OverlayKind::UpdateNotice));
}

#[test]
fn the_notice_names_the_command_of_each_install() {
    for (install, tag) in [(Install::Homebrew, "brew"), (Install::Source, "source"), (Install::Other, "archive")] {
        for (w, hh) in [(80, 24), (120, 40)] {
            for summary in [true, false] {
                if !summary && (w, hh) != (80, 24) {
                    continue;
                }
                let s = State::new("look");
                let mut h = with_notice(&s, "99.0.0");
                let n = h.app.overlays.update_notice_mut().unwrap();
                (n.install, n.running) = (install, v("0.39.0"));
                if !summary {
                    n.summary = None;
                }
                let name = format!("update_notice_{tag}{}_{w}x{hh}", if summary { "" } else { "_no_summary" });
                assert_screen!(name, Lang::En, h.draw(w, hh));
            }
        }
    }
    // Korean: the frame and words from the catalog.
    let s = State::new("look-ko");
    std::fs::write(s.file(), "latest = \"99.0.0\"\n").unwrap();
    let (mut app, _clock) = new_app_with_clock(&s.config(), Lang::Ko);
    app.set_paths(Paths { data: None, state: Some(s.0.clone()) });
    app.set_env_lookup(Arc::new(|_: &str| None));
    app.updates.fetch = Some(FakeFetch::new(newer()) as Arc<dyn Fetch>);
    app.launch(Startup::Normal);
    let text = buffer_text(&{
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        term.draw(|f| datarig_tui::screens::draw(f, &mut app)).unwrap();
        term.backend().buffer().clone()
    });
    assert!(text.contains(ko(Label::UpdateNoticeTitle)), "{text}");
    let out = ko_msg(&Msg::UpdateNoticeOut { version: "99.0.0".into(), running: Version::running().to_string() });
    assert!(text.contains(&out), "{text}");
}

/// What starts the real binary to record or measure it turns the check off, so no recording or
/// benchmark ever sends a request (a release build checks otherwise, `CI` aside).
#[test]
fn every_launcher_of_the_real_binary_turns_the_check_off() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for file in ["docs/demo/render.sh", "crates/bench/src/idle.rs", "crates/tui/tests/signals.rs"] {
        let text = std::fs::read_to_string(root.join(file)).unwrap();
        assert!(text.contains("DATARIG_NO_UPDATE_CHECK"), "{file} starts datarig without DATARIG_NO_UPDATE_CHECK");
    }
    // The benchmark's app in its own process: a release build, started with an event channel.
    let apps = std::fs::read_to_string(root.join("crates/bench/src/apps.rs")).unwrap();
    assert_eq!(apps.matches(".start(tx").count(), apps.matches("updates.paused = true").count(), "apps.rs");
}

#[test]
fn update_follows_the_setting_as_it_is_now() {
    let s = State::new("follow");
    let mut cfg = s.config();
    cfg.prefs.update_check = UpdateCheck::Off;
    let (mut h, _rx) = launch(&cfg, Some(&s), &[], FakeFetch::new(newer()));
    h.command("update");
    assert!(h.status(160, 24).contains("Update checks are off (updates.check)"));
    h.command("set updates.check=on");
    h.command("update");
    let t = h.status(160, 24);
    assert!(!t.contains("are off") && t.contains("Not checked yet"), "{t}");
    h.command("set updates.check=off");
    h.command("update");
    assert!(h.status(160, 24).contains("Update checks are off (updates.check)"));
}

#[test]
fn turning_the_checks_off_says_when_the_config_could_not_be_saved() {
    let s = State::new("unsaved");
    // The config file's folder is a file: it cannot be written.
    std::fs::write(s.0.join("blocker"), "").unwrap();
    let mut cfg = s.config();
    cfg.path = Some(s.0.join("blocker").join("config.toml"));
    std::fs::write(s.file(), "latest = \"99.0.0\"\n").unwrap();
    let (mut h, _rx) = launch(&cfg, Some(&s), &[], FakeFetch::new(newer()));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::UpdateNotice));
    h.key(KeyCode::Char('o'));
    let t = h.status(200, 24);
    assert!(!t.contains("Update checks are off."), "the failure is not hidden: {t}");
}

#[test]
fn skipping_is_said_only_once_it_is_kept() {
    let s = State::new("skip-unwritable");
    let mut h = with_notice(&s, "99.0.0");
    // update.toml cannot be written now: a folder in its place.
    std::fs::remove_file(s.file()).unwrap();
    std::fs::create_dir_all(s.file()).unwrap();
    h.key(KeyCode::Char('s'));
    let t = h.status(200, 24);
    assert!(t.contains("Not skipped: update.toml could not be") && !t.contains("Not told about"), "{t}");
}

#[test]
fn a_paused_run_neither_asks_nor_tells() {
    let s = State::new("paused");
    std::fs::write(s.file(), "latest = \"99.0.0\"\n").unwrap();
    let fetch = FakeFetch::new(newer());
    let (mut app, _clock) = new_app_with_clock(&s.config(), Lang::En);
    app.set_secret_store(Arc::new(MemoryStore::new()) as Arc<dyn SecretStore>);
    app.set_env_lookup(Arc::new(|_: &str| None));
    app.set_paths(Paths { data: None, state: Some(s.0.clone()) });
    app.updates.fetch = Some(fetch.clone() as Arc<dyn Fetch>);
    app.updates.paused = true;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    {
        let _rt = RT.enter();
        app.start(tx, Startup::Normal);
    }
    app.first_frame_drawn();
    quiet(&mut rx);
    assert_eq!(fetch.calls(), 0);
    assert!(app.overlays.update_notice().is_none());
}
