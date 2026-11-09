//! What the filter line of a table tab says back: completion in its fields (the condition
//! through the editor's engine on the statement it goes into, the order from the table's
//! columns; `:where` and `:orderby` the same), our refusals underlined where they are, a
//! server's error marked where it points in the condition (PostgreSQL's character position,
//! MySQL's quoted text when it is found once), and a failed query kept in the line with the
//! earlier rows dimmed under it. Paging again past a closed portal says when the order is not
//! unique.

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbError, DbEvent, ServerMessage};
use datarig_core::policy::Policy;
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::complete::Kind;
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_i18n::Lang;
use datarig_tui::app::table_query::Field;
use datarig_tui::app::{PopupOwner, Results};
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use ratatui::style::Modifier;
use std::time::Duration;

const ALL: &str = r#"SELECT * FROM "shop"."users""#;

/// Open `table` of `shop` from the explorer (a table tab, which runs its query).
fn open(h: &mut Harness, table: &str) {
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

/// Answer the active tab's current run with a first page of `n0`, `n1`, `n2` (or `more`).
fn answer_with(h: &mut Harness, more: bool) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
    let rows = (0..3).map(|i| vec![Some(i.to_string()), Some(format!("n{i}"))]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more, elapsed: Duration::from_millis(2) });
}

fn answer(h: &mut Harness) {
    answer_with(h, false);
}

/// The server refused the active tab's current run with `error`.
fn fail(h: &mut Harness, error: DbError) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Failed { id, error, cancelled: false });
}

/// A server message at 1-based character `position` of the statement.
fn at(text: &str, position: Option<u32>) -> DbError {
    DbError::Server(ServerMessage { text: text.into(), position, hint: None })
}

/// `shop.users` open and answered, the grid focused, nothing sent since.
fn users() -> Harness {
    let mut h = Harness::connected(Lang::En);
    open(&mut h, "users");
    answer(&mut h);
    h.sent();
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    // The inspector hidden: its room is the line's.
    h.keys("i");
    h
}

