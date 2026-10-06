//! Nerd Font icons are asked for once instead of guessed from the terminal's name (step
//! 2.7.4): `icons = auto` only turned them on in Ghostty, so over SSH and in iTerm2 the key
//! marks were text. A config that has not decided (no `icons` key, or the old default `auto`)
//! draws text marks and, in a terminal, asks once with a live preview; the answer is saved as
//! `on` or `off` in place, the rest of the file kept.

mod common;

use common::*;
use datarig_core::config::{self, Config, IconsSetting};
use datarig_core::i18n::{I18n, Label, Lang};
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::{App, Startup};
use ratatui::crossterm::event::KeyCode;
use std::path::PathBuf;
use std::sync::Arc;

fn temp_config(tag: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("datarig-icons-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("config.toml");
    std::fs::write(&p, body).unwrap();
    p
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// The binary's launch in a terminal (`ask`) or not, on the config at `path`.
fn launched(cfg: &Config, lang: Lang, ask: bool) -> Harness {
    launched_with(cfg, None, lang, ask)
}

/// [`launched`] on a config that loaded with `error`.
fn launched_with(cfg: &Config, error: Option<config::ConfigError>, lang: Lang, ask: bool) -> Harness {
    let mut app = App::new(cfg, error, lang);
    let clock = FakeClock::new();
    let c = clock.clone();
    app.set_clock(Arc::new(move || c.now()));
    app.set_ask_icons(ask);
    let store = Arc::new(MemoryStore::new());
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.launch(Startup::Normal);
    let driver = FakeDriver::default();
    Harness { app, cancelled: driver.any_cancel.clone(), driver, store, clock }
}

fn asking(app: &App) -> bool {
    app.overlays.icons_ask().is_some()
}

/// The first run without a decision: the question is on top of the new-profile form, with the
/// glyphs and No focused; Enter says no, which is saved, and the form is there below.
#[test]
fn the_first_run_asks_once_with_a_preview_and_no_focused() {
    let path = temp_config("first", "");
    let (cfg, _) = config::load(Some(path.clone()));
    let mut h = launched(&cfg, Lang::En, true);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::IconsAsk), "on top");
    assert!(!h.app.icons_on(), "text marks until answered");
    let screen = h.screen(80, 24);
    assert!(screen.contains("Do these icons show correctly?"), "{screen}");
    assert!(screen.contains(&datarig_tui::icons::preview()) && screen.contains(datarig_tui::icons::KEY_PK));
    assert!(screen.contains("[ No ]") && !screen.contains("[ Yes ]"), "No has the focus:\n{screen}");
    insta::assert_snapshot!("icons_ask_en_80x24", h.draw(80, 24).backend());
    h.key(KeyCode::Enter);
    assert!(!asking(&h.app) && !h.app.icons_on());
    assert_eq!(h.app.icons, IconsSetting::Off);
    assert!(std::fs::read_to_string(&path).unwrap().contains("icons = \"off\""));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ProfileForm), "the first-run form is there");
    // Asked once: the next launch has the answer.
    let (cfg, _) = config::load(Some(path.clone()));
    assert!(!asking(&launched(&cfg, Lang::En, true).app));
    cleanup(&path);
}

/// The old default `icons = "auto"` has not decided either: asked, and a yes replaces it in
/// place (comments and key order kept). Yes is reached with Tab or the arrows; `y` answers at
/// once, `n` says no; `Esc` asks again later (nothing saved).
#[test]
fn an_old_auto_is_asked_and_the_answer_replaces_it_in_place() {
    let body = "# mine\nicons = \"auto\" # the old default\npage_size = 200\n";
    let path = temp_config("auto", body);
    let (cfg, _) = config::load(Some(path.clone()));
    let mut h = launched(&cfg, Lang::En, true);
    assert!(asking(&h.app));
    h.key(KeyCode::Tab);
    assert!(h.screen(80, 24).contains("[ Yes ]"));
    h.key(KeyCode::Right);
    assert!(h.screen(80, 24).contains("[ No ]"), "Right goes to No");
    h.key(KeyCode::Left);
    h.key(KeyCode::Enter);
    assert!(h.app.icons_on());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# mine\nicons = \"on\" # the old default\npage_size = 200\n"), "{text}");
    // `y` answers at once and `n` says no, both saved; Esc closes the question and saves
    // nothing (text marks for now), so the next launch asks again.
    for (key, on, saved) in [
        (KeyCode::Char('y'), true, "icons = \"on\""),
        (KeyCode::Char('n'), false, "icons = \"off\""),
        (KeyCode::Esc, false, "icons = \"auto\""),
    ] {
        let path = temp_config("keys", body);
        let (cfg, _) = config::load(Some(path.clone()));
        let mut h = launched(&cfg, Lang::En, true);
        h.key(key);
        assert_eq!((asking(&h.app), h.app.icons_on()), (false, on), "{key:?}");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(saved), "{key:?}: {text}");
        let (cfg, _) = config::load(Some(path.clone()));
        assert_eq!(asking(&launched(&cfg, Lang::En, true).app), key == KeyCode::Esc, "{key:?}: asked again");
        cleanup(&path);
    }
    cleanup(&path);
}

