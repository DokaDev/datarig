//! Editing a cell's value of several lines: a value with line breaks or a JSON column's opens the
//! query editor's widget in a box (`Ctrl+L` takes the cell editor's text there); `Enter` in
//! Normal mode or `:w` keeps the text as the cell editor's `Enter` does, `Esc` or `:q` cancels.

use crate::common;

use common::*;
use datarig_core::driver::{ColumnMeta, DbCommand, DbEvent};
use datarig_core::edit::Value;
use datarig_i18n::{Label, Lang, Msg};
use datarig_tui::app::edit::{EditState, RowAt};
use datarig_tui::app::{Focus, Results};
use datarig_tui::keymap::Ctx;
use datarig_tui::widgets::editor::Mode;
use ratatui::crossterm::event::{Event, KeyCode, MouseButton, MouseEventKind};
use std::time::Duration;

fn s(v: &str) -> Option<String> {
    Some(v.to_string())
}

/// `shop.users`' id, email, nickname and profile (JSON).
fn columns() -> Vec<ColumnMeta> {
    vec![
        meta_from("id", "int8", true, USERS, 1),
        meta_from("email", "text", false, USERS, 2),
        meta_from("nickname", "text", false, USERS, 4),
        meta_from("profile", "jsonb", false, USERS, 5),
    ]
}

/// Three rows: the first two nicknames of two lines each.
fn rows() -> Vec<Vec<Option<String>>> {
    vec![
        vec![s("1"), s("a@x"), s("one\ntwo"), s("{\"a\": 1}")],
        vec![s("2"), s("b@x"), s("x\ny"), None],
        vec![s("3"), s("c@x"), s("cy"), s("{}")],
    ]
}

fn answer(h: &mut Harness, rows: Vec<Vec<Option<String>>>) {
    h.app.run(vec!["SELECT id, email, nickname, profile FROM shop.users".to_string()]);
    let id = h.app.tab().exec.query_id;
    h.db(DbEvent::Page { id, columns: Some(columns()), rows, more: false, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
}

/// Resolved (the structure read on an `e` on the first email, cancelled), the cursor there,
/// nothing sent or said since.
fn ready(lang: Lang) -> Harness {
    ready_with(lang, rows())
}

/// As [`ready`], the grid's rows `rows`.
fn ready_with(lang: Lang, rows: Vec<Vec<Option<String>>>) -> Harness {
    let mut h = Harness::connected(lang);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    answer(&mut h, rows);
    h.keys("le");
    let name = h.app.profiles[0].name.clone();
    let result = Ok(Box::new(users_structure()));
    h.meta_db(&name, DbEvent::Structure { schema: "shop".into(), table: "users".into(), result });
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    h.key(KeyCode::Esc);
    h.app.transient = None;
    h.sent();
    h
}

fn edits(h: &Harness) -> &EditState {
    match &h.app.tab().results {
        Results::Rows(rs) => &rs.edit,
        _ => panic!("no row result"),
    }
}

fn value(h: &Harness, row: usize, col: usize) -> Option<Value> {
    edits(h).batch.value(row, col).cloned()
}

fn text(h: &Harness) -> String {
    h.app.tab().cell_edit.as_ref().expect("the cell editor is open").text()
}

fn mode(h: &Harness) -> Mode {
    h.app.tab().cell_edit.as_ref().and_then(|e| e.lines.as_ref()).expect("the editor of several lines").editor.mode
}

fn said(h: &mut Harness) -> Option<Msg> {
    h.app.transient.take().map(|(n, _)| n.msg)
}

/// `:` and `command` typed, then `Enter`.
fn ex(h: &mut Harness, command: &str) {
    h.keys(":");
    h.type_text(command);
    h.key(KeyCode::Enter);
}

#[test]
fn a_value_of_several_lines_opens_the_editor_of_several_lines_and_enter_keeps_it() {
    let mut h = ready(Lang::En);
    h.keys("le");
    assert_eq!(h.app.key_context(), Ctx::CellEditor);
    assert_eq!(text(&h), "one\ntwo", "the value as read");
    assert_eq!(mode(&h), Mode::Normal);
    // `j` is vim's here, not the grid's.
    h.keys("jA");
    assert_eq!(mode(&h), Mode::Insert);
    h.type_text("!");
    // Enter in Insert mode is a line break; Esc goes to Normal mode, the editor stays.
    h.key(KeyCode::Enter);
    h.type_text("three");
    h.key(KeyCode::Esc);
    assert_eq!((h.app.key_context(), mode(&h)), (Ctx::CellEditor, Mode::Normal));
    assert!(edits(&h).batch.is_empty(), "nothing kept yet");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert_eq!(value(&h, 0, 2), Some(Value::Text("one\ntwo!\nthree".into())), "line breaks as typed, no \\r");
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (0, 2), "the grid's cursor did not move");
    // Opened again: the pending value.
    h.keys("e");
    assert_eq!(text(&h), "one\ntwo!\nthree");
}

#[test]
fn a_json_column_opens_it_whatever_its_value_and_esc_cancels() {
    let mut h = ready(Lang::En);
    // `{}`, one line, of a JSON column.
    h.keys("jjlle");
    assert_eq!(h.app.key_context(), Ctx::CellEditor);
    assert_eq!(text(&h), "{}");
    h.keys("x");
    assert_eq!(text(&h), "}");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert!(edits(&h).batch.is_empty(), "Esc changes nothing");
    // NULL: empty.
    h.keys("ke");
    assert_eq!(text(&h), "");
    // Esc from Insert mode goes to Normal mode; a second one cancels.
    h.keys("i");
    h.type_text("[1]");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.key_context(), Ctx::CellEditor);
    h.key(KeyCode::Esc);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert!(edits(&h).batch.is_empty());
}

