//! The result grid scrolled sideways by character: a sideways wheel (or Shift+wheel) moves the
//! view four characters an event, a burst of them in one frame; the columns at the view's edges
//! show their part on screen (a wide character cut there is a blank); keys and clicks bring the
//! selected column into view moving the view as little as it takes; hit tests, the search's
//! highlight and the copy flash cover the part on screen only.

mod common;

use common::*;
use datarig_core::driver::{DbEvent, Outcome};
use datarig_core::i18n::Lang;
use datarig_tui::app::Focus;
use datarig_tui::input::burst;
use datarig_tui::widgets::editor::Editor;
use datarig_tui::widgets::grid::GridState;
use ratatui::crossterm::event::{Event, KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::time::Duration;

const W: u16 = 80;
const H: u16 = 24;

/// `n` rows of `cols` text columns named `column_00`… (each `"│ " + 9 + " "`: 12 characters),
/// the values `r{row}c{col}`; the inspector hidden, the grid focused and drawn at `W`×`H`.
fn wide(cols: usize, n: usize) -> Harness {
    let mut h = Harness::connected(Lang::En);
    h.app.detail.visible = false;
    let metas = (0..cols).map(|i| meta(&format!("column_{i:02}"), "text", false, false)).collect();
    let rows = (0..n).map(|r| (0..cols).map(|c| Some(format!("r{r}c{c}"))).collect()).collect();
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::Page { id, columns: Some(metas), rows, more: false, elapsed: Duration::from_millis(2) });
    h.sent();
    h.app.focus = Focus::Results;
    h.draw(W, H);
    h
}

fn grid(h: &Harness) -> &GridState {
    &h.app.tab().grid
}

/// A point in the middle of the results pane.
fn middle(h: &Harness) -> (u16, u16) {
    let r = h.app.layout.results;
    (r.x + r.width / 2, r.y + r.height / 2)
}

fn wheel(h: &mut Harness, kind: MouseEventKind, modifiers: KeyModifiers) {
    let (x, y) = middle(h);
    h.mouse_mod(kind, x, y, modifiers);
}

/// The text of screen row `y` as drawn at `W`×`H`.
fn line(h: &mut Harness, y: u16) -> String {
    let t = h.draw(W, H);
    row_text(t.backend().buffer(), y)
}

/// What the grid's view shows on screen row `y` (right of the row numbers), drawn at `W`×`H`.
fn shown(h: &mut Harness, y: u16) -> String {
    let t = h.draw(W, H);
    let (g, buf) = (grid(h), t.backend().buffer());
    (g.gutter_x.1..g.gutter_x.1 + g.view_w).map(|x| buf[(x, y)].symbol().to_string()).collect()
}

#[test]
fn the_sideways_wheel_and_shift_wheel_move_the_view_four_characters() {
    let mut h = wide(30, 20);
    assert_eq!(grid(&h).x_off, 0);
    wheel(&mut h, MouseEventKind::ScrollRight, KeyModifiers::NONE);
    assert_eq!(grid(&h).x_off, 4);
    wheel(&mut h, MouseEventKind::ScrollDown, KeyModifiers::SHIFT);
    assert_eq!(grid(&h).x_off, 8, "Shift+wheel down: right");
    wheel(&mut h, MouseEventKind::ScrollUp, KeyModifiers::SHIFT);
    assert_eq!(grid(&h).x_off, 4, "Shift+wheel up: left");
    h.draw(W, H);
    let g = grid(&h);
    assert_eq!((g.x_off, g.row, g.col), (4, 0, 0), "the view stays where the wheel put it, the cell too");
    // The first column shows its last characters: "│ co" is under the left edge.
    let y = grid(&h).data_y;
    let row = shown(&mut h, y);
    assert!(row.starts_with("c0      │ r0c1"), "{row}");
    // Clamped at both ends: never left of the first column, never past the last one's end.
    for _ in 0..3 {
        wheel(&mut h, MouseEventKind::ScrollLeft, KeyModifiers::NONE);
    }
    assert_eq!(grid(&h).x_off, 0);
    for _ in 0..200 {
        wheel(&mut h, MouseEventKind::ScrollRight, KeyModifiers::NONE);
    }
    h.draw(W, H);
    let g = grid(&h);
    assert_eq!(g.x_off, g.x_max());
    assert_eq!(g.x_off, 30 * 12 - g.view_w as usize, "the last column ends at the view's right edge");
    assert!(whole_on_screen(g, 29));
    let y = grid(&h).header_y;
    let header = line(&mut h, y);
    assert!(header.contains("│ column_29 │"), "the last column whole, no empty band after it: {header}");
    // Plain wheel still scrolls rows.
    wheel(&mut h, MouseEventKind::ScrollDown, KeyModifiers::NONE);
    assert_eq!((grid(&h).top, grid(&h).x_off), (3, grid(&h).x_max()));
}

