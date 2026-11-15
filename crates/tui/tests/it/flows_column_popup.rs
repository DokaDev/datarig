//! A column's popup from a table tab's grid. A press on a header's name or type arms it: the
//! release over the same header of the same grid opens the column's condition popup under the
//! header (the cursor on the column); a drag from it selects whole columns as before, Shift
//! extends the selection, the sort zone sorts. Anything that changes between the press and the
//! release (the result, the columns shown, the view scrolled sideways, a dialog over it) leaves
//! the release doing nothing.
//!
//! `F`, a right click on a header and the grid menu's entry open the same popup with lines
//! above it: `Hide column` (`C`), and on a cell `Only 'v'` / `Exclude 'v'`, dimmed with why
//! when they cannot apply. A value line runs the text it showed only while the grid, its
//! result, its columns and the cells' text are what they were; else nothing is sent and the
//! lines are written again.

use crate::common;

use common::*;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_i18n::Lang;
use datarig_tui::app::filter_dialog::view::PopupKindId;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::{Focus, Results};
use datarig_tui::keymap::Ctx;
use datarig_tui::widgets::grid::Shape;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use std::time::Duration;

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

/// Answer the active tab's current run with three rows of `id`, `name` and `email`, each
/// `name` `width` characters long and each `email` as long past its four.
fn answer_wide(h: &mut Harness, width: usize) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = Some(vec![
        meta("id", "int8", true, false),
        meta("name", "text", false, false),
        meta("email", "text", false, false),
    ]);
    let rows = (0..3)
        .map(|i| {
            let name = format!("n{i}{}", "x".repeat(width.saturating_sub(2)));
            let email = format!("u{i}@x{}", "x".repeat(width.saturating_sub(2)));
            vec![Some(i.to_string()), Some(name), Some(email)]
        })
        .collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: false, elapsed: Duration::from_millis(2) });
}

fn answer(h: &mut Harness) {
    answer_wide(h, 2);
}

/// `shop.users` open and answered, the grid focused, the inspector closed and the screen drawn
/// at 120x34, nothing sent since.
fn users() -> Harness {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    answer(&mut h);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h.app.detail.visible = false;
    h.draw(120, 34);
    h.sent();
    h
}

/// The grid's cursor on (`row`, `col`), no selection.
fn at(h: &mut Harness, row: usize, col: usize) {
    let g = &mut h.app.tab_mut().grid;
    (g.row, g.col, g.anchor) = (row, col, None);
    h.draw(120, 34);
}

/// Where column `col`'s header name is, drawn at 120x34 (`ty`: its type row).
fn head(h: &mut Harness, col: usize, ty: bool) -> (u16, u16) {
    h.draw(120, 34);
    let g = h.app.tab().grid.clone();
    let x = g.hit_cols.iter().find(|c| c.2 == col).unwrap_or_else(|| panic!("{col} drawn: {:?}", g.hit_cols)).0 + 1;
    (x, g.header_y + u16::from(ty))
}

fn down(h: &mut Harness, (x, y): (u16, u16)) {
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
}

