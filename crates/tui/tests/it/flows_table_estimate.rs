//! `#` on a table tab: unfiltered rows are counted at once (the explorer's estimate stays in the
//! title); filtered rows open a choice, the planner's estimate or the exact count. The estimate
//! is the statement the rows were read with, sent as `DbCommand::Estimate` only when the filter
//! calls nothing outside the allowlist (MySQL: no subquery), no transaction is open and the
//! server has a plan format; otherwise its entry is dimmed with why. Its answer is shown as the
//! planner's (`page 1 / ~N (planner)`), and a late one of rows replaced since is dropped. The
//! count has a time limit (`[results] count_timeout`), left out inside a PostgreSQL transaction
//! (said so); a count the server stopped, one cancelled and one behind a lock are told apart,
//! and none of them is 0.

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbError, DbEvent};
use datarig_core::profile::ConnectionConfig;
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_i18n::Lang;
use datarig_tui::app::Level;
use datarig_tui::app::overlay::OverlayKind;
use ratatui::crossterm::event::KeyCode;
use std::time::Duration;

const PG_ALL: &str = r#"SELECT * FROM "shop"."users""#;
const MY_ALL: &str = "SELECT * FROM `shop`.`users`";
const LIMIT: Option<Duration> = Some(Duration::from_secs(30));

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

/// Answer the active tab's run with a full first page, more rows past it, nothing held.
fn page(h: &mut Harness) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Released { id });
    let columns = Some(vec![meta("id", "int8", true, false)]);
    let rows = (0..h.app.page_size).map(|i| vec![Some(i.to_string())]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: true, elapsed: Duration::from_millis(2) });
}

/// PostgreSQL: `shop.users` with `filter` applied (`None`: every row), a page shown with more
/// past it, the grid focused, nothing sent since.
fn pg(filter: Option<&str>) -> Harness {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    page(&mut h);
    if let Some(f) = filter {
        h.command(&format!("where {f}"));
        page(&mut h);
    }
    h.app.focus = datarig_tui::app::Focus::Results;
    h.sent();
    h
}

/// MySQL: as [`pg`].
fn my(filter: Option<&str>) -> Harness {
    let driver = FakeDriver::default();
    driver.mysql.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut p = ConnectionConfig::test_db();
    (p.name, p.driver) = ("local-my".into(), "mysql".into());
    let cfg = Config { connections: vec![p], ..Default::default() };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["shop".into()])));
    h.db(DbEvent::Catalog(Ok(catalog())));
    open_users(&mut h);
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Language(Language::Sql(Dialect::MySql(MySqlMode::default()))));
    h.tab_db(index, DbEvent::Connected);
    page(&mut h);
    if let Some(f) = filter {
        h.command(&format!("where {f}"));
        page(&mut h);
    }
    h.app.focus = datarig_tui::app::Focus::Results;
    h.sent();
    h
}

