//! The result inspector's Row tab and the grid stay on the same column: a column chosen in the
//! grid is highlighted in the inspector (scrolled into view), `j`/`k` in the focused inspector
//! and a click on one of its entries move the grid's cell (scrolling the grid sideways), and
//! `Enter` there shows the value whole as it does in the grid.

mod common;

use common::*;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_core::i18n::Lang;
use datarig_tui::app::hover::PointerOn;
use datarig_tui::app::overlay::{ARM_DELAY, OverlayKind};
use datarig_tui::app::{DetailTab, Focus};
use datarig_tui::theme;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use ratatui::style::Color;
use std::time::Duration;

const W: u16 = 120;
const H: u16 = 30;

fn lang_tag(l: Lang) -> &'static str {
    if l == Lang::En { "en" } else { "ko" }
}

/// The edge-case users, the grid's cell moved with `keys`, then the inspector clicked (it has
/// the focus).
fn focused_inspector(keys: &str) -> Harness {
    let mut h = with_edge_results(Lang::En);
    h.key(KeyCode::Tab);
    h.keys(keys);
    h.draw(160, 45);
    let d = h.app.layout.detail;
    h.mouse(MouseEventKind::Down(MouseButton::Left), d.x + 5, d.y + 4);
    h.mouse(MouseEventKind::Up(MouseButton::Left), d.x + 5, d.y + 4);
    assert_eq!(h.app.focus, Focus::Inspector);
    h
}

/// Two rows of `n` columns `c00`, `c01`, … whose values (`row 1 value of column 00`, …) are
/// wide enough that the grid shows only a few columns at a time; the grid focused.
fn wide(lang: Lang, n: usize) -> Harness {
    let mut h = Harness::connected(lang);
    h.sent();
    h.app.run(vec!["SELECT * FROM wide".into()]);
    let sent = h.sent();
    let [DbCommand::Execute { id, .. }] = &sent[..] else { panic!("one run: {sent:?}") };
    let cols = (0..n).map(|c| meta(&format!("c{c:02}"), "text", false, false)).collect();
    let rows = (1..=2).map(|r| (0..n).map(|c| Some(format!("row {r} value of column {c:02}"))).collect()).collect();
    h.db(DbEvent::Page { id: *id, columns: Some(cols), rows, more: false, elapsed: Duration::from_millis(1) });
    h.app.focus = Focus::Results;
    h
}

/// The inspector's entry of column `col` as drawn `W`×`H`: its text and its background.
fn entry(h: &mut Harness, col: usize) -> (String, Color) {
    let t = h.draw(W, H);
    let l = h.app.layout.detail_list;
    let y = l.y + (col - h.app.detail.first) as u16;
    let buf = t.backend().buffer();
    let text = (l.x..l.x + l.width).map(|x| buf[(x, y)].symbol().to_string()).collect();
    (text, buf[(l.x + 1, y)].bg)
}

/// The selection's background, as the grid's selected cell has it.
fn selected() -> Color {
    theme::DARK.selection.bg.expect("a background")
}

/// A left press and release at (x, y).
fn click(h: &mut Harness, x: u16, y: u16) {
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

/// `Enter` in the focused inspector opens the cell viewer, as the Cell tab's last line says
/// (on both tabs: the Row tab's entries are cut to one line).
#[test]
fn enter_in_the_inspector_shows_the_value_whole() {
    let mut h = focused_inspector("$");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::CellViewer));
    assert_eq!(h.viewer().unwrap().text.lines().next(), Some("{"));
    h.key(KeyCode::Esc);
    h.keys("I");
    assert_eq!(h.app.detail.tab, DetailTab::Row);
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::CellViewer));
}

