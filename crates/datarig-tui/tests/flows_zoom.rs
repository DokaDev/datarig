//! The workspace's layout beyond a tab's split: `Space z` zooms the focused pane to the whole
//! workspace as tmux does (the results' maximise is their zoom), and moving the focus to a pane
//! it hides ends it; the explorer can be hidden (`Space b`) and resized (`Space <`, `Space >`, a
//! drag of its border); all of it is drawn and hit-tested from one `Layout`, and the explorer's
//! state survives a restart.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::i18n::{Label, Lang};
use datarig_core::paths::Paths;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_tui::app::pane::{EXPLORER_MIN, explorer_max};
use datarig_tui::app::{Focus, Startup};
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use std::path::PathBuf;
use std::sync::Arc;

/// Rows in the results, the editor focused, drawn at 160×45 (the tab area is 160 by 43).
fn with_results() -> Harness {
    let mut h = with_edge_results(Lang::En);
    assert_eq!(h.app.focus, Focus::Editor);
    h.draw(160, 45);
    h
}

fn zoom(h: &Harness) -> Option<Focus> {
    h.app.tab().pane.zoom
}

#[test]
fn every_pane_zooms_to_the_whole_workspace_and_back() {
    let mut h = with_results();
    let l = h.app.layout;
    assert!(l.tree.width > 0 && l.editor.height > 0 && l.results.height > 0 && l.detail.width > 0);
    assert!(!h.status(160, 45).contains("ZOOM"));
    let whole = ratatui::layout::Rect { x: 0, y: 1, width: 160, height: 43 };

    // The editor.
    h.keys(" z");
    assert_eq!(zoom(&h), Some(Focus::Editor));
    let status = h.status(160, 45);
    assert!(status.contains(" ZOOM "), "{status}");
    let l = h.app.layout;
    assert_eq!(l.editor, whole);
    assert_eq!((l.tree.width, l.results.height, l.detail.width, l.divider.height), (0, 0, 0, 0));
    assert_eq!(l.tab_bar.width, 160, "the tab bar stays, as wide as the workspace");
    h.keys(" z");
    assert_eq!(zoom(&h), None);
    assert!(!h.status(160, 45).contains("ZOOM"));

    // The explorer.
    h.key(KeyCode::BackTab);
    assert_eq!(h.app.focus, Focus::Tree);
    h.keys(" z");
    h.draw(160, 45);
    let l = h.app.layout;
    assert_eq!(l.tree, whole);
    assert_eq!((l.editor.height, l.results.height, l.explorer_edge.width), (0, 0, 0));
    h.keys(" z");

    // The results (with their inspector), by `Space z` and by their own `z`: one zoom.
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Results);
    h.keys(" z");
    assert_eq!(zoom(&h), Some(Focus::Results));
    h.draw(160, 45);
    let l = h.app.layout;
    assert_eq!((l.results.x, l.results.y, l.results.height), (0, 1, 43));
    assert_eq!(l.results.width + l.detail.width, 160);
    assert_eq!((l.tree.width, l.editor.height), (0, 0));
    h.keys("z");
    assert_eq!(zoom(&h), None, "`z` restores the zoom `Space z` made");
    h.keys(" rz");
    assert_eq!(zoom(&h), Some(Focus::Results));
    h.keys(" z");
    assert_eq!(zoom(&h), None, "`Space z` restores the maximised results");

    // The inspector, alone.
    h.draw(160, 45);
    let d = h.app.layout.detail;
    h.mouse(MouseEventKind::Down(MouseButton::Left), d.x + 5, d.y + 4);
    assert_eq!(h.app.focus, Focus::Inspector);
    h.keys(" z");
    assert_eq!(zoom(&h), Some(Focus::Inspector));
    h.draw(160, 45);
    let l = h.app.layout;
    assert_eq!(l.detail, whole);
    assert_eq!((l.results.width, l.editor.height, l.tree.width), (0, 0, 0));
    // Its grid is hidden: going back to it ends the zoom.
    h.key(KeyCode::Esc);
    assert_eq!((h.app.focus, zoom(&h)), (Focus::Results, None));
}