/// [`users`] on a read-only profile.
fn users_read_only() -> Harness {
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("prod".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert("prod", Policy { read_only: true, ..Policy::default() });
    let mut h = Harness::connected_with(&cfg, Lang::En);
    open(&mut h, "users");
    answer(&mut h);
    h.sent();
    h.key(KeyCode::Tab);
    h.keys("i");
    h
}

fn field_text(h: &Harness, f: Field) -> String {
    h.app.tab().doc.table_query.as_ref().expect("a table query").line.field(f).text().to_string()
}

fn labels(h: &Harness) -> Vec<String> {
    h.popup().map(|p| p.items.iter().map(|i| i.label.clone()).collect()).unwrap_or_default()
}

fn ctrl_space(h: &mut Harness) {
    h.key_mod(KeyCode::Char(' '), KeyModifiers::CONTROL);
}

fn structure_asks(h: &mut Harness) -> usize {
    h.sent().into_iter().filter(|c| matches!(c, DbCommand::LoadStructure { .. })).count()
}

/// The cells of the screen line holding `text` that are underlined, as `(column, symbol)`.
fn underlined(h: &mut Harness, w: u16, hh: u16, text: &str) -> Vec<(u16, String)> {
    let term = h.draw(w, hh);
    let buf = term.backend().buffer();
    let screen = h.screen(w, hh);
    let y = screen.lines().position(|l| l.contains(text)).unwrap_or_else(|| panic!("{text:?} not on:\n{screen}"));
    (0..w)
        .filter(|&x| buf[(x, y as u16)].modifier.contains(Modifier::UNDERLINED))
        .map(|x| (x, buf[(x, y as u16)].symbol().to_string()))
        .collect()
}

/// The screen column where `text` starts on its line.
fn column_of(h: &mut Harness, w: u16, hh: u16, text: &str) -> u16 {
    let screen = h.screen(w, hh);
    let line = screen.lines().find(|l| l.contains(text)).unwrap_or_else(|| panic!("{text:?} not on:\n{screen}"));
    let i = line.find(text).unwrap();
    ratatui::text::Line::raw(&line[..i]).width() as u16
}

fn pin(h: &mut Harness, name: &str) {
    assert_screen!(format!("filter_feedback_{name}_en_80x24"), Lang::En, h.draw(80, 24));
    assert_screen!(format!("filter_feedback_{name}_en_120x30"), Lang::En, h.draw(120, 30));
}

#[test]
fn the_condition_completes_the_tables_columns() {
    let mut h = users();
    h.keys("f");
    // Asked for with nothing typed: the table's columns.
    ctrl_space(&mut h);
    let p = h.popup().expect("a popup");
    assert_eq!(p.owner, PopupOwner::Filter(Field::Where));
    assert_eq!(labels(&h)[..5], ["id", "email", "name", "nickname", "profile"]);
    h.key(KeyCode::Esc);
    assert!(h.popup().is_none());
    assert_eq!(h.app.key_context(), Ctx::FilterLine, "Esc closes the popup, the field keeps the keys");
    // Typed: after the completion delay, as in the editor.
    h.type_text("id > 1 AND na");
    assert!(h.popup().is_none(), "not at once");
    h.settle();
    assert_eq!(labels(&h)[0], "name");
    assert_eq!(h.popup().unwrap().items[0].kind, Kind::Column);
    // Tab takes the label (it does not apply), the cursor after it.
    h.key(KeyCode::Tab);
    assert_eq!(field_text(&h, Field::Where), "id > 1 AND name");
    assert!(runs(&mut h).is_empty());
    assert_eq!(h.app.key_context(), Ctx::FilterLine);
    // Keywords of the engine too.
    h.type_text(" IS NO");
    ctrl_space(&mut h);
    assert!(labels(&h).iter().any(|l| l == "NOT"), "{:?}", labels(&h));
    h.key(KeyCode::Tab);
    assert_eq!(field_text(&h, Field::Where), "id > 1 AND name IS NOT");
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
fn the_order_completes_columns_then_directions() {
    let mut h = users();
    h.keys(" ro");
    h.type_text("na");
    ctrl_space(&mut h);
    assert_eq!(labels(&h), ["name"]);
    assert_eq!(h.popup().unwrap().owner, PopupOwner::Filter(Field::Order));
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
    assert_eq!(field_text(&h, Field::Order), "name DESC, ");
    // `Tab` without a popup moves to the condition: the popup belongs to its field only.
    h.key(KeyCode::Tab);
    assert_eq!(h.app.tab().doc.table_query.as_ref().and_then(|q| q.line.focus), Some(Field::Where));
}

#[test]
fn a_late_values_answer_for_a_literal_the_cursor_left_is_dropped() {
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Catalog(Ok(values_catalog())));
    open(&mut h, "orders");
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
    assert_eq!(field_text(&h, Field::Where), "status = 'refunded'");
}

#[test]
fn where_and_orderby_complete_their_argument() {
    let mut h = users();
    h.ctrl('k');
    h.type_text("where id > 1 and na");
    let c = h.cmdline().expect("the command line");
    assert_eq!(c.sql.as_ref().map(|s| s.items[0].label.clone()).as_deref(), Some("name"));
    // Picked with Tab, taken with Enter: written into the argument, the line stays.
    h.key(KeyCode::Tab);
    h.key(KeyCode::Enter);
    assert_eq!(h.cmdline().map(|c| c.input.text().to_string()).as_deref(), Some("where id > 1 and name"));
    h.type_text(" <> 'x'");
    // Typed, not picked: Enter runs the text as it is.
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 1 and name <> 'x')")]]);
    answer(&mut h);
    h.ctrl('k');
    h.type_text("orderby name d");
    let c = h.cmdline().expect("the command line");
    assert_eq!(
        c.sql.as_ref().map(|s| s.items.iter().map(|i| i.label.clone()).collect::<Vec<_>>()),
        Some(vec!["DESC".to_string()])
    );
    h.key(KeyCode::Esc);
}

#[test]
fn a_refused_part_is_underlined_in_the_field() {
    let mut h = users();
    h.keys("f");
    h.type_text("id = 1; DROP TABLE x");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    let r = h.app.tab().doc.table_query.as_ref().and_then(|q| q.refusal.clone()).expect("a refusal");
    assert_eq!((r.field, r.at.clone()), (Field::Where, Some(6..7)));
    assert_eq!(h.app.key_context(), Ctx::FilterLine, "the field keeps the keys");
    assert!(h.status(120, 30).contains("Filter refused: ; ends the statement"), "{}", h.status(120, 30));
    let x = column_of(&mut h, 120, 30, "; DROP");
    assert_eq!(underlined(&mut h, 120, 30, "; DROP"), [(x, ";".to_string())]);
    pin(&mut h, "refused");
    // A comment: the comment, after a wide character (bytes are not columns).
    h.ctrl('l');
    h.type_text("name = '日本' -- x");
    h.key(KeyCode::Enter);
    let r = h.app.tab().doc.table_query.as_ref().and_then(|q| q.refusal.clone()).expect("a refusal");
    assert_eq!(r.at, Some(16..20), "the bytes of `-- x`");
    let x = column_of(&mut h, 120, 30, "-- x");
    let marks: Vec<u16> = underlined(&mut h, 120, 30, "-- x").into_iter().map(|(c, _)| c).collect();
    assert_eq!(marks, (x..x + 4).collect::<Vec<_>>());
    // Edited since: the mark is about another text, so it goes.
    h.key(KeyCode::Backspace);
    assert!(underlined(&mut h, 120, 30, "-- ").is_empty());
}

