//! A range pasted into the result grid: the terminal's paste (it reads no clipboard) or
//! `space d v` (the system clipboard, only with `clipboard = "auto"` or `"system"`), laid over
//! the grid from the cursor or the selection's corner as one step `u` undoes; refused whole,
//! changing nothing, when it does not fit; values a column does not take kept and marked, and
//! the changes not applied while any is; `space d V` reads the last paste again as the next
//! format; a paste of many rows asks first.

mod common;

use common::*;
use datarig_core::config::{ClipboardSetting, Config};
use datarig_core::driver::structure::{ColumnFill, TableStructure};
use datarig_core::driver::{ColumnMeta, DbCommand, DbEvent};
use datarig_core::edit::Value;
use datarig_core::i18n::{Label, Lang, Msg};
use datarig_core::policy::Policy;
use datarig_core::profile::ConnectionConfig;
use datarig_tui::app::Focus;
use datarig_tui::app::Results;
use datarig_tui::app::action::Action;
use datarig_tui::app::edit::{ANSWER_GUARD, Applied, EditState, RowAt};
use datarig_tui::app::paging::Paging;
use datarig_tui::app::tabs::ResultView;
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use std::time::Duration;

fn s(v: &str) -> Option<String> {
    Some(v.to_string())
}

fn t(v: &str) -> Value {
    Value::Text(v.to_string())
}

fn users_columns() -> Vec<ColumnMeta> {
    vec![
        meta_from("id", "int8", true, USERS, 1),
        meta_from("email", "text", false, USERS, 2),
        meta_from("nickname", "text", false, USERS, 4),
    ]
}

fn users_rows() -> Vec<Vec<Option<String>>> {
    vec![vec![s("1"), s("a@x"), s("ann")], vec![s("2"), s("b@x"), None], vec![s("3"), s("c@x"), s("cy")]]
}

const SQL: &str = "SELECT id, email, nickname FROM shop.users";

fn structure(h: &mut Harness, st: TableStructure) {
    let name = h.app.profiles[0].name.clone();
    h.meta_db(&name, DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Ok(Box::new(st)) });
}

/// Run the users' query and let its rows come (`more`: the server has more), the grid focused.
fn answer(h: &mut Harness, more: bool) {
    h.db(DbEvent::Keys(Ok(shop_keys())));
    h.app.run(vec![SQL.to_string()]);
    let id = h.app.tab().exec.query_id;
    h.db(DbEvent::Page { id, columns: Some(users_columns()), rows: users_rows(), more, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
}

/// `h` with the users' rows, resolved with `st` (`e` on the first row's email, then the editor
/// closed: the cursor stays there); nothing sent, nothing said.
fn resolved(mut h: Harness, st: TableStructure, more: bool) -> Harness {
    answer(&mut h, more);
    h.keys("le");
    structure(&mut h, st);
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    h.key(KeyCode::Esc);
    h.app.transient = None;
    h.sent();
    h
}

/// `shop.users` whose `name` (NOT NULL, not in the results here) has a default: new rows can be
/// added.
fn named() -> TableStructure {
    let mut st = users_structure();
    st.columns[2].default = Some("''::text".into());
    st
}

fn ready(lang: Lang) -> Harness {
    resolved(Harness::connected(lang), named(), false)
}

fn edits(h: &Harness) -> &EditState {
    match &h.app.tab().results {
        Results::Rows(rs) => &rs.edit,
        _ => panic!("no row result"),
    }
}

fn revision(h: &Harness) -> u64 {
    edits(h).batch.revision()
}

fn said(h: &mut Harness) -> String {
    let n = h.app.transient.take().map(|(n, _)| n).expect("something said");
    n.render(&h.app.i18n).to_string()
}

fn paste(h: &mut Harness, text: &str) {
    h.event(Event::Paste(text.to_string()));
}

/// The value pending in row `r`, column `c` of the rows read.
fn pending(h: &Harness, r: usize, c: usize) -> Option<Value> {
    edits(h).batch.value(r, c).cloned()
}

/// The cells of the new rows, in order.
fn drafts(h: &Harness) -> Vec<Vec<Value>> {
    edits(h).batch.drafts().iter().map(|d| d.cells.clone()).collect()
}

fn screen(h: &mut Harness, w: u16, hh: u16) -> String {
    buffer_text(h.draw(w, hh).backend().buffer())
}

/// The grid line of row `r` (from 0) as drawn 80×24.
fn grid_line(h: &mut Harness, r: usize) -> String {
    let screen = screen(h, 80, 24);
    let y = h.app.tab().grid.data_y as usize + r;
    screen.lines().nth(y).unwrap_or_default().to_string()
}

/// Type `v` into the cursor's cell.
fn type_cell(h: &mut Harness, v: &str) {
    h.keys("e");
    let n = h.app.tab().cell_edit.as_ref().map_or(0, |e| e.input.text().chars().count());
    for _ in 0..n {
        h.key(KeyCode::Backspace);
    }
    h.type_text(v);
    h.key(KeyCode::Enter);
}

fn clipboard(h: &mut Harness) -> FakeClipboard {
    FakeClipboard::attach(h, false, &[])
}

#[test]
fn a_terminal_paste_sets_the_cells_from_the_cursor_and_reads_no_clipboard() {
    let mut h = ready(Lang::En);
    let c = clipboard(&mut h);
    let before = revision(&h);
    paste(&mut h, "x@x\tbo\ny@y\t\\N\n");
    assert_eq!(pending(&h, 0, 1), Some(t("x@x")));
    assert_eq!(pending(&h, 0, 2), Some(t("bo")));
    assert_eq!(pending(&h, 1, 1), Some(t("y@y")));
    // Row 2's nickname is NULL already: no change.
    assert_eq!(pending(&h, 1, 2), None);
    assert_eq!(pending(&h, 2, 1), None);
    assert_eq!(revision(&h), before + 1, "one step");
    assert_eq!(said(&mut h), "Pasted as TSV · 2 rows × 2 columns · 3 cells changed · u undo");
    assert_eq!(c.read_count(), 0, "a terminal's paste reads no clipboard");
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (0, 1));
    // `u` undoes all of it.
    h.keys("u");
    assert!(edits(&h).batch.is_empty());
    assert_eq!(c.read_count(), 0);
}

#[test]
fn a_paste_on_a_result_not_resolved_yet_waits_for_its_table() {
    let mut h = Harness::connected(Lang::En);
    answer(&mut h, false);
    h.keys("l");
    paste(&mut h, "x@x");
    assert!(edits(&h).batch.is_empty(), "the table is read first");
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::LoadStructure { .. })));
    structure(&mut h, named());
    assert_eq!(pending(&h, 0, 1), Some(t("x@x")));
}

#[test]
fn a_block_goes_to_the_selections_corner_and_says_so() {
    let mut h = ready(Lang::En);
    // From row 2's email to row 3's nickname.
    h.keys("jvjl");
    paste(&mut h, "p\tq\n");
    assert_eq!(pending(&h, 1, 1), Some(t("p")));
    assert_eq!(pending(&h, 1, 2), Some(t("q")));
    assert_eq!(pending(&h, 2, 1), None, "the selection's size is not the paste's");
    let text = said(&mut h);
    assert!(text.contains("1 row × 2 columns · pasted 1×2 at the selection's corner · 2 cells changed"), "{text}");
    assert!(h.app.tab().grid.anchor.is_none(), "the selection is dropped");
}

#[test]
fn one_value_fills_every_selected_cell() {
    let mut h = ready(Lang::En);
    h.keys("lvjj");
    paste(&mut h, "zed\n");
    for r in 0..3 {
        assert_eq!(pending(&h, r, 2), Some(t("zed")), "row {r}");
    }
    assert_eq!(said(&mut h), "Pasted as one value · 3 rows × 1 column · 3 cells changed · u undo");
}

#[test]
fn rows_past_the_last_are_new_rows_with_defaults() {
    let mut h = ready(Lang::En);
    h.keys("jj");
    paste(&mut h, "c2@x\tc2\nd@x\td\ne@x\t\\N\n");
    assert_eq!(pending(&h, 2, 1), Some(t("c2@x")));
    assert_eq!(pending(&h, 2, 2), Some(t("c2")));
    assert_eq!(drafts(&h), vec![vec![Value::Default, t("d@x"), t("d")], vec![Value::Default, t("e@x"), Value::Null]]);
    let text = said(&mut h);
    assert!(text.contains("3 rows × 2 columns · 2 cells changed · 2 new rows · u undo"), "{text}");
    // A paste over the new rows sets their cells.
    h.keys("j");
    paste(&mut h, "dd@x");
    assert_eq!(drafts(&h)[0][1], t("dd@x"));
    // One undo each.
    h.keys("u");
    assert_eq!(drafts(&h)[0][1], t("d@x"));
    h.keys("u");
    assert!(edits(&h).batch.is_empty());
}

/// A refusal: said as `reason`, the changes as they were.
fn refused(h: &mut Harness, text: &str, reason: &str) {
    let before = revision(h);
    let batch = edits(h).batch.clone();
    paste(h, text);
    assert_eq!(said(h), format!("Not pasted: {reason}"));
    assert_eq!(revision(h), before, "nothing changed");
    assert_eq!(edits(h).batch, batch);
}

