//! The SQL section of the filter dialog (`Q`, `SQL ▸`): the statement as it would be sent above
//! the draft's `WHERE` text and order, each in a field. What is typed there is read back into
//! the chips and the keys when the field is left, the section closes or the draft is applied: a
//! text not typed in is sent byte for byte, an order that is not one keeps the section open.
//! Completion in the fields is that of `:where` and `:orderby`. `Open in editor` puts the
//! draft's statement, checked as `Space r v` checks it, in a new console.

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_core::policy::Policy;
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::complete::Kind;
use datarig_i18n::Lang;
use datarig_tui::app::PopupOwner;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::table_query::Field;
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use ratatui::style::Modifier;
use std::time::Duration;

const ALL: &str = r#"SELECT * FROM "shop"."users""#;

/// Open `table` of `shop` from the explorer (a table tab, which runs its query).
fn open_table(h: &mut Harness, table: &str) {
    h.key(KeyCode::BackTab);
    h.goto("shop");
    h.key(KeyCode::Char('l'));
    let tables = vec!["orders".to_string(), "users".to_string()];
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok((tables, Vec::new()).into()) });
    h.goto(table);
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

/// `shop.users` open and answered, the grid focused, nothing sent since.
fn users_with(cfg: &Config) -> Harness {
    let mut h = Harness::connected_with(cfg, Lang::En);
    open_table(&mut h, "users");
    answer(&mut h);
    h.sent();
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h
}

fn users() -> Harness {
    users_with(&test_db_config())
}

/// [`users`] on a read-only profile.
fn users_read_only() -> Harness {
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("prod".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert("prod", Policy { read_only: true, ..Policy::default() });
    users_with(&cfg)
}

/// `:where text`, run and answered: what the tab shows.
fn applied(h: &mut Harness, text: &str) {
    h.command(&format!("where {text}"));
    runs(h);
    answer(h);
}

fn dialog_open(h: &Harness) -> bool {
    h.app.overlays.is_open(OverlayKind::FilterDialog)
}

/// The SQL section's `WHERE` and `ORDER BY` fields.
fn fields(h: &Harness) -> (String, String) {
    h.app.filter_dialog_sql_fields().expect("the SQL section is open")
}

/// The draft's `WHERE` text and order, as they would be sent.
fn draft(h: &Harness) -> (String, String) {
    h.app.filter_dialog_draft().expect("the dialog is open")
}

fn labels(h: &Harness) -> Vec<String> {
    h.popup().map(|p| p.items.iter().map(|i| i.label.clone()).collect()).unwrap_or_default()
}

fn ctrl_space(h: &mut Harness) {
    h.key_mod(KeyCode::Char(' '), KeyModifiers::CONTROL);
}

/// Clears the SQL section's field with the keys.
fn clear_field(h: &mut Harness) {
    h.key(KeyCode::End);
    h.ctrl('u');
}

fn structure_asks(h: &mut Harness) -> usize {
    h.sent().into_iter().filter(|c| matches!(c, DbCommand::LoadStructure { .. })).count()
}

/// The cells of the screen line holding `text` that are underlined, as one string.
fn underlined(h: &mut Harness, w: u16, hh: u16, text: &str) -> String {
    let term = h.draw(w, hh);
    let buf = term.backend().buffer();
    let screen = h.screen(w, hh);
    let y = screen.lines().position(|l| l.contains(text)).unwrap_or_else(|| panic!("{text:?} not on:\n{screen}"));
    (0..w)
        .filter(|&x| buf[(x, y as u16)].modifier.contains(Modifier::UNDERLINED))
        .map(|x| buf[(x, y as u16)].symbol().to_string())
        .collect()
}

/// A `WHERE` text the dialog did not write (its blanks, its case, an `OR` in parentheses) is
/// sent byte for byte after the SQL section was opened and closed, and after its field was
/// left without a change.
#[test]
fn opening_and_closing_the_sql_sends_the_where_text_as_written() {
    let text = "id  >  1   and (name = 'a'  OR  name = 'b')";
    let mut h = users();
    applied(&mut h, text);
    h.keys("f");
    h.keys("Q");
    assert_eq!(fields(&h).0, text);
    assert_eq!(h.app.key_context(), Ctx::FilterDialogText, "the keys are in the WHERE field");
    // Left and back, then closed: nothing read back.
    h.key(KeyCode::Tab);
    h.key(KeyCode::BackTab);
    h.key(KeyCode::Esc);
    assert!(dialog_open(&h) && h.app.filter_dialog_sql_fields().is_none(), "Esc closes the section first");
    assert_eq!(draft(&h).0, text);
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE ({text})")]]);
    // `Enter` with the section open sends it as written too.
    answer(&mut h);
    h.keys("f");
    h.keys("Q");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE ({text})")]]);
}

/// A term typed in the `WHERE` field reads back as the text given anew: an `OR` at the top makes
/// the whole condition one expression chip, sent as typed; it is one step of undo.
#[test]
fn an_or_typed_in_the_where_field_makes_one_expression_chip() {
    let mut h = users();
    applied(&mut h, "id > 1");
    h.keys("f");
    h.keys("Q");
    h.key(KeyCode::End);
    h.type_text(" OR name = 'x'");
    assert_eq!(draft(&h).0, "id > 1", "not read back while typed");
    h.key(KeyCode::Esc);
    assert_eq!(draft(&h).0, "id > 1 OR name = 'x'");
    let screen = h.screen(120, 34);
    let row = screen.lines().find(|l| l.contains("Where ")).unwrap_or_else(|| panic!("{screen}"));
    assert_eq!(row.matches("expr").count(), 1, "{row}");
    assert!(row.contains("id > 1 OR name = 'x'"), "{row}");
    // One step back: the condition as it was, its chip a column's again.
    h.keys("u");
    assert_eq!(draft(&h).0, "id > 1");
    h.keys("Q");
    h.key(KeyCode::End);
    h.type_text(" OR name = 'x'");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 1 OR name = 'x')")]]);
}