#[test]
fn w_keeps_and_q_cancels() {
    let mut h = ready(Lang::En);
    // The JSON column: always in several lines.
    h.keys("lle");
    h.keys("dd");
    ex(&mut h, "w");
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert_eq!(value(&h, 0, 3), Some(Value::Text(String::new())), "empty: the empty string");
    for keep in ["wq", "x"] {
        h.keys("e");
        h.keys("A");
        h.type_text(keep);
        h.key(KeyCode::Esc);
        ex(&mut h, keep);
        assert_eq!(h.app.key_context(), Ctx::Grid, ":{keep}");
    }
    assert_eq!(value(&h, 0, 3), Some(Value::Text("wqx".into())));
    for cancel in ["q", "q!"] {
        h.keys("e");
        h.keys("dd");
        ex(&mut h, cancel);
        assert_eq!(h.app.key_context(), Ctx::Grid, ":{cancel}");
    }
    assert_eq!(value(&h, 0, 3), Some(Value::Text("wqx".into())), "cancelled: unchanged");
    // The editor's own commands run on its text; `Esc` closes the `:` line only.
    h.keys("e");
    ex(&mut h, "s/w/W/g");
    assert_eq!(text(&h), "Wqx");
    h.keys(":");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.key_context(), Ctx::CellEditor);
    ex(&mut h, "nope");
    assert!(matches!(said(&mut h), Some(Msg::EditorExUnsupported { .. })));
    assert_eq!(h.app.key_context(), Ctx::CellEditor);
}

#[test]
fn a_value_not_of_the_columns_type_keeps_it_open_with_the_reason() {
    let mut h = ready(Lang::En);
    // The id, in the cell editor's line, taken to several lines.
    h.keys("he");
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    h.ctrl('l');
    assert_eq!((h.app.key_context(), mode(&h)), (Ctx::CellEditor, Mode::Insert));
    h.key(KeyCode::Enter);
    h.type_text("2");
    h.key(KeyCode::Esc);
    h.key(KeyCode::Enter);
    assert_eq!(h.app.key_context(), Ctx::CellEditor, "refused: it stays open");
    assert_eq!(text(&h), "1\n2", "with the text");
    let reason = h.app.tab().cell_edit.as_ref().and_then(|e| e.error.clone()).expect("why");
    assert_eq!(said(&mut h), Some(Msg::EditRefused { reason: reason.clone() }));
    // Said for as long as it is open.
    let status = h.status(120, 40);
    assert!(status.contains(&reason), "{status}");
    ex(&mut h, "w");
    assert_eq!(h.app.key_context(), Ctx::CellEditor);
    // An edit forgets the reason.
    h.keys("jdd");
    assert_eq!(h.app.tab().cell_edit.as_ref().and_then(|e| e.error.clone()), None);
    h.key(KeyCode::Enter);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert!(edits(&h).batch.is_empty(), "1 as read: no change");
}

