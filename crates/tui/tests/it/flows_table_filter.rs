//! A table tab's filter and order (`:where`, `:orderby`, `:filter clear`, the run key): the
//! statement sent is the compiled query; a refused filter or order sends nothing; calls of
//! functions whose effects are not known are confirmed (`Enter` cancels, `y` runs, once per
//! text) and refused on a read-only profile; pending edits are asked about first; the rows shown
//! are of the run that sent them, never of a late answer of another; paging and counting check
//! the shown filter again.

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_core::policy::Policy;
use datarig_core::profile::ConnectionConfig;
use datarig_core::query::{Dir, Filter, OrderKey, Query, TableName};
use datarig_i18n::Lang;
use datarig_tui::app::Results;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::table_query::{Field, same_query};
use ratatui::crossterm::event::KeyCode;
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

/// Answer the active tab's run `id` with a first page of `shop.users` rows (`more`: past it).
fn page_of(h: &mut Harness, id: u64, more: bool) {
    let index = h.app.tabs.active_index();
    let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
    let rows = (0..3).map(|i| vec![Some(i.to_string()), Some(format!("n{i}"))]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more, elapsed: Duration::from_millis(2) });
}

/// Answer the active tab's current run with a first page.
fn answer(h: &mut Harness) {
    let id = h.app.tab().exec.query_id;
    page_of(h, id, false);
}

/// `shop.users` open and answered, nothing sent since.
fn users(lang: Lang) -> Harness {
    let mut h = Harness::connected(lang);
    open_users(&mut h);
    answer(&mut h);
    h.sent();
    h
}

/// The rows shown are of `q` (what the app understood of its filter left out).
fn assert_shown(h: &Harness, q: Query) {
    let shown = shown(h).expect("a shown query");
    assert!(same_query(&shown, &q), "{shown:?}");
}

fn shown(h: &Harness) -> Option<Query> {
    h.app.tab().doc.table_query.as_ref().and_then(|q| q.shown.clone())
}

fn users_query(filter: Option<&str>, order: Vec<OrderKey>) -> Query {
    let table = TableName { database: None, schema: "shop".into(), name: "users".into() };
    let filter = filter.map(|t| Filter { text: t.into(), recognised: None });
    Query { filter, order, ..Query::table(table) }
}

fn status(h: &Harness) -> String {
    h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

/// The shown query of the active tab, for a test to change what the rows are said to be of.
fn shown_mut(h: &mut Harness) -> &mut Query {
    h.app.tab_mut().doc.table_query.as_mut().and_then(|q| q.shown.as_mut()).expect("a shown query")
}

#[test]
fn the_statement_sent_is_the_compiled_query() {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    assert_eq!(runs(&mut h), vec![vec![ALL.to_string()]], "opening runs the table's own query");
    answer(&mut h);
    assert_eq!(shown(&h).map(|q| q.filter.is_none() && q.order.is_empty()), Some(true));
    h.command("where id > 1");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 1)")]]);
    answer(&mut h);
    let q = shown(&h).expect("shown");
    assert_eq!(q.filter.as_ref().map(|f| f.text.as_str()), Some("id > 1"));
    h.command("orderby NAME desc, id");
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} WHERE (id > 1) ORDER BY "name" DESC, "id" ASC"#)]]);
    answer(&mut h);
    let order = vec![OrderKey::user("name", Dir::Desc), OrderKey::user("id", Dir::Asc)];
    assert_eq!(shown(&h).map(|q| q.order), Some(order));
    // The run key runs the query again, filter and order with it.
    h.ctrl('e');
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} WHERE (id > 1) ORDER BY "name" DESC, "id" ASC"#)]]);
    answer(&mut h);
    // A blank `:where` drops the filter, keeps the order; `:filter clear` drops both.
    h.command("where");
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} ORDER BY "name" DESC, "id" ASC"#)]]);
    answer(&mut h);
    h.command("filter clear");
    assert_eq!(runs(&mut h), vec![vec![ALL.to_string()]]);
    answer(&mut h);
    assert_eq!(shown(&h).map(|q| q.filter.is_none() && q.order.is_empty()), Some(true));
}