#[test]
fn a_postgres_error_position_marks_the_condition_by_characters() {
    let mut h = users();
    h.keys("f");
    let filter = "name = '日本' AND idd = 1";
    h.type_text(filter);
    h.key(KeyCode::Enter);
    let sql = format!("{ALL} WHERE ({filter})");
    assert_eq!(runs(&mut h), vec![vec![sql.clone()]]);
    // PostgreSQL counts characters (1-based) of the statement as sent.
    let position = sql[..sql.find("idd").unwrap()].chars().count() as u32 + 1;
    fail(&mut h, at("ERROR: column \"idd\" does not exist", Some(position)));
    let tq = h.app.tab().doc.table_query.as_ref().unwrap();
    let start = filter.find("idd").unwrap();
    assert_eq!(tq.failure().and_then(|f| f.at.clone()), Some(start..start + 3));
    // The caret under `idd`, in screen columns (the wide characters before it count twice).
    let x = column_of(&mut h, 120, 30, "idd = 1");
    assert_eq!(underlined(&mut h, 120, 30, "idd = 1"), [(x, "i".into()), (x + 1, "d".into()), (x + 2, "d".into())]);
    assert!(h.status(120, 30).contains("column \"idd\" does not exist"), "{}", h.status(120, 30));
    pin(&mut h, "pg_caret");
    // A position outside the condition (the table's name): the message only.
    h.keys("f");
    h.key(KeyCode::Enter);
    runs(&mut h);
    fail(&mut h, at("ERROR: permission denied for table users", Some(15)));
    let tq = h.app.tab().doc.table_query.as_ref().unwrap();
    assert_eq!(tq.failure().map(|f| f.at.clone()), Some(None));
    assert!(underlined(&mut h, 120, 30, "idd = 1").is_empty());
}

#[test]
fn a_mysql_snippet_marks_the_condition_only_when_found_once() {
    // A MySQL profile, `shop.users` open on it.
    let driver = FakeDriver::default();
    driver.mysql.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut p = datarig_core::profile::ConnectionConfig::test_db();
    (p.name, p.driver) = ("local-my".into(), "mysql".into());
    let cfg = datarig_core::config::Config { connections: vec![p], ..Default::default() };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["shop".into()])));
    h.db(DbEvent::Catalog(Ok(catalog())));
    open(&mut h, "users");
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Language(Language::Sql(Dialect::MySql(MySqlMode::default()))));
    answer(&mut h);
    h.sent();
    h.key(KeyCode::Tab);
    let my = "SELECT * FROM `shop`.`users`";
    let near = |s: &str| {
        DbError::Server(
            format!("ERROR 1064 (42000): You have an error in your SQL syntax; check the manual that corresponds to your MySQL server version for the right syntax to use near '{s}' at line 1").into(),
        )
    };
    h.keys("f");
    h.type_text("id = 1 AND name = 'x'");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{my} WHERE (id = 1 AND name = 'x')")]], "{}", h.status(120, 30));
    // Once in the statement, from inside the condition: marked up to the condition's end.
    fail(&mut h, near("name = 'x')"));
    let tq = h.app.tab().doc.table_query.as_ref().unwrap();
    assert_eq!(tq.failure().and_then(|f| f.at.clone()), Some(11..21));
    // Twice: not located, the message only.
    h.keys("f");
    h.ctrl('l');
    h.type_text("id = 1 OR id = 1");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h).len(), 1);
    fail(&mut h, near("id = 1"));
    let tq = h.app.tab().doc.table_query.as_ref().unwrap();
    assert_eq!(tq.failure().map(|f| f.at.clone()), Some(None));
    // After the condition (the statement's closing parenthesis): the message only.
    h.keys("f");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h).len(), 1);
    fail(&mut h, near(")"));
    let tq = h.app.tab().doc.table_query.as_ref().unwrap();
    assert_eq!(tq.failure().map(|f| f.at.clone()), Some(None));
    assert!(h.status(120, 30).contains("ERROR 1064"), "{}", h.status(120, 30));
}

