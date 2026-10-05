//! The pointer over a menu or the keyboard help (no button pressed): the item under it is
//! selected, as the keys would, and the keys go on from there. A move anywhere else, or one
//! that stays on the selected item, changes nothing and draws no frame.

mod common;

use common::*;
use datarig_core::driver::DbCommand;
use datarig_core::i18n::Lang;
use datarig_tui::app::overlay::OverlayKind;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use ratatui::layout::Rect;

/// Move the pointer to (x, y); `true` when the move needs a frame.
fn hover(h: &mut Harness, x: u16, y: u16) -> bool {
    h.mouse(MouseEventKind::Moved, x, y);
    !h.app.take_idle_event()
}

/// The explorer's menu of the connected profile, drawn at 120x30; the area of its items.
fn profile_menu() -> (Harness, Rect) {
    let mut h = Harness::connected(Lang::En);
    h.draw(120, 30);
    let area = h.app.explorer.area;
    h.mouse(MouseEventKind::Down(MouseButton::Right), area.x + 4, area.y + 1);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
    h.draw(120, 30);
    let list = h.app.overlays.menu().unwrap().list;
    (h, list)
}

fn menu_selected(h: &Harness) -> usize {
    h.app.overlays.menu().unwrap().selected
}

/// The background of the menu row at `y`, as drawn.
fn row_bg(h: &mut Harness, list: Rect, y: u16) -> ratatui::style::Color {
    let t = h.draw(120, 30);
    t.backend().buffer()[(list.x + 1, y)].bg
}

#[test]
fn the_pointer_selects_the_menu_item_under_it() {
    let (mut h, list) = profile_menu();
    assert_eq!(menu_selected(&h), 1, "the first item, under the node's heading");
    // Onto the fourth item: selected, drawn as the keys' selection.
    assert!(hover(&mut h, list.x + 2, list.y + 3), "the selection moved: a frame");
    assert_eq!(menu_selected(&h), 3);
    assert_eq!(row_bg(&mut h, list, list.y + 3), datarig_tui::theme::DARK.selection.bg.unwrap());
    assert_eq!(row_bg(&mut h, list, list.y), datarig_tui::theme::DARK.surface);
    // Along the same item, onto the border, off the menu: nothing changes, no frame.
    assert!(!hover(&mut h, list.x + 6, list.y + 3));
    assert!(!hover(&mut h, list.x + 2, list.y - 1));
    assert!(!hover(&mut h, list.x + list.width + 3, list.y + 1));
    assert!(!hover(&mut h, 0, 0));
    assert_eq!(menu_selected(&h), 3, "leaving the menu keeps the selection");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
    // The keys go on from the item the pointer chose.
    h.key(KeyCode::Down);
    assert_eq!(menu_selected(&h), 4);
    // A heading is not selected.
    assert!(!hover(&mut h, list.x + 2, list.y));
    assert_eq!(menu_selected(&h), 4);
    // Enter runs the item under the pointer: the second reloads the schemas.
    assert!(hover(&mut h, list.x + 2, list.y + 2));
    h.sent();
    h.key(KeyCode::Enter);
    assert!(h.overlay_kind().is_none());
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::LoadSchemas)));
}

#[test]
fn a_click_after_the_pointer_moved_runs_the_item_under_it() {
    let (mut h, list) = profile_menu();
    for y in list.y..list.y + 5 {
        hover(&mut h, list.x + 2, y);
    }
    assert!(hover(&mut h, list.x + 2, list.y + 3));
    assert_eq!(menu_selected(&h), 3);
    h.mouse(MouseEventKind::Down(MouseButton::Left), list.x + 2, list.y + 3);
    assert!(h.form_open(), "the third item: edit the profile");
}