#[test]
fn moving_the_focus_to_a_hidden_pane_ends_the_zoom() {
    let mut h = with_results();
    h.keys(" z");
    // Tab to the results: shown again, with the focus.
    h.key(KeyCode::Tab);
    assert_eq!((h.app.focus, zoom(&h)), (Focus::Results, None));
    // F6 and Shift+Tab, the same.
    h.keys(" z");
    h.key(KeyCode::BackTab);
    assert_eq!((h.app.focus, zoom(&h)), (Focus::Editor, None));
    h.keys(" z");
    h.key(KeyCode::F(6));
    assert_eq!((h.app.focus, zoom(&h)), (Focus::Results, None));
    // A key that keeps the focus keeps the zoom: the grid moves under it.
    h.keys(" z");
    h.keys("jjl");
    assert_eq!(zoom(&h), Some(Focus::Results));
    // The zoom is the tab's: another tab has its own layout, and coming back the zoomed pane
    // has the focus again.
    h.ctrl('t');
    assert_eq!((h.app.focus, zoom(&h)), (Focus::Editor, None));
    h.key_mod(KeyCode::PageUp, KeyModifiers::CONTROL);
    assert_eq!((h.app.focus, zoom(&h)), (Focus::Results, Some(Focus::Results)));
    // The action menu and the command line offer it.
    h.command("toggle pane zoom");
    assert_eq!(zoom(&h), None);
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Editor);
    h.draw(160, 45);
    h.keys("  ");
    let labels = h.menu_labels();
    for want in ["Toggle pane zoom", "Toggle explorer"] {
        assert!(labels.iter().any(|l| l.contains(want)), "{want}: {labels:?}");
    }
    h.menu_pick("Toggle pane zoom");
    assert_eq!(zoom(&h), Some(Focus::Editor));
}

#[test]
fn a_zoom_survives_a_resize_and_the_mouse_hits_only_the_zoomed_pane() {
    let mut h = with_results();
    h.keys(" z");
    for (w, ht) in [(100, 30), (80, 24), (200, 60)] {
        h.draw(w, ht);
        let l = h.app.layout;
        assert_eq!((l.editor.x, l.editor.y, l.editor.width, l.editor.height), (0, 1, w, ht - 2), "{w}x{ht}");
        assert_eq!((l.tree.width, l.results.height), (0, 0));
    }
    assert_eq!(zoom(&h), Some(Focus::Editor));
    h.draw(160, 45);
    // Where the explorer and the results were drawn is the editor now: clicks, the wheel and
    // the right button land there and the zoom stays.
    for (x, y) in [(3, 5), (100, 40), (159, 43)] {
        h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
        h.mouse(MouseEventKind::ScrollDown, x, y);
        assert_eq!((h.app.focus, zoom(&h)), (Focus::Editor, Some(Focus::Editor)), "({x}, {y})");
    }
    h.mouse(MouseEventKind::Down(MouseButton::Right), 3, 20);
    assert_eq!(h.app.focus, Focus::Editor, "the editor's menu, not the explorer's");
    h.key(KeyCode::Esc);
    // The results zoomed: the old editor's place is the grid.
    h.key(KeyCode::Tab);
    h.keys("z");
    h.draw(160, 45);
    h.mouse(MouseEventKind::Down(MouseButton::Left), 3, 10);
    h.mouse(MouseEventKind::Up(MouseButton::Left), 3, 10);
    assert_eq!((h.app.focus, zoom(&h)), (Focus::Results, Some(Focus::Results)));
    // Nothing to drag: no divider, no explorer border.
    let before = h.app.tab().pane.share;
    h.drag((40, 1), &[(40, 20)]);
    assert_eq!((h.app.tab().pane.share, h.app.explorer.width), (before, None));
}

#[test]
fn the_zoom_marker_is_localized_and_snapshotted() {
    let mut h = with_results();
    h.keys(" z");
    insta::assert_snapshot!("editor_zoomed_en_120x34", h.draw(120, 34).backend());
    let mut k = with_edge_results(Lang::Ko);
    k.keys(" z");
    let status = k.status(120, 34);
    assert!(status.contains(&format!(" {} ", ko(Label::StatusZoom))), "{status}");
    assert!(!status.contains("ZOOM"), "{status}");
}

