//! The settings screen through the real `App` event
//! path: `Space ,` and `:settings` open it, a change applies at once, is saved to the config
//! file with its comments, and `:set` and the screen always show the same values; a config
//! file with errors is said so.

use crate::common;

use common::*;
use datarig_core::config::{self, ClipboardSetting, CommandsPosition, DetailView, IconsSetting, MonitorInterval};
use datarig_i18n::Lang;
use datarig_tui::app::action::LangSetting;
use datarig_tui::app::overlay::OverlayKind;
use ratatui::crossterm::event::KeyCode;
use std::path::PathBuf;

fn temp_config(tag: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("datarig-settings-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("config.toml");
    std::fs::write(&p, body).unwrap();
    p
}

/// The row of the settings screen showing setting `name` (not a category of the same name), as
/// drawn at 160×45.
fn row(h: &mut Harness, name: &str) -> String {
    h.screen(160, 48).lines().find(|l| l.contains(name) && l.contains('‹')).unwrap_or_default().to_string()
}

/// Put the screen's cursor on the setting labelled `name` (moving with `j`).
fn select(h: &mut Harness, name: &str) {
    for _ in 0..40 {
        let screen = h.screen(160, 48);
        if screen.lines().any(|l| l.contains(name) && l.contains("‹")) && selected_is(h, name) {
            return;
        }
        h.keys("j");
    }
    panic!("no setting {name}");
}

fn selected_is(h: &mut Harness, name: &str) -> bool {
    let t = h.draw(160, 48);
    let buf = t.backend().buffer();
    (0..45).any(|y| {
        let line = row_text(buf, y);
        line.contains(name) && (0..160).any(|x| buf[(x, y)].bg == datarig_tui::theme::DARK.selection.bg.unwrap())
    })
}

#[test]
fn space_comma_and_the_command_open_it_and_changes_are_live_and_saved() {
    let path = temp_config("live", "# my settings\nlanguage = \"en\" # keep me\n");
    let (cfg, err) = config::load(Some(path.clone()));
    assert!(err.is_none());
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.key(KeyCode::Esc); // Normal mode
    h.keys(" ,");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Settings));
    let screen = h.screen(160, 48);
    for want in ["Display", "Editor", "Results", "Clipboard", "Language", "Secrets", "Command line position"] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
    assert!(row(&mut h, "Command line position").contains("‹ popup"));
    // The first row (icons) is selected: its description and the glyph preview with the hint.
    assert!(screen.contains("Preview: "), "{screen}");
    assert!(screen.contains("If these show as □ or ?, your terminal font lacks Nerd Font glyphs"), "{screen}");
    // l: the next value, at once and saved.
    h.keys("l");
    assert_eq!(h.app.icons, IconsSetting::On);
    assert!(std::fs::read_to_string(&path).unwrap().contains("icons = \"on\""));
    select(&mut h, "Command line position");
    h.keys("l");
    assert_eq!(h.app.prefs.commands_position, CommandsPosition::Bottom);
    assert!(row(&mut h, "Command line position").contains("‹ bottom"));
    select(&mut h, "Cell detail");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.prefs.detail_view, DetailView::Statusbar);
    select(&mut h, "Clipboard");
    h.keys("h");
    assert_eq!(h.app.prefs.clipboard, ClipboardSetting::Osc52, "h: the previous value, wrapping");
    select(&mut h, "UI language");
    h.keys("l");
    assert_eq!(h.app.lang_setting, LangSetting::Ko);
    assert!(
        h.screen(160, 48).contains(ko(datarig_i18n::Label::SettingClipboard)),
        "the screen follows the language at once"
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# my settings\nlanguage = \"ko\" # keep me\n"), "comments kept: {text}");
    for want in
        ["icons = \"on\"", "detail_view = \"statusbar\"", "clipboard = \"osc52\"", "[commands]\nposition = \"bottom\""]
    {
        assert!(text.contains(want), "{want}: {text}");
    }
    let (cfg, err) = config::load(Some(path.clone()));
    assert!(err.is_none());
    assert_eq!(cfg.prefs, h.app.prefs);
    h.key(KeyCode::Esc);
    assert!(h.overlay_kind().is_none());
    // `:set` and the screen agree.
    h.command("set clipboard=system");
    h.command("settings");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Settings));
    let clipboard = ko(datarig_i18n::Label::SettingClipboard);
    // (The editor's "yanks to the clipboard" row has the word too: the row with the value.)
    let screen = h.screen(160, 48);
    assert!(screen.lines().any(|l| l.contains(clipboard) && l.contains("‹ system")), "{screen}");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn the_monitor_interval_is_set_on_the_screen_and_with_set() {
    let path = temp_config("monitor", "");
    let (cfg, err) = config::load(Some(path.clone()));
    assert!(err.is_none());
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.key(KeyCode::Esc);
    h.command("settings");
    select(&mut h, "Server monitor interval");
    assert!(row(&mut h, "Server monitor interval").contains("‹ 2s"), "{}", row(&mut h, "Server monitor interval"));
    h.keys("l");
    assert_eq!(h.app.prefs.monitor_interval, MonitorInterval::S5);
    assert!(row(&mut h, "Server monitor interval").contains("Every 5 seconds"));
    assert!(std::fs::read_to_string(&path).unwrap().contains("[monitor]\ninterval = \"5s\""));
    h.key(KeyCode::Esc);
    h.command("set monitor.interval=60s");
    assert_eq!(h.app.prefs.monitor_interval, MonitorInterval::S60);
    h.command("set monitor.interval=2s");
    assert_eq!(h.app.prefs.monitor_interval, MonitorInterval::S2);
    assert!(!std::fs::read_to_string(&path).unwrap().contains("interval"), "2s is the default: the key goes");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_config_file_with_errors_is_said_and_nothing_is_saved() {
    let path = temp_config("broken", "clipboard = \"pbcopy\"\n");
    let (cfg, err) = config::load(Some(path.clone()));
    assert!(err.is_some());
    let mut h = Harness::launched(
        &cfg,
        Lang::En,
        std::sync::Arc::new(datarig_core::secret::MemoryStore::new()),
        datarig_tui::app::Startup::Normal,
    );
    h.app = datarig_tui::app::App::new(&cfg, err, Lang::En);
    h.app.launch(datarig_tui::app::Startup::Normal);
    h.command("settings");
    let screen = h.screen(160, 48);
    assert!(
        screen.contains("clipboard") && screen.contains("pbcopy") && screen.contains("auto, system, osc52"),
        "{screen}"
    );
    assert!(screen.contains("Changes apply now but are not saved: the config file has errors"), "{screen}");
    h.keys("l");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "clipboard = \"pbcopy\"\n", "the file is left alone");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