#[test]
fn ctrl_l_takes_the_lines_text_to_several_lines() {
    let mut h = ready(Lang::En);
    h.keys("e");
    h.type_text("y");
    h.ctrl('l');
    assert_eq!(h.app.key_context(), Ctx::CellEditor);
    assert_eq!((text(&h), mode(&h)), ("a@xy".to_string(), Mode::Insert), "typing on at its end");
    h.key(KeyCode::Enter);
    h.type_text("z");
    h.event(Event::Paste("p\r\nq".into()));
    h.key(KeyCode::Esc);
    h.key(KeyCode::Enter);
    assert_eq!(value(&h, 0, 1), Some(Value::Text("a@xy\nzp\nq".into())));
}

#[test]
fn ctrl_l_shows_on_the_cell_editors_hint_line() {
    let mut h = ready(Lang::En);
    h.keys("e");
    let status = h.status(160, 45);
    assert!(status.contains("Ctrl+L several lines"), "{status}");
}

#[test]
fn cells_selected_in_one_column_take_the_text_as_one_change() {
    let mut h = ready(Lang::En);
    h.keys("lvj");
    h.keys("e");
    assert_eq!(text(&h), "x\ny", "the cursor's value");
    h.keys("A");
    h.type_text("z");
    h.key(KeyCode::Esc);
    ex(&mut h, "w");
    let kept = Some(Value::Text("xz\ny".into()));
    assert_eq!((value(&h, 0, 2), value(&h, 1, 2)), (kept.clone(), kept));
    h.keys("u");
    assert!(edits(&h).batch.is_empty(), "one undo step");
}

#[test]
fn a_new_rows_cell_takes_several_lines() {
    let mut h = ready(Lang::En);
    h.keys("o");
    h.app.transient = None;
    let id = edits(&h).batch.drafts()[0].id;
    h.app.tab_mut().grid.col = 2;
    h.keys("e");
    assert_eq!(h.app.key_context(), Ctx::CellEdit, "DEFAULT: an empty line");
    h.ctrl('l');
    let at = h.app.tab().cell_edit.as_ref().map(|e| e.at);
    assert_eq!(at, Some((RowAt::New(id), 2)));
    let screen = h.screen(120, 40);
    assert!(screen.contains("Edit nickname (new row)"), "{screen}");
    h.type_text("a");
    h.key(KeyCode::Enter);
    h.type_text("b");
    h.key(KeyCode::Esc);
    ex(&mut h, "w");
    assert_eq!(edits(&h).batch.drafts()[0].cells[2], Value::Text("a\nb".into()));
}

#[test]
fn a_click_outside_closes_it_and_the_wheel_too_changing_nothing() {
    let mut h = ready(Lang::En);
    h.keys("le");
    h.keys("x");
    h.draw(120, 40);
    let (area, text_area) = {
        let l = h.app.tab().cell_edit.as_ref().and_then(|e| e.lines.as_ref()).expect("open");
        (l.area, l.text_area)
    };
    // Inside, on the text's second line: the cursor goes there, the box stays.
    h.mouse(MouseEventKind::Down(MouseButton::Left), text_area.x + 5, text_area.y + 1);
    h.mouse(MouseEventKind::Up(MouseButton::Left), text_area.x + 5, text_area.y + 1);
    assert_eq!(h.app.key_context(), Ctx::CellEditor);
    let row = h.app.tab().cell_edit.as_ref().and_then(|e| e.lines.as_ref()).map(|l| l.editor.row);
    assert_eq!(row, Some(1));
    h.mouse(MouseEventKind::Down(MouseButton::Left), area.x.saturating_sub(2), area.y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), area.x.saturating_sub(2), area.y);
    assert!(h.app.tab().cell_edit.is_none(), "closed");
    assert!(edits(&h).batch.is_empty(), "nothing changed");
    // The press went on to the explorer: back to the grid's cell.
    assert_eq!(h.app.focus, Focus::Tree);
    h.app.focus = Focus::Results;
    (h.app.tab_mut().grid.row, h.app.tab_mut().grid.col) = (0, 2);
    h.keys("e");
    h.draw(120, 40);
    let l = h.app.tab().cell_edit.as_ref().and_then(|e| e.lines.as_ref()).map(|l| l.area).expect("open");
    h.mouse(MouseEventKind::ScrollDown, l.x + 2, l.y + 2);
    assert!(h.app.tab().cell_edit.is_none(), "the wheel closed it");
    assert!(edits(&h).batch.is_empty());
}