#[test]
fn the_explorer_hides_and_shows_again() {
    let mut h = with_results();
    h.key(KeyCode::BackTab);
    assert_eq!(h.app.focus, Focus::Tree);
    // Hidden: the focus leaves it for the editor and the tabs take the whole width.
    h.keys(" b");
    assert!(h.app.explorer.hidden);
    assert_eq!(h.app.focus, Focus::Editor);
    h.draw(160, 45);
    let l = h.app.layout;
    assert_eq!((l.tree.width, l.explorer_edge.width, l.editor.x, l.editor.width, l.tab_bar.x), (0, 0, 0, 160, 0));
    assert!(!h.status(160, 45).contains("ZOOM"), "hidden is not a zoom");
    // Pane cycling skips it; a click where it was is the editor's.
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Editor);
    h.mouse(MouseEventKind::Down(MouseButton::Left), 2, 10);
    assert_eq!(h.app.focus, Focus::Editor);
    // The width keys need it drawn.
    assert!(!(datarig_tui::app::action::by_id("explorer.wider").unwrap().when)(&h.app));
    // Shown again by its key.
    h.keys(" b");
    h.draw(160, 45);
    assert!(!h.app.explorer.hidden && h.app.layout.tree.width == 40);
    // Under a zoom of another pane it is not drawn: its key shows it, and the zoom ends.
    h.keys(" z");
    h.keys(" b");
    assert_eq!((zoom(&h), h.app.explorer.hidden), (None, false));
    h.draw(160, 45);
    assert_eq!(h.app.layout.tree.width, 40);
    // Without a tab the explorer is all there is: it comes back, with the focus.
    h.keys(" b");
    h.ctrl('w');
    assert!(h.app.tabs.is_empty());
    assert_eq!((h.app.focus, h.app.explorer.hidden), (Focus::Tree, false));
}

#[test]
fn the_explorer_resizes_by_keys_within_its_limits() {
    let mut h = with_results();
    assert_eq!(h.app.layout.tree.width, 40, "a quarter of 160 by default");
    h.keys(" >");
    assert_eq!(h.app.explorer.width, Some(44));
    h.draw(160, 45);
    assert_eq!((h.app.layout.tree.width, h.app.layout.editor.x), (44, 44));
    h.keys(" <");
    h.keys(" <");
    h.draw(160, 45);
    assert_eq!(h.app.layout.tree.width, 36);
    // Held keys repeat; the width stops at its limits.
    for _ in 0..20 {
        h.keys(" >");
        h.draw(160, 45);
    }
    assert_eq!(h.app.explorer.width, Some(explorer_max(160)));
    assert_eq!(h.app.layout.tree.width, 96);
    for _ in 0..30 {
        h.keys(" <");
        h.draw(160, 45);
    }
    assert_eq!(h.app.explorer.width, Some(EXPLORER_MIN));
    // A narrow terminal draws a wide preference narrower and keeps it.
    for _ in 0..30 {
        h.keys(" >");
        h.draw(160, 45);
    }
    h.draw(80, 24);
    assert_eq!(h.app.layout.tree.width, explorer_max(80));
    h.keys(" >");
    assert_eq!(h.app.explorer.width, Some(96), "a key at the limit does not overwrite it");
    h.draw(160, 45);
    assert_eq!(h.app.layout.tree.width, 96);
    // The command line has them too.
    h.command("narrow explorer");
    assert_eq!(h.app.explorer.width, Some(92));
}

#[test]
fn dragging_the_explorers_border_resizes_it() {
    let mut h = with_results();
    let edge = h.app.layout.explorer_edge;
    assert_eq!((edge.x, edge.width), (39, 1));
    // To column 59: the explorer ends there, 60 wide.
    h.drag((edge.x, 20), &[(45, 20), (59, 21)]);
    assert_eq!(h.app.explorer.width, Some(60));
    h.draw(160, 45);
    assert_eq!((h.app.layout.tree.width, h.app.layout.editor.x), (60, 60));
    // Beyond the limits it stops at them.
    let edge = h.app.layout.explorer_edge;
    h.drag((edge.x, 5), &[(2, 5)]);
    assert_eq!(h.app.explorer.width, Some(EXPLORER_MIN));
    h.draw(160, 45);
    let edge = h.app.layout.explorer_edge;
    h.drag((edge.x, 5), &[(150, 5)]);
    assert_eq!(h.app.explorer.width, Some(explorer_max(160)));
    // A drag that starts in the explorer does not resize it.
    h.draw(160, 45);
    h.drag((10, 8), &[(30, 8)]);
    assert_eq!(h.app.explorer.width, Some(explorer_max(160)));
}

