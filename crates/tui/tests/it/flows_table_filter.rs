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
    assert!(status(&h).contains("Filter refused: ; ends the statement"), "{}", status(&h));
    let refusal = h.app.tab().doc.table_query.as_ref().and_then(|q| q.refusal.clone()).expect("refusal");
    assert_eq!((refusal.field, refusal.at), (Field::Where, Some(6..7)));
    // A row lock in a subquery, a comment, a call that waits: refused too.
    for text in ["id IN (SELECT id FROM shop.users FOR UPDATE)", "id = 1 -- x", "pg_sleep(1) IS NULL"] {
        h.command(&format!("where {text}"));
        assert!(runs(&mut h).is_empty(), "{text}");
        assert!(status(&h).starts_with("Filter refused: "), "{text}: {}", status(&h));
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
    assert!(status(&h).contains("comments are not allowed"), "{}", status(&h));
    h.keys("#");
    assert!(h.sent().is_empty());
    assert!(status(&h).starts_with("Rows are not counted for you: comments"), "{}", status(&h));
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

/// `space r v` opens the statement a run would send, checked as the run is: a draft refused
/// on a read-only profile (its first run cancelled, so no rows are held) is not opened.
#[test]
fn the_statement_opened_in_a_console_is_checked_as_a_run() {
    let mut h = read_only(Lang::En);
    open_users(&mut h);
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Failed { id, error: datarig_core::driver::DbError::Cancelled, cancelled: true });
    assert!(h.app.tab().doc.table_query.as_ref().unwrap().shown.is_none());
    h.sent();
    h.command("where my_fn(id) > 0");
    assert!(runs(&mut h).is_empty(), "refused");
    let tabs = h.app.tabs.len();
    h.app.focus = datarig_tui::app::Focus::Results;
    h.keys(" rv");
    assert_eq!(h.app.tabs.len(), tabs, "no console opened");
    assert!(h.app.tab().is_table());
    assert!(status(&h).contains("read-only connection"), "{}", status(&h));
    // A filter of known functions opens.
    h.command("where id > 1");
    h.keys(" rv");
    assert_eq!(h.app.tabs.len(), tabs + 1);
    assert!(h.app.tab().editor.text().contains("WHERE (id > 1)"), "{}", h.app.tab().editor.text());
}

/// The tab's title mark and the tab list describe the rows held: a filter whose run brought
/// no rows (cancelled) stays in the filter line only.
#[test]
fn the_title_mark_is_the_held_rows_filter() {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    answer(&mut h);
    h.command("where id > 1");
    let id = h.app.tab().exec.query_id;
    assert_eq!(datarig_tui::app::table_query::applied_filter(h.app.tab()), None, "in flight: the rows held are all");
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Failed { id, error: datarig_core::driver::DbError::Cancelled, cancelled: true });
    assert_eq!(datarig_tui::app::table_query::applied_filter(h.app.tab()), None);
    h.command("where id > 2");
    answer(&mut h);
    assert_eq!(datarig_tui::app::table_query::applied_filter(h.app.tab()), Some("id > 2"));
}

/// The export's statement is checked again just before it is sent: an environment turned
/// read-only while the export waited (its question open) refuses a filter calling a function
/// it cannot check, and nothing is sent.
#[test]
fn a_filtered_export_is_checked_again_when_its_statement_goes() {
    let dir = std::env::temp_dir().join(format!("datarig-table-filter-recheck-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    answer(&mut h);
    h.command("where my_fn(id) > 0");
    assert!(h.app.overlays.is_open(OverlayKind::Confirm));
    h.key(KeyCode::Char('y'));
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Released { id });
    let columns = Some(vec![meta("id", "int8", true, false)]);
    let rows = (0..h.app.page_size).map(|i| vec![Some(i.to_string())]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: true, elapsed: Duration::from_millis(2) });
    h.app.focus = datarig_tui::app::Focus::Results;
    h.sent();
    let path = dir.join("f.csv");
    h.command(&format!("export {}", path.display()));
    // Read-only from now on (the session too, so only the filter's check refuses).
    h.app.policies.insert("default", Policy { read_only: true, ..Policy::default() });
    h.app.tab_mut().exec.read_only = true;
    if h.app.overlays.is_open(OverlayKind::Confirm) {
        h.key(KeyCode::Char('y'));
    }
    let start = std::time::Instant::now();
    while h.app.exporting(h.app.tab().id) && start.elapsed() < Duration::from_secs(10) {
        h.app.pump_exports(Duration::from_millis(50));
    }
    let sent = h.sent();
    assert!(!sent.iter().any(|c| matches!(c, DbCommand::Export { .. })), "{sent:?}");
    assert!(!h.app.exporting(h.app.tab().id));
    let _ = std::fs::remove_dir_all(&dir);
}

