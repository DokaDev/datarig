//! What's new (`:whatsnew`, and once at launch after an update): which versions show, what is
//! written to `update.toml`, the keys, the wheel and the dialog's look.
mod common;
use common::*;
use datarig_core::config::{Config, WhatsNewSetting};
use datarig_core::paths::Paths;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_core::update::{self, Version, notes};
use datarig_i18n::{Label, Lang};
use datarig_tui::app::Startup;
use datarig_tui::app::overlay::{Overlay, OverlayKind};
use datarig_tui::app::whats_new::WhatsNewView;
use ratatui::crossterm::event::{KeyCode, MouseEventKind};
use std::path::PathBuf;
use std::sync::Arc;

/// Notes of three versions, fixed (the screens do not change with each release's notes).
const FIXTURE: &str = "# What's new\n\nIntro.\n\n\
## 0.39.0 (2026-11-10)\n\nThe release opens with a summary line that is long enough to wrap at 80 columns, with `code` and **bold** in it.\n\n\
### Added\n\n- `:whatsnew` shows the notes of every version this build carries, newest first; an item long enough\n  to wrap shows its next rows under its text.\n- A second item.\n\n\
### Fixed\n\n- A fix with a link to [the guide](https://example.org/guide).\n\n\
## 0.38.3 (2026-11-06)\n\nScreen text uses British spelling.\n\n### Changed\n\n- Titles use a colon in place of a dash.\n\n\
## 0.38.2 (2026-11-05)\n\n### Added\n\n- One.\n- Two.\n- Three.\n- Four.\n- Five.\n- Six.\n- Seven.\n- Eight.\n";

fn fixture() -> Vec<notes::Section> {
    notes::parse(FIXTURE).unwrap().sections
}

/// A state directory of its own, removed at the end.
struct State(PathBuf);

impl State {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("datarig-whatsnew-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        State(dir)
    }
    fn update(&self) -> PathBuf {
        self.0.join(update::FILE)
    }
    fn seen(&self) -> Option<Version> {
        update::load(&self.update()).unwrap().seen
    }
    fn write_seen(&self, v: &str) {
        std::fs::write(self.update(), format!("seen = \"{v}\"\n")).unwrap();
    }
    /// A usage file: datarig ran here before.
    fn used_before(&self) {
        std::fs::write(self.0.join(datarig_core::usage::FILE), "").unwrap();
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn no_profiles() -> Config {
    Config { connections: Vec::new(), ..Default::default() }
}

fn launch_at(cfg: &Config, state: &State) -> Harness {
    let (mut app, clock) = new_app_with_clock(cfg, Lang::En);
    app.whats_new_paused = false;
    let store = Arc::new(MemoryStore::new());
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.set_paths(Paths { data: None, state: Some(state.0.clone()) });
    let mut h = Harness { app, cancelled: Default::default(), driver: FakeDriver::default(), store, clock };
    h.app.launch(Startup::Normal);
    h
}

/// The versions the dialog shows, if it is open.
fn shown(h: &Harness) -> Option<Vec<String>> {
    h.app.overlays.whats_new().map(|v| v.sections.iter().map(|s| s.version.to_string()).collect())
}

fn running() -> Version {
    Version::running()
}

#[test]
fn a_first_start_shows_nothing_and_keeps_the_version() {
    let s = State::new("fresh");
    let h = launch_at(&no_profiles(), &s);
    assert_eq!(shown(&h), None);
    assert_eq!(s.seen(), Some(running()));
}

#[test]
fn a_user_from_before_whats_new_sees_the_running_version_once() {
    let s = State::new("before");
    s.used_before();
    let h = launch_at(&no_profiles(), &s);
    assert_eq!(shown(&h), Some(vec![running().to_string()]));
    let title = h.app.overlays.whats_new().unwrap().title.to_string();
    assert_eq!(title, format!("What's new in datarig {}", running()));
    assert_eq!(s.seen(), Some(running()));
    // Once: the next launch shows nothing.
    drop(h);
    let h = launch_at(&no_profiles(), &s);
    assert_eq!(shown(&h), None);
    // Profiles in the config count as having used it too.
    let s = State::new("profiles");
    let h = launch_at(&sample_config(None), &s);
    assert_eq!(shown(&h), Some(vec![running().to_string()]));
}

#[test]
fn an_update_shows_every_version_since_the_last_one_seen() {
    let all = datarig_tui::app::whats_new::notes();
    // The oldest two versions of the notes: the update came from the older one.
    let oldest = all.sections.last().unwrap().version;
    let s = State::new("update");
    s.write_seen(&oldest.to_string());
    let h = launch_at(&no_profiles(), &s);
    let want: Vec<String> = all.between(&oldest, &running()).iter().map(|x| x.version.to_string()).collect();
    assert!(!want.is_empty() && !want.contains(&oldest.to_string()));
    assert_eq!(shown(&h), Some(want));
    assert_eq!(s.seen(), Some(running()));
}

#[test]
fn the_same_or_an_older_version_shows_nothing_and_seen_stays() {
    for later in [running().to_string(), "99.0.0".to_string()] {
        let s = State::new("same");
        s.write_seen(&later);
        let h = launch_at(&no_profiles(), &s);
        assert_eq!(shown(&h), None, "{later}");
        assert_eq!(s.seen().unwrap().to_string(), later);
    }
}

#[test]
fn turned_off_it_shows_nothing_but_seen_moves_on() {
    let s = State::new("off");
    s.write_seen("0.1.0");
    let mut cfg = no_profiles();
    cfg.prefs.whats_new = WhatsNewSetting::Off;
    let h = launch_at(&cfg, &s);
    assert_eq!(shown(&h), None);
    assert_eq!(s.seen(), Some(running()));
}

#[test]
fn a_second_instance_and_an_unreadable_file_show_and_write_nothing() {
    let s = State::new("second");
    s.write_seen("0.1.0");
    let _lock = datarig_core::workspace::acquire(&s.0).unwrap();
    let h = launch_at(&no_profiles(), &s);
    assert!(h.app.read_only);
    assert_eq!(shown(&h), None);
    assert_eq!(s.seen(), Some(Version::parse("0.1.0").unwrap()));
    drop(h);

    let s = State::new("unreadable");
    s.used_before();
    std::fs::write(s.update(), "seen = 39\n").unwrap();
    let h = launch_at(&no_profiles(), &s);
    assert_eq!(shown(&h), None);
    assert_eq!(std::fs::read_to_string(s.update()).unwrap(), "seen = 39\n");
}

#[test]
fn whatsnew_shows_every_version_and_any_time() {
    let mut h = Harness::new(Lang::En);
    h.command("whatsnew");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::WhatsNew));
    let v = h.app.overlays.whats_new().unwrap();
    assert_eq!(v.title.to_string(), "What's new");
    assert_eq!(v.sections.len(), datarig_tui::app::whats_new::notes().sections.len());
    h.key(KeyCode::Esc);
    assert_eq!(h.overlay_kind(), None);
}