fn status(h: &Harness) -> String {
    h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

/// The results title row.
fn title(h: &mut Harness, w: u16, hh: u16) -> String {
    let t = h.draw(w, hh);
    row_text(t.backend().buffer(), h.app.layout.results.y)
}

/// The counts sent since the last look: their statement and limit.
fn counts(sent: &[DbCommand]) -> Vec<(String, Option<Duration>)> {
    sent.iter()
        .filter_map(|c| match c {
            DbCommand::Count { sql, limit, .. } => Some((sql.clone(), *limit)),
            _ => None,
        })
        .collect()
}

/// The estimates sent since the last look: their id and statement.
fn estimates(sent: &[DbCommand]) -> Vec<(u64, String)> {
    sent.iter()
        .filter_map(|c| match c {
            DbCommand::Estimate { id, sql } => Some((*id, sql.clone())),
            _ => None,
        })
        .collect()
}

fn menu_open(h: &Harness) -> bool {
    h.app.overlays.is_open(OverlayKind::ContextMenu)
}

/// Pick the entry of the open choice whose label matches `what`.
fn pick(h: &mut Harness, what: &str) {
    assert!(menu_open(h), "the choice is open");
    h.keys(what);
    h.key(KeyCode::Enter);
}

/// The choice's line naming `entry`, as drawn.
fn entry_line(h: &mut Harness, entry: &str) -> String {
    h.screen(120, 30).lines().find(|l| l.contains(entry)).unwrap_or_default().to_string()
}

#[test]
fn unfiltered_rows_are_counted_at_once_with_the_time_limit() {
    let mut h = pg(None);
    h.keys("#");
    assert!(!menu_open(&h), "no choice without a filter");
    assert_eq!(counts(&h.sent()), vec![(format!("SELECT count(*) FROM (\n{PG_ALL}\n) AS datarig_count"), LIMIT)]);
    assert!(status(&h).starts_with("Counting rows, sent for you: "), "{}", status(&h));
    // The limit off: none.
    let mut h = pg(None);
    h.app.prefs.count_timeout = None;
    h.keys("#");
    assert_eq!(counts(&h.sent())[0].1, None);
    let mut h = pg(None);
    h.app.prefs.count_timeout = Some(Duration::from_secs(120));
    h.keys("#");
    assert_eq!(counts(&h.sent())[0].1, Some(Duration::from_secs(120)));
}

#[test]
fn filtered_rows_offer_the_estimate_or_the_exact_count() {
    let mut h = pg(Some("id > 5"));
    h.keys("#");
    assert!(menu_open(&h));
    assert!(h.sent().is_empty(), "nothing is sent before a pick");
    assert!(entry_line(&mut h, "Estimate (planner)").contains("nothing is read"));
    assert!(entry_line(&mut h, "Exact count").contains("stops after 30s"));
    assert_screen!("table_count_choice_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("table_count_choice_en_120x30", Lang::En, h.draw(120, 30));
    // The exact count: the statement the rows were read with, counted with its limit.
    pick(&mut h, "exact");
    let sent = h.sent();
    assert_eq!(
        counts(&sent),
        vec![(format!("SELECT count(*) FROM (\n{PG_ALL} WHERE (id > 5)\n) AS datarig_count"), LIMIT)]
    );
    assert!(estimates(&sent).is_empty());
    // The estimate: the statement itself, the driver plans it.
    let mut h = pg(Some("id > 5"));
    h.keys("#");
    pick(&mut h, "estimate");
    let sent = h.sent();
    let qid = h.app.tab().exec.query_id;
    assert_eq!(estimates(&sent), vec![(qid, format!("{PG_ALL} WHERE (id > 5)"))]);
    assert!(counts(&sent).is_empty());
    assert!(status(&h).starts_with("Asking the planner for an estimate (EXPLAIN, no row read), sent for you: "));
    assert!(title(&mut h, 120, 30).contains("estimating…"), "{}", title(&mut h, 120, 30));
    assert_screen!("table_estimating_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("table_estimating_en_120x30", Lang::En, h.draw(120, 30));
    assert!(h.app.tab_busy(h.app.tab().id), "nothing else runs meanwhile");
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Estimated { id: qid, result: Ok(5_900) });
    assert!(h.app.tab().exec.running.is_none());
    assert!(status(&h).contains("Planner estimate: about 5,900 rows (not a count)"), "{}", status(&h));
    let wide = title(&mut h, 160, 45);
    assert!(wide.contains("page 1 / ~12 (planner)"), "{wide}");
    assert_screen!("table_estimate_title_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("table_estimate_title_en_120x30", Lang::En, h.draw(120, 30));
    // A late answer of another request changes nothing.
    h.tab_db(index, DbEvent::Estimated { id: qid + 7, result: Ok(1) });
    assert!(title(&mut h, 160, 45).contains("~12 (planner)"));
}

/// The status bar's spinner turns with the app's clock, as the tab bar's does: what is drawn does
/// not depend on how long the machine took to get here.
#[test]
fn the_status_bar_spinner_turns_with_the_apps_clock() {
    let mut h = pg(Some("id > 5"));
    h.keys("#");
    pick(&mut h, "estimate");
    let first = h.status(120, 30);
    assert!(first.contains("\u{280b} estimating"), "{first}");
    h.advance(Duration::from_millis(250));
    let later = h.status(120, 30);
    assert!(later.contains("\u{2839} estimating"), "{later}");
}

#[test]
fn an_estimate_of_rows_replaced_since_is_dropped() {
    let mut h = pg(Some("id > 5"));
    h.keys("#");
    pick(&mut h, "estimate");
    let [(old, _)] = &estimates(&h.sent())[..] else { panic!("one estimate") };
    let old = *old;
    // The user cancels it and filters again before its answer comes.
    h.ctrl('c');
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Estimated { id: old, result: Err(DbError::Cancelled) });
    assert!(status(&h).contains("Estimate cancelled"), "{}", status(&h));
    h.command("where id > 6");
    page(&mut h);
    h.tab_db(index, DbEvent::Estimated { id: old, result: Ok(9_999) });
    assert!(title(&mut h, 120, 30).contains("page 1 / ?"), "{}", title(&mut h, 120, 30));
    assert!(!status(&h).contains("9,999"));
}

#[test]
fn the_estimate_is_dimmed_with_why_when_it_cannot_be_asked() {
    // A filter calling a function the allowlist does not know, even one the user let run.
    let mut h = pg(None);
    h.command("where my_fn(id) > 0");
    h.keys("y");
    page(&mut h);
    h.app.focus = datarig_tui::app::Focus::Results;
    h.sent();
    h.keys("#");
    assert!(entry_line(&mut h, "Estimate (planner)").contains("unchecked calls"));
    assert_screen!("table_count_choice_dimmed_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("table_count_choice_dimmed_en_120x30", Lang::En, h.draw(120, 30));
    // Rows with a call off the allowlist are not run again for the user: no count either.
    assert!(entry_line(&mut h, "Exact count").contains("refused now"));
    h.key(KeyCode::Enter);
    assert!(counts(&h.sent()).is_empty());
    // A transaction open on the session: no estimate, and the count has no limit there.
    let mut h = pg(Some("id > 5"));
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Block(true));
    h.tab_db(index, DbEvent::TxOpen(true));
    h.keys("#");
    assert!(entry_line(&mut h, "Estimate (planner)").contains("tx is open"));
    assert!(entry_line(&mut h, "Exact count").contains("no time limit inside your transaction"));
    pick(&mut h, "exact");
    assert_eq!(counts(&h.sent())[0].1, None);
    assert!(status(&h).starts_with("Counting rows (no time limit inside your transaction), sent for you: "));
    // Picking a dimmed entry by its key says why, and sends nothing.
    let mut h = pg(Some("id > 5"));
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::TxOpen(true));
    h.keys("#");
    pick(&mut h, "estimate");
    assert!(estimates(&h.sent()).is_empty());
}

#[test]
fn mysql_has_no_estimate_with_a_subquery_or_without_plans_and_always_a_limit() {
    let mut h = my(Some("id IN (SELECT id FROM orders)"));
    h.keys("#");
    assert!(entry_line(&mut h, "Estimate (planner)").contains("subquery inside"));
    h.key(KeyCode::Esc);
    let mut h = my(Some("id > 5"));
    h.keys("#");
    pick(&mut h, "estimate");
    let qid = h.app.tab().exec.query_id;
    assert_eq!(estimates(&h.sent()), vec![(qid, format!("{MY_ALL} WHERE (id > 5)"))]);
    // A server without a plan format (MariaDB).
    let mut h = my(Some("id > 5"));
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Explain(None));
    h.keys("#");
    assert!(entry_line(&mut h, "Estimate (planner)").contains("no plans here"));
    // A transaction open: the count keeps its limit (for the statement alone).
    let mut h = my(Some("id > 5"));
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Isolation(Some(datarig_core::driver::Isolation::RepeatableRead)));
    h.tab_db(index, DbEvent::TxOpen(true));
    h.keys("#");
    assert!(entry_line(&mut h, "Estimate (planner)").contains("tx is open"));
    pick(&mut h, "exact");
    assert_eq!(counts(&h.sent())[0].1, LIMIT);
}