/// The statement shows what `Enter` would send with what the fields hold, the key the app
/// appends to the order dimmed.
#[test]
fn the_statement_follows_the_fields_and_dims_the_appended_key() {
    let mut h = users();
    keys_came(&mut h, "local-pg", &["id"]);
    h.keys("f");
    h.keys("Q");
    h.type_text("name <> 'x'");
    h.key(KeyCode::Down);
    h.type_text("name DESC");
    let sql = format!(r#"{ALL} WHERE (name <> 'x') ORDER BY "name" DESC, "id" ASC"#);
    let screen = h.screen(120, 34);
    assert!(screen.contains(&sql), "{screen}");
    let term = h.draw(120, 34);
    let buf = term.backend().buffer();
    let y = screen.lines().position(|l| l.contains(&sql)).unwrap() as u16;
    let x = screen.lines().nth(usize::from(y)).unwrap().find(r#""id" ASC"#).unwrap() as u16;
    let (tie, key) = (&buf[(x, y)], &buf[(x - 8, y)]);
    assert_ne!(tie.fg, key.fg, "the appended key is dimmed");
    // Read back as the field is left.
    h.key(KeyCode::Tab);
    assert_eq!(draft(&h), ("name <> 'x'".into(), "name DESC".into()));
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![sql]]);
}

/// An order that does not read keeps the section open: said, underlined where it is wrong, the
/// keys in its field, the order not read back and nothing sent; fixed, it reads back.
#[test]
fn an_order_refused_keeps_the_sql_open() {
    let mut h = users();
    h.keys("f");
    h.keys("Q");
    h.type_text("id > 1");
    h.key(KeyCode::Down);
    h.type_text("name, nope DESC");
    h.key(KeyCode::Esc);
    if h.popup().is_some() {
        h.key(KeyCode::Esc);
    }
    assert!(h.app.filter_dialog_sql_fields().is_some(), "still open");
    let screen = h.screen(120, 34);
    assert!(screen.contains("Order refused: the table has no column nope"), "{screen}");
    assert_eq!(underlined(&mut h, 120, 34, "name, nope DESC"), "nope");
    assert_eq!(draft(&h), ("id > 1".into(), String::new()), "the WHERE field was read back as it was left");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert!(dialog_open(&h));
    clear_field(&mut h);
    h.type_text("name DESC");
    h.key(KeyCode::Esc);
    assert!(h.app.filter_dialog_sql_fields().is_none());
    assert_eq!(draft(&h), ("id > 1".into(), "name DESC".into()));
}

/// A refusal of the filter applied from the section underlines its part in the `WHERE` field.
#[test]
fn a_refused_where_is_underlined_in_its_field() {
    let mut h = users();
    h.keys("f");
    h.keys("Q");
    h.type_text("id > 1 FOR UPDATE");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert!(dialog_open(&h) && h.app.filter_dialog_sql_fields().is_some());
    let screen = h.screen(120, 34);
    assert!(screen.contains("row locks"), "{screen}");
    let field = screen
        .lines()
        .find(|l| l.trim_start_matches(['\u{2502}', ' ']).starts_with("WHERE ") && l.contains("FOR UPDATE"))
        .unwrap_or_else(|| panic!("{screen}"))
        .to_string();
    assert!(underlined(&mut h, 120, 34, &field).contains("FOR UPDATE"), "{field}");
}