/// The grid's column is the Row tab's highlighted entry; one out of view scrolls the entries
/// just enough to show it, one still in view leaves them where they are.
#[test]
fn the_row_tab_follows_the_grids_column_and_keeps_it_in_view() {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = wide(lang, 40);
        h.keys("I");
        h.draw(W, H);
        let room = h.app.layout.detail_list.height as usize;
        assert!(room > 3 && room < 40, "the entries do not all fit: {room}");
        assert_eq!(h.app.detail.first, 0);
        let (text, bg) = entry(&mut h, 0);
        assert!(text.contains("c00: row 1 value"), "{text}");
        assert_eq!(bg, unfocused());
        // The last column: the entries scroll to show it at the bottom.
        h.keys("$");
        assert_eq!(h.app.tab().grid.col, 39);
        let (text, bg) = entry(&mut h, 39);
        assert_eq!(h.app.detail.first, 40 - room);
        assert!(text.contains("c39: row 1 value"), "{text}");
        assert_eq!(bg, unfocused());
        assert_screen!(format!("inspector_row_last_column_{}_{W}x{H}", lang_tag(lang)), lang, h.draw(W, H));
        // A column still in view: highlighted where it is, nothing scrolls.
        h.keys("hh");
        let (text, bg) = entry(&mut h, 37);
        assert_eq!(h.app.detail.first, 40 - room, "no scroll");
        assert!(text.contains("c37:"), "{text}");
        assert_eq!(bg, unfocused());
        assert_ne!(entry(&mut h, 39).1, unfocused(), "only the grid's column");
        // The first column again: back at the top; another row: its values.
        h.keys("0j");
        let (text, bg) = entry(&mut h, 0);
        assert_eq!(h.app.detail.first, 0);
        assert!(text.contains("c00: row 2 value"), "{text}");
        assert_eq!(bg, unfocused());
    }
}

/// `j`/`k` and the arrows in the focused inspector move to the next and previous column: the
/// grid's cell goes there (scrolling the grid sideways), on the same row; at the ends they stop.
#[test]
fn j_and_k_in_the_inspector_move_the_grids_column() {
    let mut h = wide(Lang::En, 40);
    h.keys("jI");
    h.draw(W, H);
    let d = h.app.layout.detail;
    click(&mut h, d.x + 3, d.y + d.height - 1);
    assert_eq!(h.app.focus, Focus::Inspector);
    let cell = |h: &Harness| (h.app.tab().grid.row, h.app.tab().grid.col);
    h.keys("j");
    assert_eq!(cell(&h), (1, 1));
    h.keys("jj");
    h.key(KeyCode::Down);
    assert_eq!(cell(&h), (1, 4));
    h.keys("k");
    h.key(KeyCode::Up);
    assert_eq!(cell(&h), (1, 2));
    assert_eq!(h.app.focus, Focus::Inspector, "the inspector keeps the focus");
    assert_eq!(entry(&mut h, 2).1, selected());
    // Past what the grid shows: it scrolls to the cell; the inspector follows too.
    h.draw(W, H);
    assert!(!h.app.tab().grid.hit_cols.iter().any(|c| c.2 == 30), "column 30 is off screen at first");
    h.keys(&"j".repeat(50));
    assert_eq!(cell(&h), (1, 39), "stops at the last column");
    h.draw(W, H);
    assert!(h.app.tab().grid.left > 0 && h.app.tab().grid.hit_cols.iter().any(|c| c.2 == 39));
    assert_eq!(entry(&mut h, 39).1, selected());
    h.keys(&"k".repeat(50));
    assert_eq!(cell(&h), (1, 0));
    h.draw(W, H);
    assert_eq!((h.app.tab().grid.left, h.app.detail.first), (0, 0));
}