fn up(h: &mut Harness, (x, y): (u16, u16)) {
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

fn click(h: &mut Harness, at: (u16, u16)) {
    down(h, at);
    up(h, at);
}

fn popup(h: &Harness) -> Option<PopupKindId> {
    h.app.filter_dialog_popup()
}

/// The column the open condition popup is on: the name on its top border.
fn popup_on(h: &mut Harness) -> Option<String> {
    popup(h)?;
    let screen = h.screen(120, 34);
    let line = screen.lines().find(|l| l.contains("\u{256d}\u{2500} "))?;
    let at = line.find("\u{256d}\u{2500} ")? + "\u{256d}\u{2500} ".len();
    line[at..].split_whitespace().next().map(str::to_string)
}

/// The lines above the open popup: `label | key or why`, dimmed ones marked `~`.
fn lines(h: &Harness) -> Vec<String> {
    h.app
        .column_popup_lines()
        .into_iter()
        .map(|(l, k, dim)| format!("{}{l} | {k}", if dim { "~" } else { "" }))
        .collect()
}

fn flashed(h: &Harness) -> String {
    h.app.transient.as_ref().map(|(n, _)| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

/// A click on a header's name or type opens the column's condition popup under the header, the
/// cursor on the column; nothing is sent, nothing selected, and the popup has no lines above it.
#[test]
fn a_header_click_opens_the_columns_popup_under_it() {
    for ty in [false, true] {
        let mut h = users();
        let at = head(&mut h, 1, ty);
        click(&mut h, at);
        assert_eq!(popup(&h), Some(PopupKindId::Condition), "type row: {ty}");
        assert_eq!(popup_on(&mut h).as_deref(), Some("name"));
        assert_eq!((h.app.tab().grid.col, h.app.tab().grid.anchor), (1, None));
        assert!(lines(&h).is_empty(), "{:?}", lines(&h));
        assert!(runs(&mut h).is_empty());
        // Its top border on the row under the header, from the header cell's left edge.
        let t = h.draw(120, 34);
        let g = h.app.tab().grid.clone();
        let (a, ..) = *g.hit_cols.iter().find(|c| c.2 == 1).unwrap();
        assert_eq!(t.backend().buffer()[(a, g.data_y)].symbol(), "\u{256d}");
    }
}

#[test]
fn the_header_popup_screens() {
    let mut h = users();
    let at = head(&mut h, 1, false);
    click(&mut h, at);
    assert_screen!("column_popup_header_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("column_popup_header_en_120x34", Lang::En, h.draw(120, 34));
}

/// A drag from a header selects whole columns as before (from the column pressed to the one
/// under the pointer); a drag that stays on its header selects that column. No popup.
#[test]
fn a_drag_from_a_header_selects_whole_columns() {
    let mut h = users();
    let (from, to) = (head(&mut h, 0, false), head(&mut h, 1, true));
    down(&mut h, from);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), to.0, to.1);
    up(&mut h, to);
    assert_eq!(popup(&h), None);
    assert_eq!(h.app.tab().grid.selection(3, 3), Some((0..3, 0..2)));
    assert_eq!(h.app.tab().grid.shape, Shape::Cols);
    // On the same header.
    let mut h = users();
    let at = head(&mut h, 2, false);
    down(&mut h, at);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), at.0 + 1, at.1);
    up(&mut h, (at.0 + 1, at.1));
    assert_eq!(popup(&h), None);
    assert_eq!(h.app.tab().grid.selection(3, 3), Some((0..3, 2..3)));
    assert!(runs(&mut h).is_empty());
}

/// Shift+click on a header extends the selection as before; no popup.
#[test]
fn shift_click_on_a_header_extends_the_selection() {
    let mut h = users();
    let (a, b) = (head(&mut h, 0, false), head(&mut h, 2, false));
    h.mouse_mod(MouseEventKind::Down(MouseButton::Left), a.0, a.1, KeyModifiers::SHIFT);
    h.mouse_mod(MouseEventKind::Up(MouseButton::Left), a.0, a.1, KeyModifiers::SHIFT);
    let g = &h.app.tab().grid;
    assert_eq!((g.shape, g.anchor, g.col), (Shape::Cols, Some((0, 0)), 0));
    h.mouse_mod(MouseEventKind::Down(MouseButton::Left), b.0, b.1, KeyModifiers::SHIFT);
    h.mouse_mod(MouseEventKind::Up(MouseButton::Left), b.0, b.1, KeyModifiers::SHIFT);
    assert_eq!(popup(&h), None);
    assert_eq!(h.app.tab().grid.selection(3, 3), Some((0..3, 0..3)));
}

/// The sort zone of the cursor's header sorts as it did; no popup.
#[test]
fn the_sort_zone_still_sorts() {
    let mut h = users();
    h.draw(120, 34);
    let g = h.app.tab().grid.clone();
    let (x, _, c) = g.sort_zones.iter().copied().find(|z| z.2 == 0).expect("a sort zone");
    click(&mut h, (x, g.header_y));
    assert_eq!(popup(&h), None);
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} ORDER BY "id" ASC"#)]]);
    assert_eq!(h.app.tab().grid.col, c);
}