/// A trackpad sends dozens of sideways events a swipe: the event loop handles those of one
/// burst together and draws once (bounded), and the view moves by all of them.
#[test]
fn a_burst_of_wheel_events_is_one_frame() {
    let mut h = wide(30, 20);
    let (x, y) = middle(&h);
    let ev = |kind, modifiers| Event::Mouse(MouseEvent { kind, column: x, row: y, modifiers });
    let swipe: Vec<_> = std::iter::repeat_n(ev(MouseEventKind::ScrollRight, KeyModifiers::NONE), 10).collect();
    assert_eq!(burst::frames(&swipe), 1);
    for e in swipe {
        h.event(e);
    }
    h.draw(W, H);
    assert_eq!(grid(&h).x_off, 40, "ten events, four characters each, one frame");
    // Shift+wheel bursts too; a key between two bursts makes them two frames.
    let shift = ev(MouseEventKind::ScrollUp, KeyModifiers::SHIFT);
    let key = Event::Key(ratatui::crossterm::event::KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    let mixed = [shift.clone(), shift.clone(), key, shift.clone()];
    assert_eq!(burst::frames(&mixed), 3);
    let long: Vec<_> = std::iter::repeat_n(shift, burst::PER_FRAME + 1).collect();
    assert_eq!(burst::frames(&long), 2, "bounded");
}

/// `l` at the right edge scrolls just enough to show the next column whole (never a jump of
/// several columns); `h` back moves nothing until the cursor's column is cut on the left, then
/// just enough to show it whole.
#[test]
fn h_and_l_bring_the_column_into_view_moving_as_little_as_it_takes() {
    let mut h = wide(30, 20);
    let view = grid(&h).view_w as usize;
    let mut was = 0;
    for _ in 0..20 {
        h.keys("l");
        h.draw(W, H);
        let g = grid(&h);
        assert!(whole_on_screen(g, g.col), "column {} whole", g.col);
        let end = g.col_x[g.col + 1];
        let want = end.saturating_sub(view);
        assert_eq!(g.x_off, want.max(was), "column {}: just far enough", g.col);
        assert!(g.x_off - was <= 12, "never more than one column's width at a time");
        was = g.x_off;
    }
    assert_eq!(grid(&h).col, 20);
    assert!(was > 0);
    // Back: the view stays while the cursor's column is whole on screen.
    let first_whole = (0..30).find(|&c| whole_on_screen(grid(&h), c)).unwrap();
    for _ in 0..(20 - first_whole) {
        h.keys("h");
        h.draw(W, H);
        assert_eq!(grid(&h).x_off, was, "column {} is whole: the view stays", grid(&h).col);
    }
    h.keys("h");
    h.draw(W, H);
    let g = grid(&h);
    assert_eq!(g.col, first_whole - 1);
    assert_eq!(g.x_off, g.col_x[g.col], "the column cut on the left shows from its start");
    // `0` and `$` follow the same rule.
    h.keys("$");
    h.draw(W, H);
    assert_eq!(grid(&h).x_off, grid(&h).x_max());
    h.keys("0");
    h.draw(W, H);
    assert_eq!(grid(&h).x_off, 0);
}

/// A click on a column cut by an edge selects its cell, and the view moves just enough to show
/// it whole (a header click selects the whole column the same way).
#[test]
fn a_click_on_a_partly_shown_cell_selects_it_and_shows_it_whole() {
    let mut h = wide(30, 20);
    // The column cut by the right edge.
    let g = grid(&h);
    let &(a, _, c) = g.hit_cols.iter().find(|h| !whole_on_screen(g, h.2)).expect("a column cut on the right");
    let y = g.data_y + 2;
    h.mouse(MouseEventKind::Down(MouseButton::Left), a + 2, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), a + 2, y);
    h.draw(W, H);
    let g = grid(&h);
    assert_eq!((g.row, g.col), (2, c));
    assert!(whole_on_screen(g, c));
    assert_eq!(g.x_off, g.col_x[c + 1] - g.view_w as usize, "just far enough");
    // Now the first column shown is cut on the left: a click on its part selects it.
    let first = g.hit_cols[0];
    assert!(!whole_on_screen(g, first.2));
    assert_eq!(first.0, g.gutter_x.1, "its part starts after the row numbers");
    h.mouse(MouseEventKind::Down(MouseButton::Left), first.0, y + 1);
    h.mouse(MouseEventKind::Up(MouseButton::Left), first.0, y + 1);
    h.draw(W, H);
    let g = grid(&h);
    assert_eq!((g.row, g.col), (3, first.2));
    assert_eq!(g.x_off, g.col_x[first.2], "shown from its start");
    // A header cut on the right: the whole column, shown whole.
    let g = grid(&h);
    let &(a, _, c) = g.hit_cols.iter().rev().find(|h| !whole_on_screen(g, h.2)).unwrap();
    let hy = g.header_y;
    h.mouse(MouseEventKind::Down(MouseButton::Left), a + 1, hy);
    h.mouse(MouseEventKind::Up(MouseButton::Left), a + 1, hy);
    h.draw(W, H);
    let g = grid(&h);
    assert_eq!((g.col, g.shape), (c, datarig_tui::widgets::grid::Shape::Cols));
    assert!(whole_on_screen(g, c));
    // The row numbers are not a cell of a column cut under them.
    let gutter = (g.gutter_x.1 - 1, g.data_y);
    let total = 20;
    assert_eq!(g.cell_at(gutter.0, gutter.1, total), None);
}