#[test]
fn a_paste_wider_than_the_columns_left_is_refused() {
    let mut h = ready(Lang::En);
    refused(&mut h, "a\tb\tc\n", "the paste is 3 columns wide; 2 columns from the cursor to the last column");
}

#[test]
fn a_paste_over_a_column_that_takes_no_value_is_refused() {
    let mut st = named();
    st.columns[3].fill = ColumnFill::Stored("lower(email)".into());
    let mut h = resolved(Harness::connected(Lang::En), st, false);
    refused(&mut h, "a\tb\n", "nickname is generated by the database");
}

#[test]
fn a_paste_over_a_row_marked_for_deletion_is_refused() {
    let mut h = ready(Lang::En);
    h.keys("jDk");
    refused(&mut h, "a\nb\n", "row 2 is marked for deletion");
}

#[test]
fn a_paste_past_the_last_row_read_while_the_server_has_more_is_refused() {
    let mut h = resolved(Harness::connected(Lang::En), named(), true);
    h.keys("jj");
    refused(&mut h, "a\nb\n", "the result has more rows on the server; fetch them (n) or paste fewer rows");
    // Within the rows read it goes.
    h.keys("k");
    paste(&mut h, "a\nb\n");
    assert_eq!(pending(&h, 2, 1), Some(t("b")));
}

#[test]
fn a_paste_past_the_end_where_no_row_can_be_added_is_refused() {
    // `name` is NOT NULL without a default, and not in the result.
    let mut h = resolved(Harness::connected(Lang::En), users_structure(), false);
    h.keys("jj");
    refused(
        &mut h,
        "a\nb\n",
        "new rows can't be added here: name is NOT NULL, has no default and is not in the result",
    );
}

#[test]
fn a_paste_into_a_read_only_result_or_one_being_applied_is_refused() {
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("prod".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert("prod", Policy { read_only: true, ..Policy::default() });
    let mut h = Harness::connected_with(&cfg, Lang::En);
    answer(&mut h, false);
    paste(&mut h, "x");
    assert_eq!(said(&mut h), "Read-only: the connection's environment is read-only");
    assert!(edits(&h).batch.is_empty());
    // Being applied.
    let mut h = ready(Lang::En);
    paste(&mut h, "z@x");
    h.keys(" da");
    h.key(KeyCode::Char('y'));
    assert!(edits(&h).applying.is_some());
    h.app.transient = None;
    let before = revision(&h);
    paste(&mut h, "q@x");
    assert_eq!(said(&mut h), format!("Can't edit: {}", h.app.i18n.label(Label::EditWhyApplying)));
    assert_eq!(revision(&h), before);
}

#[test]
fn text_that_cannot_be_read_is_refused_and_space_d_capital_v_reads_it_as_one_value() {
    let mut h = ready(Lang::En);
    refused(&mut h, "a\tb\nc\n", "row 2 has 1 value, row 1 has 2 · Space d V reads it again as one value");
    // A lone quote: not closed.
    refused(
        &mut h,
        "\"",
        "row 1 has a quote that is not closed, or text after a closing quote · Space d V reads it again as one value",
    );
    h.keys(" dV");
    assert_eq!(pending(&h, 0, 1), Some(t("\"")));
    assert!(said(&mut h).starts_with("Pasted as one value"));
}

#[test]
fn the_paste_tab_of_changes_says_to_paste_into_the_grid() {
    let mut h = ready(Lang::En);
    paste(&mut h, "z@x");
    h.keys(" dp");
    assert_eq!(h.app.tab().exec.view, ResultView::Changes);
    h.app.transient = None;
    let before = revision(&h);
    paste(&mut h, "q@x");
    assert_eq!(said(&mut h), "Paste into the grid");
    assert_eq!(revision(&h), before);
}

#[test]
fn space_d_capital_v_reads_the_last_paste_again_in_its_place() {
    let mut h = ready(Lang::En);
    paste(&mut h, "q@x,qq\n");
    h.keys("c");
    assert_eq!((pending(&h, 0, 1), pending(&h, 0, 2)), (Some(t("q@x")), Some(t("qq"))));
    assert!(said(&mut h).starts_with("Pasted as CSV"));
    h.keys(" dV");
    assert_eq!((pending(&h, 0, 1), pending(&h, 0, 2)), (Some(t("q@x,qq")), None), "one value, the CSV undone");
    assert!(said(&mut h).starts_with("Pasted as one value"));
    h.keys(" dV");
    assert!(said(&mut h).starts_with("Pasted as TSV"));
    assert_eq!(pending(&h, 0, 1), Some(t("q@x,qq")));
    // Another change since: nothing to read again.
    h.keys("j");
    paste(&mut h, "r@x");
    h.keys("k");
    type_cell(&mut h, "s@x");
    h.app.transient = None;
    let before = revision(&h);
    h.keys(" dV");
    assert_eq!(said(&mut h), "Nothing to re-read: the last change was not a paste");
    assert_eq!(revision(&h), before);
}

/// An invalid pasted value's control characters are spelled out once, wherever it is said.
#[test]
fn an_invalid_pasted_value_says_its_control_characters_once() {
    let mut h = ready(Lang::En);
    h.keys("h");
    paste(&mut h, "a\u{7}b\\n");
    let why = r"not a whole number: 'a\x07b\\n'";
    let inv = h.app.shown_invalid().expect("resolved");
    assert_eq!(inv.cells.get(&(RowAt::Read(0), 0)).map(String::as_str), Some(why));
    let m = h.app.shown_changes().expect("a model");
    assert_eq!(m.problems[0].text, format!("id=1 \u{B7} id: {why}"));
    assert_eq!(m.problems[0].bare, format!("id: {why}"));
}

#[test]
fn invalid_values_are_kept_marked_listed_first_and_block_the_apply() {
    let mut h = ready(Lang::En);
    h.keys("h");
    paste(&mut h, "abc\n\n");
    assert_eq!(pending(&h, 0, 0), Some(t("abc")));
    assert_eq!(pending(&h, 1, 0), Some(t("")));
    let text = said(&mut h);
    assert!(text.contains("2 rows × 1 column · 2 invalid (first: id=1 · id) · 2 cells changed · u undo"), "{text}");
    let inv = h.app.shown_invalid().expect("resolved");
    assert_eq!(inv.cells.get(&(RowAt::Read(0), 0)).map(String::as_str), Some("not a whole number: 'abc'"));
    assert_eq!(inv.cells.get(&(RowAt::Read(1), 0)).map(String::as_str), Some("empty value — paste \\N for NULL"));
    // The gutter's `!`.
    assert!(grid_line(&mut h, 0).contains("│ 1!│   abc │"), "{:?}", grid_line(&mut h, 0));
    // The Changes tab lists them first, with the reason.
    let m = h.app.shown_changes().expect("a model");
    assert_eq!(m.problems[0].text, "id=1 · id: not a whole number: 'abc'");
    assert_eq!(m.problems[1].text, "id=2 · id: empty value — paste \\N for NULL");
    // Listed under their rows, without the rows' names.
    assert_eq!(m.problems[0].bare, "id: not a whole number: 'abc'");
    // The Apply is refused: the problems shown, nothing sent.
    h.sent();
    h.keys(" da");
    assert!(h.app.status_ask_line().is_none());
    assert_eq!(h.app.tab().exec.view, ResultView::Changes);
    assert!(!h.sent().iter().any(|c| matches!(c, DbCommand::ApplyEdits { .. })));
    // `u` undoes the whole paste: nothing is invalid.
    h.keys("u");
    assert!(h.app.shown_invalid().unwrap().cells.is_empty());
}

#[test]
fn an_invalid_value_is_fixed_in_the_cell_editor_which_says_why() {
    let mut h = ready(Lang::En);
    h.keys("h");
    paste(&mut h, "abc");
    h.app.transient = None;
    h.keys("e");
    let e = h.app.tab().cell_edit.as_ref().expect("the editor");
    assert_eq!(e.input.text(), "abc", "the pasted text");
    assert_eq!(e.error.as_deref(), Some("not a whole number: 'abc'"));
    // Said as the value's reason, not as a refusal: the editor is open.
    let line = h.app.status_line().map(|(m, _, _)| m.to_string());
    assert_eq!(line.as_deref(), Some("Not valid: not a whole number: 'abc'"));
    for _ in 0..3 {
        h.key(KeyCode::Backspace);
    }
    h.type_text("7");
    h.key(KeyCode::Enter);
    assert_eq!(pending(&h, 0, 0), Some(t("7")));
    assert!(h.app.shown_invalid().unwrap().cells.is_empty(), "the mark is gone");
}

#[test]
fn a_null_pasted_into_a_not_null_column_is_invalid_and_the_token_is_a_setting() {
    let mut h = ready(Lang::En);
    paste(&mut h, "\\N");
    assert_eq!(pending(&h, 0, 1), Some(Value::Null));
    let inv = h.app.shown_invalid().unwrap();
    assert_eq!(inv.cells.get(&(RowAt::Read(0), 1)).map(String::as_str), Some("email is NOT NULL"));
    h.keys("u");
    h.command("set edit.paste_null=NULL");
    assert_eq!(h.app.prefs.paste_null, datarig_core::config::PasteNull::Null);
    assert_eq!(said(&mut h), "Paste NULL token: The word NULL");
    h.app.focus = Focus::Results;
    paste(&mut h, "NULL");
    assert_eq!(pending(&h, 0, 1), Some(Value::Null));
    h.keys("u");
    paste(&mut h, "\\N");
    assert_eq!(pending(&h, 0, 1), Some(t("\\N")), "no longer the token");
}

#[test]
fn a_paste_of_more_than_a_thousand_rows_asks_first_and_any_other_key_cancels() {
    let mut h = ready(Lang::En);
    h.keys("l");
    let text: String = (0..1_001).map(|i| format!("n{i}\n")).collect();
    let before = revision(&h);
    paste(&mut h, &text);
    assert_eq!(h.app.key_context(), Ctx::StatusAsk);
    let forms = h.app.status_ask_line().map(|(f, _)| f).unwrap_or_default();
    assert_eq!(forms[0], "Paste 1,001 rows (998 new)? y paste · any other key cancels");
    assert_eq!(revision(&h), before, "nothing before y");
    // Another key cancels, and does nothing else.
    let cursor = (h.app.tab().grid.row, h.app.tab().grid.col);
    h.keys("j");
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), cursor, "the key was taken");
    assert_eq!(said(&mut h), "Paste cancelled");
    assert_eq!(revision(&h), before);
    // A paste while it asks cancels it too, and is not pasted.
    paste(&mut h, &text);
    paste(&mut h, "zz");
    assert_eq!(said(&mut h), "Paste cancelled");
    assert_eq!(revision(&h), before);
    // `y` pastes.
    paste(&mut h, &text);
    h.keys("y");
    assert_eq!(revision(&h), before + 1);
    assert_eq!(drafts(&h).len(), 998);
    assert_eq!(pending(&h, 0, 2), Some(t("n0")));
    // A change while it asks drops it.
    h.keys("u");
    paste(&mut h, &text);
    h.app.transient = None;
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        rs.edit.batch.set(&[(0, 1)], &t("w@x"), |_| vec![s("1"), s("a@x"), s("ann")]);
    }
    // Any event sweeps the question.
    h.db(DbEvent::Keys(Ok(shop_keys())));
    assert!(h.app.status_ask_line().is_none());
    assert_eq!(said(&mut h), "Paste cancelled: the rows changed");
}