/// A release over another header, a cell, or off the grid does nothing; so does one after the
/// pointer moved with no button held (its release was lost).
#[test]
fn a_release_elsewhere_does_nothing() {
    let mut h = users();
    let (a, b) = (head(&mut h, 0, false), head(&mut h, 1, false));
    let cell = (b.0, h.app.tab().grid.data_y);
    for to in [b, cell, (0, 0)] {
        down(&mut h, a);
        up(&mut h, to);
        assert_eq!(popup(&h), None, "{to:?}");
        assert_eq!(h.app.tab().grid.anchor, None, "{to:?}: nothing selected");
    }
    down(&mut h, a);
    h.mouse(MouseEventKind::Moved, a.0, a.1);
    up(&mut h, a);
    assert_eq!(popup(&h), None, "a lost release");
    assert!(runs(&mut h).is_empty());
}

/// Between the press and the release the result is replaced, a column is hidden (the headers
/// move), or the view scrolls sideways: the release opens nothing.
#[test]
fn a_change_between_press_and_release_opens_nothing() {
    // Another result.
    let mut h = users();
    let at = head(&mut h, 1, false);
    down(&mut h, at);
    h.command("where id > 0");
    assert_eq!(runs(&mut h).len(), 1);
    answer(&mut h);
    h.draw(120, 34);
    up(&mut h, at);
    assert_eq!(popup(&h), None, "another result");
    // The middle column hidden: the third column's header comes under the pointer.
    let mut h = users();
    let at = head(&mut h, 2, false);
    down(&mut h, at);
    at_col(&mut h, 1);
    h.keys("C");
    h.draw(120, 34);
    up(&mut h, at);
    assert_eq!(popup(&h), None, "a column hidden");
    // The view scrolled sideways.
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    answer_wide(&mut h, 200);
    h.key(KeyCode::Tab);
    h.app.detail.visible = false;
    let at = head(&mut h, 1, false);
    down(&mut h, at);
    let before = h.app.tab().grid.x_off;
    h.mouse(MouseEventKind::ScrollRight, at.0, at.1 + 3);
    h.draw(120, 34);
    assert_ne!(h.app.tab().grid.x_off, before, "scrolled");
    up(&mut h, at);
    assert_eq!(popup(&h), None, "scrolled sideways");
}

/// The cursor on column `col` of the row it is on, as `h`/`l` would put it.
fn at_col(h: &mut Harness, col: usize) {
    h.app.tab_mut().grid.col = col;
}

/// A dialog that comes over the grid between the press and the release (a question, a popup
/// opened by keys) takes the press: once it is gone the release does nothing.
#[test]
fn a_dialog_over_the_press_takes_it() {
    // A question.
    let mut h = users();
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        assert!(rs.edit.batch.delete(&[2], |_| Vec::new()));
    }
    let at = head(&mut h, 1, false);
    down(&mut h, at);
    h.command("where id > 0");
    assert!(matches!(h.app.overlays.confirm().map(|c| c.action), Some(ConfirmAction::LeaveEdits(_))));
    h.draw(120, 34);
    h.key(KeyCode::Esc);
    h.draw(120, 34);
    assert_eq!(h.overlay_kind(), None);
    up(&mut h, at);
    assert_eq!(popup(&h), None, "covered and uncovered");
    // Another popup opened and closed by keys.
    let mut h = users();
    let at = head(&mut h, 1, false);
    down(&mut h, at);
    h.keys(" ro");
    assert_eq!(popup(&h), Some(PopupKindId::Order));
    h.draw(120, 34);
    h.key(KeyCode::Esc);
    h.draw(120, 34);
    up(&mut h, at);
    assert_eq!(popup(&h), None, "a popup by keys between");
    assert!(runs(&mut h).is_empty());
}

