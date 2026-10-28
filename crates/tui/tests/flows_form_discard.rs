//! Closing a form that has changes not saved, headless: `Esc` (or its Cancel button) asks
//! first in every form (profile, SSH tunnel, environment, default rules, Startup SQL); `Enter`
//! and `n` keep editing, `y` discards; the question's buttons take the mouse. A form left as it
//! opened, or changed back, closes at once as before.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::i18n::Lang;
use datarig_core::secret::MemoryStore;
use datarig_tui::app::explorer::RowKind;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::{Focus, Startup, TestState};
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// A scratch directory, removed when dropped.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(tag: &str) -> Scratch {
    let d = std::env::temp_dir().join(format!("datarig-discard-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Scratch(d)
}

/// The sample profiles, written to `dir` (a file the forms can save to: the explorer offers
/// its sections).
fn harness(dir: &Scratch) -> Harness {
    let mut cfg = sample_config(None);
    let path = dir.0.join("config.toml");
    cfg.path = Some(path.clone());
    datarig_core::config::save(
        &path,
        datarig_core::config::Settings {
            version: 2,
            language: "en",
            icons: Default::default(),
            theme: datarig_core::theme::DEFAULT,
            default_source: Default::default(),
            prefs: Default::default(),
        },
        Some(datarig_core::config::Profiles {
            connections: &cfg.connections,
            tunnels: &[],
            sessions: Some(&cfg.sessions),
            environments: Some(cfg.policies.environments()),
            default_rules: None,
            folders: &cfg.folders,
            last_used: None,
        }),
    )
    .unwrap();
    let (cfg, err): (Config, _) = datarig_core::config::load(Some(path));
    assert!(err.is_none(), "{err:?}");
    Harness::launched(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal).with_fake_driver()
}

fn on_row(h: &mut Harness, kind: RowKind) {
    h.app.focus = Focus::Tree;
    h.app.explorer.select_kind(kind);
}

/// The discard question is on top of the form.
fn asking(h: &mut Harness) -> bool {
    h.overlay_kind() == Some(OverlayKind::Confirm) && h.form_open() && h.screen(120, 40).contains("Discard changes?")
}

/// Click the lowest `text` on a 120x40 screen (a button, under the dialog's words), after the
/// dialog's arming delay.
fn click_on(h: &mut Harness, text: &str) {
    let screen = h.screen(120, 40);
    let (x, y) = screen
        .lines()
        .enumerate()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .find_map(|(y, row)| row.find(text).map(|i| (datarig_tui::text::width(&row[..i]) as u16 + 1, y as u16)))
        .unwrap_or_else(|| panic!("no {text:?}:\n{screen}"));
    h.advance(Duration::from_millis(500));
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

/// The profile form: a typed change makes `Esc` ask; `Enter` and `n` keep the form with the
/// change, `y` closes it and nothing is saved.
#[test]
fn esc_on_an_edited_profile_form_asks_before_discarding() {
    let dir = scratch("profile");
    let mut h = harness(&dir);
    h.explore("local-pg");
    h.keys("e");
    assert!(h.form_open());
    h.type_text("-x");
    h.key(KeyCode::Esc);
    assert!(asking(&mut h), "{}", h.screen(120, 40));
    for (w, ht) in [(80, 24), (120, 40)] {
        insta::assert_snapshot!(format!("form_discard_en_{w}x{ht}"), h.draw(w, ht).backend());
    }
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ProfileForm), "Enter keeps editing");
    assert_eq!(h.form().name.text(), "local-pg-x");
    h.key(KeyCode::Esc);
    h.key(KeyCode::Char('n'));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ProfileForm), "n keeps editing");
    h.key(KeyCode::Esc);
    assert!(asking(&mut h));
    h.key(KeyCode::Char('y'));
    assert!(!h.form_open(), "y discards");
    assert_eq!(h.overlay_kind(), None);
    assert!(h.app.profiles.iter().any(|p| p.name == "local-pg"), "nothing saved");
}

/// A form left as it opened closes at once, and so does one whose change was undone.
#[test]
fn an_unchanged_form_closes_without_asking() {
    let dir = scratch("unchanged");
    let mut h = harness(&dir);
    h.explore("local-pg");
    h.keys("e");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Esc);
    assert!(!h.form_open(), "moving around is no change");
    h.explore("local-pg");
    h.keys("e");
    h.type_text("x");
    h.key(KeyCode::Backspace);
    h.key(KeyCode::Esc);
    assert!(!h.form_open(), "changed back");
}

/// The form's Cancel button asks the same way; the question's buttons take the mouse.
#[test]
fn the_cancel_button_asks_and_the_question_takes_the_mouse() {
    let dir = scratch("mouse");
    let mut h = harness(&dir);
    h.explore("local-pg");
    h.keys("e");
    h.type_text("-x");
    click_on(&mut h, "Cancel");
    assert!(asking(&mut h), "{}", h.screen(120, 40));
    click_on(&mut h, "Keep editing");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ProfileForm));
    h.key(KeyCode::Esc);
    click_on(&mut h, "Discard");
    assert!(!h.form_open());
}

/// Every other form asks too: an SSH tunnel's, an environment's, the default rules' and a
/// Startup SQL preset's.
#[test]
fn every_form_asks_before_discarding() {
    let dir = scratch("kinds");
    let mut h = harness(&dir);
    for (row, key) in [
        (RowKind::TunnelsHeader, "n"),
        (RowKind::EnvironmentsHeader, "n"),
        (RowKind::DefaultRules, "e"),
        (RowKind::SessionsHeader, "n"),
    ] {
        on_row(&mut h, row.clone());
        h.keys(key);
        assert!(h.form_open(), "{row:?}");
        // A change where the focus is: typed into a text field, or a choice stepped with →.
        h.type_text("zz");
        h.key(KeyCode::Right);
        h.key(KeyCode::Esc);
        assert!(asking(&mut h), "{row:?}\n{}", h.screen(120, 40));
        h.key(KeyCode::Char('y'));
        assert!(!h.form_open(), "{row:?}");
    }
}

/// The Cancel button pressed while a test runs: the test stops before the question comes, so a
/// question the test would raise (a host key to trust) cannot take its place under a `y`.
#[test]
fn cancel_while_a_test_runs_stops_the_test_before_asking() {
    let dir = scratch("testing");
    let mut h = harness(&dir);
    h.explore("local-pg");
    h.keys("e");
    h.type_text("-x");
    h.ctrl('t');
    assert!(h.app.conn_test.as_ref().is_some_and(|t| t.state == TestState::Running), "a test runs");
    click_on(&mut h, "Cancel");
    assert!(asking(&mut h), "{}", h.screen(120, 40));
    assert!(h.app.conn_test.as_ref().is_some_and(|t| t.state == TestState::Cancelled), "stopped first");
}