#[test]
fn a_resized_explorer_is_snapshotted() {
    let mut h = with_results();
    h.draw(120, 34);
    for _ in 0..4 {
        h.keys(" >");
        h.draw(120, 34);
    }
    assert_eq!(h.app.layout.tree.width, 46);
    insta::assert_snapshot!("explorer_wider_en_120x34", h.draw(120, 34).backend());
}

struct State(PathBuf);

impl Drop for State {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn launch(cfg: &Config, state: &State) -> Harness {
    let (mut app, clock) = new_app_with_clock(cfg, Lang::En);
    let store = Arc::new(MemoryStore::new());
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.set_paths(Paths { data: None, state: Some(state.0.clone()) });
    let h = Harness { app, cancelled: Default::default(), driver: FakeDriver::default(), store, clock };
    let mut h = h.with_fake_driver();
    h.app.launch(Startup::Normal);
    h
}

/// The explorer's hidden state and width come back after a restart; a file of an earlier build
/// (without them, the results' `maximized` of its own) loads, and keys of a later one stay.
#[test]
fn the_explorers_layout_survives_a_restart() {
    let state = State(std::env::temp_dir().join(format!("datarig-zoom-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&state.0);
    std::fs::create_dir_all(state.0.join("consoles")).unwrap();
    std::fs::write(state.0.join("consoles").join("aaaa.sql"), "select 1").unwrap();
    std::fs::write(
        state.0.join("workspace.toml"),
        "version = 3\nactive = 0\nfuture_top = 1\n\n[explorer]\nscripts_expanded = true\nfuture = \"x\"\n\n\
         [[tabs]]\nid = \"aaaa\"\nkind = \"console\"\nresults = { share = 50, hidden = false, maximized = true }\n",
    )
    .unwrap();
    let cfg = test_db_config();
    let mut h = launch(&cfg, &state);
    assert_eq!((h.app.explorer.hidden, h.app.explorer.width), (false, None));
    assert_eq!(h.app.tabs.len(), 1);
    assert_eq!(zoom(&h), Some(Focus::Results), "an earlier build's maximised results are zoomed");
    assert_eq!(h.app.zoomed(), None, "waiting for the results, as before");
    h.draw(160, 45);
    if h.app.focus != Focus::Editor {
        h.key(KeyCode::Tab);
    }
    assert_eq!(h.app.focus, Focus::Editor);
    h.keys(" >");
    h.keys(" b");
    assert!(h.app.explorer.hidden);
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    let text = std::fs::read_to_string(state.0.join("workspace.toml")).unwrap();
    assert!(text.contains("hidden = true") && text.contains("width = 44"), "{text}");
    assert!(text.contains("future = \"x\"") && text.contains("future_top = 1"), "{text}");
    assert!(text.contains("maximized = true"), "{text}");
    drop(h);

    let mut h = launch(&cfg, &state);
    assert_eq!((h.app.explorer.hidden, h.app.explorer.width), (true, Some(44)));
    h.draw(160, 45);
    assert_eq!(h.app.layout.tree.width, 0);
    h.keys(" b");
    h.draw(160, 45);
    assert_eq!(h.app.layout.tree.width, 44);
}

/// Focus the inspector by a click and zoom it.
fn zoomed_inspector() -> Harness {
    let mut h = with_results();
    let d = h.app.layout.detail;
    h.mouse(MouseEventKind::Down(MouseButton::Left), d.x + 5, d.y + 4);
    h.mouse(MouseEventKind::Up(MouseButton::Left), d.x + 5, d.y + 4);
    h.keys(" z");
    assert_eq!((h.app.focus, zoom(&h)), (Focus::Inspector, Some(Focus::Inspector)));
    h.draw(160, 45);
    h
}

/// The action menu of a zoomed inspector (the grid's actions) keeps the zoom under it and after
/// it, and opens in the inspector's corner, not at the screen's edge.
#[test]
fn the_menu_of_a_zoomed_inspector_keeps_the_zoom() {
    let mut h = zoomed_inspector();
    h.keys("  ");
    assert!(h.app.overlays.menu().is_some());
    assert_eq!(zoom(&h), Some(Focus::Inspector), "under the menu");
    assert_eq!(h.app.overlays.menu().unwrap().at, (2, 2), "the inspector's corner");
    h.draw(160, 45);
    assert_eq!(h.app.layout.detail.width, 160, "still drawn zoomed");
    h.key(KeyCode::Esc);
    assert_eq!((h.app.focus, zoom(&h)), (Focus::Inspector, Some(Focus::Inspector)));
}

/// A tab whose explorer is zoomed does not show the explorer another tab hid: the hidden state
/// is for every tab, the zoom only draws it there.
#[test]
fn a_zoomed_explorer_in_one_tab_keeps_it_hidden_in_another() {
    let mut h = with_results();
    h.key(KeyCode::BackTab);
    h.keys(" z");
    assert_eq!(zoom(&h), Some(Focus::Tree));
    h.ctrl('t');
    h.keys(" b");
    assert!(h.app.explorer.hidden);
    h.key_mod(KeyCode::PageUp, KeyModifiers::CONTROL);
    assert_eq!((h.app.focus, zoom(&h)), (Focus::Tree, Some(Focus::Tree)), "the zoomed explorer, focused");
    h.draw(160, 45);
    assert_eq!(h.app.layout.tree.width, 160);
    h.key_mod(KeyCode::PageDown, KeyModifiers::CONTROL);
    assert!(h.app.explorer.hidden, "still hidden in the other tab");
    assert_eq!(h.app.focus, Focus::Editor, "the focus left the hidden explorer");
    // Ending the explorer's zoom leaves it hidden, the focus on the editor.
    h.key_mod(KeyCode::PageUp, KeyModifiers::CONTROL);
    h.keys(" z");
    assert_eq!((h.app.focus, h.app.explorer.hidden), (Focus::Editor, true));
}

/// A press on the explorer's border whose release was lost (let go outside the window): the
/// next press starts afresh, so a drag in the explorer does not resize it.
#[test]
fn a_lost_release_does_not_keep_a_border_drag() {
    let mut h = with_results();
    let edge = h.app.layout.explorer_edge;
    h.mouse(MouseEventKind::Down(MouseButton::Left), edge.x, 10);
    h.mouse(MouseEventKind::Down(MouseButton::Left), 5, 8);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), 20, 8);
    h.mouse(MouseEventKind::Up(MouseButton::Left), 20, 8);
    assert_eq!(h.app.explorer.width, None);
    // The results' divider, the same.
    let (divider, share) = (h.app.layout.divider, h.app.tab().pane.share);
    h.mouse(MouseEventKind::Down(MouseButton::Left), divider.x + 20, divider.y);
    let e = h.app.layout.editor_text;
    h.mouse(MouseEventKind::Down(MouseButton::Left), e.x + 2, e.y);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), e.x + 2, e.y + 3);
    assert_eq!(h.app.tab().pane.share, share);
}