#[test]
fn a_failed_filter_keeps_the_earlier_rows_dimmed() {
    let mut h = users();
    h.keys("f");
    h.type_text("idd > 1");
    h.key(KeyCode::Enter);
    runs(&mut h);
    fail(&mut h, at("ERROR: column \"idd\" does not exist", Some(43)));
    let tq = h.app.tab().doc.table_query.as_ref().unwrap();
    assert!(tq.failure().is_some());
    assert_eq!(tq.shown.as_ref().and_then(|q| q.filter.as_ref()), None, "the rows held are still the unfiltered ones");
    assert!(matches!(h.app.tab().results, Results::Rows(_)), "the rows stay");
    assert_eq!(field_text(&h, Field::Where), "idd > 1", "the failed condition stays in the line");
    let screen = h.screen(120, 30);
    assert!(screen.contains("filter failed: showing previous rows"), "{screen}");
    assert!(screen.contains("\u{2717}WHERE idd > 1"), "{screen}");
    // The rows are dimmed.
    let term = h.draw(120, 30);
    let y = screen.lines().position(|l| l.contains("n1")).unwrap() as u16;
    let x = column_of(&mut h, 120, 30, "n1");
    assert!(term.backend().buffer()[(x, y)].modifier.contains(Modifier::DIM));
    pin(&mut h, "failed");
    // Fixed: the new rows, nothing dimmed or said.
    h.keys("f");
    h.ctrl('l');
    h.type_text("id > 1");
    h.key(KeyCode::Enter);
    answer(&mut h);
    let screen = h.screen(120, 30);
    assert!(!screen.contains("filter failed"), "{screen}");
    assert!(h.app.tab().doc.table_query.as_ref().unwrap().failure().is_none());
}

/// A table run that ends without its first page (cancelled, its session lost) leaves the
/// earlier rows: the line keeps the query as not applied, the rows are dimmed and the line
/// under the grid says why. The rows are never shown as that query's answer.
#[test]
fn a_run_that_brings_no_rows_leaves_its_query_not_applied() {
    let cases: [(DbEvent, &str); 2] = [
        (DbEvent::Failed { id: 0, error: DbError::Cancelled, cancelled: true }, "cancelled: showing previous rows"),
        (DbEvent::Lost { error: DbError::Closed }, "filter not applied: showing previous rows"),
    ];
    for (ev, note) in cases {
        let mut h = users();
        h.keys("f");
        h.type_text("id > 1");
        h.key(KeyCode::Enter);
        assert_eq!(runs(&mut h).len(), 1, "{note}");
        let index = h.app.tabs.active_index();
        let ev = match ev {
            DbEvent::Failed { error, cancelled, .. } => {
                DbEvent::Failed { id: h.app.tab().exec.query_id, error, cancelled }
            }
            ev => ev,
        };
        h.tab_db(index, ev);
        let tq = h.app.tab().doc.table_query.as_ref().unwrap();
        assert!(tq.failure().is_some(), "{note}: the query is not applied");
        assert_eq!(
            tq.shown.as_ref().and_then(|q| q.filter.as_ref()),
            None,
            "{note}: the rows held are the unfiltered ones"
        );
        assert!(matches!(h.app.tab().results, Results::Rows(_)), "{note}: the rows stay");
        let screen = h.screen(120, 30);
        assert!(screen.contains(note), "{note}:\n{screen}");
        assert!(screen.contains("\u{2717}WHERE id > 1"), "{note}:\n{screen}");
        let term = h.draw(120, 30);
        let y = screen.lines().position(|l| l.contains("n1")).unwrap() as u16;
        let x = column_of(&mut h, 120, 30, "n1");
        assert!(term.backend().buffer()[(x, y)].modifier.contains(Modifier::DIM), "{note}: the rows are dimmed");
    }
}

#[test]
fn completion_in_the_condition_screen() {
    let mut h = users();
    h.keys("f");
    h.type_text("id > 1 AND n");
    ctrl_space(&mut h);
    assert_eq!(labels(&h)[..2], ["name", "nickname"]);
    pin(&mut h, "completion");
}