/// A decided config is never asked; nor is a run that is not in a terminal (the tests' own
/// harnesses) — `auto` then simply draws text marks.
#[test]
fn a_decided_config_or_no_terminal_is_not_asked() {
    for (body, on) in [("icons = \"on\"\n", true), ("icons = \"off\"\n", false)] {
        let path = temp_config("decided", body);
        let (cfg, _) = config::load(Some(path.clone()));
        let h = launched(&cfg, Lang::En, true);
        assert!(!asking(&h.app), "{body}");
        assert_eq!(h.app.icons_on(), on);
        cleanup(&path);
    }
    let h = launched(&Config::default(), Lang::En, false);
    assert!(!asking(&h.app) && !h.app.icons_on());
}

/// The question in Korean (its words come from the Korean catalog), and asked again from the
/// settings: choosing `auto` there opens it on top.
#[test]
fn the_question_is_translated_and_can_be_asked_again_from_the_settings() {
    let ko = I18n::new(Lang::Ko);
    let path = temp_config("ko", "icons = \"off\"\n");
    let (cfg, _) = config::load(Some(path.clone()));
    let mut h = launched(&cfg, Lang::Ko, true);
    assert!(!asking(&h.app));
    h.command("ui.icons.auto");
    assert!(asking(&h.app));
    let screen = h.screen(100, 30);
    assert!(screen.contains(&*ko.label(Label::IconsAskQuestion)), "{screen}");
    assert!(screen.contains(&*ko.label(Label::IconsAskNo)), "{screen}");
    assert_eq!(h.app.icons, IconsSetting::Off, "unchanged until answered");
    h.key(KeyCode::Char('y'));
    assert!(std::fs::read_to_string(&path).unwrap().contains("icons = \"on\""));
    cleanup(&path);
}

/// A config file with errors is not written, so the question (whose answer could
/// not be saved) is not asked; the first launch after the file is fixed asks it.
#[test]
fn a_config_with_errors_is_not_asked_until_it_is_fixed() {
    let path = temp_config("broken", "no_such_key = 1\n");
    let (cfg, error) = config::load(Some(path.clone()));
    assert!(error.is_some(), "the file has errors");
    let h = launched_with(&cfg, error, Lang::En, true);
    assert!(!asking(&h.app), "not asked while the file has errors");
    assert!(!h.app.icons_on(), "text marks");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "no_such_key = 1\n", "the file is untouched");
    // Fixed: asked on the next launch.
    std::fs::write(&path, "").unwrap();
    let (cfg, _) = config::load(Some(path.clone()));
    assert!(asking(&launched(&cfg, Lang::En, true).app));
    cleanup(&path);
}

/// The mouse on the icons question: the pointer underlines Yes without moving the focus (Enter
/// still says no); a click on Yes says yes.
#[test]
fn the_icons_question_takes_clicks() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let path = temp_config("mouse", "");
    let (cfg, _) = config::load(Some(path.clone()));
    let mut h = launched(&cfg, Lang::En, true);
    h.draw(80, 24);
    let yes = h.app.overlays.icons_ask().unwrap().buttons.rects[0];
    h.mouse(MouseEventKind::Moved, yes.x + 1, yes.y);
    assert!(!h.app.take_idle_event());
    let q = h.app.overlays.icons_ask().unwrap();
    assert!(!q.yes_focused && q.buttons.hover == Some(0));
    h.mouse(MouseEventKind::Down(MouseButton::Left), 0, 0);
    assert!(asking(&h.app), "a click outside does nothing");
    h.mouse(MouseEventKind::Down(MouseButton::Left), yes.x + 1, yes.y);
    assert!(!asking(&h.app) && h.app.icons_on());
    assert_eq!(h.app.icons, IconsSetting::On);
    cleanup(&path);
}