/// A press on the border that changes nothing on screen keeps the preference: a wider one drawn
/// narrower on a small terminal, and none (the default, which follows the terminal's width).
#[test]
fn touching_the_border_keeps_the_explorers_preference() {
    let mut h = with_results();
    for pref in [Some(96), None] {
        h.app.explorer.width = pref;
        h.draw(80, 24);
        let e = h.app.layout.explorer_edge;
        h.drag((e.x, 5), &[(e.x, 6), (e.x, 9)]);
        assert_eq!(h.app.explorer.width, pref);
    }
}

/// A results zoom waiting for the results (an earlier build's maximise) takes the focus when
/// the run that brings them starts in the background (queued while the profile's password is
/// read, sent once the connection is up), not only at the next key: the next key never goes to
/// the hidden editor.
#[test]
fn results_arriving_for_a_waiting_zoom_take_the_focus() {
    let mut h = Harness::new(Lang::En);
    assert_eq!(h.app.focus, Focus::Editor);
    h.app.tabs.active_mut().pane.zoom = Some(Focus::Results);
    // The password not read yet (a keychain read off the UI thread): the run is queued.
    let id = h.app.profiles[0].id;
    let profile = h.app.conns.entry(id).resolved.take();
    h.ctrl('e');
    assert_eq!((h.app.tab().ran, h.app.zoomed()), (false, None), "the run waits");
    h.app.conns.entry(id).resolved = profile;
    h.db(datarig_core::driver::DbEvent::Connected);
    assert!(h.app.tab().ran, "the queued run started");
    assert_eq!(h.app.zoomed(), Some(Focus::Results));
    assert_eq!(h.app.focus, Focus::Results, "the focus is on the zoomed results");
    h.keys("i");
    assert_eq!(h.app.tab().editor.mode, datarig_tui::widgets::editor::Mode::Normal, "nothing typed into the editor");
}