fn open_fixture(h: &mut Harness) {
    let title = h.app.i18n.label(Label::WhatsNewTitle);
    h.app.overlays.push(Overlay::WhatsNew(WhatsNewView::new(title, fixture(), None)));
}

#[test]
fn keys_scroll_by_row_page_and_version() {
    let mut h = Harness::new(Lang::En);
    open_fixture(&mut h);
    h.draw(80, 24);
    let v = |h: &Harness| h.app.overlays.whats_new().unwrap().clone();
    let (max, heads, page) = (v(&h).max_scroll, v(&h).heads.clone(), v(&h).page);
    assert!(max > 0 && heads.len() == 3 && page > 2, "{max} {heads:?} {page}");
    h.key(KeyCode::Char('j'));
    h.draw(80, 24);
    assert_eq!(v(&h).scroll, 1);
    h.key(KeyCode::Char('k'));
    h.key(KeyCode::Char('k'));
    h.draw(80, 24);
    assert_eq!(v(&h).scroll, 0);
    // The next version's heading at the top, then the one after, then back.
    h.key(KeyCode::Char(']'));
    assert_eq!(v(&h).scroll, heads[1].min(max));
    h.key(KeyCode::Char('['));
    assert_eq!(v(&h).scroll, heads[0]);
    h.key(KeyCode::Char('G'));
    assert_eq!(v(&h).scroll, max);
    h.key(KeyCode::Char('g'));
    assert_eq!(v(&h).scroll, 0);
    h.key(KeyCode::PageDown);
    assert_eq!(v(&h).scroll, page.min(max));
    h.key(KeyCode::PageUp);
    assert_eq!(v(&h).scroll, 0);
    h.ctrl('d');
    assert_eq!(v(&h).scroll, (page / 2).min(max));
    h.ctrl('u');
    assert_eq!(v(&h).scroll, 0);
    // The wheel scrolls; Close closes.
    h.mouse(MouseEventKind::ScrollDown, 40, 12);
    assert_eq!(v(&h).scroll, 1);
    h.mouse(MouseEventKind::ScrollUp, 40, 12);
    assert_eq!(v(&h).scroll, 0);
    for k in [KeyCode::Char('q'), KeyCode::Enter, KeyCode::Esc] {
        if h.overlay_kind().is_none() {
            open_fixture(&mut h);
        }
        h.key(k);
        assert_eq!(h.overlay_kind(), None, "{k:?}");
    }
}