#[test]
fn a_refused_filter_or_order_sends_nothing_and_keeps_where() {
    let mut h = users(Lang::En);
    h.command("where id = 1; DROP TABLE shop.users");
    assert!(runs(&mut h).is_empty());
    assert!(status(&h).contains("Not run: the filter is refused"), "{}", status(&h));
    let refusal = h.app.tab().doc.table_query.as_ref().and_then(|q| q.refusal.clone()).expect("refusal");
    assert_eq!((refusal.field, refusal.at), (Field::Where, Some(6..7)));
    // A row lock in a subquery, a comment, a call that waits: refused too.
    for text in ["id IN (SELECT id FROM shop.users FOR UPDATE)", "id = 1 -- x", "pg_sleep(1) IS NULL"] {
        h.command(&format!("where {text}"));
        assert!(runs(&mut h).is_empty(), "{text}");
        assert!(status(&h).starts_with("Not run: the filter is refused"), "{text}: {}", status(&h));
    }
    // An order that names no column of the table.
    h.command("orderby nope DESC");
    assert!(runs(&mut h).is_empty());
    assert!(status(&h).contains("the table has no column nope"), "{}", status(&h));
    let refusal = h.app.tab().doc.table_query.as_ref().and_then(|q| q.refusal.clone()).expect("refusal");
    assert_eq!((refusal.field, refusal.at), (Field::Order, Some(0..4)));
    // The rows shown are still of the last query that ran.
    assert_shown(&h, users_query(None, vec![]));
    // A filter let through clears the refusal.
    h.command("where id > 0");
    assert_eq!(runs(&mut h).len(), 1);
    assert!(h.app.tab().doc.table_query.as_ref().is_some_and(|q| q.refusal.is_none()));
}

#[test]
fn unchecked_calls_are_confirmed_once_per_text() {
    let mut h = users(Lang::En);
    h.command("where my_fn(id) > 0");
    assert_eq!(h.app.overlays.confirm().map(|c| c.action), Some(ConfirmAction::FilterCalls(h.app.tab().id)));
    assert!(runs(&mut h).is_empty(), "nothing sent while it asks");
    // Enter cancels.
    h.key(KeyCode::Enter);
    assert!(!h.app.overlays.is_open(OverlayKind::Confirm));
    assert!(runs(&mut h).is_empty());
    assert!(status(&h).contains("functions were not confirmed"), "{}", status(&h));
    // Asked again; `y` runs.
    h.command("where my_fn(id) > 0");
    assert!(h.app.overlays.is_open(OverlayKind::Confirm));
    h.keys("y");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (my_fn(id) > 0)")]]);
    answer(&mut h);
    // The same text is not asked about again in this tab; another one is.
    h.command("orderby id");
    assert!(!h.app.overlays.is_open(OverlayKind::Confirm));
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} WHERE (my_fn(id) > 0) ORDER BY "id" ASC"#)]]);
    answer(&mut h);
    h.command("where my_fn(id) > 1");
    assert!(h.app.overlays.is_open(OverlayKind::Confirm));
}

/// The profile in the read-only environment `prod`, connected.
fn read_only(lang: Lang) -> Harness {
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("prod".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert("prod", Policy { read_only: true, ..Policy::default() });
    Harness::connected_with(&cfg, lang)
}

#[test]
fn a_read_only_profile_refuses_unchecked_calls_before_sending() {
    let mut h = read_only(Lang::En);
    open_users(&mut h);
    answer(&mut h);
    h.sent();
    h.command("where my_fn(id) > 0");
    assert!(!h.app.overlays.is_open(OverlayKind::Confirm), "refused, not asked");
    assert!(runs(&mut h).is_empty());
    assert!(status(&h).contains("read-only connection"), "{}", status(&h));
    // A filter of known functions runs.
    h.command("where lower(name) = 'a'");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (lower(name) = 'a')")]]);
}

#[test]
fn pending_edits_are_asked_about_before_a_filter_runs() {
    let mut h = users(Lang::En);
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        assert!(rs.edit.batch.delete(&[0], |_| Vec::new()));
    }
    h.command("where id > 1");
    assert_eq!(
        h.app.overlays.confirm().map(|c| c.action),
        Some(ConfirmAction::LeaveEdits(datarig_tui::app::edit::Leave::Run(h.app.tab().id)))
    );
    // Enter keeps the edits: nothing runs.
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert_shown(&h, users_query(None, vec![]));
    // `y` drops them and runs the filter.
    h.command("where id > 1");
    h.keys("y");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 1)")]]);
    answer(&mut h);
    assert_shown(&h, users_query(Some("id > 1"), vec![]));
}

#[test]
fn a_late_answer_of_an_earlier_filter_is_not_shown() {
    let mut h = users(Lang::En);
    h.command("where id > 1");
    let first = h.app.tab().exec.query_id;
    // A second filter while the first runs: refused as any run is.
    h.command("where id > 2");
    assert_eq!(runs(&mut h).len(), 1, "the second waits for nothing: refused");
    // The first is cancelled, the second runs.
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Failed { id: first, error: datarig_core::driver::DbError::Cancelled, cancelled: true });
    h.command("where id > 2");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 2)")]]);
    // The cancelled run's rows come late: not shown, not taken for the filter's.
    page_of(&mut h, first, false);
    assert_shown(&h, users_query(None, vec![]));
    answer(&mut h);
    assert_shown(&h, users_query(Some("id > 2"), vec![]));
}