/// A click on an entry (released on it) moves the grid's cell to that column of the same row,
/// the grid scrolled sideways to show it; a second click on it shows the value whole.
#[test]
fn a_click_on_an_entry_moves_the_grids_cell_and_scrolls_the_grid() {
    let mut h = wide(Lang::En, 40);
    h.keys("jI");
    h.draw(W, H);
    h.advance(ARM_DELAY);
    let l = h.app.layout.detail_list;
    let k = l.height as usize - 1;
    h.draw(W, H);
    assert!(!h.app.tab().grid.hit_cols.iter().any(|c| c.2 == k), "column {k} is off the grid at first");
    click(&mut h, l.x + 3, l.y + k as u16);
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (1, k));
    assert_eq!(h.app.focus, Focus::Inspector);
    assert!(h.viewer().is_none(), "one click selects");
    h.draw(W, H);
    assert!(h.app.tab().grid.left > 0 && h.app.tab().grid.hit_cols.iter().any(|c| c.2 == k));
    assert_eq!(entry(&mut h, k).1, selected());
    // Again on the same entry: the cell viewer, as a second click on a grid cell.
    click(&mut h, l.x + 3, l.y + k as u16);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::CellViewer));
    assert_eq!(h.viewer().unwrap().text, format!("row 2 value of column {k:02}"));
}

/// The pointer lights the entry under it (not the selected one) and a frame that scrolls the
/// entries drops it; only a release over the entry pressed picks it, and not a press right after
/// the entries appeared (as on a dialog's rows).
#[test]
fn the_pointer_lights_an_entry_and_only_a_release_on_the_pressed_one_picks_it() {
    let mut h = wide(Lang::En, 40);
    h.keys("I");
    h.draw(W, H);
    h.advance(ARM_DELAY);
    let l = h.app.layout.detail_list;
    h.mouse(MouseEventKind::Moved, l.x + 3, l.y + 2);
    assert_eq!(h.app.pointer_on, Some(PointerOn::Detail(2)));
    assert_eq!(entry(&mut h, 2).1, theme::DARK.surface_alt, "lit");
    assert_eq!(entry(&mut h, 0).1, unfocused());
    assert_eq!(entry(&mut h, 3).1, theme::DARK.bg);
    h.mouse(MouseEventKind::Moved, l.x + 3, l.y);
    assert_eq!(h.app.pointer_on, None, "the selected one is not lit");
    // Pressed on one entry, released on another or outside the inspector: nothing.
    h.mouse(MouseEventKind::Down(MouseButton::Left), l.x + 3, l.y + 2);
    h.mouse(MouseEventKind::Up(MouseButton::Left), l.x + 3, l.y + 4);
    assert_eq!(h.app.tab().grid.col, 0);
    let r = h.app.layout.results;
    h.mouse(MouseEventKind::Down(MouseButton::Left), l.x + 3, l.y + 2);
    h.mouse(MouseEventKind::Up(MouseButton::Left), r.x + 5, r.y + 5);
    assert_eq!(h.app.tab().grid.col, 0);
    // A press that lost its release (a move came first) is not picked by a later release.
    h.mouse(MouseEventKind::Down(MouseButton::Left), l.x + 3, l.y + 2);
    h.mouse(MouseEventKind::Moved, l.x + 3, l.y + 2);
    h.mouse(MouseEventKind::Up(MouseButton::Left), l.x + 3, l.y + 2);
    assert_eq!(h.app.tab().grid.col, 0);
    // The entries scroll (the grid's column moved far): the lit entry goes.
    h.mouse(MouseEventKind::Moved, l.x + 3, l.y + 2);
    assert_eq!(h.app.pointer_on, Some(PointerOn::Detail(2)));
    h.app.tab_mut().grid.col = 39;
    h.draw(W, H);
    assert_eq!(h.app.pointer_on, None);
    h.app.tab_mut().grid.col = 0;
    // Shown again (the Cell tab, then the Row tab): a press at once does not count.
    h.keys("I");
    h.draw(W, H);
    h.keys("I");
    h.draw(W, H);
    click(&mut h, l.x + 3, l.y + 3);
    assert_eq!(h.app.tab().grid.col, 0, "too soon");
    h.advance(ARM_DELAY);
    h.draw(W, H);
    click(&mut h, l.x + 3, l.y + 3);
    assert_eq!(h.app.tab().grid.col, 3);
}

/// The grid's cursor background where its pane is not focused.
fn unfocused() -> Color {
    theme::DARK.cursor_line.bg.expect("a background")
}

