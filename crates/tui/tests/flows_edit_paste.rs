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
use datarig_tui::app::edit::{EditState, RowAt};
use datarig_tui::app::tabs::ResultView;
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::{Event, KeyCode};
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
    refused(&mut h, "a\tb\nc\n", "row 2 has 1 values, row 1 has 2 · Space d V reads it again as one value");
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

#[test]
fn invalid_values_are_kept_marked_listed_first_and_block_the_apply() {
    let mut h = ready(Lang::En);
    h.keys("h");
    paste(&mut h, "abc\n\n");
    assert_eq!(pending(&h, 0, 0), Some(t("abc")));
    assert_eq!(pending(&h, 1, 0), Some(t("")));
    let text = said(&mut h);
    assert!(text.contains("2 cells changed · 2 invalid (first: row 1 · id) · u undo"), "{text}");
    let inv = h.app.shown_invalid().expect("resolved");
    assert_eq!(inv.cells.get(&(RowAt::Read(0), 0)).map(String::as_str), Some("not a whole number: 'abc'"));
    assert_eq!(inv.cells.get(&(RowAt::Read(1), 0)).map(String::as_str), Some("empty value — paste \\N for NULL"));
    // The gutter's `!`.
    assert!(grid_line(&mut h, 0).contains("│ 1!│   abc │"), "{:?}", grid_line(&mut h, 0));
    // The Changes tab lists them first, with the reason.
    let m = h.app.shown_changes().expect("a model");
    assert_eq!(m.problems[0].text, "Row 1 · id: not a whole number: 'abc'");
    assert_eq!(m.problems[1].text, "Row 2 · id: empty value — paste \\N for NULL");
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
