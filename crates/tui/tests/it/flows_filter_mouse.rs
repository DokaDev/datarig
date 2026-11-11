//! The mouse in the filter dialog: a click does what the key for the part clicked does
//! (columns, values, the operator picker, sort choices, the `×` of conditions and chips, the
//! buttons of the footer and the hint line), on the release over the part pressed and not
//! right after the dialog opened; a text field takes the focus and the cursor on the press; a
//! field of the SQL section left by a click is read back as `Tab` reads it back; the wheel moves
//! through the lists; the pointer lights what is under it.

use crate::common;

use common::*;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_i18n::Lang;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use ratatui::style::Modifier;
use std::time::Duration;

const W: u16 = 120;
const H: u16 = 34;
const ALL: &str = r#"SELECT * FROM "shop"."users""#;

/// Open `shop.users` from the explorer (a table tab, which runs its query).
fn open_users(h: &mut Harness) {
    h.key(KeyCode::BackTab);
    h.goto("shop");
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["users".into()], Vec::new()).into()) });
    h.goto("users");
    h.key(KeyCode::Enter);
    assert!(h.app.tab().is_table());
}

/// The statements of the runs sent since the last look.
fn runs(h: &mut Harness) -> Vec<Vec<String>> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { statements, .. } => Some(statements),
            _ => None,
        })
        .collect()
}

/// Answer the active tab's current run with a first page of `shop.users` rows.
fn answer(h: &mut Harness) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
    let rows = (0..3).map(|i| vec![Some(i.to_string()), Some(format!("n{i}"))]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: false, elapsed: Duration::from_millis(2) });
}

/// `shop.users` open and answered, the grid focused, the dialog open on `id` and drawn,
/// nothing sent since.
fn dialog() -> Harness {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    answer(&mut h);
    h.sent();
    h.key(KeyCode::Tab);
    h.keys("f");
    assert!(open(&h));
    h.draw(W, H);
    h
}

fn open(h: &Harness) -> bool {
    h.app.overlays.is_open(OverlayKind::FilterDialog)
}

/// Where `text` starts on the screen (the `n`th match, top to bottom) right of column `from`.
fn find_nth(h: &mut Harness, text: &str, from: u16, n: usize) -> (u16, u16) {
    let t = h.draw(W, H);
    let buf = t.backend().buffer();
    (0..H)
        .flat_map(|y| {
            let row = row_text(buf, y);
            row.match_indices(text)
                .map(|(i, _)| (datarig_tui::text::width(&row[..i]) as u16, y))
                .filter(|(x, _)| *x >= from)
                .collect::<Vec<_>>()
        })
        .nth(n)
        .unwrap_or_else(|| panic!("no {text:?} on screen:\n{}", h.screen(W, H)))
}

fn find(h: &mut Harness, text: &str) -> (u16, u16) {
    find_nth(h, text, 0, 0)
}

/// A press and a release at (x, y), past the arming delay.
fn click(h: &mut Harness, (x, y): (u16, u16)) {
    h.advance(Duration::from_millis(500));
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    h.draw(W, H);
}

/// Click on `text` (its `dx`th column).
fn click_on(h: &mut Harness, text: &str, dx: u16) {
    let (x, y) = find(h, text);
    click(h, (x + dx, y));
}

/// The draft's `WHERE` text and order.
fn draft(h: &Harness) -> (String, String) {
    h.app.filter_dialog_draft().expect("the dialog is open")
}

/// The first column of the right pane (right of the column list).
fn pane_x(h: &mut Harness) -> u16 {
    find(h, "\u{2502} id").0 + 1
}

/// A condition `= value` on the selected column, by the keys.
fn condition(h: &mut Harness, value: &str) {
    h.key(KeyCode::Right);
    h.key(KeyCode::Char(' '));
    h.type_text(value);
    h.draw(W, H);
}

#[test]
fn enter_apply_sends_once_the_dialog_takes_presses() {
    let mut h = dialog();
    condition(&mut h, "1");
    // A press right after the dialog came on top is not taken.
    let at = find(&mut h, "Enter apply");
    h.mouse(MouseEventKind::Down(MouseButton::Left), at.0 + 1, at.1);
    h.mouse(MouseEventKind::Up(MouseButton::Left), at.0 + 1, at.1);
    assert!(open(&h) && runs(&mut h).is_empty());
    // Pressed on one button, released on another: nothing.
    h.advance(Duration::from_millis(500));
    let cancel = find(&mut h, "Esc cancel");
    h.mouse(MouseEventKind::Down(MouseButton::Left), at.0 + 1, at.1);
    h.mouse(MouseEventKind::Up(MouseButton::Left), cancel.0 + 1, cancel.1);
    assert!(open(&h) && runs(&mut h).is_empty());
    click_on(&mut h, "Enter apply", 1);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id = 1)")]]);
    assert!(!open(&h));
}

#[test]
fn esc_cancel_drops_the_draft() {
    let mut h = dialog();
    condition(&mut h, "1");
    click_on(&mut h, "Esc cancel", 1);
    assert!(!open(&h));
    assert!(runs(&mut h).is_empty());
    assert!(h.app.tab().doc.table_query.as_ref().unwrap().draft.filter.is_none());
}