/// A double click on a header: the first click opens the popup, the second (on the popup's
/// first moments) leaves it open as it was; nothing is selected.
#[test]
fn a_double_click_on_a_header_opens_the_popup_once() {
    let mut h = users();
    let at = head(&mut h, 1, false);
    click(&mut h, at);
    h.draw(120, 34);
    click(&mut h, at);
    assert_eq!(popup(&h), Some(PopupKindId::Condition));
    assert_eq!(popup_on(&mut h).as_deref(), Some("name"));
    assert_eq!(h.app.tab().grid.anchor, None);
    // After its first moments a press outside it closes it, and is not passed through.
    h.advance(Duration::from_millis(500));
    h.draw(120, 34);
    click(&mut h, at);
    assert_eq!(popup(&h), None);
    assert_eq!(h.app.tab().grid.anchor, None);
}

/// `F` on a cell: the popup of the cell's column, the value lines and hiding above its
/// conditions; `↑` from the first line of the pane reaches them, `Enter` runs one.
#[test]
fn f_on_a_cell_has_the_value_lines() {
    let mut h = users();
    at(&mut h, 0, 1);
    h.keys("F");
    assert_eq!(popup_on(&mut h).as_deref(), Some("name"));
    assert_eq!(lines(&h), ["Only 'n0' | ", "Exclude 'n0' | ", "Hide column | C"]);
    assert_eq!(h.app.column_popup_focus(), None, "the keys on `+ add condition`");
    h.key(KeyCode::Up);
    assert_eq!(h.app.column_popup_focus().as_deref(), Some("Hide column"));
    h.keys("k");
    assert_eq!(h.app.column_popup_focus().as_deref(), Some("Exclude 'n0'"));
    let screen = h.screen(120, 34);
    assert!(screen.contains("\u{2191}\u{2193} line · Enter run"), "{screen}");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), None, "closed");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (name IS DISTINCT FROM 'n0')")]]);
    answer(&mut h);
    // `Space` runs a line too; `Only` replaces the column's own conditions.
    at(&mut h, 1, 1);
    h.keys("F");
    h.key(KeyCode::Up);
    h.keys("kk ");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (name = 'n1')")]]);
}

