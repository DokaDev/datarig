//! Themes through the real `App` event path: `:set theme=` with its completion, the settings
//! screen's preview and `Esc`, theme files (with `extends`, broken, with a built-in name), an
//! unknown name, and a family following the terminal's background. Every config file is a
//! scratch one; the background is injected, never asked of a real terminal.

mod common;

use common::*;
use datarig_core::config::{self, Config};
use datarig_core::i18n::Lang;
use datarig_core::secret::MemoryStore;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::{Level, Startup};
use datarig_tui::theme::{self, Background};
use ratatui::crossterm::event::KeyCode;
use ratatui::style::Color;
use std::path::PathBuf;
use std::sync::Arc;

/// A scratch config directory with `config.toml` (`body`) and `themes/<name>.toml` files;
/// removed when dropped.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(tag: &str, body: &str, themes: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!("datarig-themes-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        std::fs::write(dir.join("config.toml"), body).unwrap();
        for (name, text) in themes {
            std::fs::write(dir.join("themes").join(format!("{name}.toml")), text).unwrap();
        }
        Scratch { dir }
    }

    fn path(&self) -> PathBuf {
        self.dir.join("config.toml")
    }

    fn config(&self) -> Config {
        let (cfg, err) = config::load(Some(self.path()));
        assert!(err.is_none(), "{err:?}");
        cfg
    }

    fn text(&self) -> String {
        std::fs::read_to_string(self.path()).unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The startup flow; without profiles the new-profile form opens, and is closed here.
fn launched(cfg: &Config) -> Harness {
    let mut h = Harness::launched(cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal);
    if h.form_open() {
        h.key(KeyCode::Esc);
    }
    assert!(h.overlay_kind().is_none());
    h
}

/// The launch notices as text.
fn notices(h: &Harness) -> Vec<(String, Level)> {
    h.app.notices.iter().map(|n| (n.render(&h.app.i18n).to_string(), n.level)).collect()
}

#[test]
fn set_theme_draws_it_saves_it_and_completes_the_names() {
    let s = Scratch::new(
        "set",
        "# mine\nlanguage = \"en\"\n",
        &[("mine", "extends = \"dark\"\n[colors]\nwarning = \"yellow\"\n")],
    );
    let mut h = Harness::with_config(&s.config(), Lang::En);
    h.key(KeyCode::Esc);
    // The completion lists the built-in names and the theme file.
    h.ctrl('k');
    h.type_text("set theme=");
    let rows: Vec<String> = h.app.command_rows().iter().map(|r| r.name.to_string()).collect();
    assert_eq!(rows.first().map(String::as_str), Some("theme=terminal"), "{rows:?}");
    assert!(rows.iter().any(|r| r == "theme=catppuccin") && rows.last().map(String::as_str) == Some("theme=mine"));
    h.type_text("nor");
    let rows: Vec<String> = h.app.command_rows().iter().map(|r| r.name.to_string()).collect();
    assert_eq!(rows, ["theme=nord"]);
    h.key(KeyCode::Enter);
    assert!(h.overlay_kind().is_none());
    assert_eq!(*h.app.theme, theme::NORD);
    assert_eq!(h.app.theme_name, "nord");
    assert_eq!(s.text(), "# mine\nlanguage = \"en\"\ntheme = \"nord\"\n", "saved, comments kept");
    let frame = h.draw(80, 24);
    assert!(frame.backend().buffer().content().iter().any(|c| c.bg == theme::NORD.bg), "drawn with it");
    // A theme file, typed in full.
    h.command("set theme=mine");
    assert_eq!(h.app.theme.warning, Color::Yellow);
    assert_eq!(h.app.theme.bg, theme::DARK.bg, "the rest is dark's");
    assert!(s.text().contains("theme = \"mine\""));
    // Back to the default: the key goes.
    h.command("set theme=terminal");
    assert_eq!(*h.app.theme, theme::TERMINAL);
    assert_eq!(s.text(), "# mine\nlanguage = \"en\"\n");
}

#[test]
fn set_theme_with_an_unknown_name_or_a_broken_file_changes_nothing_and_says_why() {
    let s = Scratch::new("set-bad", "theme = \"dark\"\n", &[("broken", "[colors]\nfg = \"#12345\"\n")]);
    let mut h = Harness::with_config(&s.config(), Lang::En);
    h.key(KeyCode::Esc);
    h.command("set theme=nope");
    let err = h.cmdline().and_then(|c| c.error.clone()).expect("the command line says why");
    let text = err.render(&h.app.i18n).to_string();
    assert!(text.contains("No theme named “nope”") && text.contains("nope.toml"), "{text}");
    h.key(KeyCode::Esc);
    h.command("set theme=broken");
    let text = h.cmdline().and_then(|c| c.error.clone()).unwrap().render(&h.app.i18n).to_string();
    assert!(text.contains("broken.toml:2: “#12345” is not a color"), "{text}");
    assert_eq!(*h.app.theme, theme::DARK, "nothing changed");
    assert_eq!(s.text(), "theme = \"dark\"\n", "nothing saved");
}

#[test]
fn the_settings_screen_previews_themes_and_esc_goes_back() {
    let s = Scratch::new("preview", "theme = \"dark\"\n", &[("zz-broken", "extends = \"nope\"\n")]);
    let mut h = Harness::with_config(&s.config(), Lang::En);
    h.key(KeyCode::Esc);
    h.command("settings");
    h.keys("jj");
    assert!(h.screen(160, 45).contains("Terminal background: not detected"), "{}", h.screen(160, 45));
    // l: the next theme is drawn, not saved.
    h.keys("l");
    assert_eq!(*h.app.theme, theme::LIGHT);
    assert_eq!(h.app.theme_name, "dark");
    let screen = h.screen(160, 45);
    assert!(
        screen.contains("‹ light") && screen.contains("Previewing light: Enter keeps it, Esc goes back to dark"),
        "{screen}"
    );
    h.keys("l");
    assert_eq!(*h.app.theme, theme::HIGH_CONTRAST);
    assert_eq!(s.text(), "theme = \"dark\"\n", "a preview is not saved");
    // Esc: the theme before the preview comes back.
    h.key(KeyCode::Esc);
    assert!(h.overlay_kind().is_none());
    assert_eq!(*h.app.theme, theme::DARK);
    // Again, and Enter keeps it (saved like every setting).
    h.command("settings");
    h.keys("jjhh");
    assert_eq!(h.app.theme_name, "dark");
    // h from dark wraps past terminal to the last name: the broken file.
    assert!(h.screen(160, 45).contains("‹ zz-broken"));
    let preview = h.app.overlays.settings().and_then(|s| s.themes.preview.as_ref()).expect("a preview");
    let error = preview.error.as_ref().expect("why it cannot be used").render(&h.app.i18n).to_string();
    assert!(error.ends_with("zz-broken.toml:1: extends = “nope”: not a built-in theme"), "{error}");
    assert_eq!(*h.app.theme, theme::DARK, "a broken theme is not drawn");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.theme_name, "dark", "nor kept");
    h.keys("h");
    h.key(KeyCode::Enter);
    assert_eq!(*h.app.theme, theme::DRACULA);
    assert_eq!(h.app.theme_name, "dracula");
    assert_eq!(s.text(), "theme = \"dracula\"\n");
    assert!(h.screen(160, 45).contains("‹ dracula"));
    h.key(KeyCode::Esc);
    assert_eq!(*h.app.theme, theme::DRACULA, "Esc after Enter keeps the kept theme");
    assert_eq!(h.overlay_kind(), None::<OverlayKind>);
}