#[test]
fn a_count_stopped_cancelled_or_lost_is_never_a_number() {
    let answer = |error: DbError| {
        let mut h = pg(None);
        h.keys("#");
        h.sent();
        let (qid, index) = (h.app.tab().exec.query_id, h.app.tabs.active_index());
        h.tab_db(index, DbEvent::Counted { id: qid, result: Err(error), snapshot: false });
        assert!(h.app.tab().exec.running.is_none());
        let t = title(&mut h, 120, 30);
        assert!(t.contains("page 1 / ?"), "{t}");
        // A count that did not come is said as a warning (a failure of the server's as an error).
        assert_eq!(h.app.tab().status.as_ref().map(|n| n.level), Some(Level::Warning), "{}", status(&h));
        status(&h)
    };
    assert_eq!(
        answer(DbError::TimedOut(Duration::from_secs(30))),
        "Count stopped after 30s: the number of rows is not known"
    );
    assert_eq!(
        answer(DbError::TimedOut(Duration::from_millis(1500))),
        "Count stopped after 1500ms: the number of rows is not known"
    );
    assert!(
        answer(DbError::LockTimeout("ERROR: canceling statement due to lock timeout".into()))
            .starts_with("Count stopped: it waited too long for a lock")
    );
    assert_eq!(answer(DbError::Cancelled), "Count cancelled");
    // MySQL's refusal to use up the next transaction's settings: said as a refusal with why,
    // as a warning, never as a failure.
    let refused = answer(DbError::NextTransactionSet);
    assert!(refused.starts_with("Rows not counted: your next transaction has settings"), "{refused}");
    // The connection lost while it counts: the usual words, no count.
    let mut h = pg(None);
    h.keys("#");
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Lost { error: DbError::Closed });
    assert!(h.app.tab().exec.running.is_none());
    assert!(!status(&h).contains("Counted"), "{}", status(&h));
    let mut h = pg(None);
    h.keys("#");
    let (qid, index) = (h.app.tab().exec.query_id, h.app.tabs.active_index());
    let stopped = Err(DbError::TimedOut(Duration::from_secs(30)));
    h.tab_db(index, DbEvent::Counted { id: qid, result: stopped, snapshot: false });
    assert_screen!("table_count_stopped_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("table_count_stopped_en_120x30", Lang::En, h.draw(120, 30));
}