/// A press on an entry armed for one result is not picked by a release over the same place once
/// another result is shown (a run's rows came between).
#[test]
fn a_release_after_the_result_changed_picks_nothing() {
    let mut h = wide(Lang::En, 40);
    h.keys("I");
    h.draw(W, H);
    h.advance(ARM_DELAY);
    let l = h.app.layout.detail_list;
    h.mouse(MouseEventKind::Down(MouseButton::Left), l.x + 3, l.y + 3);
    // Another run's rows, the same columns: another result.
    h.app.run(vec!["SELECT * FROM wide".into()]);
    let id = h.app.tab().exec.query_id;
    let cols = (0..40).map(|c| meta(&format!("c{c:02}"), "text", false, false)).collect();
    let rows = vec![(0..40).map(|c| Some(format!("v{c:02}"))).collect()];
    h.db(DbEvent::Page { id, columns: Some(cols), rows, more: false, elapsed: Duration::from_millis(1) });
    h.draw(W, H);
    h.mouse(MouseEventKind::Up(MouseButton::Left), l.x + 3, l.y + 3);
    assert_eq!(h.app.tab().grid.col, 0, "not the column pressed in the result before");
}

/// A second click opens the cell viewer only when the first one picked the same entry: a first
/// press too soon after the entries appeared (ignored) makes the second one a single click.
#[test]
fn a_double_click_whose_first_press_was_ignored_only_picks() {
    let mut h = wide(Lang::En, 40);
    h.keys("I");
    h.draw(W, H);
    let l = h.app.layout.detail_list;
    click(&mut h, l.x + 3, l.y + 4);
    assert_eq!(h.app.tab().grid.col, 0, "too soon");
    h.advance(ARM_DELAY);
    h.draw(W, H);
    click(&mut h, l.x + 3, l.y + 4);
    assert_eq!(h.app.tab().grid.col, 4);
    assert!(h.viewer().is_none(), "picked, not opened");
}

/// The lit entry goes with the Row tab: the Cell tab, a hidden inspector or the Chart tab draw
/// no entries, and none is lit when they come back.
#[test]
fn the_lit_entry_goes_with_the_row_tab() {
    let mut h = wide(Lang::En, 40);
    h.keys("I");
    h.draw(W, H);
    let l = h.app.layout.detail_list;
    for keys in ["I", "i", "c"] {
        h.mouse(MouseEventKind::Moved, l.x + 3, l.y + 2);
        assert_eq!(h.app.pointer_on, Some(PointerOn::Detail(2)), "{keys}");
        h.keys(keys);
        h.draw(W, H);
        assert_eq!(h.app.pointer_on, None, "{keys}: no entries drawn");
        h.keys(keys);
        assert_eq!(entry(&mut h, 2).1, theme::DARK.bg, "{keys}: not lit again");
    }
}

/// The selected entry is as strong as the grid's selected cell: the selection while the
/// inspector has the focus, the grid's unfocused cursor while the grid has it.
#[test]
fn the_selected_entry_is_weak_while_the_grid_has_the_focus() {
    let mut h = wide(Lang::En, 40);
    h.keys("I");
    assert_eq!(h.app.focus, Focus::Results);
    assert_eq!(entry(&mut h, 0).1, unfocused());
    assert_screen!(format!("inspector_row_unfocused_en_{W}x{H}"), Lang::En, h.draw(W, H));
    let d = h.app.layout.detail;
    click(&mut h, d.x + 3, d.y + d.height - 1);
    assert_eq!(h.app.focus, Focus::Inspector);
    assert_eq!(entry(&mut h, 0).1, selected());
    // The grid's own cell the other way round.
    let t = h.draw(W, H);
    let g = &h.app.tab().grid;
    let (x, y) = (g.hit_cols[0].0 + 2, g.data_y);
    assert_eq!(Some(t.backend().buffer()[(x, y)].bg), theme::DARK.cursor_line.bg);
}