#[test]
fn space_d_v_reads_the_system_clipboard_only_when_the_setting_allows() {
    let mut h = ready(Lang::En);
    let c = clipboard(&mut h);
    c.hold("q@x\tqq");
    h.app.prefs.clipboard = ClipboardSetting::System;
    h.keys(" dv");
    assert_eq!(c.read_count(), 1);
    assert_eq!((pending(&h, 0, 1), pending(&h, 0, 2)), (Some(t("q@x")), Some(t("qq"))));
    assert!(said(&mut h).starts_with("Pasted as TSV"));
    // With OSC 52 it is never read.
    let mut h = ready(Lang::En);
    let c = clipboard(&mut h);
    c.hold("q@x");
    h.app.prefs.clipboard = ClipboardSetting::Osc52;
    h.keys(" dv");
    assert_eq!(said(&mut h), "Reading the clipboard needs clipboard = \"system\"; paste with your terminal instead");
    assert_eq!(c.read_count(), 0);
    assert!(edits(&h).batch.is_empty());
}

#[test]
fn the_badge_counts_the_invalid_values() {
    let mut h = ready(Lang::En);
    h.keys("h");
    paste(&mut h, "abc\n9\n");
    let wide = screen(&mut h, 220, 30);
    assert!(wide.contains("2 pending · 1 invalid"), "{wide}");
    // Short where the whole badge does not fit.
    let narrow = screen(&mut h, 120, 30);
    assert!(narrow.contains("Messages      2 · 1!"), "{narrow}");
    let badge = h.app.i18n.msg(&Msg::EditBadgePendingInvalidShort { pending: "2".into(), count: 1 }).to_string();
    assert_eq!(badge, "2 · 1!");
}

/// The grid with an invalid value marked, the Changes tab with its problem first and the status
/// line, at 80x24 and 120x40.
#[test]
fn snapshots_of_a_paste_with_an_invalid_value() {
    for (w, hh) in [(80, 24), (120, 40)] {
        let mut h = ready(Lang::En);
        h.keys("h");
        paste(&mut h, "abc\tz@x\n9\ty@x\n");
        insta::assert_snapshot!(format!("paste_grid_en_{w}x{hh}"), screen(&mut h, w, hh));
        h.app.transient = None;
        h.keys(" dp");
        insta::assert_snapshot!(format!("paste_changes_en_{w}x{hh}"), screen(&mut h, w, hh));
    }
}

/// Keys `keys` handed to the app without the answers of the reads they start.
fn keys_now(h: &mut Harness, keys: &str) {
    for c in keys.chars() {
        let m = if c.is_ascii_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
        h.app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(c), m)));
    }
}

/// The status line as drawn `w` columns wide (24 rows).
fn status_at(h: &mut Harness, w: u16) -> String {
    screen(h, w, 24).lines().last().unwrap_or_default().to_string()
}

#[test]
fn a_reread_that_is_refused_puts_the_paste_back_as_it_was() {
    let mut h = ready(Lang::En);
    // Two columns wide as TSV, three as CSV.
    paste(&mut h, "a\tb,c,d");
    assert_eq!((pending(&h, 0, 1), pending(&h, 0, 2)), (Some(t("a")), Some(t("b,c,d"))));
    h.app.transient = None;
    let batch = edits(&h).batch.clone();
    h.keys(" dV");
    assert_eq!(said(&mut h), "Not pasted: the paste is 3 columns wide; 2 columns from the cursor to the last column");
    assert_eq!(edits(&h).batch, batch, "the TSV paste is back, its history too");
    assert_eq!((pending(&h, 0, 1), pending(&h, 0, 2)), (Some(t("a")), Some(t("b,c,d"))));
    // It is still the paste read again: as the format after CSV, in its place.
    h.keys(" dV");
    assert!(said(&mut h).starts_with("Pasted as one value"));
    assert_eq!((pending(&h, 0, 1), pending(&h, 0, 2)), (Some(t("a\tb,c,d")), None));
    h.keys("u");
    assert!(edits(&h).batch.is_empty(), "one step");
}

#[test]
fn a_reread_of_many_rows_cancelled_or_dropped_puts_the_paste_back() {
    // Two columns as TSV, one as CSV: 1,001 rows either way.
    let text: String = (0..1_001).map(|i| format!("v{i}\tw{i}\n")).collect();
    for how in ["cancel", "sweep"] {
        let mut h = ready(Lang::En);
        paste(&mut h, &text);
        h.keys("y");
        assert!(said(&mut h).starts_with("Pasted as TSV"), "{how}");
        let batch = edits(&h).batch.clone();
        h.keys(" dV");
        assert_eq!(h.app.key_context(), Ctx::StatusAsk, "{how}");
        match how {
            "cancel" => {
                h.keys("j");
                assert_eq!(said(&mut h), "Paste cancelled");
            }
            _ => {
                h.app.dispatch(Action::Help);
                h.db(DbEvent::Keys(Ok(shop_keys())));
                assert!(h.app.status_ask_line().is_none());
                assert_eq!(said(&mut h), "Paste cancelled: the rows changed");
                h.key(KeyCode::Esc);
            }
        }
        assert_eq!(edits(&h).batch, batch, "{how}: the TSV paste is back");
        assert_eq!(pending(&h, 0, 1), Some(t("v0")), "{how}");
    }
}

#[test]
fn a_clipboard_answer_after_an_apply_question_or_the_cell_editor_pastes_nothing() {
    let big: String = (0..1_001).map(|i| format!("n{i}\n")).collect();
    for text in ["q@x", big.as_str()] {
        let mut h = ready(Lang::En);
        let c = clipboard(&mut h);
        h.app.prefs.clipboard = ClipboardSetting::System;
        paste(&mut h, "z@x");
        h.app.transient = None;
        c.hold(text);
        keys_now(&mut h, " dv");
        keys_now(&mut h, " da");
        let asked = h.app.status_ask_line().map(|(f, _)| f).expect("the Apply question");
        let batch = edits(&h).batch.clone();
        h.app.pump_clipboard_reads(Duration::from_secs(30));
        assert_eq!(c.read_count(), 1);
        assert_eq!(edits(&h).batch, batch, "nothing pasted");
        assert_eq!(h.app.status_ask_line().map(|(f, _)| f), Some(asked), "the Apply question stays");
    }
    // The cell editor opened before the answer.
    let mut h = ready(Lang::En);
    let c = clipboard(&mut h);
    h.app.prefs.clipboard = ClipboardSetting::System;
    c.hold("q@x");
    keys_now(&mut h, " dv");
    keys_now(&mut h, "e");
    assert!(h.app.cell_edit_open());
    h.app.pump_clipboard_reads(Duration::from_secs(30));
    assert!(edits(&h).batch.is_empty(), "nothing pasted under the editor");
    assert!(h.app.cell_edit_open());
    assert_eq!(said(&mut h), "The clipboard's text came after the grid moved; nothing pasted");
}