#[test]
fn the_cell_popup_screens() {
    let mut h = users();
    at(&mut h, 0, 1);
    h.keys("F");
    h.key(KeyCode::Up);
    assert_screen!("column_popup_cell_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("column_popup_cell_en_120x34", Lang::En, h.draw(120, 34));
}

/// A right click on a header: the popup of that column with the line that hides it (no value
/// lines: no cell), the cursor on the column. `C` hides the column and closes the popup;
/// nothing runs.
#[test]
fn a_right_click_on_a_header_has_the_hide_line() {
    let mut h = users();
    let at = head(&mut h, 2, false);
    h.mouse(MouseEventKind::Down(MouseButton::Right), at.0, at.1);
    assert_eq!(popup_on(&mut h).as_deref(), Some("email"));
    assert_eq!(h.app.tab().grid.col, 2);
    assert_eq!(lines(&h), ["Hide column | C"]);
    h.keys("C");
    assert_eq!(h.overlay_kind(), None);
    assert_eq!(h.app.tab().grid.hidden, [false, false, true]);
    assert!(runs(&mut h).is_empty());
    // A click on the line does it too.
    let at = head(&mut h, 1, false);
    h.mouse(MouseEventKind::Down(MouseButton::Right), at.0, at.1);
    h.draw(120, 34);
    h.advance(Duration::from_millis(500));
    let screen = h.screen(120, 34);
    let y = screen.lines().position(|l| l.contains("Hide column")).expect("drawn") as u16;
    let x = screen.lines().nth(usize::from(y)).unwrap().find("Hide column").unwrap();
    let x = screen.lines().nth(usize::from(y)).unwrap()[..x].chars().count() as u16;
    click(&mut h, (x, y));
    assert_eq!(h.app.tab().grid.hidden, [false, true, true]);
}

/// A right click on a cell keeps the grid's menu; its column entry (`F`) opens the popup of
/// the cell's column, with the cell's value lines.
#[test]
fn a_right_click_on_a_cell_keeps_the_menu_whose_entry_opens_the_popup() {
    let mut h = users();
    at(&mut h, 0, 0);
    h.draw(120, 34);
    let g = h.app.tab().grid.clone();
    let x = g.hit_cols.iter().find(|c| c.2 == 2).unwrap().0 + 1;
    h.mouse(MouseEventKind::Down(MouseButton::Right), x, g.data_y + 1);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
    let entry = "Table → this column's conditions, sort and hiding";
    assert!(h.app.menu_rows().iter().any(|(l, k)| *l == entry && k == "F"), "{:?}", h.menu_labels());
    h.menu_pick(entry);
    assert_eq!(popup_on(&mut h).as_deref(), Some("email"));
    assert_eq!(lines(&h), ["Only 'u1@x' | ", "Exclude 'u1@x' | ", "Hide column | C"]);
}

/// Pick line `label` of the open popup's lines by the keys (`↑` from the pane), then `Enter`.
fn run_line(h: &mut Harness, label: &str) {
    for _ in 0..8 {
        if h.app.column_popup_focus().as_deref() == Some(label) {
            return h.key(KeyCode::Enter);
        }
        h.key(KeyCode::Up);
    }
    panic!("{label} not reached: {:?}", lines(h));
}

/// The value lines run what they showed only for the grid they were written for: the result
/// read again, a selection made, a column hidden or an edit on the cell since send nothing,
/// say so, and write the lines again (a second press runs the new text, or says why not).
#[test]
fn stale_value_lines_send_nothing() {
    // The result read again (another result, the same rows).
    let mut h = users();
    at(&mut h, 0, 1);
    h.keys("F");
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        rs.id += 1_000_000;
    }
    run_line(&mut h, "Only 'n0'");
    assert!(runs(&mut h).is_empty(), "nothing sent");
    assert!(flashed(&h).contains("the cells changed"), "{}", flashed(&h));
    assert_eq!(popup(&h), Some(PopupKindId::Condition), "the popup stays");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (name = 'n0')")]], "written again, then run");
    answer(&mut h);
    // A range selected since.
    at(&mut h, 0, 1);
    h.keys("F");
    h.app.tab_mut().grid.anchor = Some((1, 1));
    run_line(&mut h, "Only 'n0'");
    assert!(runs(&mut h).is_empty(), "a range");
    assert_eq!(lines(&h)[0], "Only 'n0', 'n1' | ", "written for the range");
    h.key(KeyCode::Esc);
    // A column hidden since (the cell's column still shown).
    at(&mut h, 0, 2);
    h.keys("F");
    let mut hidden = h.app.tab().grid.hidden.clone();
    hidden.resize(3, false);
    hidden[1] = true;
    h.app.tab_mut().grid.set_hidden(hidden, 3);
    run_line(&mut h, "Exclude 'u0@x'");
    assert!(runs(&mut h).is_empty(), "a column hidden");
    h.key(KeyCode::Esc);
    // An edit on the cell since: the line is dimmed with why.
    let mut h = users();
    at(&mut h, 0, 1);
    h.keys("F");
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        let orig = vec![Some("0".to_string()), Some("n0".to_string()), Some("u0@x".to_string())];
        assert!(rs.edit.batch.set(&[(0, 1)], &datarig_core::edit::Value::Text("x".into()), |_| orig.clone()));
    }
    run_line(&mut h, "Only 'n0'");
    assert!(runs(&mut h).is_empty(), "an edit");
    assert_eq!(lines(&h)[0], "~Only this value | edited");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty(), "a dimmed line runs nothing");
    assert!(flashed(&h).contains("a selected cell shows an edit"), "{}", flashed(&h));
}