#[test]
fn a_broken_theme_file_at_launch_draws_the_terminal_theme_and_says_where() {
    let s = Scratch::new(
        "broken",
        "theme = \"mine\"\n",
        &[("mine", "extends = \"dark\"\n\n[styles]\nselection = { bg = \"#223344\", modifiers = [\"blinking\"] }\n")],
    );
    let mut h = launched(&s.config());
    assert_eq!(*h.app.theme, theme::TERMINAL, "never a silent fallback: terminal, and said");
    let n = notices(&h);
    let (text, level) = n.iter().find(|(t, _)| t.starts_with("Theme not used")).expect("a notice").clone();
    assert_eq!(level, Level::Error);
    assert!(text.contains("mine.toml:4: “blinking” is not a modifier"), "{text}");
    assert!(text.ends_with("Drawing with the terminal theme."), "{text}");
    // The settings screen says it too, and the row says the theme is not used.
    h.command("settings");
    let screen = h.screen(160, 45);
    assert!(screen.contains("Theme not used:") && screen.contains("mine.toml:4"), "{screen}");
    assert!(screen.contains("‹ mine") && screen.contains("Not used: drawing with the"), "{screen}");
    // Saving another setting keeps the configured name.
    h.keys("l");
    let text = s.text();
    assert!(text.contains("theme = \"mine\"") && text.contains("icons = \"on\""), "{text}");
}