#[test]
fn an_estimate_that_could_not_be_had_says_why_and_shows_no_pages() {
    let answer = |error: DbError| {
        let mut h = pg(Some("id > 5"));
        h.keys("#");
        pick(&mut h, "estimate");
        let (qid, index) = (h.app.tab().exec.query_id, h.app.tabs.active_index());
        h.tab_db(index, DbEvent::Estimated { id: qid, result: Err(error) });
        assert!(h.app.tab().exec.running.is_none());
        assert!(title(&mut h, 120, 30).contains("page 1 / ?"));
        status(&h)
    };
    assert_eq!(answer(DbError::TimedOut(Duration::from_secs(5))), "Estimate stopped after 5s: no estimate");
    assert!(answer(DbError::LockTimeout("lock".into())).starts_with("Estimate stopped: it waited too long for a lock"));
    assert!(answer(DbError::NotSupported).starts_with("No estimate: the session did not ask"));
    assert_eq!(answer(DbError::NoResult), "The server gave no estimate");
    assert!(answer(DbError::Closed).starts_with("Could not get an estimate: "));
    // An estimate below the rows already read is not shown (it would read `page 1 / ~1`).
    let mut h = pg(Some("id > 5"));
    h.keys("#");
    pick(&mut h, "estimate");
    let (qid, index) = (h.app.tab().exec.query_id, h.app.tabs.active_index());
    h.tab_db(index, DbEvent::Estimated { id: qid, result: Ok(3) });
    assert!(title(&mut h, 120, 30).contains("page 1 / ?"));
}

#[test]
fn a_relation_the_server_will_not_have_planned_dims_the_estimate_with_why() {
    use datarig_core::sql::risk::repeat::NotRepeatable;
    let mut h = pg(Some("id > 5"));
    h.keys("#");
    pick(&mut h, "estimate");
    let (qid, index) = (h.app.tab().exec.query_id, h.app.tabs.active_index());
    assert_eq!(estimates(&h.sent()).len(), 1);
    let why = NotRepeatable::PlannerCode("public.users".into());
    h.tab_db(index, DbEvent::Estimated { id: qid, result: Err(DbError::NotRepeatable(why)) });
    assert!(status(&h).contains("planning public.users may run code"), "{}", status(&h));
    h.keys("#");
    assert!(entry_line(&mut h, "Estimate (planner)").contains("refused now"));
    pick(&mut h, "estimate");
    assert!(estimates(&h.sent()).is_empty(), "not asked again");
    assert!(status(&h).contains("planning public.users may run code"), "{}", status(&h));
}

/// Rows that cannot be run again (only their first page is shown, a confirmed call off the
/// allowlist) get no exact count: its entry is dimmed, not offered and refused, with the reason
/// a count would be refused with said as the choice opens.
#[test]
fn rows_that_cannot_be_run_again_dim_the_exact_count_with_why() {
    let mut h = pg(None);
    h.command("where my_fn(id) > 0");
    h.keys("y");
    page(&mut h);
    h.app.focus = datarig_tui::app::Focus::Results;
    h.sent();
    assert!(h.screen(120, 30).contains("first page only"));
    h.keys("#");
    let line = entry_line(&mut h, "Exact count");
    assert!(line.contains("refused now"), "{line}");
    assert!(!line.contains("time limit"), "not offered with what it would do: {line}");
    let screen = h.screen(120, 30);
    assert!(screen.contains("Rows are not counted for you: it calls a function that is not a built-in"), "{screen}");
    h.key(KeyCode::Enter);
    assert!(counts(&h.sent()).is_empty());
    assert!(h.app.tab().exec.running.is_none());
}

#[test]
fn a_held_result_dims_the_estimate_and_keeps_paging() {
    let mut h = my(Some("id > 5"));
    h.app.tab_mut().exec.paging =
        datarig_tui::app::paging::Paging::Open { since: std::time::Instant::now(), in_block: false };
    h.keys("#");
    assert!(entry_line(&mut h, "Estimate (planner)").contains("result held"));
    pick(&mut h, "estimate");
    assert!(estimates(&h.sent()).is_empty());
    assert!(h.app.tab().exec.running.is_none(), "nothing waits behind the held result");
}
