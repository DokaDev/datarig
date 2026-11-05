//! Cells copied in the grid (`y`, `Y`) are kept as they are shown, NULL as NULL, and `g p`
//! pastes them at the cursor through the paste's own way: cells set, rows past the end new
//! rows, values a column does not take kept and marked, one step `u` undoes, refused whole when
//! they do not fit, a question first for more than a thousand rows. The copy is kept whatever
//! the clipboard does with it, across results and tabs.

mod common;

use common::*;
use datarig_core::config::{ClipboardSetting, Config};
use datarig_core::driver::structure::TableStructure;
use datarig_core::driver::{ColumnMeta, DbEvent};
use datarig_core::edit::Value;
use datarig_core::i18n::Lang;
use datarig_core::policy::Policy;
use datarig_core::profile::ConnectionConfig;
use datarig_tui::app::Focus;
use datarig_tui::app::Results;
use datarig_tui::app::edit::{EditState, RowAt};
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::KeyCode;
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
    assert_eq!(
        said(&mut h),
        "Nothing copied to paste: y copies a cell or a range, Y a row, then g p pastes it at the cursor"
    );
    assert!(edits(&h).batch.is_empty());
    assert_eq!(cursor(&h), (0, 1), "p (the previous page) did not run either");
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
    assert_eq!(said(&mut h), "Not pasted: the paste is 2 columns wide; 1 columns from the cursor to the last column");
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
    assert_eq!(said(&mut h), "Not kept for g p: more than 5,000 rows, the most a paste takes");
    let status = h.app.status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default();
    assert!(status.starts_with("Copied 5,001 rows"), "the copy itself is done: {status}");
    h.keys("gp");
    assert!(said(&mut h).starts_with("Nothing copied to paste"));
}