#[test]
fn a_chips_x_takes_it_out_and_leaves_the_others_as_written() {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    answer(&mut h);
    h.command("where id  >  1 AND name = 'x'");
    runs(&mut h);
    answer(&mut h);
    h.key(KeyCode::Tab);
    h.keys("f");
    h.draw(W, H);
    let (x, y) = find(&mut h, "id  >  1 \u{00d7}");
    click(&mut h, (x + 9, y));
    assert_eq!(draft(&h).0, "name = 'x'");
    // Undo puts it back, as written.
    click_on(&mut h, " undo ", 1);
    assert_eq!(draft(&h).0, "id  >  1 AND name = 'x'");
    // The `×` of a condition's row takes it out too (`name`'s row, on its column).
    let px = pane_x(&mut h);
    let (x, y) = find_nth(&mut h, "name", 0, 0);
    assert!(x < px, "the column list's `name`");
    click(&mut h, (x, y));
    let (_, ry) = find_nth(&mut h, "\u{25be}", px, 0);
    let (xx, _) = find_nth(&mut h, "\u{00d7}", px, 0);
    click(&mut h, (xx, ry));
    assert_eq!(draft(&h).0, "id  >  1");
}

#[test]
fn a_click_on_a_column_selects_it_and_one_on_a_value_ticks_it() {
    let mut h = dialog();
    let px = pane_x(&mut h);
    let (x, y) = find_nth(&mut h, "name", 0, 0);
    assert!(x < px);
    click(&mut h, (x, y));
    let screen = h.screen(W, H);
    assert!(screen.contains("~ count in the 3 loaded rows"), "{screen}");
    click_on(&mut h, "] n1", 2);
    assert_eq!(draft(&h).0, "name = 'n1'");
    click_on(&mut h, "] n2", 2);
    assert_eq!(draft(&h).0, "name IN ('n1', 'n2')");
    // Clicked again: unticked.
    click_on(&mut h, "] n1", 2);
    assert_eq!(draft(&h).0, "name = 'n2'");
    click_on(&mut h, "Enter apply", 1);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (name = 'n2')")]]);
}

#[test]
fn a_click_on_the_operator_opens_the_picker_and_one_on_an_entry_picks_it() {
    let mut h = dialog();
    condition(&mut h, "1");
    let px = pane_x(&mut h);
    let op = find_nth(&mut h, "\u{25be}", px, 0);
    click(&mut h, op);
    assert!(h.screen(W, H).contains("IS DISTINCT FROM"), "the picker is open");
    // `≠`, the third entry (`=`, `between`, `≠`).
    let (x, y) = find_nth(&mut h, "\u{2260}", px, 0);
    click(&mut h, (x, y));
    assert!(!h.screen(W, H).contains("IS DISTINCT FROM"), "picked: the picker closed");
    assert_eq!(draft(&h).0, "id <> 1");
    // Open again: a press outside it closes it and does nothing else.
    let op = find_nth(&mut h, "\u{2260} ", px, 0);
    click(&mut h, (op.0 + 1, op.1));
    assert!(h.screen(W, H).contains("IS DISTINCT FROM"));
    let (x, y) = find_nth(&mut h, "name", 0, 0);
    click(&mut h, (x, y));
    assert!(!h.screen(W, H).contains("IS DISTINCT FROM"));
    assert!(h.screen(W, H).contains("id  int8"), "still on `id`");
    assert_eq!(draft(&h).0, "id <> 1");
    assert!(!h.screen(W, H).contains("name  text"), "the press only closed the picker");
}

#[test]
fn a_click_in_a_field_puts_the_cursor_under_the_pointer() {
    let mut h = dialog();
    condition(&mut h, "123");
    let px = pane_x(&mut h);
    let (x, y) = find_nth(&mut h, "123", px, 0);
    click(&mut h, (x, y));
    assert_eq!(h.app.key_context(), Ctx::FilterDialogText);
    h.type_text("9");
    assert_eq!(draft(&h).0, "id = 9123");
    // The `Limit` field: focused, the cursor where it was clicked.
    let (lx, ly) = find(&mut h, "Limit");
    click(&mut h, (lx + 8, ly));
    h.type_text("50");
    click_on(&mut h, "Enter apply", 1);
    // The cap is the tab's, not written into the grid's statement.
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id = 9123)")]]);
    assert_eq!(h.app.tab().doc.table_query.as_ref().unwrap().applied().limit, Some(50));
}

