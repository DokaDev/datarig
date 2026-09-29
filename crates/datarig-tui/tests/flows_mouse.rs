//! Mouse selection: dragging, double and triple clicks in the editor.

mod common;

use common::*;
use datarig_core::config::EditorMode;
use datarig_core::driver::DbCommand;
use datarig_core::i18n::Lang;
use datarig_tui::widgets::editor::{Editor, Mode};
use ratatui::crossterm::event::{MouseButton, MouseEventKind};

/// A connected harness drawn at 100x30, the editor holding `text`; the screen position of
/// (line, column) of the editor's text.
fn editor_with(text: &str) -> (Harness, impl Fn(usize, u16) -> (u16, u16)) {
    let mut h = Harness::connected(Lang::En);
    h.app.tab_mut().editor = Editor::new(text);
    h.draw(100, 30);
    let t = h.app.layout.editor_text;
    // The gutter: line numbers (3 wide) and a blank.
    (h, move |line: usize, col: u16| (t.x + 4 + col, t.y + line as u16))
}

fn selection(h: &Harness) -> Option<String> {
    h.app.tab().editor.selection()
}

#[test]
fn dragging_selects_text_grapheme_by_grapheme() {
    let (mut h, at) = editor_with("SELECT '漢字' AS 甲, 1;\nSELECT 2;");
    h.drag(at(0, 0), &[at(0, 3), at(0, 5)]);
    assert_eq!(h.app.tab().editor.mode, Mode::Visual);
    assert_eq!(selection(&h).as_deref(), Some("SELECT"));
    // Onto the second column of a wide letter: that letter is in (never half of it).
    h.drag(at(0, 7), &[at(0, 11)]);
    assert_eq!(selection(&h).as_deref(), Some("'漢字"));
    // Backwards and across lines.
    h.drag(at(1, 5), &[at(0, 17)]);
    assert_eq!(selection(&h).as_deref(), Some("甲, 1;\nSELECT"));
    // A click without a drag is no selection.
    h.drag(at(1, 2), &[]);
    assert_eq!(h.app.tab().editor.mode, Mode::Normal);
    assert_eq!((h.app.tab().editor.row, h.app.tab().editor.col), (1, 2));
}

#[test]
fn double_click_selects_a_word_and_triple_click_the_line() {
    let (mut h, at) = editor_with("SELECT user_name, 漢字名前 FROM t;");
    let click = |h: &mut Harness, p: (u16, u16)| {
        h.mouse(MouseEventKind::Down(MouseButton::Left), p.0, p.1);
        h.mouse(MouseEventKind::Up(MouseButton::Left), p.0, p.1);
    };
    click(&mut h, at(0, 9));
    click(&mut h, at(0, 9));
    assert_eq!(selection(&h).as_deref(), Some("user_name"));
    click(&mut h, at(0, 9));
    assert_eq!(selection(&h).as_deref(), Some("SELECT user_name, 漢字名前 FROM t;"));
    // Elsewhere: a word of wide letters.
    h.key(ratatui::crossterm::event::KeyCode::Esc);
    click(&mut h, at(0, 20));
    click(&mut h, at(0, 20));
    assert_eq!(selection(&h).as_deref(), Some("漢字名前"));
}

#[test]
fn dragging_past_the_bottom_scrolls() {
    let text: String = (1..=80).map(|i| format!("SELECT {i};")).collect::<Vec<_>>().join("\n");
    let (mut h, at) = editor_with(&text);
    let bottom = h.app.layout.editor_text.y + h.app.layout.editor_text.height;
    h.mouse(MouseEventKind::Down(MouseButton::Left), at(0, 0).0, at(0, 0).1);
    for _ in 0..20 {
        h.mouse(MouseEventKind::Drag(MouseButton::Left), at(0, 3).0, bottom + 2);
        h.draw(100, 30);
    }
    h.mouse(MouseEventKind::Up(MouseButton::Left), at(0, 3).0, bottom + 2);
    let e = &h.app.tab().editor;
    assert!(e.row > e.view_height(), "the selection went below the first screen: row {}", e.row);
    assert!(e.top > 0, "the view scrolled");
    assert!(selection(&h).unwrap().starts_with("SELECT 1;\nSELECT 2;"));
}

