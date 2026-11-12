//! What a table tab says back about its query: completion of `:where` and `:orderby` (the
//! condition through the editor's engine on the statement it goes into, the order from the
//! table's columns), our refusals with where they are, a server's error marked where it points
//! in the condition (PostgreSQL's character position, MySQL's quoted text when it is found
//! once; the filter dialog underlines that chip), and a failed query with the earlier rows
//! dimmed. Paging again past a closed portal says when the order is not unique.

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::keys::OrderKeys;
use datarig_core::driver::{DbCommand, DbError, DbEvent, ServerMessage};
use datarig_core::policy::Policy;
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_i18n::Lang;
use datarig_tui::app::Results;
use datarig_tui::app::table_query::Field;
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::KeyCode;
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

/// A completion of `:where` belongs to where the cursor is: moved back into a column name, the
/// list is that name's, and taking it replaces only the name, the rest of the condition kept.
#[test]
fn a_command_argument_completes_where_the_cursor_moved() {
    let mut h = users();
    h.ctrl('k');
    h.type_text("where na <> 'x' and id > 1");
    for _ in 0.." <> 'x' and id > 1".len() {
        h.key(KeyCode::Left);
    }
    let c = h.cmdline().expect("the command line");
    assert_eq!(c.sql.as_ref().and_then(|s| s.items.first()).map(|i| i.label.clone()).as_deref(), Some("name"));
    h.key(KeyCode::Tab);
    h.key(KeyCode::Enter);
    assert_eq!(h.cmdline().map(|c| c.input.text().to_string()).as_deref(), Some("where name <> 'x' and id > 1"));
    h.key(KeyCode::End);
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (name <> 'x' and id > 1)")]]);
}

#[test]
fn a_refused_part_is_said_with_where_it_is() {
    let mut h = users();
    h.command("where id = 1; DROP TABLE x");
    assert!(runs(&mut h).is_empty());
    let r = h.app.tab().doc.table_query.as_ref().and_then(|q| q.refusal.clone()).expect("a refusal");
    assert_eq!((r.field, r.at.clone()), (Field::Where, Some(6..7)));
    assert!(h.status(120, 30).contains("Filter refused: ; ends the statement"), "{}", h.status(120, 30));
    pin(&mut h, "refused");
    // A comment: the comment, after a wide character (bytes are not columns).
    h.command("where name = '日本' -- x");
    let r = h.app.tab().doc.table_query.as_ref().and_then(|q| q.refusal.clone()).expect("a refusal");
    assert_eq!(r.at, Some(16..20), "the bytes of `-- x`");
}