#[test]
fn it_closes_when_its_result_goes() {
    let mut h = ready(Lang::En);
    h.keys("le");
    h.keys("x");
    assert_eq!(h.app.key_context(), Ctx::CellEditor);
    // The query runs again: another result.
    answer(&mut h, rows());
    assert!(h.app.tab().cell_edit.is_none());
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert!(h.sent().iter().all(|c| !matches!(c, DbCommand::LoadStructure { .. })));
}

#[test]
fn a_yank_goes_to_the_clipboard_as_the_editors_setting_says() {
    let mut h = ready(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("le");
    h.keys("yy");
    assert_eq!(clip.last().as_deref(), Some("one\n"));
    h.app.prefs.editor_clipboard = datarig_core::config::EditorClipboard::Off;
    h.keys("jyy");
    assert_eq!(clip.all(), ["one\n"], "off: the editor's registers only");
    h.keys("p");
    assert_eq!(text(&h), "one\ntwo\ntwo");
}

/// The box in Normal mode (as opened) and in Insert mode, at both sizes, English pinned and
/// Korean checked.
#[test]
fn the_editor_of_several_lines_shows_over_the_screen() {
    for (name, insert) in [("edit_lines_normal", false), ("edit_lines_insert", true)] {
        for lang in [Lang::En, Lang::Ko] {
            let mut h = ready(lang);
            h.keys("le");
            if insert {
                h.keys("jA");
                h.type_text(" three");
            }
            let tag = format!("{lang:?}").to_lowercase();
            for (w, hh) in [(120, 40), (80, 24)] {
                assert_screen!(format!("{name}_{tag}_{w}x{hh}"), lang, h.draw(w, hh));
            }
        }
    }
}

#[test]
fn the_keys_read_in_korean() {
    let mut h = ready(Lang::Ko);
    h.keys("le");
    let screen = h.screen(80, 24);
    assert!(screen.contains(&ko(Label::EditLinesKeys).to_string()), "{screen}");
    assert!(screen.contains(&ko(Label::StatusModeNormal).to_string()), "{screen}");
}

/// The first two rows' nicknames: `first` and `second`.
fn nicknames(first: &str, second: &str) -> Vec<Vec<Option<String>>> {
    let mut rows = rows();
    rows[0][2] = s(first);
    rows[1][2] = s(second);
    rows
}

#[test]
fn a_value_of_windows_line_breaks_keeps_them() {
    let mut h = ready_with(Lang::En, nicknames("one\r\ntwo", "a\r\nb\r\n"));
    h.keys("le");
    assert_eq!(h.app.key_context(), Ctx::CellEditor);
    // Typed at the first line's end, and a line opened below it.
    h.keys("A");
    h.type_text("X");
    h.key(KeyCode::Esc);
    h.keys("o");
    h.type_text("new");
    h.key(KeyCode::Esc);
    h.key(KeyCode::Enter);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert_eq!(value(&h, 0, 2), Some(Value::Text("oneX\r\nnew\r\ntwo".into())), "\\r\\n throughout");
    // Kept as it was: no change.
    h.keys("je");
    assert_eq!(h.app.key_context(), Ctx::CellEditor);
    h.key(KeyCode::Enter);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert_eq!(value(&h, 1, 2), None, "unchanged");
}

#[test]
fn a_value_of_mixed_line_breaks_is_not_edited() {
    for value in ["one\r\ntwo\nthree", "one\rtwo"] {
        let mut h = ready_with(Lang::En, nicknames(value, "x"));
        h.keys("le");
        assert_eq!(h.app.key_context(), Ctx::Grid, "{value:?}");
        let Some(Msg::EditRefused { reason }) = said(&mut h) else { panic!("refused: {value:?}") };
        assert_eq!(reason, "nickname mixes kinds of line break; it can't be edited here");
        assert!(edits(&h).batch.is_empty());
    }
}

#[test]
fn ctrl_k_does_not_open_the_command_line_over_it() {
    let mut h = ready(Lang::En);
    h.keys("lle");
    h.ctrl('k');
    assert_eq!(h.overlay_kind(), None, "no command line");
    assert_eq!(h.app.key_context(), Ctx::CellEditor);
}