/// `Ctrl+Z` in a field typed in takes the typing back first; then the draft's steps.
#[test]
fn undo_in_a_field_takes_its_typing_back_first() {
    let mut h = users();
    applied(&mut h, "id > 1");
    h.keys("f");
    h.keys("Q");
    h.type_text(" AND name = 'x'");
    h.key_mod(KeyCode::Char('z'), KeyModifiers::CONTROL);
    assert_eq!(fields(&h).0, "id > 1");
    h.type_text(" AND name = 'y'");
    h.key(KeyCode::Down);
    assert_eq!(draft(&h).0, "id > 1 AND name = 'y'");
    h.key_mod(KeyCode::Char('z'), KeyModifiers::CONTROL);
    assert_eq!(draft(&h).0, "id > 1");
    assert_eq!(fields(&h).0, "id > 1", "the field follows the draft");
}

/// `Open in editor`: the draft's statement (with its limit) in a new console, checked as
/// `Space r v` checks it; nothing runs, the dialog closes and the tab keeps what is applied.
#[test]
fn open_in_editor_opens_the_checked_draft_in_a_console() {
    let mut h = users();
    keys_came(&mut h, "local-pg", &["id"]);
    applied(&mut h, "id > 0");
    h.keys("f");
    h.keys("Q");
    clear_field(&mut h);
    h.type_text("name <> 'x'");
    h.key(KeyCode::Down);
    h.type_text("name");
    h.key(KeyCode::Down);
    assert!(h.screen(120, 34).contains("Open in editor"));
    let tabs = h.app.tabs.len();
    h.key(KeyCode::Char(' '));
    assert!(!dialog_open(&h));
    assert_eq!(h.app.tabs.len(), tabs + 1);
    assert!(!h.app.tab().is_table());
    assert_eq!(h.app.tab().editor.text(), format!(r#"{ALL} WHERE (name <> 'x') ORDER BY "name" ASC, "id" ASC"#));
    assert!(runs(&mut h).is_empty(), "opened, not run");
}

/// What a run refuses, `Open in editor` refuses: said in the dialog, which stays open; a
/// function to confirm is asked about above it, and `y` opens the draft's statement (not the
/// one of the rows held).
#[test]
fn open_in_editor_refuses_and_asks_as_a_run_does() {
    let mut h = users_read_only();
    h.keys("f");
    h.keys("Q");
    h.type_text("lower2(name) = 'a'");
    h.key(KeyCode::Down);
    h.key(KeyCode::Down);
    let tabs = h.app.tabs.len();
    h.key(KeyCode::Char(' '));
    assert_eq!(h.app.tabs.len(), tabs, "nothing opened");
    assert!(dialog_open(&h));
    let screen = h.screen(120, 34);
    assert!(screen.contains("Filter refused: lower2"), "{screen}");
    let mut h = users();
    applied(&mut h, "id > 0");
    h.keys("f");
    h.keys("Q");
    clear_field(&mut h);
    h.type_text("lower2(name) = 'a'");
    h.key(KeyCode::Down);
    h.key(KeyCode::Down);
    h.key(KeyCode::Char(' '));
    assert!(matches!(h.app.overlays.top(), Some(o) if o.kind() == OverlayKind::Confirm), "the function is asked about");
    assert!(matches!(h.app.overlays.confirm().map(|c| c.action), Some(ConfirmAction::FilterCalls(_))));
    h.keys("y");
    assert!(!dialog_open(&h));
    assert_eq!(h.app.tab().editor.text(), format!("{ALL} WHERE (lower2(name) = 'a')"));
    assert!(runs(&mut h).is_empty());
}

#[test]
fn the_sql_screens() {
    let mut h = users();
    keys_came(&mut h, "local-pg", &["id"]);
    applied(&mut h, "id > 1 AND (name = 'a' OR name = 'b')");
    h.command("orderby name desc");
    runs(&mut h);
    answer(&mut h);
    h.keys("f");
    h.keys("Q");
    assert_screen!("filter_sql_open_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_sql_open_en_120x34", Lang::En, h.draw(120, 34));
    h.key(KeyCode::End);
    h.type_text(" AND id < 1000 AND name IS NOT NULL AND name <> 'a long text to wrap the field'");
    h.key(KeyCode::Down);
    h.type_text(", nope");
    h.key(KeyCode::Esc);
    if h.popup().is_some() {
        h.key(KeyCode::Esc);
    }
    assert_screen!("filter_sql_refused_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_sql_refused_en_120x34", Lang::En, h.draw(120, 34));
}

#[test]
fn the_where_field_completes_the_tables_columns() {
    let mut h = users();
    h.keys("f");
    h.keys("Q");
    // Asked for with nothing typed: the table's columns.
    ctrl_space(&mut h);
    let p = h.popup().expect("a popup");
    assert_eq!(p.owner, PopupOwner::Dialog(Field::Where));
    assert_eq!(labels(&h)[..3], ["id", "email", "name"]);
    h.key(KeyCode::Esc);
    assert!(h.popup().is_none());
    assert!(h.app.filter_dialog_sql_fields().is_some(), "Esc closes the popup, the section stays");
    assert_eq!(h.app.key_context(), Ctx::FilterDialogText);
    // Typed: after the completion delay, as in the editor.
    h.type_text("id > 1 AND na");
    assert!(h.popup().is_none(), "not at once");
    h.settle();
    assert_eq!(labels(&h)[0], "name");
    assert_eq!(h.popup().unwrap().items[0].kind, Kind::Column);
    // A list that opened by itself does not take Tab: it moves on (the field read back).
    h.key(KeyCode::Tab);
    assert!(h.popup().is_none());
    assert_eq!(draft(&h).0, "id > 1 AND na");
    h.key(KeyCode::BackTab);
    // Asked for, Tab takes the label (it does not apply), the cursor after it.
    ctrl_space(&mut h);
    assert_eq!(labels(&h)[0], "name");
    h.key(KeyCode::Tab);
    assert_eq!(fields(&h).0, "id > 1 AND name");
    assert!(runs(&mut h).is_empty());
    // Keywords of the engine too.
    h.type_text(" IS NO");
    ctrl_space(&mut h);
    assert!(labels(&h).iter().any(|l| l == "NOT"), "{:?}", labels(&h));
    h.key(KeyCode::Tab);
    assert_eq!(fields(&h).0, "id > 1 AND name IS NOT");
    // Moving the cursor closes it; applying runs the text.
    ctrl_space(&mut h);
    h.key(KeyCode::Left);
    assert!(h.popup().is_none());
    h.key(KeyCode::End);
    h.type_text(" NULL");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 1 AND name IS NOT NULL)")]]);
}

#[test]
fn the_order_field_completes_columns_then_directions() {
    let mut h = users();
    h.keys("f");
    h.keys("Q");
    h.key(KeyCode::Down);
    h.type_text("na");
    ctrl_space(&mut h);
    assert_eq!(labels(&h), ["name"]);
    assert_eq!(h.popup().unwrap().owner, PopupOwner::Dialog(Field::Order));
    h.key(KeyCode::Tab);
    h.type_text(" ");
    ctrl_space(&mut h);
    assert_eq!(labels(&h), ["ASC", "DESC", "NULLS FIRST", "NULLS LAST"]);
    h.type_text("d");
    assert_eq!(labels(&h), ["DESC"], "an open popup follows the typing");
    h.key(KeyCode::Tab);
    h.type_text(", ");
    ctrl_space(&mut h);
    assert_eq!(labels(&h)[0], "id", "after a comma: the columns again");
    h.key(KeyCode::Esc);
    assert_eq!(fields(&h).1, "name DESC, ");
    // `↑` without a popup goes to the WHERE field: the popup belongs to its field only.
    h.key(KeyCode::Backspace);
    h.key(KeyCode::Backspace);
    h.key(KeyCode::Up);
    assert!(h.popup().is_none());
    assert_eq!(draft(&h).1, "name DESC");
}

#[test]
fn a_late_values_answer_for_a_literal_the_cursor_left_is_dropped() {
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Catalog(Ok(values_catalog())));
    open_table(&mut h, "orders");
    answer(&mut h);
    h.sent();
    h.key(KeyCode::Tab);
    let structure = |values: &str| {
        let mut st = orders_structure();
        st.checks[0].expression = format!("status = ANY (ARRAY[{values}])");
        DbEvent::Structure { schema: "shop".into(), table: "orders".into(), result: Ok(Box::new(st)) }
    };
    // In a literal compared with an enumerated column: its values, and its checks asked for.
    h.keys("f");
    h.keys("Q");
    h.type_text("status = 're");
    ctrl_space(&mut h);
    assert!(h.popup().is_none(), "no value of the type starts with it");
    assert_eq!(structure_asks(&mut h), 1);
    // The cursor leaves the literal before the answer: it shows nothing.
    h.key(KeyCode::Home);
    h.db(structure("'refunded'::text"));
    assert!(h.popup().is_none(), "a late answer for a literal the cursor left");
    // Still in it when the answer comes: the check's value shows.
    h.key(KeyCode::End);
    h.type_text("x");
    h.key(KeyCode::Backspace);
    h.key(KeyCode::Backspace);
    // A catalog read again forgets the checks: asked again.
    h.db(DbEvent::Catalog(Ok(values_catalog())));
    ctrl_space(&mut h);
    assert_eq!(structure_asks(&mut h), 1);
    h.db(structure("'refunded'::text, 'returned'::text"));
    assert_eq!(labels(&h), ["refunded", "returned"]);
    h.key(KeyCode::Tab);
    assert_eq!(fields(&h).0, "status = 'refunded'");
}