/// A line that cannot apply is dimmed with why; pressed, it says why in full and sends nothing,
/// the popup staying open; its hint line says why too.
#[test]
fn dimmed_value_lines_say_why() {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = Some(vec![meta("id", "int8", true, false), meta("doc", "json", false, true)]);
    let rows = vec![vec![Some("1".into()), Some("{}".into())]];
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: false, elapsed: Duration::from_millis(2) });
    h.key(KeyCode::Tab);
    h.app.detail.visible = false;
    h.sent();
    at(&mut h, 0, 1);
    h.keys("F");
    assert_eq!(
        lines(&h),
        ["~Only this value | type not compared", "~Exclude this value | type not compared", "Hide column | C"]
    );
    let why = h.app.column_popup_line_notice("Only this value").expect("dimmed").render(&h.app.i18n).to_string();
    assert!(why.contains("how json compares is not known"), "{why}");
    run_line(&mut h, "Only this value");
    assert!(runs(&mut h).is_empty());
    assert_eq!(popup(&h), Some(PopupKindId::Condition));
    assert!(flashed(&h).contains("how json compares is not known"), "{}", flashed(&h));
    assert_screen!("column_popup_refused_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("column_popup_refused_en_120x34", Lang::En, h.draw(120, 34));
}

/// `⋯ Advanced` (`A`) turns the popup with lines into the whole dialog on the column; the lines
/// go with the popup.
#[test]
fn advanced_from_the_popup_drops_the_lines() {
    let mut h = users();
    at(&mut h, 0, 1);
    h.keys("F");
    h.keys("A");
    assert!(h.app.overlays.is_open(OverlayKind::FilterDialog));
    assert_eq!(popup(&h), None);
    assert!(lines(&h).is_empty());
    assert_eq!(h.app.focus, Focus::Results);
}

/// `shop.users` open, its key-only read answered with a catalogue that lists only `id` and
/// `name` (read before `email` was added), the rows answered with all three columns.
fn users_with_old_catalogue() -> Harness {
    use datarig_core::driver::keys::{CatalogColumn, OrderKeys};
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    let ask = h.sent().iter().find_map(|c| match c {
        DbCommand::LoadTableKeys { id, .. } => Some(*id),
        _ => None,
    });
    let col = |name: &str, ty: &str| CatalogColumn {
        name: name.into(),
        type_name: ty.into(),
        not_null: false,
        primary_key: false,
        foreign_key: false,
        invisible: false,
    };
    let keys = OrderKeys { columns: Some(vec![col("id", "bigint"), col("name", "text")]), ..OrderKeys::default() };
    h.meta_db("local-pg", DbEvent::TableKeys { id: ask.expect("the keys read"), result: Ok(keys) });
    answer(&mut h);
    h.key(KeyCode::Tab);
    h.app.detail.visible = false;
    h.draw(120, 34);
    h.sent();
    h
}

/// A column of the rows shown that the table's columns as read do not list opens no popup
/// (it would be another column's): said, nothing sent. A column they list opens as ever.
#[test]
fn a_column_the_catalogue_does_not_list_opens_no_popup() {
    let mut h = users_with_old_catalogue();
    at(&mut h, 0, 2);
    h.keys("F");
    assert_eq!(popup(&h), None, "F on email");
    assert!(flashed(&h).contains("\u{201c}email\u{201d}"), "{}", flashed(&h));
    let at2 = head(&mut h, 2, false);
    click(&mut h, at2);
    assert_eq!(popup(&h), None, "a click on email's header");
    at(&mut h, 0, 1);
    h.keys("F");
    assert_eq!(popup_on(&mut h).as_deref(), Some("name"));
    assert!(runs(&mut h).is_empty());
}

/// A change between the press and the release that is undone before the release (the Chart
/// shown and the rows again, the view scrolled sideways and back) still leaves the release
/// doing nothing: the press is over at the first change.
#[test]
fn a_change_undone_before_the_release_still_opens_nothing() {
    let mut h = users();
    let at = head(&mut h, 1, false);
    down(&mut h, at);
    h.keys("L");
    assert_ne!(h.app.tab().exec.view, datarig_tui::app::tabs::ResultView::Rows, "another view");
    h.keys("H");
    assert_eq!(h.app.tab().exec.view, datarig_tui::app::tabs::ResultView::Rows);
    h.draw(120, 34);
    up(&mut h, at);
    assert_eq!(popup(&h), None, "a view away and back");
    // Sideways and back.
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    answer_wide(&mut h, 200);
    h.key(KeyCode::Tab);
    h.app.detail.visible = false;
    let at = head(&mut h, 1, false);
    down(&mut h, at);
    let before = h.app.tab().grid.x_off;
    h.mouse(MouseEventKind::ScrollRight, at.0, at.1 + 3);
    assert_ne!(h.app.tab().grid.x_off, before);
    h.mouse(MouseEventKind::ScrollLeft, at.0, at.1 + 3);
    assert_eq!(h.app.tab().grid.x_off, before, "back");
    h.draw(120, 34);
    up(&mut h, at);
    assert_eq!(popup(&h), None, "sideways and back");
}