#[test]
fn space_d_capital_v_reads_the_paste_again_once_it_is_the_newest_change_again() {
    let mut h = ready(Lang::En);
    paste(&mut h, "q@x,qq\n");
    h.keys("c");
    h.keys("j");
    type_cell(&mut h, "s@x");
    h.keys("u");
    h.app.transient = None;
    h.keys("k dV");
    assert!(said(&mut h).starts_with("Pasted as one value"));
    assert_eq!((pending(&h, 0, 1), pending(&h, 0, 2)), (Some(t("q@x,qq")), None));
    assert_eq!(pending(&h, 1, 1), None);
    // Two reads again are still one step.
    h.keys(" dV");
    assert!(said(&mut h).starts_with("Pasted as TSV"));
    h.keys("u");
    assert!(edits(&h).batch.is_empty());
}

#[test]
fn one_value_fills_a_selection_from_the_rows_read_into_new_rows_while_the_server_has_more() {
    let mut h = resolved(Harness::connected(Lang::En), named(), true);
    h.app.tab_mut().exec.paging = Paging::Open { since: std::time::Instant::now(), in_block: false };
    h.keys("jjo");
    if h.app.cell_edit_open() {
        h.key(KeyCode::Esc);
    }
    assert_eq!(drafts(&h).len(), 1);
    h.app.transient = None;
    // From row 3's nickname to the new row's.
    let g = &mut h.app.tab_mut().grid;
    (g.row, g.col) = (2, 2);
    h.keys("vj");
    paste(&mut h, "zed");
    assert!(said(&mut h).starts_with("Pasted as one value"));
    assert_eq!(pending(&h, 2, 2), Some(t("zed")));
    assert_eq!(drafts(&h)[0][2], t("zed"));
    // A block starting on the new row may add rows while the server has more.
    h.keys("j");
    paste(&mut h, "p\nq\n");
    assert_eq!(drafts(&h).len(), 2);
    assert_eq!(drafts(&h)[1][2], t("q"));
}

#[test]
fn the_refusal_past_the_rows_read_says_what_to_do_as_the_next_page_would() {
    for (paging, reason) in [
        (
            Paging::Open { since: std::time::Instant::now(), in_block: false },
            "the result has more rows on the server; fetch them (n) or paste fewer rows",
        ),
        (Paging::Released, "the result has more rows on the server, not read here; paste fewer rows"),
    ] {
        let mut h = resolved(Harness::connected(Lang::En), named(), true);
        h.app.tab_mut().exec.paging = paging;
        h.keys("jj");
        refused(&mut h, "a\nb\n", reason);
    }
}