/// The search's highlight and the copy flash paint the part of a cell on screen, never the
/// row numbers left of it.
#[test]
fn the_highlight_and_the_copy_flash_cover_the_part_on_screen() {
    let theme = datarig_tui::theme::DARK;
    let mut h = wide(30, 20);
    h.keys("/");
    h.type_text("r2c0");
    h.key(KeyCode::Enter);
    // Then the view moved by the wheel: column 0 is cut on the left.
    wheel(&mut h, MouseEventKind::ScrollRight, KeyModifiers::NONE);
    let t = h.draw(W, H);
    let buf = t.backend().buffer();
    let g = grid(&h);
    assert_eq!(g.cut_col, Some(0));
    let cell = g.cell_rect(2, 0, 20).expect("its part on screen");
    assert_eq!(cell.x, g.gutter_x.1, "from the row numbers' end");
    assert_eq!(cell.width, 12 - 4, "the part on screen: its last eight characters");
    let bg = |x: u16| buf[(x, cell.y)].bg;
    assert_eq!(bg(cell.x), theme.grid_match_current.bg.unwrap());
    assert_eq!(bg(cell.x + cell.width - 1), theme.grid_match_current.bg.unwrap());
    assert_ne!(bg(cell.x - 1), theme.grid_match_current.bg.unwrap(), "the row numbers stay");
    assert_ne!(bg(cell.x + cell.width), theme.grid_match_current.bg.unwrap(), "the next cell stays");
    h.key(KeyCode::Esc);
    // A copy of the row: its cells flash, the one cut on the left on its part only.
    let _clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("Y");
    wheel(&mut h, MouseEventKind::ScrollRight, KeyModifiers::NONE);
    let t = h.draw(W, H);
    let buf = t.backend().buffer();
    let g = grid(&h);
    let row = g.row;
    let cell = g.cell_rect(row, 0, 20).expect("its part on screen");
    assert_eq!((cell.x, cell.width as usize), (g.gutter_x.1, 12 - g.x_off));
    let bg = |x: u16| buf[(x, cell.y)].bg;
    assert_eq!(Some(bg(cell.x)), theme.yank_flash.bg);
    assert_ne!(Some(bg(cell.x - 1)), theme.yank_flash.bg, "the row numbers do not flash");
    // The column cut on the right flashes up to the view's edge.
    let last = *g.hit_cols.last().unwrap();
    let cell = g.cell_rect(row, last.2, 20).unwrap();
    assert_eq!(cell.x + cell.width, g.gutter_x.1 + g.view_w);
    assert_eq!(Some(buf[(cell.x + cell.width - 1, cell.y)].bg), theme.yank_flash.bg);
}

