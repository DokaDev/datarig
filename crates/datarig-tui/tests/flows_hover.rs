//! The pointer over a menu or the keyboard help (no button pressed): the item under it is
//! selected, as the keys would, and the keys go on from there. A move anywhere else, or one
//! that stays on the selected item, changes nothing and draws no frame.

mod common;

use common::*;
use datarig_core::driver::DbCommand;
use datarig_core::i18n::Lang;
use datarig_tui::app::hover::PointerOn;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::widgets::tabbar::TabHit;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::Modifier;

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
    h.mouse(MouseEventKind::Down(MouseButton::Right), area.x + 4, area.y + 2);
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

/// A burst of moves over the panes, the status line and a menu's surroundings draws no frame
/// and changes nothing on screen (the tab bar and the result tab strip, whose targets light up
/// under the pointer, aside).
#[test]
fn moves_off_the_menus_draw_no_frame() {
    let mut h = with_edge_results(Lang::En);
    let before = h.screen(160, 45);
    let (bar, strip) = (h.app.layout.tab_bar, h.app.layout.strip);
    let mut frames = 0;
    for i in 0..2_000u16 {
        let (x, y) = (i % 160, (i / 160) % 45);
        let at = ratatui::layout::Position::new(x, y);
        if bar.contains(at) || strip.contains(at) {
            continue;
        }
        if hover(&mut h, x, y) {
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

// ── the workspace's small targets: the tab bar (the strip: flows_runs, the arrows: flows_paging) ──

/// Two console tabs, connected, drawn at 100x30; the hits of the tab bar.
fn two_tabs() -> Harness {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('t');
    assert_eq!(h.app.tabs.len(), 2);
    h.draw(100, 30);
    h
}

fn hit_x(h: &Harness, want: TabHit) -> u16 {
    h.app.tab_hits().iter().find(|t| t.2 == want).unwrap_or_else(|| panic!("{want:?}")).0
}

/// The `×` of a tab changes its text color under the pointer (no bold, reverse or underline),
/// on the background the whole tab takes under the pointer (the surface, between the bar's and
/// the active tab's); the tab's text keeps its color and is not underlined; leaving clears it;
/// moves along the same target draw nothing.
#[test]
fn the_pointer_lights_up_a_tabs_close_button() {
    let mut h = two_tabs();
    let y = h.app.layout.tab_bar.y;
    let x = hit_x(&h, TabHit::Close(0));
    let body = hit_x(&h, TabHit::Tab(0));
    let plain = h.draw(100, 30).backend().buffer()[(x, y)].clone();
    let plain_text = h.draw(100, 30).backend().buffer()[(body + 1, y)].clone();
    assert!(hover(&mut h, x, y), "onto the ×: a frame");
    assert_eq!(h.app.pointer_on, Some(PointerOn::Tab(TabHit::Close(0))));
    let t = h.draw(100, 30);
    let cell = &t.backend().buffer()[(x, y)];
    assert_eq!(cell.symbol(), "×");
    assert_eq!(cell.fg, datarig_tui::theme::DARK.error);
    assert_ne!(cell.fg, plain.fg);
    assert_eq!(cell.modifier, plain.modifier, "no modifier added");
    assert_eq!(cell.bg, datarig_tui::theme::DARK.surface, "the hovered tab's background");
    assert_ne!(plain.bg, cell.bg);
    // The tab's body: its background, its text as it was (not underlined), the × back to its
    // color.
    assert!(hover(&mut h, body, y));
    assert!(!hover(&mut h, body + 1, y), "along the same tab: no frame");
    let t = h.draw(100, 30);
    let text = &t.backend().buffer()[(body + 1, y)];
    assert!(!text.modifier.contains(Modifier::UNDERLINED));
    assert_eq!((text.symbol(), text.fg, text.modifier), (plain_text.symbol(), plain_text.fg, plain_text.modifier));
    assert_eq!(text.bg, datarig_tui::theme::DARK.surface);
    assert_eq!(t.backend().buffer()[(x, y)].fg, plain.fg);
    // Off the bar: cleared, as before.
    assert!(hover(&mut h, body, y + 5));
    assert_eq!(h.app.pointer_on, None);
    assert!(!hover(&mut h, body, y + 6));
    let t = h.draw(100, 30);
    assert!(!t.backend().buffer()[(body + 1, y)].modifier.contains(Modifier::UNDERLINED));
    assert_eq!(t.backend().buffer()[(body + 1, y)], plain_text);
}

/// On every built-in theme the hovered `×` has another text color than the plain one, on the
/// hovered tab's background, no bold, reverse or underline (the terminal theme uses an ANSI
/// color, and italics for the hovered tab).
#[test]
fn a_lit_close_button_reads_on_every_built_in_theme() {
    for (name, theme) in datarig_tui::theme::BUILTINS {
        let mut h = two_tabs();
        h.app.theme = std::sync::Arc::new((*theme).clone());
        let y = h.app.layout.tab_bar.y;
        let x = hit_x(&h, TabHit::Close(0));
        let plain = h.draw(100, 30).backend().buffer()[(x, y)].clone();
        hover(&mut h, x, y);
        let lit = h.draw(100, 30).backend().buffer()[(x, y)].clone();
        assert_ne!(lit.fg, plain.fg, "{name}: the × changes its text color");
        assert_eq!(lit.fg, theme.error, "{name}: to the error color");
        assert_ne!(lit.fg, lit.bg, "{name}: its text reads");
        assert_eq!(lit.bg, theme.surface, "{name}: the hovered tab's background");
        assert!((lit.modifier - plain.modifier).difference(Modifier::ITALIC).is_empty(), "{name}: {:?}", lit.modifier);
    }
}

/// The light goes when the bar is laid out otherwise: a tab closed (by that very ×), a tab
/// added, the screen resized.
#[test]
fn a_lit_close_button_goes_out_when_the_bar_changes() {
    let mut h = two_tabs();
    let y = h.app.layout.tab_bar.y;
    let x = hit_x(&h, TabHit::Close(0));
    hover(&mut h, x, y);
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    assert_eq!(h.app.tabs.len(), 1, "closed");
    let t = h.draw(100, 30);
    assert_eq!(h.app.pointer_on, None, "nothing lit after the close");
    let buf = t.backend().buffer();
    let sel = datarig_tui::theme::DARK.selection.bg.unwrap();
    assert!((0..100).all(|x| buf[(x, y)].bg != sel), "no cell of the bar is lit");
    // A tab added, and a resize.
    let x = hit_x(&h, TabHit::Close(0));
    hover(&mut h, x, y);
    h.ctrl('t');
    h.draw(100, 30);
    assert_eq!(h.app.pointer_on, None, "a tab added");
    let x = hit_x(&h, TabHit::Close(1));
    hover(&mut h, x, y);
    assert!(h.app.pointer_on.is_some());
    h.draw(90, 30);
    assert_eq!(h.app.pointer_on, None, "resized");
    // A dialog over the bar draws no light.
    h.draw(100, 30);
    let x = hit_x(&h, TabHit::Close(1));
    hover(&mut h, x, y);
    h.keys(":");
    let t = h.draw(100, 30);
    assert_ne!(t.backend().buffer()[(x, y)].bg, sel, "not lit under the command line");
}

/// Another tab than the active one under the pointer takes the surface's background, whole
/// (its number, title, marks and `×`): apart from the bar's background and the active tab's on
/// every built-in theme (italics where the terminal theme has one background for all), its text
/// readable on it; the active tab under the pointer stays as it is; leaving clears it.
#[test]
fn a_hovered_tab_takes_a_background_of_its_own() {
    for (name, th) in datarig_tui::theme::BUILTINS.iter().copied() {
        let mut h = two_tabs();
        h.app.theme = std::sync::Arc::new(th.clone());
        let y = h.app.layout.tab_bar.y;
        let span = |h: &Harness, i: usize| -> (u16, u16) {
            let hits: Vec<_> = h
                .app
                .tab_hits()
                .iter()
                .filter(|t| matches!(t.2, TabHit::Tab(j) | TabHit::Close(j) if j == i))
                .copied()
                .collect();
            (hits.iter().map(|t| t.0).min().unwrap(), hits.iter().map(|t| t.1).max().unwrap())
        };
        h.draw(100, 30);
        let (a, b) = span(&h, 0);
        let plain: Vec<_> = (a..b).map(|x| h.draw(100, 30).backend().buffer()[(x, y)].clone()).collect();
        let (c, d) = span(&h, 1);
        let active: Vec<_> = (c..d).map(|x| h.draw(100, 30).backend().buffer()[(x, y)].clone()).collect();
        assert!(hover(&mut h, a + 1, y), "{name}");
        let t = h.draw(100, 30);
        let buf = t.backend().buffer();
        for (i, x) in (a..b).enumerate() {
            let cell = &buf[(x, y)];
            if cell.bg == datarig_tui::theme::PROFILE_COLORS[0] {
                continue;
            }
            if th.surface == th.bg {
                assert!(cell.modifier.contains(Modifier::ITALIC), "{name}: column {x}");
            } else {
                assert_eq!(cell.bg, th.surface, "{name}: column {x}");
                assert_ne!(cell.bg, th.bg, "{name}");
                assert_ne!(cell.bg, th.surface_alt, "{name}: not the active tab's");
                assert_eq!(cell.fg, plain[i].fg, "{name}: column {x}: its text keeps its color");
            }
            assert!(!cell.modifier.contains(Modifier::UNDERLINED), "{name}");
        }
        // The active tab under the pointer: as it was.
        assert!(hover(&mut h, c + 1, y), "{name}");
        let t = h.draw(100, 30);
        let now: Vec<_> = (c..d).map(|x| t.backend().buffer()[(x, y)].clone()).collect();
        assert_eq!(now, active, "{name}");
        // Off the bar: the first tab as it was.
        hover(&mut h, a + 1, y + 6);
        let t = h.draw(100, 30);
        let now: Vec<_> = (a..b).map(|x| t.backend().buffer()[(x, y)].clone()).collect();
        assert_eq!(now, plain, "{name}");
    }
}

// ── the grid and the inspector ──

const GW: u16 = 160;
const GH: u16 = 45;

/// The edge-case users in a console's results with the inspector, the grid focused, the
/// pointer armed.
fn grid_and_inspector() -> Harness {
    let mut h = with_edge_results(Lang::En);
    h.app.focus = datarig_tui::app::Focus::Results;
    h.draw(GW, GH);
    h
}

/// Where the grid drew cell (row, col): a column inside it.
fn cell_xy(h: &Harness, row: usize, col: usize) -> (u16, u16) {
    let g = &h.app.tab().grid;
    let x = g.hit_cols.iter().find(|c| c.2 == col).unwrap_or_else(|| panic!("column {col} drawn")).0 + 2;
    (x, g.data_y + (row - g.top) as u16)
}

/// The cell of the grid at (row, col) and the inspector's entry of `col`, as drawn now.
fn cell_and_entry(h: &mut Harness, row: usize, col: usize) -> (ratatui::buffer::Cell, ratatui::buffer::Cell) {
    let t = h.draw(GW, GH);
    let (x, y) = cell_xy(h, row, col);
    let l = h.app.layout.detail_list;
    let buf = t.backend().buffer();
    (buf[(x, y)].clone(), buf[(l.x + 1, l.y + (col - h.app.detail.first) as u16)].clone())
}

/// The hover's look of a cell or an entry: the list hover's background, underlined.
fn lit(c: &ratatui::buffer::Cell) -> bool {
    c.bg == datarig_tui::theme::DARK.surface_alt && c.modifier.contains(Modifier::UNDERLINED)
}

/// A grid cell under the pointer is lit (the hover's look, never the selection), and so is the
/// inspector's entry of its column when it is in the selected row; the entry under the pointer
/// lights the grid's cell of its column in the selected row. The selection and the focus never
/// move; leaving clears both.
#[test]
fn the_grid_and_the_inspector_light_each_others_hovered_cell() {
    let mut h = grid_and_inspector();
    let (row, col) = (h.app.tab().grid.row, h.app.tab().grid.col);
    assert_eq!((row, col), (0, 0));
    let (cell, entry) = cell_and_entry(&mut h, 0, 2);
    assert!(!lit(&cell) && !lit(&entry), "nothing lit yet");
    // A cell of the selected row.
    let (x, y) = cell_xy(&h, 0, 2);
    assert!(hover(&mut h, x, y), "a frame");
    assert_eq!(h.app.pointer_on, Some(PointerOn::Cell(0, 2)));
    let (cell, entry) = cell_and_entry(&mut h, 0, 2);
    assert!(lit(&cell), "{cell:?}");
    assert!(lit(&entry), "{entry:?}");
    assert!(!hover(&mut h, x + 1, y), "along the same cell: no frame");
    // A cell of another row: lit alone.
    let (x, y) = cell_xy(&h, 2, 3);
    assert!(hover(&mut h, x, y));
    let (cell, _) = cell_and_entry(&mut h, 2, 3);
    assert!(lit(&cell));
    let (_, entry) = cell_and_entry(&mut h, 0, 3);
    assert!(!lit(&entry), "the inspector shows the selected row");
    // The selected cell: nothing lit.
    let (x, y) = cell_xy(&h, 0, 0);
    hover(&mut h, x, y);
    assert_eq!(h.app.pointer_on, None);
    // An inspector entry: it and the grid's cell of its column in the selected row.
    let l = h.app.layout.detail_list;
    assert!(hover(&mut h, l.x + 3, l.y + 1));
    assert_eq!(h.app.pointer_on, Some(PointerOn::Detail(1)));
    let (cell, entry) = cell_and_entry(&mut h, 0, 1);
    assert!(lit(&cell) && lit(&entry));
    let (cell, _) = cell_and_entry(&mut h, 1, 1);
    assert!(!lit(&cell), "only the selected row's cell");
    // The selection and the focus stayed.
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (0, 0));
    assert_eq!(h.app.focus, datarig_tui::app::Focus::Results);
    // Off both: nothing lit.
    let r = h.app.layout.results;
    assert!(hover(&mut h, r.x + 1, r.y));
    assert_eq!(h.app.pointer_on, None);
    let (cell, entry) = cell_and_entry(&mut h, 0, 1);
    assert!(!lit(&cell) && !lit(&entry));
}

/// A hovered cell goes when the grid moves under the pointer (a key scrolls it, another result).
#[test]
fn a_hovered_cell_goes_when_the_grid_moves() {
    let mut h = grid_and_inspector();
    let (x, y) = cell_xy(&h, 1, 1);
    hover(&mut h, x, y);
    assert_eq!(h.app.pointer_on, Some(PointerOn::Cell(1, 1)));
    h.app.tab_mut().grid.top = 1;
    h.draw(GW, GH);
    assert_eq!(h.app.pointer_on, None);
}

/// The selected cell and entry of a pane without the focus keep a clear look: the selection a
/// little dimmer, not the faint hover.
#[test]
fn unfocused_selection_is_the_dimmed_selection() {
    let mut h = grid_and_inspector();
    let unfocused = datarig_tui::theme::DARK.selection_unfocused().bg.unwrap();
    h.app.focus = datarig_tui::app::Focus::Editor;
    let (cell, entry) = cell_and_entry(&mut h, 0, 0);
    assert_eq!((cell.bg, entry.bg), (unfocused, unfocused));
    h.app.focus = datarig_tui::app::Focus::Results;
    let (cell, entry) = cell_and_entry(&mut h, 0, 0);
    assert_eq!((cell.bg, entry.bg), (datarig_tui::theme::DARK.selection.bg.unwrap(), unfocused));
}

/// A cell lit in one tab's grid is not lit in another tab's after a switch with the keys (the
/// pointer is over neither), nor its inspector entry.
#[test]
fn a_hovered_cell_goes_when_another_tab_comes() {
    let mut h = grid_and_inspector();
    let (x, y) = cell_xy(&h, 0, 2);
    hover(&mut h, x, y);
    assert_eq!(h.app.pointer_on, Some(PointerOn::Cell(0, 2)));
    let first = h.app.tab().id;
    h.ctrl('t');
    assert_ne!(h.app.tab().id, first);
    h.draw(GW, GH);
    assert_eq!(h.app.pointer_on, None);
}