#[test]
fn a_paste_over_an_identity_always_or_a_repeated_column_or_applied_rows_is_refused() {
    let mut st = named();
    st.columns[0].fill = ColumnFill::IdentityAlways;
    let mut h = resolved(Harness::connected(Lang::En), st, false);
    h.keys("h");
    refused(&mut h, "5", "id is GENERATED ALWAYS AS IDENTITY");
    // A column shown twice.
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    h.app.run(vec!["SELECT id, email, email FROM shop.users".to_string()]);
    let id = h.app.tab().exec.query_id;
    let columns = vec![
        meta_from("id", "int8", true, USERS, 1),
        meta_from("email", "text", false, USERS, 2),
        meta_from("email", "text", false, USERS, 2),
    ];
    let rows = vec![vec![s("1"), s("a@x"), s("a@x")]];
    h.db(DbEvent::Page { id, columns: Some(columns), rows, more: false, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
    h.keys("l");
    paste(&mut h, "x@x");
    structure(&mut h, named());
    assert_eq!(said(&mut h), "Not pasted: email is shown more than once");
    assert!(edits(&h).batch.is_empty());
    // Rows an Apply changed.
    let mut h = ready(Lang::En);
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        rs.edit.applied = Some(Box::new(Applied { new: vec![Vec::new()], ..Applied::default() }));
    }
    paste(&mut h, "x@x");
    assert_eq!(said(&mut h), format!("Can't edit: {}", h.app.i18n.label(Label::EditWhyApplied)));
    assert!(edits(&h).batch.is_empty());
}

#[test]
fn a_paste_while_messages_or_a_plan_is_shown_says_to_paste_into_the_grid() {
    let mut h = ready(Lang::En);
    h.app.tab_mut().exec.view = ResultView::Messages;
    paste(&mut h, "q@x");
    assert_eq!(said(&mut h), "Paste into the grid");
    assert!(edits(&h).batch.is_empty());
}

#[test]
fn many_rows_ask_only_when_more_than_a_thousand_change() {
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    h.app.run(vec![SQL.to_string()]);
    let id = h.app.tab().exec.query_id;
    let rows: Vec<Vec<Option<String>>> =
        (0..1_001).map(|i| vec![s(&i.to_string()), s(&format!("{i}@x")), s(&format!("n{i}"))]).collect();
    h.db(DbEvent::Page { id, columns: Some(users_columns()), rows, more: false, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
    h.keys("le");
    structure(&mut h, named());
    h.key(KeyCode::Esc);
    h.keys("l");
    h.app.transient = None;
    // The values already there, but row 1's: one row changes.
    let same: String = (0..1_001).map(|i| if i == 0 { "m0\n".to_string() } else { format!("n{i}\n") }).collect();
    paste(&mut h, &same);
    assert!(h.app.status_ask_line().is_none(), "one row changes");
    assert!(said(&mut h).starts_with("Pasted as one column · 1,001 rows × 1 column · 1 cell changed"));
    h.keys("u");
    let other: String = (0..1_001).map(|i| format!("m{i}\n")).collect();
    paste(&mut h, &other);
    let forms = h.app.status_ask_line().map(|(f, _)| f).expect("asked");
    assert_eq!(forms[0], "Paste 1,001 rows? y paste · any other key cancels");
}

#[test]
fn the_paste_summary_keeps_its_invalid_values_and_undo_key_at_80_columns() {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = ready(lang);
        h.keys("h");
        paste(&mut h, "abc");
        let line = status_at(&mut h, 80);
        let invalid = h.app.i18n.msg(&Msg::EditPasteInvalidCount { count: 1 }).to_string();
        let undo = h.app.i18n.msg(&Msg::EditPasteUndo { key: "u".into() }).to_string();
        assert!(line.contains(&invalid) && line.contains(&undo), "{lang:?}: {line}");
        assert!(!line.contains('\u{2026}'), "{lang:?}: nothing cut: {line}");
    }
    // A refusal's reason reads whole.
    let mut h = ready(Lang::En);
    paste(&mut h, "a\tb\tc\n");
    let line = status_at(&mut h, 80);
    assert!(line.contains("Not pasted: 3 columns wide; 2 fit from the cursor"), "{line}");
}

/// The forms of the status line's question, longest first.
fn asked_forms(h: &Harness) -> Vec<String> {
    h.app.status_ask_line().map(|(f, _)| f).unwrap_or_default()
}

#[test]
fn text_with_commas_asks_csv_or_one_column_and_c_or_1_reads_it_so() {
    let mut h = ready(Lang::En);
    let before = revision(&h);
    paste(&mut h, "q@x,qq\nr@x,rr\n");
    assert_eq!(h.app.key_context(), Ctx::StatusAsk);
    assert_eq!(
        asked_forms(&h),
        [
            "Read as CSV (2 columns) or one column? c / 1 · any other key cancels",
            "Read as CSV (2 columns) or one column? c / 1",
            "CSV (2 columns) or one column? c / 1",
        ]
    );
    assert_eq!(revision(&h), before, "nothing before the answer");
    // `c`: CSV.
    h.keys("c");
    assert!(h.app.status_ask_line().is_none());
    let cells = |h: &Harness| [pending(h, 0, 1), pending(h, 0, 2), pending(h, 1, 1), pending(h, 1, 2)];
    assert_eq!(cells(&h), [Some(t("q@x")), Some(t("qq")), Some(t("r@x")), Some(t("rr"))]);
    assert_eq!(said(&mut h), "Pasted as CSV · 2 rows × 2 columns · 4 cells changed · u undo");
    assert_eq!(revision(&h), before + 1, "one step");
    // `space d V` goes on from CSV.
    h.keys(" dV");
    assert!(said(&mut h).starts_with("Pasted as one value"));
    h.keys("u");
    assert!(edits(&h).batch.is_empty());
    // `1`: one column, the commas the values'.
    paste(&mut h, "q@x,qq\nr@x,rr\n");
    h.keys("1");
    assert_eq!(cells(&h), [Some(t("q@x,qq")), None, Some(t("r@x,rr")), None]);
    assert_eq!(said(&mut h), "Pasted as one column · 2 rows × 1 column · 2 cells changed · u undo");
    h.keys(" dV");
    assert!(said(&mut h).starts_with("Pasted as one value"));
    assert_eq!(pending(&h, 0, 1), Some(t("q@x,qq\nr@x,rr")));
}

#[test]
fn a_line_with_commas_asks_csv_or_one_value_and_any_other_input_cancels_it() {
    let mut h = ready(Lang::En);
    let before = revision(&h);
    let cursor = |h: &Harness| (h.app.tab().grid.row, h.app.tab().grid.col);
    let at = cursor(&h);
    let _ = h.draw(120, 40);
    type Input = (&'static str, fn(&mut Harness));
    let inputs: Vec<Input> = vec![
        ("Esc", |h| h.key(KeyCode::Esc)),
        ("j", |h| h.keys("j")),
        ("y", |h| h.keys("y")),
        ("click", |h| {
            let (x, y) = (h.app.layout.results.x + 8, h.app.layout.results.y + 5);
            h.mouse(MouseEventKind::Down(MouseButton::Left), x, y)
        }),
        ("paste", |h| paste(h, "zz")),
    ];
    for (name, input) in &inputs {
        paste(&mut h, "q@x,qq");
        assert_eq!(asked_forms(&h)[1], "Read as CSV (2 columns) or one value? c / 1", "{name}");
        input(&mut h);
        assert!(h.app.status_ask_line().is_none(), "{name}");
        assert_eq!(said(&mut h), "Paste cancelled", "{name}");
        assert_eq!(revision(&h), before, "{name}: nothing changed");
        assert_eq!(cursor(&h), at, "{name}: the input was taken");
    }
    // `1`: one value.
    paste(&mut h, "q@x,qq");
    h.keys("1");
    assert_eq!((pending(&h, 0, 1), pending(&h, 0, 2)), (Some(t("q@x,qq")), None));
    assert!(said(&mut h).starts_with("Pasted as one value"));
}

#[test]
fn the_question_about_commas_is_withdrawn_when_the_changes_or_the_tab_move() {
    let mut h = ready(Lang::En);
    paste(&mut h, "q@x,qq");
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        rs.edit.batch.set(&[(1, 1)], &t("w@x"), |_| vec![s("2"), s("b@x"), None]);
    }
    let before = revision(&h);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    assert!(h.app.status_ask_line().is_none());
    assert_eq!(said(&mut h), "Paste cancelled: the rows changed");
    assert_eq!((revision(&h), pending(&h, 0, 1)), (before, None), "nothing pasted");
    // Another tab.
    let mut h = ready(Lang::En);
    paste(&mut h, "q@x,qq");
    h.app.dispatch(Action::NewTab);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    assert!(h.app.status_ask_line().is_none());
    assert_eq!(said(&mut h), "Paste cancelled: the rows changed");
}

#[test]
fn a_column_copied_whose_values_have_commas_is_asked_about_not_split() {
    let mut h = ready(Lang::En);
    let c = clipboard(&mut h);
    type_cell(&mut h, "a,b");
    h.keys("j");
    type_cell(&mut h, "c,d");
    h.keys("kvjy");
    // The column with its header: as CSV, rows of other widths.
    let copy = c.last().expect("the column copied");
    assert_eq!(copy, "email\na,b\nc,d");
    h.keys("kl");
    h.app.transient = None;
    let before = revision(&h);
    paste(&mut h, &copy);
    assert_eq!(
        asked_forms(&h),
        [
            "Not CSV (row 2 has 2 values, row 1 has 1); read as one column? 1 · any other key cancels",
            "Not CSV; read as one column? 1",
        ]
    );
    assert_eq!(revision(&h), before, "not split before the answer");
    // `c` does not read it: it cancels.
    h.keys("c");
    assert_eq!(said(&mut h), "Paste cancelled");
    assert_eq!(revision(&h), before);
    h.advance(ANSWER_GUARD);
    paste(&mut h, &copy);
    h.keys("1");
    let column: Vec<_> = (0..3).map(|r| pending(&h, r, 2)).collect();
    assert_eq!(column, [Some(t("email")), Some(t("a,b")), Some(t("c,d"))]);
}

#[test]
fn the_question_reads_whole_at_80_columns() {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = ready(lang);
        paste(&mut h, "q@x,qq\nr@x,rr");
        let line = status_at(&mut h, 80);
        assert!(line.contains("c / 1") && !line.contains('\u{2026}'), "{lang:?}: {line}");
    }
}

/// A `y` (U+315B with a Korean input source too) in the window after a question ended without
/// its answer: nothing copied, `said` and the key said to be ignored; held too, and `n` and `c`
/// as Korean jamo (U+315C, U+314A). Then `j` moves the cursor and, a second after, `y` copies.
fn answer_keys_are_ignored_for_a_second(h: &mut Harness, said_then: &str) {
    let c = clipboard(h);
    let state = |h: &Harness| (h.app.tab().grid.row, h.app.tab().grid.col, h.app.tab().grid.anchor, revision(h));
    let before = state(h);
    h.keys("y");
    assert_eq!(c.last(), None, "nothing copied");
    assert_eq!(said(h), format!("{said_then} · y ignored"));
    h.key_kind(KeyCode::Char('y'), KeyModifiers::NONE, KeyEventKind::Repeat);
    h.type_text("\u{315B}");
    for (jamo, key) in [("\u{315C}", 'n'), ("\u{314A}", 'c')] {
        h.type_text(jamo);
        assert_eq!(said(h), format!("{said_then} · {key} ignored"));
    }
    h.keys("nc1");
    assert_eq!(c.last(), None, "held, Korean or another answer key: nothing copied");
    assert_eq!(state(h), before, "nothing else done");
    h.advance(ANSWER_GUARD - Duration::from_millis(1));
    h.keys("y");
    assert_eq!(c.last(), None, "still within the second");
    h.app.transient = None;
    h.keys("j");
    assert_eq!(h.app.tab().grid.row, before.0 + 1, "other keys pass");
    h.keys("k");
    h.advance(Duration::from_millis(1));
    h.keys("y");
    assert!(c.last().is_some(), "a second after, y copies as usual");
}

#[test]
fn a_y_right_after_a_question_was_withdrawn_is_ignored_for_a_second() {
    let mut h = ready(Lang::En);
    h.keys("l");
    let text: String = (0..1_001).map(|i| format!("n{i}\n")).collect();
    paste(&mut h, &text);
    assert_eq!(h.app.key_context(), Ctx::StatusAsk);
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        rs.edit.batch.set(&[(0, 1)], &t("w@x"), |_| vec![s("1"), s("a@x"), s("ann")]);
    }
    h.db(DbEvent::Keys(Ok(shop_keys())));
    assert!(h.app.status_ask_line().is_none());
    assert_eq!(said(&mut h), "Paste cancelled: the rows changed");
    answer_keys_are_ignored_for_a_second(&mut h, "Paste cancelled: the rows changed");
}

#[test]
fn a_y_with_the_focus_on_the_inspector_right_after_a_question_ended_is_ignored_too() {
    let mut h = ready(Lang::En);
    let c = clipboard(&mut h);
    paste(&mut h, "q@x,qq");
    h.keys("a");
    assert_eq!(said(&mut h), "Paste cancelled");
    // The chart with the focus on the inspector: `y` copies its data.
    h.keys("L");
    let _ = h.draw(120, 40);
    h.app.focus = Focus::Inspector;
    assert_eq!(h.app.key_context(), Ctx::Chart);
    h.keys("y");
    assert_eq!(c.last(), None, "nothing copied");
    assert_eq!(said(&mut h), "Paste cancelled · y ignored");
    h.advance(ANSWER_GUARD);
    h.keys("y");
    assert!(c.last().is_some(), "a second after, y copies the chart's data");
}

#[test]
fn the_text_of_a_question_about_commas_not_answered_is_read_again_by_space_d_v() {
    for drop in ["key", "moved"] {
        let mut h = ready(Lang::En);
        paste(&mut h, "q@x,qq");
        assert_eq!(h.app.key_context(), Ctx::StatusAsk, "{drop}");
        match drop {
            "key" => h.keys("a"),
            _ => {
                // The cursor moved away and back: the question is withdrawn, the changes as
                // they were.
                h.app.tab_mut().grid.row = 1;
                h.db(DbEvent::Keys(Ok(shop_keys())));
                h.app.tab_mut().grid.row = 0;
            }
        }
        assert!(h.app.status_ask_line().is_none(), "{drop}");
        h.advance(ANSWER_GUARD);
        h.app.transient = None;
        h.keys(" dV");
        assert_eq!(pending(&h, 0, 1), Some(t("q@x,qq")), "{drop}: read as one value");
    }
}