#[test]
fn paging_again_says_when_the_order_is_not_unique() {
    let page_again = |h: &mut Harness| {
        let id = h.app.tab().exec.query_id;
        let index = h.app.tabs.active_index();
        h.tab_db(index, DbEvent::Released { id });
        let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
        let rows = (0..h.app.page_size).map(|i| vec![Some(i.to_string()), Some(format!("n{i}"))]).collect();
        h.tab_db(index, DbEvent::Page { id, columns, rows, more: true, elapsed: Duration::from_millis(2) });
        h.app.focus = datarig_tui::app::Focus::Results;
        h.sent();
        h.keys("n");
        assert!(h.sent().iter().any(|c| matches!(c, DbCommand::Resume { .. })));
        h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default()
    };
    // The structure not read yet: unknown, so not unique.
    let mut h = users();
    h.command("orderby name");
    let said = page_again(&mut h);
    assert!(said.contains("order not unique"), "{said}");
    // The primary key read: appended, the order is total.
    let mut h = users();
    h.meta_db(
        "local-pg",
        DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Ok(Box::new(users_structure())) },
    );
    h.command("orderby name");
    let said = page_again(&mut h);
    assert!(!said.contains("not unique") && said.starts_with("Ran the SELECT again"), "{said}");
}

/// A structure read after the rows came does not change what is said of them: the order
/// they were read in was not unique, and the line and a page read again say so (the next
/// run appends the key).
#[test]
fn a_late_structure_does_not_make_the_held_order_unique() {
    let mut h = users();
    h.command("orderby name");
    let sent = runs(&mut h);
    assert!(sent.iter().flatten().any(|s| s.ends_with("ORDER BY \"name\" ASC")), "{sent:?}");
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Released { id });
    let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
    let rows = (0..h.app.page_size).map(|i| vec![Some(i.to_string()), Some(format!("n{i}"))]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: true, elapsed: Duration::from_millis(2) });
    h.meta_db(
        "local-pg",
        DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Ok(Box::new(users_structure())) },
    );
    let screen = h.screen(120, 30);
    assert!(screen.contains("order not unique") && !screen.contains("+ id"), "{screen}");
    h.app.focus = datarig_tui::app::Focus::Results;
    h.sent();
    h.keys("n");
    let said = h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default();
    assert!(said.contains("order not unique"), "{said}");
}