#[test]
fn standard_mode_selects_too_and_ctrl_e_runs_the_selection() {
    let (mut h, at) = editor_with("SELECT 1;\nSELECT 2;\nSELECT 3;");
    h.app.editor_mode = EditorMode::Standard;
    h.drag(at(0, 0), &[at(1, 8)]);
    assert_eq!(selection(&h).as_deref(), Some("SELECT 1;\nSELECT 2;"));
    h.sent();
    h.ctrl('e');
    let sent = h.sent();
    assert!(
        matches!(&sent[..], [DbCommand::Execute { statements, .. }] if statements == &["SELECT 1", "SELECT 2"]),
        "{sent:?}"
    );
}

// ── the result grid ─────────────────────────────────────────────────────────

/// The edge-case users in the grid, drawn at 100x30 without the inspector (not every column
/// fits).
fn grid() -> Harness {
    let mut h = with_edge_results(Lang::En);
    h.app.detail.visible = false;
    h.draw(100, 30);
    h
}

fn cell_pos(h: &Harness, row: usize, col: usize) -> (u16, u16) {
    let g = &h.app.tab().grid;
    let (a, _, _) = *g.hit_cols.iter().find(|c| c.2 == col).expect("column shown");
    (a + 2, g.data_y + (row - g.top) as u16)
}

#[test]
fn dragging_in_the_grid_selects_a_rectangle() {
    use datarig_tui::app::Focus;
    let mut h = grid();
    let (from, to) = (cell_pos(&h, 1, 1), cell_pos(&h, 3, 2));
    h.drag(from, &[cell_pos(&h, 2, 1), to]);
    let g = &h.app.tab().grid;
    assert_eq!(h.app.focus, Focus::Results);
    assert_eq!((g.anchor, g.row, g.col), (Some((1, 1)), 3, 2), "the same model as v");
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("y");
    assert_eq!(clip.last().unwrap().lines().count(), 4, "header and three rows");
    // A click without a drag selects one cell and drops the range.
    h.drag(cell_pos(&h, 5, 0), &[]);
    let g = &h.app.tab().grid;
    assert_eq!((g.anchor, g.row, g.col), (None, 5, 0));
}

#[test]
fn dragging_past_the_grids_edges_scrolls_that_way() {
    let mut h = grid();
    let r = h.app.layout.results;
    let from = cell_pos(&h, 2, 1);
    // Past the right edge: one column beyond the last one shown (drawing then scrolls to it);
    // past the top: one row up.
    let last = h.app.tab().grid.hit_cols.last().unwrap().2;
    h.drag(from, &[(r.x + r.width + 3, from.1)]);
    assert_eq!(h.app.tab().grid.col, last + 1);
    h.draw(100, 30);
    assert!(h.app.tab().grid.left > 0, "the grid scrolled right");
    // (Another cell: a second press on the same one would be a double click.)
    let h_pos = cell_pos(&h, 3, h.app.tab().grid.hit_cols[0].2);
    h.drag(h_pos, &[(h_pos.0, r.y)]);
    let g = &h.app.tab().grid;
    // The first row is shown: one row above it is the first row (the spill test scrolls
    // down over rows on disk).
    assert_eq!((g.anchor.map(|a| a.0), g.row), (Some(3), 0));
}

#[test]
fn right_click_on_the_selection_keeps_it_for_the_menu() {
    use datarig_tui::app::overlay::OverlayKind;
    let mut h = grid();
    h.drag(cell_pos(&h, 1, 1), &[cell_pos(&h, 3, 2)]);
    let inside = cell_pos(&h, 2, 2);
    h.mouse(MouseEventKind::Down(MouseButton::Right), inside.0, inside.1);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
    let g = &h.app.tab().grid;
    assert_eq!((g.anchor, g.row, g.col), (Some((1, 1)), 3, 2), "the range is untouched");
    h.key(ratatui::crossterm::event::KeyCode::Esc);
    // Outside the range: that cell alone.
    let outside = cell_pos(&h, 6, 0);
    h.mouse(MouseEventKind::Down(MouseButton::Right), outside.0, outside.1);
    let g = &h.app.tab().grid;
    assert_eq!((g.anchor, g.row, g.col), (None, 6, 0));
}