#[test]
fn a_postgres_error_position_marks_the_condition_by_characters() {
    let mut h = users();
    let filter = "name = '日本' AND idd = 1";
    h.command(&format!("where {filter}"));
    let sql = format!("{ALL} WHERE ({filter})");
    assert_eq!(runs(&mut h), vec![vec![sql.clone()]]);
    // PostgreSQL counts characters (1-based) of the statement as sent.
    let position = sql[..sql.find("idd").unwrap()].chars().count() as u32 + 1;
    fail(&mut h, at("ERROR: column \"idd\" does not exist", Some(position)));
    let tq = h.app.tab().doc.table_query.as_ref().unwrap();
    let start = filter.find("idd").unwrap();
    assert_eq!(tq.failure().and_then(|f| f.at.clone()), Some(start..start + 3));
    assert!(h.status(120, 30).contains("column \"idd\" does not exist"), "{}", h.status(120, 30));
    // The filter dialog shows what the server said and underlines the chip it points into.
    h.keys("f");
    assert!(h.screen(120, 34).contains("column \"idd\" does not exist"), "{}", h.screen(120, 34));
    let marked: String = underlined(&mut h, 120, 34, "idd = 1 \u{d7}").into_iter().map(|(_, s)| s).collect();
    assert_eq!(marked, "idd = 1");
    pin(&mut h, "pg_caret");
    // A position outside the condition (the table's name): the message only.
    h.key(KeyCode::Enter);
    runs(&mut h);
    fail(&mut h, at("ERROR: permission denied for table users", Some(15)));
    let tq = h.app.tab().doc.table_query.as_ref().unwrap();
    assert_eq!(tq.failure().map(|f| f.at.clone()), Some(None));
    h.keys("f");
    assert!(underlined(&mut h, 120, 34, "idd = 1 \u{d7}").is_empty());
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
    h.tab_db(index, DbEvent::Connected);
    answer(&mut h);
    h.sent();
    h.key(KeyCode::Tab);
    let my = "SELECT * FROM `shop`.`users`";
    let near = |s: &str| {
        DbError::Server(
            format!("ERROR 1064 (42000): You have an error in your SQL syntax; check the manual that corresponds to your MySQL server version for the right syntax to use near '{s}' at line 1").into(),
        )
    };
    h.command("where id = 1 AND name = 'x'");
    assert_eq!(runs(&mut h), vec![vec![format!("{my} WHERE (id = 1 AND name = 'x')")]], "{}", h.status(120, 30));
    // Once in the statement, from inside the condition: marked up to the condition's end.
    fail(&mut h, near("name = 'x')"));
    let tq = h.app.tab().doc.table_query.as_ref().unwrap();
    assert_eq!(tq.failure().and_then(|f| f.at.clone()), Some(11..21));
    // Twice: not located, the message only.
    h.command("where id = 1 OR id = 1");
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
    h.command("where idd > 1");
    runs(&mut h);
    fail(&mut h, at("ERROR: column \"idd\" does not exist", Some(43)));
    let tq = h.app.tab().doc.table_query.as_ref().unwrap();
    assert!(tq.failure().is_some());
    assert_eq!(tq.shown.as_ref().and_then(|q| q.filter.as_ref()), None, "the rows held are still the unfiltered ones");
    assert!(matches!(h.app.tab().results, Results::Rows(_)), "the rows stay");
    let applied = tq.applied().filter.as_ref().map(|f| f.text.clone());
    assert_eq!(applied.as_deref(), Some("idd > 1"), "the failed condition stays applied");
    let screen = h.screen(120, 30);
    assert!(screen.contains("filter failed: showing previous rows"), "{screen}");
    // The rows are dimmed.
    let term = h.draw(120, 30);
    let y = screen.lines().position(|l| l.contains("n1")).unwrap() as u16;
    let x = column_of(&mut h, 120, 30, "n1");
    assert!(term.backend().buffer()[(x, y)].modifier.contains(Modifier::DIM));
    pin(&mut h, "failed");
    // Fixed: the new rows, nothing dimmed or said.
    h.command("where id > 1");
    answer(&mut h);
    let screen = h.screen(120, 30);
    assert!(!screen.contains("filter failed"), "{screen}");
    assert!(h.app.tab().doc.table_query.as_ref().unwrap().failure().is_none());
}

/// A table run that ends without its first page (cancelled, its session lost, its session
/// failing to connect) leaves the earlier rows: the query stays as not applied, the rows are
/// dimmed and the line under the grid says why. The rows are never shown as that
/// query's answer.
#[test]
fn a_run_that_brings_no_rows_leaves_its_query_not_applied() {
    let cases: [(DbEvent, &str); 3] = [
        (DbEvent::Failed { id: 0, error: DbError::Cancelled, cancelled: true }, "cancelled: showing previous rows"),
        (DbEvent::Lost { error: DbError::Closed }, "filter not applied: showing previous rows"),
        (
            DbEvent::ConnectFailed { error: "connection refused".into(), auth: false },
            "filter not applied: showing previous rows",
        ),
    ];
    for (ev, note) in cases {
        let mut h = users();
        h.command("where id > 1");
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
        let term = h.draw(120, 30);
        let y = screen.lines().position(|l| l.contains("n1")).unwrap() as u16;
        let x = column_of(&mut h, 120, 30, "n1");
        assert!(term.backend().buffer()[(x, y)].modifier.contains(Modifier::DIM), "{note}: the rows are dimmed");
    }
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
    keys_came(&mut h, "local-pg", &["id"]);
    h.command("orderby name");
    let said = page_again(&mut h);
    assert!(!said.contains("not unique") && said.starts_with("Ran the SELECT again"), "{said}");
}

/// Keys read after the rows came do not change what is said of them: the order they were
/// read in was not unique, and the line and a page read again say so (the next run appends
/// the key).
#[test]
fn late_keys_do_not_make_the_held_order_unique() {
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
    keys_came(&mut h, "local-pg", &["id"]);
    h.app.focus = datarig_tui::app::Focus::Results;
    h.sent();
    h.keys("n");
    let said = h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default();
    assert!(said.contains("order not unique"), "{said}");
}

/// The keys-only reads sent since the last look, as (request, schema.table).
fn keys_asks(sent: &[DbCommand]) -> Vec<(u64, String)> {
    sent.iter()
        .filter_map(|c| match c {
            DbCommand::LoadTableKeys { id, schema, table } => Some((*id, format!("{schema}.{table}"))),
            _ => None,
        })
        .collect()
}

fn order_keys(pk: &[&str]) -> OrderKeys {
    OrderKeys { primary_key: pk.iter().map(|c| c.to_string()).collect(), ..OrderKeys::default() }
}