/// Opening a table asks for its structure (one metadata read): an order applied once it came
/// is made total on its first run, `+ id` on the line.
#[test]
fn opening_a_table_asks_its_structure_so_the_first_order_is_total() {
    let mut h = Harness::connected(Lang::En);
    open(&mut h, "users");
    let sent = h.sent();
    let asked = sent
        .iter()
        .filter(|c| matches!(c, DbCommand::LoadStructure { schema, table } if schema == "shop" && table == "users"))
        .count();
    assert_eq!(asked, 1, "{sent:?}");
    answer(&mut h);
    h.meta_db(
        "local-pg",
        DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Ok(Box::new(users_structure())) },
    );
    h.key(KeyCode::Tab);
    h.keys("i");
    h.keys(" ro");
    h.type_text("name");
    h.key(KeyCode::Enter);
    let sent = h.sent();
    assert!(!sent.iter().any(|c| matches!(c, DbCommand::LoadStructure { .. })), "read already: {sent:?}");
    let run: Vec<&Vec<String>> = sent
        .iter()
        .filter_map(|c| if let DbCommand::Execute { statements, .. } = c { Some(statements) } else { None })
        .collect();
    assert_eq!(run, vec![&vec![format!(r#"{ALL} ORDER BY "name" ASC, "id" ASC"#)]]);
    answer(&mut h);
    let screen = h.screen(120, 30);
    assert!(screen.contains("ORDER BY name ASC + id"), "{screen}");
}

/// An order run while the structure is not known is said not unique (unknown, not absent),
/// however narrow the line: the keys are cut or shrunk first.
#[test]
fn an_order_not_known_total_says_so_at_any_width() {
    let mut h = users();
    h.keys(" ro");
    h.type_text("name");
    h.key(KeyCode::Enter);
    answer(&mut h);
    for (w, hh) in [(80, 24), (100, 30), (110, 30), (120, 30), (160, 40)] {
        let screen = h.screen(w, hh);
        let line = screen.lines().find(|l| l.contains("WHERE")).unwrap_or_else(|| panic!("{screen}"));
        assert!(line.contains("not unique"), "{w}x{hh}: {line}");
    }
}

/// In the filter line `Enter` applies what is typed while a popup opened by itself shows; it
/// takes the popup's entry only once the user turned to it (moved its selection, or asked for
/// it with `Ctrl+Space`). `Ctrl+E` applies in every case.
#[test]
fn enter_applies_unless_the_popup_was_turned_to() {
    let mut h = users();
    h.keys("f");
    h.type_text("name");
    h.settle();
    assert!(h.popup().is_some(), "opened by itself");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (name)")]]);
    assert!(h.popup().is_none());
    answer(&mut h);
    // Moved through: Enter takes the entry selected.
    h.keys("f");
    h.ctrl('l');
    h.type_text("na");
    h.settle();
    assert_eq!(labels(&h)[0], "name");
    h.key(KeyCode::Down);
    h.key(KeyCode::Up);
    h.key(KeyCode::Enter);
    assert_eq!(field_text(&h, Field::Where), "name");
    assert!(runs(&mut h).is_empty());
    // Asked for: Enter takes it, also once more is typed.
    h.ctrl('l');
    h.type_text("n");
    ctrl_space(&mut h);
    h.type_text("a");
    assert_eq!(labels(&h)[0], "name");
    h.key(KeyCode::Enter);
    assert_eq!(field_text(&h, Field::Where), "name");
    assert!(runs(&mut h).is_empty());
    // The run key applies, the popup open or not.
    h.type_text(" IS NOT NULL AND i");
    ctrl_space(&mut h);
    assert!(h.popup().is_some());
    h.ctrl('e');
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (name IS NOT NULL AND i)")]]);
    assert!(h.popup().is_none());
    answer(&mut h);
    // The order too.
    h.keys(" ro");
    h.type_text("name");
    h.settle();
    assert!(h.popup().is_some());
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} WHERE (name IS NOT NULL AND i) ORDER BY "name" ASC"#)]]);
}

/// The condition offers the words of a condition, not those of a statement.
#[test]
fn the_condition_offers_only_condition_keywords() {
    let mut h = users();
    h.keys("f");
    h.type_text("id > 1 a");
    ctrl_space(&mut h);
    let l = labels(&h);
    assert!(l.iter().any(|l| l == "and"), "{l:?}");
    assert!(!l.iter().any(|l| l == "alter" || l == "analyze"), "{l:?}");
    h.ctrl('l');
    h.type_text("id IS NOT NULL ES");
    ctrl_space(&mut h);
    assert!(labels(&h).iter().any(|l| l == "ESCAPE"), "{:?}", labels(&h));
    h.ctrl('l');
    h.type_text("name = 'al");
    ctrl_space(&mut h);
    assert!(!labels(&h).iter().any(|l| l.eq_ignore_ascii_case("alter")), "{:?}", labels(&h));
    // `:where` completes its argument the same way.
    h.key(KeyCode::Esc);
    h.keys(":");
    h.type_text("where id > 1 a");
    let shown = h.screen(120, 30);
    assert!(!shown.contains("ALTER") && !shown.contains("alter"), "{shown}");
}

/// A refusal is said whole under the grid while it stands, its essential part first in the
/// status bar.
#[test]
fn a_refusal_is_said_whole_under_the_grid() {
    let mut h = users_read_only();
    h.keys("f");
    h.type_text("lower2(name) = 'a' AND id > 1 AND name IS NOT NULL");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    let screen = h.screen(80, 24);
    let status = screen.lines().last().unwrap_or_default().to_string();
    assert!(status.contains("Filter refused: lower2"), "{status}");
    let note: String = screen
        .lines()
        .filter(|l| l.contains("side effects") || l.contains("Filter refused") || l.contains("connection)"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(note.contains("(read-only connection)"), "{screen}");
    pin(&mut h, "refusal_note");
}

/// PostgreSQL names a row lock in a filter as MySQL does, and underlines it.
#[test]
fn a_postgres_row_lock_is_refused_as_one_and_underlined() {
    let mut h = users();
    h.keys("f");
    h.type_text("id > 1 FOR UPDATE");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    let said = h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default();
    assert!(said.contains("row locks"), "{said}");
    let marked: String = underlined(&mut h, 120, 30, "FOR UPDATE").into_iter().map(|(_, s)| s).collect();
    assert_eq!(marked, "FOR UPDATE");
}