#[test]
fn the_close_button_closes_it() {
    let mut h = Harness::new(Lang::En);
    open_fixture(&mut h);
    h.draw(80, 24);
    h.advance(std::time::Duration::from_secs(1));
    h.draw(80, 24);
    let r = h.app.overlays.whats_new().unwrap().buttons.rects[0];
    h.mouse(MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left), r.x + 1, r.y);
    h.mouse(MouseEventKind::Up(ratatui::crossterm::event::MouseButton::Left), r.x + 1, r.y);
    assert_eq!(h.overlay_kind(), None);
}

#[test]
fn the_dialog_at_80x24_and_120x40() {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = Harness::new(lang);
        open_fixture(&mut h);
        for (w, hh) in [(80, 24), (120, 40)] {
            let tag = if lang == Lang::En { "en" } else { "ko" };
            assert_screen!(format!("whats_new_{tag}_{w}x{hh}"), lang, h.draw(w, hh));
        }
        // Scrolled to the last version (as far as the end allows).
        h.draw(80, 24);
        h.key(KeyCode::Char(']'));
        h.key(KeyCode::Char(']'));
        if lang == Lang::En {
            assert_screen!("whats_new_en_80x24_last_version", lang, h.draw(80, 24));
        }
    }
}

#[test]
fn the_setting_turns_it_off() {
    let mut h = Harness::new(Lang::En);
    assert_eq!(h.app.prefs.whats_new, WhatsNewSetting::On);
    h.command("set updates.whats_new=off");
    assert_eq!(h.app.prefs.whats_new, WhatsNewSetting::Off);
    h.command("set updates.whats_new=on");
    assert_eq!(h.app.prefs.whats_new, WhatsNewSetting::On);
}

#[test]
fn the_first_steps_come_first_and_a_paused_launch_writes_nothing() {
    // Used before (a usage file) but the welcome panel's first steps are not done: they show,
    // What's new waits for a later launch.
    let s = State::new("steps");
    s.used_before();
    let (mut app, clock) = new_app_with_clock(&no_profiles(), Lang::En);
    app.whats_new_paused = false;
    app.tips.paused = false;
    let store = Arc::new(MemoryStore::new());
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.set_paths(Paths { data: None, state: Some(s.0.clone()) });
    let mut h = Harness { app, cancelled: Default::default(), driver: FakeDriver::default(), store, clock };
    h.app.launch(Startup::Normal);
    assert!(h.app.onboarding_shown());
    assert_eq!(shown(&h), None);
    assert!(!s.update().exists());

    // The other tests' launches (paused by the harness): nothing shown, nothing written.
    let s = State::new("paused");
    s.used_before();
    let (mut app, _clock) = new_app_with_clock(&no_profiles(), Lang::En);
    assert!(app.whats_new_paused);
    app.set_secret_store(Arc::new(MemoryStore::new()) as Arc<dyn SecretStore>);
    app.set_paths(Paths { data: None, state: Some(s.0.clone()) });
    app.launch(Startup::Normal);
    assert!(app.overlays.whats_new().is_none());
    assert!(!s.update().exists());
}

/// Launched with tips on (the first steps can show), at `state`.
fn launch_with_tips(cfg: &Config, state: &State) -> Harness {
    let (mut app, clock) = new_app_with_clock(cfg, Lang::En);
    app.whats_new_paused = false;
    app.tips.paused = false;
    let store = Arc::new(MemoryStore::new());
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.set_paths(Paths { data: None, state: Some(state.0.clone()) });
    let mut h = Harness { app, cancelled: Default::default(), driver: FakeDriver::default(), store, clock };
    h.app.launch(Startup::Normal);
    h
}

#[test]
fn a_new_install_that_starts_on_the_first_steps_is_not_taken_for_an_update_later() {
    // A first start with tips on: the welcome panel's first steps show, and the running version
    // is kept all the same, so this install is never taken for one from before What's new.
    let s = State::new("fresh-steps");
    let mut h = launch_with_tips(&no_profiles(), &s);
    assert!(h.app.onboarding_shown());
    assert_eq!(shown(&h), None);
    assert_eq!(s.seen(), Some(running()));
    // The first steps are put away; the next start of the same version shows nothing.
    h.app.dispatch(datarig_tui::app::action::Action::DismissOnboarding);
    drop(h);
    let h = launch_with_tips(&no_profiles(), &s);
    assert!(!h.app.onboarding_shown());
    assert_eq!(shown(&h), None);
}