#[test]
fn a_y_right_after_a_question_was_cancelled_by_another_key_is_ignored_for_a_second() {
    let big: String = (0..1_001).map(|i| format!("n{i}\n")).collect();
    for ask in ["apply", "paste", "csv"] {
        let mut h = ready(Lang::En);
        let cancelled = match ask {
            "apply" => {
                type_cell(&mut h, "z@x");
                h.keys(" da");
                "Apply cancelled"
            }
            "paste" => {
                h.keys("l");
                paste(&mut h, &big);
                "Paste cancelled"
            }
            _ => {
                paste(&mut h, "q@x,qq");
                "Paste cancelled"
            }
        };
        assert_eq!(h.app.key_context(), Ctx::StatusAsk, "{ask}");
        h.keys("a");
        assert_eq!(said(&mut h), cancelled, "{ask}");
        answer_keys_are_ignored_for_a_second(&mut h, cancelled);
    }
}

/// The owner's case: a range copied with its header line (`copy_header`), pasted with the
/// terminal's paste below the last row. The header named the result's columns and became a new
/// row of invalid values; it is skipped, and said, and `space d V` keeps it.
#[test]
fn a_header_line_naming_the_columns_is_skipped_and_space_d_capital_v_keeps_it() {
    let mut h = ready(Lang::En);
    h.keys("jj");
    let before = revision(&h);
    paste(&mut h, "email\tnickname\nx@x\tbo\ny@y\t\\N\n");
    assert_eq!(pending(&h, 2, 1), Some(t("x@x")));
    assert_eq!(pending(&h, 2, 2), Some(t("bo")));
    assert_eq!(drafts(&h), vec![vec![Value::Default, t("y@y"), Value::Null]], "no row of the header");
    assert_eq!(revision(&h), before + 1);
    assert_eq!(
        said(&mut h),
        "Pasted as TSV \u{b7} Header row skipped \u{b7} Space d V keeps it \u{b7} 2 rows \u{d7} 2 columns \u{b7} 2 cells \
         changed \u{b7} 1 new row \u{b7} u undo"
    );
    assert!(h.app.shown_invalid().unwrap().cells.is_empty());
    // `space d V` reads it again keeping the header line, in the paste's place.
    h.keys(" dV");
    assert_eq!(pending(&h, 2, 1), Some(t("email")));
    assert_eq!(drafts(&h), vec![vec![Value::Default, t("x@x"), t("bo")], vec![Value::Default, t("y@y"), Value::Null]]);
    let text = said(&mut h);
    assert!(text.starts_with("Pasted as TSV \u{b7} 3 rows"), "{text}");
    // Then on to the next format, as before.
    h.keys(" dV");
    assert!(said(&mut h).starts_with("Pasted as CSV"));
}

#[test]
fn a_first_line_that_is_not_the_columns_names_is_pasted() {
    let mut h = ready(Lang::En);
    // Other names, or the names from another column on: values.
    paste(&mut h, "mail\tnickname\nx@x\tbo\n");
    assert_eq!(pending(&h, 0, 1), Some(t("mail")));
    h.keys("u");
    h.keys("h");
    paste(&mut h, "email\tnickname\n1\tx@x\n");
    assert_eq!(pending(&h, 0, 1), Some(t("nickname")), "not the names of the columns from the cursor");
    h.keys("u");
    // A single line is never a header.
    h.keys("l");
    paste(&mut h, "email\tnickname\n");
    assert_eq!(pending(&h, 0, 1), Some(t("email")));
}

/// Cells copied in the grid (`y`, `Y`) are kept as they are shown, NULL as NULL, and `g p`
/// pastes them at the cursor through the paste's own way: cells set, rows past the end new
/// rows, values a column does not take kept and marked, one step `u` undoes, refused whole when
/// they do not fit, a question first for more than a thousand rows. The copy is kept whatever
/// the clipboard does with it, across results and tabs.
mod grid_copy {
    use super::*;

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    fn t(v: &str) -> Value {
        Value::Text(v.to_string())
    }

    fn users_columns() -> Vec<ColumnMeta> {
        vec![
            meta_from("id", "int8", true, USERS, 1),
            meta_from("email", "text", false, USERS, 2),
            meta_from("nickname", "text", false, USERS, 4),
        ]
    }

    fn users_rows() -> Vec<Vec<Option<String>>> {
        vec![vec![s("1"), s("a@x"), s("ann")], vec![s("2"), s("b@x"), None], vec![s("3"), s("c@x"), s("cy")]]
    }

    const SQL: &str = "SELECT id, email, nickname FROM shop.users";