/// `Esc` in the search prompt puts the view back where it was, scrolled sideways too.
#[test]
fn esc_in_the_search_prompt_restores_the_view() {
    let mut h = wide(30, 20);
    for _ in 0..2 {
        wheel(&mut h, MouseEventKind::ScrollRight, KeyModifiers::NONE);
    }
    h.draw(W, H);
    assert_eq!(grid(&h).x_off, 8);
    h.keys("/");
    h.type_text("r5c25");
    h.draw(W, H);
    assert_eq!(grid(&h).col, 25, "the preview jumps to the match");
    assert!(grid(&h).x_off > 8 && whole_on_screen(grid(&h), 25));
    h.key(KeyCode::Esc);
    h.draw(W, H);
    let g = grid(&h);
    assert_eq!((g.row, g.col, g.x_off, g.detached), (0, 0, 8, true));
}

/// A wider view (the terminal resized) takes the view back so the last column still ends at
/// its right edge.
#[test]
fn a_resize_keeps_the_view_within_the_columns() {
    let mut h = wide(30, 20);
    for _ in 0..200 {
        wheel(&mut h, MouseEventKind::ScrollRight, KeyModifiers::NONE);
    }
    h.draw(W, H);
    let narrow = grid(&h).x_off;
    assert_eq!(narrow, grid(&h).x_max());
    h.draw(120, 40);
    let g = grid(&h);
    assert_eq!(g.x_off, 30 * 12 - g.view_w as usize);
    assert!(g.x_off < narrow);
    // So wide that every column fits: no scrolling at all.
    let mut h = wide(4, 5);
    wheel(&mut h, MouseEventKind::ScrollRight, KeyModifiers::NONE);
    assert_eq!(grid(&h).x_off, 0);
}

/// Each result of a run keeps its own place sideways.
#[test]
fn every_result_tab_keeps_its_own_view() {
    let mut h = Harness::connected(Lang::En);
    h.app.detail.visible = false;
    h.app.tab_mut().editor = Editor::new("SELECT 1;\nSELECT 2;");
    h.keys("ggvG$");
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let metas = || Some((0..30).map(|i| meta(&format!("column_{i:02}"), "text", false, false)).collect());
    let rows = |n: usize| (0..n).map(|r| (0..30).map(|c| Some(format!("r{r}c{c}"))).collect()).collect::<Vec<_>>();
    let ms = Duration::from_millis(1);
    h.db(DbEvent::Started { id, index: 0 });
    h.db(DbEvent::StepRows { id, index: 0, columns: metas(), rows: rows(3), more: false });
    h.db(DbEvent::Finished { id, index: 0, outcome: Outcome::Command("SELECT".into()), elapsed: ms });
    h.db(DbEvent::Started { id, index: 1 });
    h.db(DbEvent::Page { id, columns: metas(), rows: rows(4), more: false, elapsed: ms });
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Results);
    h.draw(W, H);
    for _ in 0..3 {
        wheel(&mut h, MouseEventKind::ScrollRight, KeyModifiers::NONE);
    }
    h.draw(W, H);
    assert_eq!(grid(&h).x_off, 12);
    h.keys("H");
    h.draw(W, H);
    assert_eq!(grid(&h).x_off, 0, "the first result: its own view");
    wheel(&mut h, MouseEventKind::ScrollRight, KeyModifiers::NONE);
    h.keys("L");
    h.draw(W, H);
    assert_eq!(grid(&h).x_off, 12, "the view the second one had");
    h.keys("H");
    h.draw(W, H);
    assert_eq!(grid(&h).x_off, 4);
}