/// `space r v` on a draft calling functions datarig cannot check asks as a run asks: Enter
/// opens nothing, `y` opens it, and the text once let through opens without asking again.
#[test]
fn the_statement_opened_in_a_console_asks_about_unchecked_calls() {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Failed { id, error: datarig_core::driver::DbError::Cancelled, cancelled: true });
    h.command("where my_fn(id) > 0");
    assert!(h.app.overlays.is_open(OverlayKind::Confirm));
    h.key(KeyCode::Enter);
    h.sent();
    let (tabs, table) = (h.app.tabs.len(), h.app.tab().id);
    h.app.focus = datarig_tui::app::Focus::Results;
    h.keys(" rv");
    assert_eq!(h.app.overlays.confirm().map(|c| c.action), Some(ConfirmAction::FilterCalls(table)));
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tabs.len(), tabs, "declined: nothing opened");
    h.keys(" rv");
    h.keys("y");
    assert_eq!(h.app.tabs.len(), tabs + 1);
    assert!(h.app.tab().editor.text().contains("WHERE (my_fn(id) > 0)"), "{}", h.app.tab().editor.text());
    assert!(runs(&mut h).is_empty(), "opened, not run");
    // Let through in that tab: opened at once.
    let at = h.app.tabs.position(table).unwrap();
    h.app.tabs.activate(at);
    h.app.focus = datarig_tui::app::Focus::Results;
    h.keys(" rv");
    assert!(!h.app.overlays.is_open(OverlayKind::Confirm));
    assert_eq!(h.app.tabs.len(), tabs + 2);
}

/// A table run that waited for its connection is checked again when it goes: the environment
/// turned read-only meanwhile refuses its filter's unchecked function, and nothing is sent.
#[test]
fn a_run_that_waited_for_its_connection_checks_its_filter_again() {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    answer(&mut h);
    let tab = h.app.tab().id;
    h.app.dispatch(datarig_tui::app::action::Action::DisconnectCurrent);
    if h.app.overlays.is_open(OverlayKind::Confirm) {
        h.keys("y");
    }
    h.sent();
    h.command("where my_fn(id) > 0");
    assert!(h.app.overlays.is_open(OverlayKind::Confirm));
    h.keys("y");
    assert!(h.app.is_queued(tab), "waits for the connection");
    h.app.policies.insert("default", Policy { read_only: true, ..Policy::default() });
    h.db(DbEvent::Connected);
    assert!(runs(&mut h).is_empty(), "refused when it went");
    assert!(status(&h).contains("read-only connection"), "{}", status(&h));
}

/// A refused export leaves a held portal open: checked before the portal is closed.
#[test]
fn a_refused_export_keeps_the_held_portal() {
    use datarig_core::driver::PagingMode;
    let dir = std::env::temp_dir().join(format!("datarig-table-filter-portal-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = Harness::connected(Lang::En);
    h.app.policies.insert("default", Policy { paging: PagingMode::Hold, ..Policy::default() });
    open_users(&mut h);
    answer(&mut h);
    h.command("where my_fn(id) > 0");
    h.keys("y");
    let id = h.app.tab().exec.query_id;
    page_of(&mut h, id, true);
    assert!(matches!(h.app.tab().exec.paging, datarig_tui::app::Paging::Open { .. }), "held");
    h.app.focus = datarig_tui::app::Focus::Results;
    h.sent();
    h.command(&format!("export {}", dir.join("p.csv").display()));
    h.app.policies.insert("default", Policy { read_only: true, paging: PagingMode::Hold, ..Policy::default() });
    h.app.tab_mut().exec.read_only = true;
    if h.app.overlays.is_open(OverlayKind::Confirm) {
        h.key(KeyCode::Char('y'));
    }
    let start = std::time::Instant::now();
    while h.app.exporting(h.app.tab().id) && start.elapsed() < Duration::from_secs(10) {
        h.app.pump_exports(Duration::from_millis(50));
    }
    let sent = h.sent();
    assert!(!sent.iter().any(|c| matches!(c, DbCommand::Export { .. } | DbCommand::ClosePortal { .. })), "{sent:?}");
    assert!(matches!(h.app.tab().exec.paging, datarig_tui::app::Paging::Open { .. }), "the portal stays");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `Esc` in the line puts back the query the next run sends, not only the texts: after a
/// refused apply, and after an apply whose pending-edits question was declined, the run key
/// runs what is applied.
#[test]
fn esc_puts_back_the_query_the_next_run_sends() {
    let mut h = users(Lang::En);
    h.command("where id > 1");
    answer(&mut h);
    h.sent();
    h.app.focus = datarig_tui::app::Focus::Results;
    h.keys("f");
    h.ctrl('l');
    h.type_text("id = 1;");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty(), "refused");
    h.key(KeyCode::Esc);
    h.ctrl('e');
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 1)")]]);
    answer(&mut h);
    // Pending edits: the question declined, Esc, then the run key (its question taken).
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        assert!(rs.edit.batch.delete(&[0], |_| Vec::new()));
    }
    h.app.focus = datarig_tui::app::Focus::Results;
    h.keys("f");
    h.ctrl('l');
    h.type_text("id > 5");
    h.key(KeyCode::Enter);
    assert!(h.app.overlays.is_open(OverlayKind::Confirm));
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    h.keys("f");
    h.key(KeyCode::Esc);
    h.ctrl('e');
    assert!(h.app.overlays.is_open(OverlayKind::Confirm));
    h.keys("y");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 1)")]]);
}

/// With many tabs their names are shortened: a filtered table tab keeps its mark, its name is
/// cut before it.
#[test]
fn a_shortened_title_keeps_the_filter_mark() {
    let mut h = users(Lang::En);
    h.command("where id > 1");
    answer(&mut h);
    for _ in 0..5 {
        h.command("tabnew");
    }
    h.keys(" 2");
    assert!(h.app.tab().is_table());
    let screen = h.screen(80, 24);
    let bar = screen.lines().next().unwrap_or_default().to_string();
    assert!(!bar.contains("shop.users"), "shortened: {bar}");
    let tab = bar.split(" 2 ").nth(1).unwrap_or_default();
    assert!(tab.split('\u{d7}').next().is_some_and(|t| t.trim_end().ends_with('*')), "{bar}");
}