#[test]
fn completion_in_the_where_field_screen() {
    let mut h = users();
    h.keys("f");
    h.keys("Q");
    h.type_text("id > 1 AND n");
    ctrl_space(&mut h);
    assert_eq!(labels(&h)[..1], ["name"]);
    assert_screen!("filter_sql_completion_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_sql_completion_en_120x34", Lang::En, h.draw(120, 34));
}

/// In a field `Enter` applies what is typed while a popup opened by itself shows; it takes the
/// popup's entry only once the user turned to it (moved its selection, or asked for it).
#[test]
fn enter_applies_unless_the_popup_was_turned_to() {
    let mut h = users();
    h.keys("f");
    h.keys("Q");
    h.type_text("name");
    h.settle();
    assert!(h.popup().is_some(), "opened by itself");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (name)")]]);
    assert!(h.popup().is_none());
    answer(&mut h);
    // Moved through: Enter takes the entry selected.
    h.keys("f");
    h.keys("Q");
    clear_field(&mut h);
    h.type_text("na");
    h.settle();
    assert_eq!(labels(&h)[0], "name");
    h.key(KeyCode::Down);
    h.key(KeyCode::Up);
    h.key(KeyCode::Enter);
    assert_eq!(fields(&h).0, "name");
    assert!(runs(&mut h).is_empty());
    // Asked for: Enter takes it, also once more is typed.
    clear_field(&mut h);
    h.type_text("n");
    ctrl_space(&mut h);
    h.type_text("a");
    assert_eq!(labels(&h)[0], "name");
    h.key(KeyCode::Enter);
    assert_eq!(fields(&h).0, "name");
    assert!(runs(&mut h).is_empty());
    // The order too.
    h.type_text(" IS NOT NULL");
    h.key(KeyCode::Down);
    h.type_text("name");
    h.settle();
    assert!(h.popup().is_some());
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} WHERE (name IS NOT NULL) ORDER BY "name" ASC"#)]]);
}