/// Opening a table reads its keys with the key-only catalog read (never the full structure,
/// whose functions open the table): an order applied once they came is made total on its
/// first run, `+ id` on the line. Applying an order asks nothing more.
#[test]
fn opening_a_table_reads_its_keys_so_the_first_order_is_total() {
    let mut h = Harness::connected(Lang::En);
    open(&mut h, "users");
    let sent = h.sent();
    assert!(!sent.iter().any(|c| matches!(c, DbCommand::LoadStructure { .. })), "no structure read: {sent:?}");
    let asks = keys_asks(&sent);
    assert_eq!(asks.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(), ["shop.users"], "{sent:?}");
    answer(&mut h);
    h.meta_db("local-pg", DbEvent::TableKeys { id: asks[0].0, result: Ok(order_keys(&["id"])) });
    h.key(KeyCode::Tab);
    h.keys("i");
    h.command("orderby name");
    let sent = h.sent();
    assert!(
        !sent.iter().any(|c| matches!(c, DbCommand::LoadStructure { .. } | DbCommand::LoadTableKeys { .. })),
        "read already: {sent:?}"
    );
    let run: Vec<&Vec<String>> = sent
        .iter()
        .filter_map(|c| if let DbCommand::Execute { statements, .. } = c { Some(statements) } else { None })
        .collect();
    assert_eq!(run, vec![&vec![format!(r#"{ALL} ORDER BY "name" ASC, "id" ASC"#)]]);
    answer(&mut h);
    // The filter dialog's order row says what makes it total.
    h.keys(" ro");
    let screen = h.screen(120, 34);
    let line = screen.lines().find(|l| l.contains("Order ")).unwrap_or_else(|| panic!("{screen}"));
    assert!(line.contains("name \u{2191}") && line.contains("+ id"), "{line}");
}

/// An answer of another request (a late one) is dropped; a failed read leaves the keys not
/// known (the order is not unique, never "no key") and the next run asks again.
#[test]
fn a_stale_or_failed_keys_answer_leaves_the_order_not_unique() {
    let mut h = Harness::connected(Lang::En);
    open(&mut h, "users");
    let asks = keys_asks(&h.sent());
    answer(&mut h);
    h.meta_db("local-pg", DbEvent::TableKeys { id: asks[0].0 + 100, result: Ok(order_keys(&["id"])) });
    h.command("orderby name");
    let sent = h.sent();
    assert!(keys_asks(&sent).is_empty(), "still asked: {sent:?}");
    let ran = sent.iter().any(
        |c| matches!(c, DbCommand::Execute { statements, .. } if statements[0].ends_with(r#"ORDER BY "name" ASC"#)),
    );
    assert!(ran, "no tie-breaker from a stale answer: {sent:?}");
    answer(&mut h);
    let said = order_said(&mut h);
    assert!(said.contains("not unique"), "{said}");
    // Its own answer, a failure: not known.
    h.meta_db("local-pg", DbEvent::TableKeys { id: asks[0].0, result: Err(DbError::Locked) });
    h.ctrl('e');
    let sent = h.sent();
    assert_eq!(keys_asks(&sent).len(), 1, "asked again: {sent:?}");
    let ran = sent.iter().any(
        |c| matches!(c, DbCommand::Execute { statements, .. } if statements[0].ends_with(r#"ORDER BY "name" ASC"#)),
    );
    assert!(ran, "{sent:?}");
    answer(&mut h);
    let said = order_said(&mut h);
    assert!(said.contains("not unique"), "{said}");
}

/// Answer the active tab's current run with a first page of one column, `col`.
fn answer_with_column(h: &mut Harness, col: &str) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = Some(vec![meta(col, "text", false, false)]);
    let rows = (0..3).map(|i| vec![Some(format!("v{i}"))]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: false, elapsed: Duration::from_millis(2) });
}

/// What the filter dialog's `Order` row says of the order (opened on it, then closed).
fn order_said(h: &mut Harness) -> String {
    use datarig_tui::app::action::Action;
    use datarig_tui::app::table_filter::TableAction;
    h.app.dispatch(Action::Table(TableAction::EditOrder));
    let screen = h.screen(120, 34);
    h.key(KeyCode::Esc);
    order_line(&screen, Lang::En)
}

/// The filter dialog's `Order` row on `screen`.
fn order_line(screen: &str, lang: Lang) -> String {
    let label = if lang == Lang::En { "Order " } else { "\u{c21c}\u{c11c} " };
    screen.lines().find(|l| l.contains(label)).unwrap_or_else(|| panic!("{screen}")).to_string()
}

/// A long column name at 80 columns: the keys are cut, what is said of the order keeps its
/// room (English and Korean).
#[test]
fn a_long_order_column_keeps_the_not_unique_note() {
    const COL: &str = "very_long_order_column_name_extra";
    for (lang, said) in [(Lang::En, "not unique"), (Lang::Ko, "\u{c720}\u{c77c}\u{d558}\u{c9c0} \u{c54a}\u{c74c}")] {
        let mut h = Harness::connected(lang);
        open(&mut h, "users");
        answer_with_column(&mut h, COL);
        h.sent();
        h.key(KeyCode::Tab);
        h.keys("i");
        h.command(&format!("orderby {COL}"));
        answer_with_column(&mut h, COL);
        h.keys(" ro");
        let screen = h.screen(80, 24);
        let line = order_line(&screen, lang);
        assert!(line.contains(said), "{lang:?}: {line}");
        assert!(line.contains("very") && line.contains('\u{2191}'), "{lang:?}: {line}");
    }
}

/// An order while the keys are not known is said not unique (unknown, not absent), however
/// narrow the dialog: the keys are cut first.
#[test]
fn an_order_not_known_total_says_so_at_any_width() {
    let mut h = users();
    h.command("orderby name");
    answer(&mut h);
    h.keys(" ro");
    for (w, hh) in [(80, 24), (100, 30), (110, 30), (120, 30), (160, 40)] {
        let screen = h.screen(w, hh);
        let line = order_line(&screen, Lang::En);
        assert!(line.contains("not unique"), "{w}x{hh}: {line}");
    }
}

/// A refusal is said whole under the grid while it stands, its essential part first in the
/// status bar.
#[test]
fn a_refusal_is_said_whole_under_the_grid() {
    let mut h = users_read_only();
    h.command("where lower2(name) = 'a' AND id > 1 AND name IS NOT NULL");
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

/// PostgreSQL names a row lock in a filter as MySQL does, and says where it is.
#[test]
fn a_postgres_row_lock_is_refused_as_one_and_underlined() {
    let mut h = users();
    h.command("where id > 1 FOR UPDATE");
    assert!(runs(&mut h).is_empty());
    let said = h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default();
    assert!(said.contains("row locks"), "{said}");
    let r = h.app.tab().doc.table_query.as_ref().and_then(|q| q.refusal.clone()).expect("a refusal");
    assert_eq!(r.at.map(|at| r.text[at].to_string()).as_deref(), Some("FOR UPDATE"));
}

/// Whether an order is total comes from the key-only read alone: the explorer's structure of
/// the table (which does not say whether a key is deferred, valid or compares as an ORDER BY
/// does) changes nothing, before or after it is cached.
#[test]
fn the_explorers_structure_does_not_make_an_order_total() {
    let mut h = users();
    // The key-only read found no key an order may rely on (a deferrable primary key, say).
    keys_came(&mut h, "local-pg", &[]);
    let order = |h: &mut Harness| {
        h.command("orderby name");
        let sent = runs(h);
        answer(h);
        (sent, order_said(h).contains("not unique"))
    };
    let before = order(&mut h);
    h.meta_db(
        "local-pg",
        DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Ok(Box::new(users_structure())) },
    );
    h.command("filter clear");
    answer(&mut h);
    h.sent();
    let after = order(&mut h);
    assert_eq!(before, after);
    assert_eq!(after.0, vec![vec![format!(r#"{ALL} ORDER BY "name" ASC"#)]]);
    assert!(after.1);
}

/// Keys are the metadata session's they were read on: the profile's session opened again
/// drops them (the next run asks again), and an answer of the session before is not taken.
#[test]
fn keys_are_read_again_when_the_metadata_session_opens_again() {
    let mut h = users();
    keys_came(&mut h, "local-pg", &["id"]);
    h.command("orderby name");
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} ORDER BY "name" ASC, "id" ASC"#)]]);
    answer(&mut h);
    // The connection is lost (the metadata session and the tab's): the next run opens both again.
    h.meta_db("local-pg", DbEvent::Lost { error: DbError::Closed });
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Lost { error: DbError::Closed });
    h.ctrl('e');
    let sent = h.sent();
    assert_eq!(keys_asks(&sent).len(), 1, "asked again: {sent:?}");
    let ran = sent.iter().any(
        |c| matches!(c, DbCommand::Execute { statements, .. } if statements[0].ends_with(r#"ORDER BY "name" ASC"#)),
    );
    assert!(ran, "not total on the new session before its answer: {sent:?}");
    answer(&mut h);
    keys_came(&mut h, "local-pg", &["id"]);
    h.ctrl('e');
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} ORDER BY "name" ASC, "id" ASC"#)]]);
}

/// What is said of an order not known total wins over the column's name at every width: with
/// the explorer at its widest on 80 columns the keys shrink to their arrow, the note stays
/// (English and Korean, a short and a long column).
#[test]
fn the_not_unique_note_stays_at_the_explorers_widest() {
    for (lang, said) in [(Lang::En, "not unique"), (Lang::Ko, "\u{c720}\u{c77c}\u{d558}\u{c9c0} \u{c54a}\u{c74c}")] {
        for col in ["name", "very_long_order_column_name_extra"] {
            let mut h = Harness::connected(lang);
            open(&mut h, "users");
            answer_with_column(&mut h, col);
            h.sent();
            h.key(KeyCode::Tab);
            h.keys("i");
            h.command(&format!("orderby {col}"));
            answer_with_column(&mut h, col);
            h.app.explorer.width = Some(200);
            h.keys(" ro");
            let screen = h.screen(80, 24);
            let line = order_line(&screen, lang);
            assert!(line.contains(said), "{lang:?} {col}: {line}");
            assert!(line.contains('\u{2191}'), "{lang:?} {col}: the arrow stays: {line}");
        }
    }
}

/// `shop.users` open with its key `id` read, then run ordered by `name`: the key is appended.
/// The table tab stays active, nothing sent since.
fn ordered_with_its_key() -> Harness {
    let mut h = users();
    keys_came(&mut h, "local-pg", &["id"]);
    h.command("orderby name");
    let sent = runs(&mut h);
    assert!(sent.iter().flatten().any(|s| s.ends_with(r#"ORDER BY "name" ASC, "id" ASC"#)), "{sent:?}");
    answer(&mut h);
    h.sent();
    h
}

/// The table tab's order run again: its statement must not append the key read before (it may
/// be gone), and the keys are asked for again with it.
fn run_again_asks_the_keys(h: &mut Harness) {
    h.command("orderby name");
    let sent = h.sent();
    let ran = sent.iter().any(
        |c| matches!(c, DbCommand::Execute { statements, .. } if statements[0].ends_with(r#"ORDER BY "name" ASC"#)),
    );
    assert!(ran, "no tie-breaker from keys that may be out of date: {sent:?}");
    assert_eq!(keys_asks(&sent).iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(), ["shop.users"], "{sent:?}");
    answer(h);
    let said = order_said(h);
    assert!(said.contains("not unique"), "unknown until read, never no key: {said}");
    // Read again: the order is total again.
    keys_came(h, "local-pg", &["uid"]);
    h.command("orderby name");
    let sent = runs(h);
    assert!(sent.iter().flatten().any(|s| s.ends_with(r#"ORDER BY "name" ASC, "uid" ASC"#)), "{sent:?}");
}

/// DDL committed in a console of the profile (its key column renamed) drops the keys its table
/// tabs read: the next ordered run does not append the old column and reads them again.
#[test]
fn ddl_in_a_console_makes_table_tabs_read_their_keys_again() {
    use datarig_core::driver::Outcome;
    let mut h = ordered_with_its_key();
    let table = h.app.tabs.active_index();
    let console = h.app.tabs.iter().position(|t| !t.is_table()).expect("a console");
    h.app.tabs.activate(console);
    h.app.run(vec!["ALTER TABLE shop.users RENAME COLUMN id TO uid".into()]);
    let id = h.app.tab().exec.query_id;
    h.tab_db(console, DbEvent::Done { id, outcome: Outcome::Command("ALTER TABLE".into()), elapsed: Duration::ZERO });
    h.app.tabs.activate(table);
    h.sent();
    run_again_asks_the_keys(&mut h);
}

/// Reading the profile's schema again in the explorer drops the keys its table tabs read.
#[test]
fn a_metadata_refresh_makes_table_tabs_read_their_keys_again() {
    let mut h = ordered_with_its_key();
    let table = h.app.tabs.active_index();
    h.explore("local-pg");
    h.keys("r");
    h.app.tabs.activate(table);
    h.sent();
    run_again_asks_the_keys(&mut h);
}

/// A server's error of an ordered run (another session renamed the key column) drops the keys
/// read: the next run reads them again rather than append the column again.
#[test]
fn a_failed_ordered_run_reads_the_keys_again() {
    let mut h = ordered_with_its_key();
    h.command("orderby name");
    h.sent();
    fail(&mut h, at(r#"column "id" does not exist"#, None));
    h.sent();
    run_again_asks_the_keys(&mut h);
}