/// The popup's box (its top border from corner to corner) at 120x34.
fn popup_box(h: &mut Harness) -> (usize, usize) {
    let screen = h.screen(120, 34);
    let line = screen.lines().find(|l| l.contains("\u{256d}\u{2500} ")).expect("the popup").to_string();
    let from = line[..line.find("\u{256d}\u{2500} ").unwrap()].chars().count();
    let to = line.chars().skip(from + 1).position(|c| c == '\u{256e}').expect("its corner") + from + 1;
    (from, to)
}

/// The popup keeps its width while the keys move onto its lines and back (its hint line
/// changes with them).
#[test]
fn the_popup_keeps_its_width_while_the_keys_move() {
    let mut h = users();
    at(&mut h, 0, 1);
    h.keys("F");
    let first = popup_box(&mut h);
    h.key(KeyCode::Up);
    assert_eq!(h.app.column_popup_focus().as_deref(), Some("Hide column"));
    assert_eq!(popup_box(&mut h), first, "on a line");
    h.key(KeyCode::Down);
    assert_eq!(popup_box(&mut h), first, "back on the pane");
}

/// A focused line has the marker every focused line of a popup has.
#[test]
fn a_focused_line_has_the_marker() {
    let mut h = users();
    at(&mut h, 0, 1);
    h.keys("F");
    h.key(KeyCode::Up);
    let screen = h.screen(120, 34);
    assert!(screen.contains("\u{2502} \u{25b8} Hide column"), "{screen}");
    assert!(screen.contains("\u{2502}   Only 'n0'"), "the others unmarked, in line: {screen}");
}

/// The table's columns read after the popup opened, without its column (the catalogue read
/// before the column was added): the popup turns into the whole dialog in place, its draft and
/// undo kept, saying why; its lines go, so nothing acts on a column the dialog does not show.
#[test]
fn a_catalogue_read_later_without_the_column_turns_the_popup_into_the_dialog() {
    use datarig_core::driver::keys::{CatalogColumn, OrderKeys};
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    let ask = h.sent().iter().find_map(|c| match c {
        DbCommand::LoadTableKeys { id, .. } => Some(*id),
        _ => None,
    });
    answer(&mut h);
    h.key(KeyCode::Tab);
    h.app.detail.visible = false;
    at(&mut h, 0, 2);
    h.keys("F");
    assert_eq!(popup_on(&mut h).as_deref(), Some("email"));
    // An edit in the popup: a condition waiting for its value.
    h.key(KeyCode::Char(' '));
    let draft = h.app.filter_dialog_draft();
    let col = |name: &str, ty: &str| CatalogColumn {
        name: name.into(),
        type_name: ty.into(),
        not_null: false,
        primary_key: false,
        foreign_key: false,
        invisible: false,
    };
    let keys = OrderKeys { columns: Some(vec![col("id", "bigint"), col("name", "text")]), ..OrderKeys::default() };
    h.meta_db("local-pg", DbEvent::TableKeys { id: ask.expect("the keys read"), result: Ok(keys) });
    assert!(h.app.overlays.is_open(OverlayKind::FilterDialog), "still open");
    assert_eq!(popup(&h), None, "the whole dialog");
    assert!(lines(&h).is_empty(), "no lines");
    assert_eq!(h.app.filter_dialog_draft(), draft, "the draft kept");
    let screen = h.screen(120, 34);
    assert!(screen.contains("\u{201c}email\u{201d}"), "said: {screen}");
    assert!(runs(&mut h).is_empty());
}