/// The `WHERE` field offers the words of a condition, not those of a statement.
#[test]
fn the_where_field_offers_only_condition_keywords() {
    let mut h = users();
    h.keys("f");
    h.keys("Q");
    h.type_text("id > 1 a");
    ctrl_space(&mut h);
    let l = labels(&h);
    assert!(l.iter().any(|l| l == "and"), "{l:?}");
    assert!(!l.iter().any(|l| l == "alter" || l == "analyze"), "{l:?}");
    h.key(KeyCode::Esc);
    clear_field(&mut h);
    h.type_text("id IS NOT NULL ES");
    ctrl_space(&mut h);
    assert!(labels(&h).iter().any(|l| l == "ESCAPE"), "{:?}", labels(&h));
    h.key(KeyCode::Esc);
    clear_field(&mut h);
    h.type_text("name = 'al");
    ctrl_space(&mut h);
    assert!(!labels(&h).iter().any(|l| l.eq_ignore_ascii_case("alter")), "{:?}", labels(&h));
}

/// A paste into a field is one line, the field holding at most its cap.
#[test]
fn a_paste_into_a_field_is_one_line_and_has_a_cap() {
    let mut h = users();
    h.keys("f");
    h.keys("Q");
    h.event(ratatui::crossterm::event::Event::Paste("id > 1\nAND\r\nname = 'x'".into()));
    assert_eq!(fields(&h).0, "id > 1 AND  name = 'x'");
    h.event(ratatui::crossterm::event::Event::Paste("x".repeat(20_000)));
    assert!(h.screen(120, 34).contains("Pasted text cut"), "{}", h.screen(120, 34));
}