/// A wide character cut by the view's left or right edge is drawn as a blank, never half of it;
/// the pane's border stays in place on every row.
#[test]
fn a_wide_character_cut_by_an_edge_is_a_blank() {
    let mut h = Harness::connected(Lang::En);
    h.app.detail.visible = false;
    // "a" then Hangul syllables (two columns each): in column 0 they start at characters 3,
    // 5, 7… of the row of columns, so an offset of 4 cuts the first one.
    let hangul: String = (0..18).map(|i| char::from_u32(0xAC00 + i).unwrap()).collect();
    let value = format!("a{hangul}");
    let metas = (0..6).map(|i| meta(&format!("column_{i:02}"), "text", false, false)).collect();
    let rows = (0..5).map(|_| (0..6).map(|_| Some(value.clone())).collect()).collect();
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::Page { id, columns: Some(metas), rows, more: false, elapsed: Duration::from_millis(2) });
    h.app.focus = Focus::Results;
    h.draw(W, H);
    let right = grid(&h).gutter_x.1 + grid(&h).view_w;
    for step in 0..12 {
        let t = h.draw(W, H);
        let buf = t.backend().buffer();
        let g = grid(&h);
        for y in g.header_y..g.data_y + 5 {
            assert_eq!(buf[(right, y)].symbol(), "│", "the pane's border at offset {}", g.x_off);
            // The row numbers' last column is a blank, never the second half of a character.
            assert_eq!(buf[(g.gutter_x.1 - 1, y)].symbol(), " ");
        }
        if step == 1 {
            assert_eq!(g.x_off, 4);
            let y = g.data_y;
            assert_eq!(buf[(g.gutter_x.1, y)].symbol(), " ", "the first syllable cut: a blank");
            assert_eq!(buf[(g.gutter_x.1 + 1, y)].symbol(), "\u{AC01}");
        }
        wheel(&mut h, MouseEventKind::ScrollRight, KeyModifiers::NONE);
    }
}

/// Six text columns, a number column with a long name (21 wide), then four more text columns;
/// the numbers `7`, NULL, an empty string, `12345`, `7`. The inspector hidden, drawn at `w`×`H`.
fn amounts(w: u16) -> Harness {
    let mut h = Harness::connected(Lang::En);
    h.app.detail.visible = false;
    let mut metas: Vec<_> = (0..6).map(|i| meta(&format!("column_{i:02}"), "text", false, false)).collect();
    metas.push(meta("amount_in_cents_total", "int8", true, false));
    metas.extend((7..11).map(|i| meta(&format!("column_{i:02}"), "text", false, false)));
    let amounts = [Some("7"), None, Some(""), Some("12345"), Some("7")];
    let rows = amounts
        .iter()
        .enumerate()
        .map(|(r, a)| (0..11).map(|c| if c == 6 { a.map(str::to_string) } else { Some(format!("r{r}c{c}")) }).collect())
        .collect();
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::Page { id, columns: Some(metas), rows, more: false, elapsed: Duration::from_millis(2) });
    h.app.focus = Focus::Results;
    h.draw(w, H);
    h
}

/// A cell cut by the right edge whose text columns on screen are blank (a right-aligned number
/// under the edge) shows a dim `›` in its last column on screen, so it never reads as empty;
/// NULL keeps its own look and an empty value is not hidden (no mark); a cell whose text shows
/// gets none. Neither does a cell of which only the separator or the blank before its text
/// shows (no text column to put it in), nor ever a header.
#[test]
fn a_cell_whose_text_is_under_the_right_edge_shows_a_mark() {
    let theme = datarig_tui::theme::DARK;
    let values = [Some(1), None, Some(0), Some(5), Some(1)];
    let (mut marked, mut slivers) = (0, 0);
    for w in 80..84 {
        let mut h = amounts(w);
        loop {
            let t = h.draw(w, H);
            let buf = t.backend().buffer();
            let g = grid(&h);
            assert!(g.page_rows >= 5);
            let (from, right) = (i32::from(g.gutter_x.1), i32::from(g.gutter_x.1 + g.view_w));
            let edge = (right - 1) as u16;
            // The column whose separator is last on screen, its text from `tx`, `width` wide.
            let c =
                (0..g.col_x.len() - 1).rev().find(|&c| from + g.col_x[c] as i32 - (g.x_off as i32) < right).unwrap();
            let width = (g.col_x[c + 1] - g.col_x[c] - 3) as i32;
            let tx = from + g.col_x[c] as i32 - g.x_off as i32 + 2;
            // Cut by the edge with a text column on screen.
            let cut = tx + width > right && tx < right;
            slivers += usize::from(tx >= right);
            for (r, n) in values.iter().enumerate() {
                let y = g.data_y + r as u16;
                // The numbers right-aligned; the other columns' values (`r0c1`…) from `tx`, on
                // screen whenever a text column is.
                let want = cut && c == 6 && n.is_some_and(|n| n > 0 && tx + width - n >= right);
                let mark = buf[(edge, y)].symbol() == "›";
                assert_eq!(mark, want, "row {r} at width {w}, offset {} (text at {tx}, edge {right})", g.x_off);
                if want {
                    marked += 1;
                    assert_ne!(Some(buf[(edge, y)].fg), theme.fg.into(), "dim, not the text's color");
                }
            }
            for y in [g.header_y, g.header_y + 1] {
                assert_ne!(buf[(edge, y)].symbol(), "›", "a header at width {w}, offset {}", g.x_off);
            }
            if g.x_off == g.x_max() {
                break;
            }
            wheel(&mut h, MouseEventKind::ScrollRight, KeyModifiers::NONE);
        }
    }
    assert!(marked > 0, "marks shown");
    assert!(slivers > 0, "a column showing its separator or blank only was seen");
}