#[test]
fn the_commands_are_only_for_table_tabs() {
    let mut h = Harness::connected(Lang::En);
    h.command("where id > 1");
    assert!(runs(&mut h).is_empty());
    let said = h.app.overlays.command_line().and_then(|c| c.error.as_ref()).map(|n| n.render(&h.app.i18n).to_string());
    assert_eq!(said.as_deref(), Some(":where is not available here"));
    let mut h = users(Lang::En);
    h.command("filter nonsense");
    assert!(runs(&mut h).is_empty());
    let said = h.app.overlays.command_line().and_then(|c| c.error.as_ref()).map(|n| n.render(&h.app.i18n).to_string());
    assert_eq!(said.as_deref(), Some("Usage: :filter clear"));
}

/// `shop.users` filtered on `id > 0`, a first page of rows and more not held (paging past them
/// runs the statement again), focused on the grid.
fn released(lang: Lang) -> Harness {
    released_with(lang, Some("id > 0"))
}

/// [`released`] with `filter` (`None`: every row).
fn released_with(lang: Lang, filter: Option<&str>) -> Harness {
    let mut h = Harness::connected(lang);
    open_users(&mut h);
    if let Some(f) = filter {
        answer(&mut h);
        h.command(&format!("where {f}"));
    }
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Released { id });
    let columns = Some(vec![meta("id", "int8", true, false)]);
    let rows = (0..h.app.page_size).map(|i| vec![Some(i.to_string())]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: true, elapsed: Duration::from_millis(2) });
    h.app.focus = datarig_tui::app::Focus::Results;
    h.sent();
    h
}

#[test]
fn paging_and_counting_check_the_shown_filter_again() {
    // As it ran: the next page runs it again, the count counts it.
    let mut h = released(Lang::En);
    h.keys("n");
    let sent = h.sent();
    assert!(
        sent.iter().any(|c| matches!(c, DbCommand::Resume { sql, .. } if sql == &format!("{ALL} WHERE (id > 0)"))),
        "{sent:?}"
    );
    // Refused now (the text says so): nothing is sent, the reason is said.
    let mut h = released(Lang::En);
    shown_mut(&mut h).filter = Some(Filter { text: "id > 0 -- x".into(), recognised: None });
    h.keys("n");
    assert!(h.sent().is_empty());
    assert!(status(&h).contains("a comment is not allowed"), "{}", status(&h));
    h.keys("#");
    assert!(h.sent().is_empty());
    assert!(status(&h).starts_with("Rows are not counted for you: a comment"), "{}", status(&h));
    // Calls the user did not let run in this tab: refused, never asked mid-paging.
    shown_mut(&mut h).filter = Some(Filter { text: "my_fn(id) > 0".into(), recognised: None });
    h.keys("n");
    assert!(!h.app.overlays.is_open(OverlayKind::Confirm));
    assert!(h.sent().is_empty());
    assert!(status(&h).contains("not confirmed in this tab"), "{}", status(&h));
}

#[test]
fn a_count_of_a_capped_query_counts_within_the_cap() {
    let mut h = released(Lang::En);
    h.keys("#");
    let plain: Vec<String> = h
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Count { sql, .. } => Some(sql),
            _ => None,
        })
        .collect();
    assert!(plain.len() == 1 && !plain[0].contains("LIMIT"), "{plain:?}");
    let index = h.app.tabs.active_index();
    let qid = h.app.tab().exec.query_id;
    h.tab_db(index, DbEvent::Counted { id: qid, result: Ok(10), snapshot: false });
    let mut h = released(Lang::En);
    shown_mut(&mut h).limit = Some(5);
    h.keys("#");
    let capped: Vec<String> = h
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Count { sql, .. } => Some(sql),
            _ => None,
        })
        .collect();
    assert!(capped.len() == 1 && capped[0].contains(&format!("{ALL} WHERE (id > 0) LIMIT 5")), "{capped:?}");
}

#[test]
fn a_filtered_table_is_not_exported_by_its_key() {
    let dir = std::env::temp_dir().join(format!("datarig-table-filter-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let key_asked = |sent: &[DbCommand]| sent.iter().any(|c| matches!(c, DbCommand::LoadStructure { .. }));
    // Every row of the table: read by its key, which its structure says.
    let mut h = released_with(Lang::En, None);
    h.command(&format!("export {}", dir.join("all.csv").display()));
    assert!(key_asked(&h.sent()));
    // Filtered: its statement runs again, as any other statement's rows are read.
    let mut h = released(Lang::En);
    h.command(&format!("export {}", dir.join("filtered.csv").display()));
    let sent = h.sent();
    assert!(!key_asked(&sent), "{sent:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