#[test]
fn an_unknown_theme_name_or_a_file_named_like_a_builtin_is_an_error_at_launch() {
    let s = Scratch::new("unknown", "theme = \"solarized\"\n", &[]);
    let h = launched(&s.config());
    assert_eq!(*h.app.theme, theme::TERMINAL);
    let n = notices(&h);
    assert!(
        n.iter().any(|(t, l)| *l == Level::Error
            && t.contains("No theme named “solarized”")
            && t.contains("solarized.toml")
            && t.contains("Drawing with the terminal theme")),
        "{n:?}"
    );
    let s = Scratch::new("shadow", "theme = \"nord\"\n", &[("nord", "[colors]\nbg = \"#000000\"\n")]);
    let h = launched(&s.config());
    assert_eq!(*h.app.theme, theme::TERMINAL);
    assert!(
        notices(&h).iter().any(|(t, _)| t.contains("nord.toml has the name of a built-in theme")),
        "{:?}",
        notices(&h)
    );
    // The default theme draws without a word.
    let s = Scratch::new("default", "", &[]);
    let h = launched(&s.config());
    assert!(!notices(&h).iter().any(|(t, _)| t.contains("Theme")), "{:?}", notices(&h));
}

#[test]
fn a_family_follows_the_terminal_background_and_a_variant_pins_it() {
    let s = Scratch::new("family", "version = 2\ntheme = \"catppuccin\"\n", &[]);
    let mut h = launched(&s.config());
    assert_eq!(*h.app.theme, theme::CATPPUCCIN_MOCHA, "not asked yet: dark");
    h.app.set_background(Background::Light);
    assert_eq!(*h.app.theme, theme::CATPPUCCIN_LATTE);
    h.app.set_background(Background::Dark);
    assert_eq!(*h.app.theme, theme::CATPPUCCIN_MOCHA);
    h.app.set_background(Background::Unknown);
    assert_eq!(*h.app.theme, theme::CATPPUCCIN_MOCHA, "no answer: dark");
    h.app.set_background(Background::Light);
    h.command("settings");
    h.keys("jj");
    assert!(h.screen(160, 45).contains("Terminal background: light (detected)"));
    h.key(KeyCode::Esc);
    for (name, want) in [("tokyo-night", &theme::TOKYO_NIGHT_DAY), ("gruvbox", &theme::GRUVBOX_LIGHT)] {
        h.command(&format!("set theme={name}"));
        assert_eq!(*h.app.theme, *want, "{name}");
    }
    // A variant's name pins it whatever the background.
    h.command("set theme=gruvbox-dark");
    assert_eq!(*h.app.theme, theme::GRUVBOX_DARK);
    h.app.set_background(Background::Dark);
    h.command("set theme=tokyo-night-day");
    assert_eq!(*h.app.theme, theme::TOKYO_NIGHT_DAY);
    assert_eq!(s.text(), "version = 2\ntheme = \"tokyo-night-day\"\n", "the name is saved, not the variant it picked");
}