/// The grid's menu with the formats of a copy scope open: the pointer selects a format; over
/// the scope's own row the formats stay, over another item they close and that item is
/// selected.
#[test]
fn the_pointer_selects_in_the_copy_formats_and_leaves_them_for_another_item() {
    let mut h = with_edge_results(Lang::En);
    h.key(KeyCode::Tab);
    h.draw(160, 45);
    let (x, y) = (h.app.tabs.active().grid.hit_cols[1].0 + 2, h.app.layout.results.y + 3 + 2);
    h.mouse(MouseEventKind::Down(MouseButton::Right), x, y);
    (0..5).for_each(|_| h.key(KeyCode::Down));
    h.key(KeyCode::Enter);
    h.draw(160, 45);
    let m = h.app.overlays.menu().unwrap();
    let (list, sub, scope) = (m.list, m.sub.as_ref().unwrap().list, m.selected);
    assert!(hover(&mut h, sub.x + 2, sub.y + 3));
    assert_eq!(h.app.overlays.menu().unwrap().sub.as_ref().unwrap().selected, 3);
    assert!(!hover(&mut h, list.x + 2, list.y + scope as u16), "its scope's row: the formats stay");
    assert!(h.app.overlays.menu().unwrap().sub.is_some());
    // The keys go on in the formats from there (a letter there is a format's key).
    h.key(KeyCode::Down);
    assert_eq!(h.app.overlays.menu().unwrap().sub.as_ref().unwrap().selected, 4);
    assert!(hover(&mut h, list.x + 2, list.y + 1));
    let m = h.app.overlays.menu().unwrap();
    assert!(m.sub.is_none() && m.selected == 1, "another item: the formats close");
}

/// The keyboard help: the pointer selects the row under it (a header too); a click still does
/// what it did; the keys go on from there.
#[test]
fn the_pointer_selects_in_the_keyboard_help() {
    let mut h = Harness::connected(Lang::En);
    h.key(KeyCode::BackTab);
    h.key(KeyCode::F(1));
    h.draw(80, 24);
    let list = h.app.overlays.help().unwrap().list;
    let selected = |h: &Harness| h.app.overlays.help().unwrap().selected;
    assert!(hover(&mut h, list.x + 2, list.y + 4));
    assert_eq!(selected(&h), 4);
    assert!(!hover(&mut h, list.x + 8, list.y + 4));
    assert!(!hover(&mut h, list.x + list.width + 1, list.y + 2), "off its rows");
    assert_eq!(selected(&h), 4);
    h.key(KeyCode::Down);
    assert_eq!(selected(&h), 5);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Help), "moving runs nothing");
}

/// A burst of moves over the panes, the tab bar, the status line and a menu's surroundings
/// draws no frame and changes nothing on screen.
#[test]
fn moves_off_the_menus_draw_no_frame() {
    let mut h = with_edge_results(Lang::En);
    let before = h.screen(160, 45);
    let mut frames = 0;
    for i in 0..2_000u16 {
        if hover(&mut h, i % 160, (i / 160) % 45) {
            frames += 1;
        }
    }
    assert_eq!(frames, 0);
    assert_eq!(h.screen(160, 45), before);
    // With a menu open: only the moves that change its selection.
    let (mut h, list) = profile_menu();
    let before = h.screen(120, 30);
    let mut frames = 0;
    for i in 0..2_000u16 {
        let (x, y) = (i % 120, (i / 120) % 30);
        if list.contains(ratatui::layout::Position::new(x, y)) {
            continue;
        }
        if hover(&mut h, x, y) {
            frames += 1;
        }
    }
    assert_eq!(frames, 0);
    assert_eq!(h.screen(120, 30), before);
    // Across the items: one frame per item entered, none along an item.
    let mut frames = 0;
    for y in list.y..list.y + list.height {
        for x in list.x..list.x + list.width {
            frames += usize::from(hover(&mut h, x, y));
        }
    }
    let m = h.app.overlays.menu().unwrap();
    let items = (0..usize::from(list.height)).filter(|i| m.item_at(*i).is_some()).count();
    assert_eq!(frames, items - 1, "the first item was selected already; headings select nothing");
}