    fn structure(h: &mut Harness, st: TableStructure) {
        let name = h.app.profiles[0].name.clone();
        h.meta_db(&name, DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Ok(Box::new(st)) });
    }

    /// Run the users' query in the active tab and let `rows` come, the grid focused.
    fn answer_rows(h: &mut Harness, rows: Vec<Vec<Option<String>>>) {
        h.db(DbEvent::Keys(Ok(shop_keys())));
        h.app.run(vec![SQL.to_string()]);
        let id = h.app.tab().exec.query_id;
        h.db(DbEvent::Page { id, columns: Some(users_columns()), rows, more: false, elapsed: Duration::ZERO });
        h.app.focus = Focus::Results;
    }

    /// `shop.users` whose `name` (NOT NULL, not in the results here) has a default: new rows can be
    /// added.
    fn named() -> TableStructure {
        let mut st = users_structure();
        st.columns[2].default = Some("''::text".into());
        st
    }

    /// The active tab's result resolved (`e` on the first row's email, then the editor closed: the
    /// cursor stays there); nothing said.
    fn resolve(h: &mut Harness) {
        h.keys("le");
        structure(h, named());
        assert_eq!(h.app.key_context(), Ctx::CellEdit);
        h.key(KeyCode::Esc);
        h.app.transient = None;
        h.sent();
    }

    fn ready_with(mut h: Harness, rows: Vec<Vec<Option<String>>>) -> Harness {
        answer_rows(&mut h, rows);
        resolve(&mut h);
        h
    }

    fn ready() -> Harness {
        ready_with(Harness::connected(Lang::En), users_rows())
    }

    fn edits(h: &Harness) -> &EditState {
        match &h.app.tab().results {
            Results::Rows(rs) => &rs.edit,
            _ => panic!("no row result"),
        }
    }

    fn revision(h: &Harness) -> u64 {
        edits(h).batch.revision()
    }

    fn said(h: &mut Harness) -> String {
        let n = h.app.transient.take().map(|(n, _)| n).expect("something said");
        n.render(&h.app.i18n).to_string()
    }

    fn pending(h: &Harness, r: usize, c: usize) -> Option<Value> {
        edits(h).batch.value(r, c).cloned()
    }

    fn drafts(h: &Harness) -> Vec<Vec<Value>> {
        edits(h).batch.drafts().iter().map(|d| d.cells.clone()).collect()
    }

    fn cursor(h: &Harness) -> (usize, usize) {
        (h.app.tab().grid.row, h.app.tab().grid.col)
    }

    #[test]
    fn g_p_pastes_the_cells_y_copied_exactly_with_null_and_new_rows_as_one_step() {
        let mut h = ready();
        // Rows 1–2, email and nickname: row 2's nickname is NULL.
        h.keys("vjly");
        assert_eq!(cursor(&h), (1, 2));
        h.keys("jh");
        let before = revision(&h);
        h.keys("gp");
        assert_eq!(pending(&h, 2, 1), Some(t("a@x")));
        assert_eq!(pending(&h, 2, 2), Some(t("ann")));
        assert_eq!(drafts(&h), vec![vec![Value::Default, t("b@x"), Value::Null]], "NULL stays NULL, not empty text");
        assert_eq!(revision(&h), before + 1, "one step");
        assert_eq!(said(&mut h), "Pasted the copied cells · 2 rows × 2 columns · 2 cells changed · 1 new row · u undo");
        assert!(h.app.shown_invalid().unwrap().cells.is_empty());
        h.keys("u");
        assert!(edits(&h).batch.is_empty(), "u undoes all of it");
    }

    #[test]
    fn the_copy_is_of_the_values_the_grid_shows_pending_ones_and_null_included() {
        let mut h = ready();
        // A pending text and a pending NULL.
        h.keys("e");
        for _ in 0.."a@x".len() {
            h.key(KeyCode::Backspace);
        }
        h.type_text("z@x");
        h.key(KeyCode::Enter);
        assert_eq!(pending(&h, 0, 1), Some(t("z@x")));
        h.keys("y");
        h.keys("jgp");
        assert_eq!(pending(&h, 1, 1), Some(t("z@x")));
        h.keys("kl dn");
        assert_eq!(pending(&h, 0, 2), Some(Value::Null));
        h.keys("yjjgp");
        assert_eq!(pending(&h, 2, 2), Some(Value::Null), "the pending NULL, not the value read");
    }

    #[test]
    fn y_capital_keeps_the_whole_row() {
        let mut h = ready();
        h.keys("Yjjh");
        h.keys("gp");
        assert_eq!(pending(&h, 2, 0), Some(t("1")));
        assert_eq!(pending(&h, 2, 1), Some(t("a@x")));
        assert_eq!(pending(&h, 2, 2), Some(t("ann")));
    }

    #[test]
    fn the_copy_is_kept_when_the_clipboard_cannot_take_it() {
        let mut h = ready();
        h.app.prefs.clipboard = ClipboardSetting::System;
        let _c = FakeClipboard::attach(&mut h, true, &[]);
        h.keys("y");
        assert!(said(&mut h).starts_with("Could not copy"), "the clipboard refused it");
        h.keys("jgp");
        assert_eq!(pending(&h, 1, 1), Some(t("a@x")));
    }

    #[test]
    fn the_copy_is_kept_with_the_editor_clipboard_off_and_osc52() {
        let mut h = ready();
        h.app.prefs.clipboard = ClipboardSetting::Osc52;
        h.app.prefs.editor_clipboard = datarig_core::config::EditorClipboard::Off;
        let c = FakeClipboard::attach(&mut h, false, &[]);
        h.keys("y");
        h.keys("jgp");
        assert_eq!(pending(&h, 1, 1), Some(t("a@x")));
        assert_eq!(c.read_count(), 0, "g p reads no clipboard");
        assert!(c.last().is_none(), "nothing went to the system clipboard");
    }

    #[test]
    fn g_p_with_nothing_copied_says_how_to_copy() {
        let mut h = ready();
        h.keys("gp");
        assert_eq!(said(&mut h), "Nothing copied to paste: y copies a cell or a range, Y a row, then g p pastes it");
        assert!(edits(&h).batch.is_empty());
        assert_eq!(cursor(&h), (0, 1), "nothing moved");
    }

    #[test]
    fn the_copy_survives_other_results_and_tabs_and_is_checked_against_the_column_it_lands_in() {
        let mut h = ready();
        // The email of row 1.
        h.keys("y");
        // A new console tab with its own result of the users.
        h.ctrl('t');
        assert_eq!(h.app.tabs.len(), 2);
        answer_rows(&mut h, users_rows());
        resolve(&mut h);
        // Over the id, an int8: kept and marked, as a pasted value.
        h.keys("h");
        h.keys("gp");
        assert_eq!(pending(&h, 0, 0), Some(t("a@x")));
        let inv = h.app.shown_invalid().unwrap();
        let why = inv.cells.get(&(RowAt::Read(0), 0)).cloned().unwrap_or_default();
        assert_eq!(why, "not a whole number: 'a@x'");
        // The first tab's result again, run anew.
        h.key_mod(KeyCode::PageDown, ratatui::crossterm::event::KeyModifiers::CONTROL);
        assert_eq!(h.app.tabs.active_index(), 0);
        answer_rows(&mut h, users_rows());
        resolve(&mut h);
        h.keys("jgp");
        assert_eq!(pending(&h, 1, 1), Some(t("a@x")));
    }

    #[test]
    fn g_p_on_a_read_only_result_is_refused() {
        let mut cfg = Config {
            connections: vec![ConnectionConfig { policy: Some("prod".into()), ..ConnectionConfig::test_db() }],
            ..Config::default()
        };
        cfg.policies.insert("prod", Policy { read_only: true, ..Policy::default() });
        let mut h = Harness::connected_with(&cfg, Lang::En);
        answer_rows(&mut h, users_rows());
        h.keys("y");
        h.keys("jgp");
        assert_eq!(said(&mut h), "Read-only: the connection's environment is read-only");
        assert!(edits(&h).batch.is_empty());
    }

    #[test]
    fn g_p_is_refused_whole_when_it_does_not_fit() {
        let mut h = ready();
        // Two columns from the nickname, the last one.
        h.keys("vly");
        h.keys("l");
        let before = revision(&h);
        h.keys("gp");
        assert_eq!(
            said(&mut h),
            "Not pasted: the paste is 2 columns wide; 1 columns from the cursor to the last column"
        );
        assert_eq!(revision(&h), before);
    }

    fn many(n: usize) -> Vec<Vec<Option<String>>> {
        (0..n).map(|i| vec![s(&i.to_string()), s(&format!("{i}@x")), s(&format!("n{i}"))]).collect()
    }

    #[test]
    fn a_copy_of_more_than_a_thousand_rows_asks_before_it_is_pasted() {
        let mut h = ready_with(Harness::connected(Lang::En), many(1_001));
        // Every email, onto the nicknames.
        h.app.tab_mut().grid.anchor = Some((1_000, 1));
        h.keys("y");
        h.keys("l");
        let before = revision(&h);
        h.keys("gp");
        let forms = h.app.status_ask_line().map(|(f, _)| f).expect("asked");
        assert_eq!(forms[0], "Paste 1,001 rows? y paste · any other key cancels");
        assert_eq!(revision(&h), before, "nothing before y");
        h.keys("y");
        assert_eq!(revision(&h), before + 1);
        assert_eq!(pending(&h, 1_000, 2), Some(t("1000@x")));
    }

    #[test]
    fn a_copy_of_more_rows_than_a_paste_takes_is_not_kept_and_says_so() {
        let mut h = ready_with(Harness::connected(Lang::En), many(5_001));
        h.app.tab_mut().grid.anchor = Some((5_000, 1));
        h.keys("y");
        assert_eq!(said(&mut h), "Not kept for p: more than 5,000 rows, the most a paste takes");
        let status = h.app.status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default();
        assert!(status.starts_with("Copied 5,001 rows"), "the copy itself is done: {status}");
        h.keys("gp");
        assert!(said(&mut h).starts_with("Nothing copied to paste"));
    }

    #[test]
    fn the_text_of_the_null_token_and_an_empty_text_paste_back_as_text() {
        let rows = vec![vec![s("1"), s("\\N"), s("")], vec![s("2"), s("b@x"), s("bo")]];
        let mut h = ready_with(Harness::connected(Lang::En), rows);
        h.keys("vly");
        h.keys("jhgp");
        assert_eq!(pending(&h, 1, 1), Some(t("\\N")), "not NULL: the text as shown");
        assert_eq!(pending(&h, 1, 2), Some(t("")), "an empty text, not NULL");
    }

    #[test]
    fn a_copy_the_grid_refuses_or_asks_about_keeps_nothing() {
        let mut st = named();
        st.columns[3].default = Some("'n'::text".into());
        let mut h = ready_with(Harness::connected(Lang::En), users_rows());
        // The table with a default for the nickname.
        if let Results::Rows(rs) = &mut h.app.tab_mut().results {
            rs.edit.target = Default::default();
        }
        h.keys("y");
        h.keys("l");
        h.keys(" dD");
        structure(&mut h, st);
        assert_eq!(pending(&h, 0, 2), Some(Value::Default));
        h.app.transient = None;
        // A pending DEFAULT has no value: the copy is refused, and the older one is gone.
        h.keys("y");
        assert_eq!(said(&mut h), "Not copied: nickname has a pending DEFAULT, which has no value yet");
        h.keys("jgp");
        assert!(said(&mut h).starts_with("Nothing copied to paste"));
        // A copy on a new row is refused too.
        h.keys("hy");
        h.keys("o");
        let ctx = h.app.key_context();
        if ctx != Ctx::Grid {
            h.key(KeyCode::Esc);
        }
        h.app.transient = None;
        h.keys("Gy");
        let got = h.app.transient.take().map(|(n, _)| n.render(&h.app.i18n).to_string());
        let at = (cursor(&h), drafts(&h).len(), ctx);
        assert_eq!(got.as_deref(), Some("A new row is not copied: it is not a row of the result yet"), "{at:?}");
        h.keys("kgp");
        assert!(said(&mut h).starts_with("Nothing copied to paste"));
    }

    #[test]
    fn a_row_not_read_whole_is_not_kept_and_a_failed_clipboard_still_says_so() {
        let rows = vec![vec![s("1"), s("a@x"), s("ann")], vec![s("2"), s("b@x")]];
        let mut h = ready_with(Harness::connected(Lang::En), rows);
        h.app.prefs.clipboard = ClipboardSetting::System;
        let _c = FakeClipboard::attach(&mut h, true, &[]);
        h.keys("jY");
        assert_eq!(said(&mut h), "Not kept for p: row 2 was not read whole");
        let status = h.app.status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default();
        assert!(status.starts_with("Could not copy"), "{status}");
        h.keys("kgp");
        assert!(said(&mut h).starts_with("Nothing copied to paste"));
    }

    #[test]
    fn more_bytes_than_kept_are_not_kept_and_say_so() {
        // Two values of just over 16 MiB: more than the 32 MiB kept.
        let big = "x".repeat((16 << 20) + 1);
        let rows = vec![vec![s("1"), s(&big), None], vec![s("2"), s(&big), None]];
        let mut h = ready_with(Harness::connected(Lang::En), rows);
        h.app.prefs.clipboard = ClipboardSetting::Osc52;
        h.keys("vjy");
        assert_eq!(said(&mut h), "Not kept for p: more than 32 MiB of values");
        h.keys("gp");
        assert!(said(&mut h).starts_with("Nothing copied to paste"));
    }

    #[test]
    fn a_large_copy_declined_keeps_nothing() {
        let mut h = ready_with(Harness::connected(Lang::En), many(10_001));
        h.keys("y");
        h.app.tab_mut().grid.anchor = Some((10_000, 1));
        h.keys("y");
        assert_eq!(h.overlay_kind(), Some(datarig_tui::app::overlay::OverlayKind::Confirm));
        h.key(KeyCode::Esc);
        assert_eq!(h.overlay_kind(), None);
        h.app.transient = None;
        h.keys("jgp");
        assert!(said(&mut h).starts_with("Nothing copied to paste"), "the cell copied before is gone");
    }

    #[test]
    fn a_timestamptz_copied_on_postgresql_pastes_into_a_timestamptz() {
        use datarig_core::driver::ValueKind;
        let mut h = Harness::connected(Lang::En);
        h.db(DbEvent::Keys(Ok(shop_keys())));
        h.app.run(vec![SQL.to_string()]);
        let id = h.app.tab().exec.query_id;
        let mut columns = users_columns();
        let origin = columns[2].origin.clone();
        columns[2] = ColumnMeta::new("nickname".into(), "timestamptz".into(), ValueKind::TimestampTz, origin);
        let rows = vec![
            vec![s("1"), s("a@x"), s("2024-01-01 09:00:00+09")],
            vec![s("2"), s("b@x"), s("2024-06-01 00:00:00+00")],
        ];
        h.db(DbEvent::Page { id, columns: Some(columns), rows, more: false, elapsed: Duration::ZERO });
        h.app.focus = Focus::Results;
        let mut st = named();
        st.columns[3].type_name = "timestamp with time zone".into();
        h.keys("le");
        structure(&mut h, st);
        h.key(KeyCode::Esc);
        h.app.transient = None;
        // Its text has its offset: the same instant in any session.
        h.keys("ly");
        h.keys("jgp");
        assert_eq!(pending(&h, 1, 2), Some(t("2024-01-01 09:00:00+09")));
        assert!(said(&mut h).starts_with("Pasted the copied cells"));
        assert!(h.app.shown_invalid().unwrap().cells.is_empty());
    }

    #[test]
    fn p_appends_the_copied_cells_as_new_rows_one_step_a_press() {
        let mut h = ready();
        h.keys("vly");
        let before = revision(&h);
        h.keys("p");
        assert_eq!(drafts(&h), vec![vec![Value::Default, t("a@x"), t("ann")]]);
        assert_eq!(revision(&h), before + 1, "one step");
        assert_eq!(said(&mut h), "Pasted the copied cells as new rows · 1 row × 2 columns · 1 new row · u undo");
        h.keys("pp");
        assert_eq!(drafts(&h).len(), 3, "each p adds another copy");
        assert_eq!(revision(&h), before + 3);
        assert_eq!(cursor(&h), (5, 1), "on the last copy's first cell");
        h.keys("u");
        assert_eq!(drafts(&h).len(), 2, "u takes one press back");
    }

    #[test]
    fn p_sets_only_the_columns_copied_wherever_the_cursor_is() {
        let mut h = ready();
        // Row 2's nickname, NULL.
        h.keys("jly");
        h.keys("hh");
        h.keys("p");
        assert_eq!(drafts(&h), vec![vec![Value::Default, Value::Default, Value::Null]]);
        // The email is NOT NULL without a default: marked as needing a value.
        let inv = h.app.shown_invalid().unwrap();
        assert!(inv.no_default.iter().any(|&(at, c)| matches!(at, RowAt::New(_)) && c == 1), "{inv:?}");
    }

    #[test]
    fn the_copy_holds_values_only_never_a_header() {
        let mut h = ready();
        h.app.prefs.clipboard = ClipboardSetting::System;
        let c = FakeClipboard::attach(&mut h, false, &[]);
        h.keys("vjly");
        assert!(c.last().unwrap_or_default().starts_with("email\tnickname\n"), "the clipboard's copy has its header");
        h.keys("p");
        assert_eq!(
            drafts(&h),
            vec![vec![Value::Default, t("a@x"), t("ann")], vec![Value::Default, t("b@x"), Value::Null]]
        );
        // Every cell of the page, and whole rows: values only too.
        h.keys("u");
        h.ctrl('a');
        h.keys("yp");
        assert_eq!(drafts(&h).len(), 3);
        assert_eq!(drafts(&h)[0], vec![t("1"), t("a@x"), t("ann")]);
        h.keys("u");
        h.keys("ggVjY");
        h.keys("p");
        assert_eq!(drafts(&h), vec![vec![t("1"), t("a@x"), t("ann")], vec![t("2"), t("b@x"), Value::Null]]);
    }

    /// `shop.orders` as the driver reports it: an identity-free serial key, a user and a status.
    fn orders_structure() -> TableStructure {
        use datarig_core::driver::structure::*;
        let col = |name: &str, ty: &str, not_null: bool, default: Option<&str>| StructureColumn {
            name: name.into(),
            type_name: ty.into(),
            not_null,
            default: default.map(str::to_string),
            fill: ColumnFill::Default,
            enum_labels: Vec::new(),
            collation: None,
        };
        let mut t = TableStructure::new(RelationKind::Table);
        t.columns = vec![
            col("id", "bigint", true, Some("nextval('shop.orders_id_seq'::regclass)")),
            col("user_id", "bigint", false, None),
            col("status", "text", false, None),
            col("total_amount", "numeric", false, None),
        ];
        t.primary_key =
            Some(KeyConstraint { name: "orders_pkey".into(), columns: vec!["id".into()], definition: String::new() });
        t
    }

    /// A new tab with the orders' ids and statuses, resolved; the grid focused, nothing said.
    fn orders_tab(h: &mut Harness) {
        h.ctrl('t');
        h.app.run(vec!["SELECT id, status FROM shop.orders".to_string()]);
        let id = h.app.tab().exec.query_id;
        let columns = vec![meta_from("id", "int8", true, ORDERS, 1), meta_from("status", "text", false, ORDERS, 3)];
        h.db(DbEvent::Page {
            id,
            columns: Some(columns),
            rows: vec![vec![s("7"), s("paid")]],
            more: false,
            elapsed: Duration::ZERO,
        });
        h.app.focus = Focus::Results;
        h.keys("le");
        let name = h.app.profiles[0].name.clone();
        let result = Ok(Box::new(orders_structure()));
        h.meta_db(&name, DbEvent::Structure { schema: "shop".into(), table: "orders".into(), result });
        h.key(KeyCode::Esc);
        h.app.transient = None;
        h.sent();
    }

    #[test]
    fn p_into_another_table_maps_by_position_checks_types_and_refuses_a_wider_copy() {
        let mut h = ready();
        // The whole users row: wider than the orders' two columns.
        h.keys("Y");
        orders_tab(&mut h);
        let before = revision(&h);
        h.keys("p");
        assert_eq!(said(&mut h), "Not pasted: the copy is 3 columns wide; this result has 2");
        assert_eq!(revision(&h), before);
        // Email and nickname: the id and the status, by position.
        h.key_mod(KeyCode::PageUp, ratatui::crossterm::event::KeyModifiers::CONTROL);
        h.keys("vly");
        h.key_mod(KeyCode::PageDown, ratatui::crossterm::event::KeyModifiers::CONTROL);
        h.app.focus = Focus::Results;
        h.keys("p");
        assert_eq!(drafts(&h), vec![vec![t("a@x"), t("ann")]]);
        let inv = h.app.shown_invalid().unwrap();
        let bad: Vec<usize> = inv.cells.keys().map(|&(_, c)| c).collect();
        assert_eq!(bad, [0], "the email is no id: {inv:?}");
    }

    #[test]
    fn p_is_refused_on_a_read_only_result_and_asks_for_more_than_a_thousand_rows() {
        let mut cfg = Config {
            connections: vec![ConnectionConfig { policy: Some("prod".into()), ..ConnectionConfig::test_db() }],
            ..Config::default()
        };
        cfg.policies.insert("prod", Policy { read_only: true, ..Policy::default() });
        let mut h = Harness::connected_with(&cfg, Lang::En);
        answer_rows(&mut h, users_rows());
        h.keys("yp");
        assert_eq!(said(&mut h), "Read-only: the connection's environment is read-only");
        assert!(edits(&h).batch.is_empty());
        // More than a thousand new rows ask first.
        let mut h = ready_with(Harness::connected(Lang::En), many(1_001));
        h.app.tab_mut().grid.anchor = Some((1_000, 1));
        h.keys("y");
        let before = revision(&h);
        h.keys("p");
        let forms = h.app.status_ask_line().map(|(f, _)| f).expect("asked");
        assert_eq!(forms[0], "Paste 1,001 rows (1,001 new)? y paste · any other key cancels");
        assert_eq!(revision(&h), before);
        h.keys("y");
        assert_eq!(drafts(&h).len(), 1_001);
    }

    #[test]
    fn p_shows_the_page_with_the_new_rows() {
        let mut h = ready();
        h.keys("y");
        // Pages of two rows, the first one shown: the new rows are on the last page.
        let g = &mut h.app.tab_mut().grid;
        (g.page_size, g.page, g.top) = (2, 0, 0);
        h.keys("p");
        let g = &h.app.tab().grid;
        assert_eq!((g.row, g.col), (3, 1));
        assert_eq!(g.page, 1, "the page of the new row is shown");
    }
}