#[test]
fn the_footer_buttons_and_the_sort_line() {
    let mut h = dialog();
    // `↓ descending` on the sort line: the order.
    click_on(&mut h, "\u{2193} desc", 0);
    assert_eq!(draft(&h).1, "id DESC");
    click_on(&mut h, "(\u{2022}) \u{2193}", 0);
    assert_eq!(draft(&h).1, "id DESC", "the one chosen stays");
    // `+ add condition`: a row waiting for its value.
    click_on(&mut h, "+ add condition", 2);
    assert!(h.screen(W, H).contains("incomplete"));
    h.type_text("2");
    assert_eq!(draft(&h).0, "id = 2");
    click_on(&mut h, " clear all ", 1);
    assert_eq!(draft(&h), (String::new(), String::new()));
    click_on(&mut h, " undo ", 1);
    assert_eq!(draft(&h), ("id = 2".into(), "id DESC".into()));
    // The key of the order: its column's sort line, where `←` steps back to ascending.
    let (kx, ky) = find_nth(&mut h, "id \u{2193}", 0, 0);
    click(&mut h, (kx, ky));
    h.key(KeyCode::Left);
    assert_eq!(draft(&h).1, "id ASC");
}

#[test]
fn the_sql_section_by_the_pointer() {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    answer(&mut h);
    let long = "name <> 'a value written long enough to wrap the field of the dialog over two of its rows, \
                which takes a good many more words than one would think' AND id > 1";
    h.command(&format!("where {long}"));
    runs(&mut h);
    answer(&mut h);
    h.key(KeyCode::Tab);
    h.keys("f");
    h.draw(W, H);
    click_on(&mut h, "SQL \u{25b8}", 0);
    assert_eq!(h.app.filter_dialog_sql_fields().map(|f| f.0), Some(long.to_string()));
    // The `WHERE` field is wrapped: a click on its second row puts the cursor before what is
    // shown first there.
    // The field's label, at the start of its row (the statement above names `WHERE` too).
    let (wx, wy) = find(&mut h, "\u{2502} WHERE ");
    let t = h.draw(W, H);
    let second = row_text(t.backend().buffer(), wy + 1);
    let fx = (wx..W).find(|&x| t.backend().buffer()[(x, wy)].symbol() == "n").unwrap();
    let shown: String = second.chars().skip(usize::from(fx)).take(6).collect();
    assert!(long.contains(&shown), "{shown:?} is on the field's second row");
    click(&mut h, (fx, wy + 1));
    h.type_text("X");
    let (where_, _) = h.app.filter_dialog_sql_fields().unwrap();
    assert!(where_.contains(&format!("X{shown}")), "{where_}");
    // A click on the `ORDER BY` field reads the `WHERE` field back first (as `Tab` does).
    click_on(&mut h, "ORDER BY", 10);
    assert_eq!(draft(&h).0, where_);
    h.type_text("name desc");
    // A click elsewhere reads it back: the order is the draft's.
    click_on(&mut h, " undo ", 1);
    assert_eq!(draft(&h).1, "", "undo in the field drops what was typed there first");
    h.type_text("name desc");
    let (x, y) = find(&mut h, "Limit");
    click(&mut h, (x + 8, y));
    assert_eq!(draft(&h).1, "name DESC");
    // `SQL ▾` closes it; `Open in editor` opens the statement in a console.
    click_on(&mut h, "SQL \u{25be}", 0);
    assert!(h.app.filter_dialog_sql_fields().is_none());
    click_on(&mut h, "SQL \u{25b8}", 0);
    let tabs = h.app.tabs.len();
    click_on(&mut h, "Open in editor", 1);
    assert!(!open(&h));
    assert_eq!(h.app.tabs.len(), tabs + 1, "a console with the statement");
}

#[test]
fn count_exactly_by_the_pointer() {
    let mut h = dialog();
    let (x, y) = find_nth(&mut h, "name", 0, 0);
    click(&mut h, (x, y));
    h.sent();
    click_on(&mut h, "# count exactly", 2);
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::GroupCount { .. })), "the count is sent");
}

#[test]
fn the_wheel_moves_through_the_columns_and_the_picker() {
    let mut h = dialog();
    let (x, y) = find_nth(&mut h, "name", 0, 0);
    h.mouse(MouseEventKind::ScrollDown, x, y);
    assert!(h.screen(W, H).contains("name  text"), "the next column");
    h.mouse(MouseEventKind::ScrollUp, x, y);
    assert!(h.screen(W, H).contains("id  int8"), "back");
    condition(&mut h, "1");
    let px = pane_x(&mut h);
    let op = find_nth(&mut h, "\u{25be}", px, 0);
    click(&mut h, op);
    h.mouse(MouseEventKind::ScrollDown, op.0, op.1 + 2);
    h.mouse(MouseEventKind::ScrollDown, op.0, op.1 + 2);
    h.key(KeyCode::Enter);
    assert_eq!(draft(&h).0, "id <> 1", "two notches down: `≠`");
}

#[test]
fn the_pointer_lights_a_button_and_keeps_the_focus() {
    let mut h = dialog();
    let (x, y) = find(&mut h, " clear all ");
    h.mouse(MouseEventKind::Moved, x + 2, y);
    let t = h.draw(W, H);
    assert!(t.backend().buffer()[(x + 2, y)].modifier.contains(Modifier::BOLD), "lit");
    assert_eq!(h.app.key_context(), Ctx::FilterDialog, "the focus stays in the column list");
    // Moved away: not lit.
    h.mouse(MouseEventKind::Moved, 1, 1);
    let t = h.draw(W, H);
    assert!(!t.backend().buffer()[(x + 2, y)].modifier.contains(Modifier::BOLD));
}