/// A drag to the right across columns wider than the view (80 columns, the inspector open:
/// a view of about 26 characters for values up to 40) goes on to the last column: the view
/// never moves back, and at the end it stays there (no flip between two places).
#[test]
fn a_drag_to_the_right_crosses_columns_wider_than_the_view() {
    let mut h = with_edge_results(Lang::En);
    h.key(KeyCode::Tab);
    h.draw(W, H);
    let g = grid(&h);
    let ncols = g.col_x.len() - 1;
    assert!((0..ncols).any(|c| g.col_x[c + 1] - g.col_x[c] > g.view_w as usize), "a column wider than the view");
    let (a, _, _) = g.hit_cols[0];
    let (y, right) = (g.data_y, g.gutter_x.1 + g.view_w);
    h.mouse(MouseEventKind::Down(MouseButton::Left), a + 2, y);
    let mut was = 0;
    for _ in 0..120 {
        h.mouse(MouseEventKind::Drag(MouseButton::Left), right + 1, y);
        h.draw(W, H);
        let g = grid(&h);
        assert!(g.x_off >= was, "moved back from {was} to {} (column {})", g.x_off, g.col);
        was = g.x_off;
    }
    let g = grid(&h);
    assert_eq!((g.col, g.x_off), (ncols - 1, g.x_max()), "the last column, the view at its end");
    h.mouse(MouseEventKind::Up(MouseButton::Left), right + 1, y);
}

/// The wheel during a drag extends it over the rows only: a pointer held past the right edge
/// does not move the view sideways on each wheel event (a drag event does).
#[test]
fn the_wheel_during_a_drag_past_the_edge_moves_rows_only() {
    let mut h = wide(30, 40);
    let g = grid(&h);
    let (a, _, _) = g.hit_cols[0];
    let (y, right) = (g.data_y, g.gutter_x.1 + g.view_w);
    h.mouse(MouseEventKind::Down(MouseButton::Left), a + 2, y);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), right, y);
    h.draw(W, H);
    assert_eq!(grid(&h).x_off, 4);
    let col = grid(&h).col;
    for _ in 0..3 {
        h.mouse(MouseEventKind::ScrollDown, right, y);
    }
    h.draw(W, H);
    let g = grid(&h);
    assert_eq!((g.x_off, g.col), (4, col), "sideways only by drag events");
    assert!(g.top > 0 && g.anchor.is_some(), "the rows scrolled, the drag goes on");
    h.mouse(MouseEventKind::Up(MouseButton::Left), right, y);
}

/// A column of which only the separator shows at the right edge is no cell: a click there
/// selects nothing.
#[test]
fn a_column_showing_only_its_separator_is_not_a_cell() {
    let mut slivers = 0;
    for w in 80..84 {
        let mut h = amounts(w);
        loop {
            h.draw(w, H);
            let g = grid(&h);
            let (from, right) = (g.gutter_x.1 as usize, (g.gutter_x.1 + g.view_w) as usize);
            for c in 0..g.col_x.len() - 1 {
                if (from + g.col_x[c]).checked_sub(g.x_off) == Some(right - 1) {
                    slivers += 1;
                    assert!(!g.hit_cols.iter().any(|h| h.2 == c), "column {c}: only its separator shows");
                    assert_ne!(g.cell_at((right - 1) as u16, g.data_y, 5).map(|x| x.1), Some(c));
                }
            }
            if g.x_off == g.x_max() {
                break;
            }
            wheel(&mut h, MouseEventKind::ScrollRight, KeyModifiers::NONE);
        }
    }
    assert!(slivers > 0, "a separator alone at the edge was seen");
}
