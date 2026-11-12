//! The value list of the filter dialog's selected column: counted in the rows loaded, with
//! the values of an earlier load once the column is filtered, or counted by the server with
//! `#` (the statement written by the query model, under the draft's other conditions only).
//! `Space` ticks a value into the column's condition (`=`, then `in list` in the same place);
//! a value the filter cannot take as read, or of a count whose scope changed, is not ticked.

use crate::common;

use common::*;
use datarig_core::driver::{
    ColumnMeta, DbCommand, DbError, DbEvent, GroupCounts, GroupValue, SessionValues, ValueKind,
};
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_i18n::Lang;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
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

/// The counts of a column's values sent since the last look: request, statement, time limit.
fn counts(h: &mut Harness) -> Vec<(u64, String, Option<Duration>)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::GroupCount { id, sql, limit } => Some((id, sql, limit)),
            _ => None,
        })
        .collect()
}

fn columns() -> Vec<ColumnMeta> {
    vec![meta("id", "int8", true, false), meta("status", "text", false, false), meta("amount", "numeric", true, false)]
}

/// `n` rows: ids from 1, statuses `paid` (5 of 12), `shipped` (4), `cancelled` (2) and NULL (1)
/// in turn, amounts all different.
fn rows(n: usize) -> Vec<Vec<Option<String>>> {
    const STATUS: [Option<&str>; 12] = [
        Some("paid"),
        Some("shipped"),
        Some("paid"),
        Some("cancelled"),
        Some("shipped"),
        Some("paid"),
        None,
        Some("shipped"),
        Some("paid"),
        Some("cancelled"),
        Some("shipped"),
        Some("paid"),
    ];
    (0..n)
        .map(|i| {
            vec![
                Some((i + 1).to_string()),
                STATUS[i % 12].map(str::to_string),
                Some(format!("{}.25", 5_448_060 + i * 7_919)),
            ]
        })
        .collect()
}

/// Only the rows of `rows(n)` whose status is `status`.
fn rows_of(n: usize, status: &str) -> Vec<Vec<Option<String>>> {
    rows(n).into_iter().filter(|r| r[1].as_deref() == Some(status)).collect()
}

/// Answer the active tab's current run with `rows`.
fn answer(h: &mut Harness, rows: Vec<Vec<Option<String>>>) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(
        index,
        DbEvent::Page { id, columns: Some(columns()), rows, more: false, elapsed: Duration::from_millis(2) },
    );
}

/// `shop.users` open with 60 rows loaded, the grid focused, nothing sent since.
fn users() -> Harness {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    answer(&mut h, rows(60));
    h.sent();
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h
}

/// The dialog opened on column `col` of the grid.
fn dialog_on(h: &mut Harness, col: usize) {
    h.app.tab_mut().grid.col = col;
    h.keys("f");
    assert!(h.app.overlays.is_open(OverlayKind::FilterDialog), "{:?} {}", h.app.key_context(), h.screen(120, 34));
}

/// From the column list to the first value of the list, past the column's `conditions`.
fn to_values(h: &mut Harness, conditions: usize) {
    h.key(KeyCode::Right);
    for _ in 0..=conditions {
        h.key(KeyCode::Down);
    }
}

/// The open dialog's `WHERE` text as it would be sent.
fn draft(h: &Harness) -> String {
    h.app.filter_dialog_draft().expect("the dialog is open").0
}

fn value(v: Option<&str>, rows: u64) -> GroupValue {
    GroupValue {
        value: v.map(str::to_string),
        whole: true,
        bytes: v.map(|v| v.len() as u64),
        rows,
        prefix: false,
        inexact: false,
    }
}

/// The server's answer to count `id`.
fn counted(h: &mut Harness, id: u64, result: Result<GroupCounts, DbError>) {
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::GroupCounted { id, result, snapshot: false });
}

/// A count of `status`: `paid` 900, `refunded` 40 (not loaded), NULL 7, a long value of 5000
/// bytes in 3 rows; more than these.
fn status_counts() -> GroupCounts {
    let long = GroupValue { value: None, whole: false, bytes: Some(5000), rows: 3, prefix: false, inexact: false };
    GroupCounts {
        values: vec![value(Some("paid"), 900), value(Some("refunded"), 40), value(None, 7), long],
        more: true,
    }
}

#[test]
fn a_ticked_value_is_sent_as_the_columns_condition() {
    let mut h = users();
    dialog_on(&mut h, 1);
    let screen = h.screen(120, 34);
    assert!(screen.contains("~ count in the 60 loaded rows"), "{screen}");
    // Most rows first: paid 25, shipped 20, cancelled 10, NULL 5.
    to_values(&mut h, 0);
    h.key(KeyCode::Char(' '));
    assert_eq!(draft(&h), "status = 'paid'");
    assert!(runs(&mut h).is_empty(), "a tick changes the draft only");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (status = 'paid')")]]);
}

#[test]
fn a_ticked_null_is_is_null_and_a_second_tick_makes_a_list() {
    let mut h = users();
    dialog_on(&mut h, 1);
    to_values(&mut h, 0);
    for _ in 0..3 {
        h.key(KeyCode::Down);
    }
    h.key(KeyCode::Char(' '));
    assert_eq!(draft(&h), "status IS NULL");
    // The cursor stays on NULL; `paid` widens the same condition.
    h.keys("g");
    h.key(KeyCode::Char(' '));
    assert_eq!(draft(&h), "(status IN ('paid') OR status IS NULL)");
    // Unticked: back to one value, then none.
    h.key(KeyCode::Char(' '));
    assert_eq!(draft(&h), "status IS NULL");
    h.keys("G");
    h.key(KeyCode::Char(' '));
    assert_eq!(draft(&h), "");
    // Each tick was one step.
    h.keys("u");
    assert_eq!(draft(&h), "status IS NULL");
}

#[test]
fn an_earlier_load_widens_the_condition_to_a_list_in_its_place() {
    let mut h = users();
    h.command("where status = 'paid' AND id > 0");
    assert_eq!(runs(&mut h).len(), 1);
    answer(&mut h, rows_of(60, "paid"));
    dialog_on(&mut h, 1);
    let screen = h.screen(120, 34);
    assert!(screen.contains("~ count: 25 loaded rows + an earlier load"), "{screen}");
    assert!(screen.contains("earlier load"), "{screen}");
    // `paid` loaded, then what the first load showed: shipped, cancelled, NULL.
    to_values(&mut h, 1);
    h.key(KeyCode::Down);
    h.key(KeyCode::Char(' '));
    assert_eq!(draft(&h), "status IN ('paid', 'shipped') AND id > 0", "the chip keeps its place");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (status IN ('paid', 'shipped') AND id > 0)")]]);
}

#[test]
fn typing_narrows_the_list_and_a_tick_takes_the_place_of_what_was_typed() {
    let mut h = users();
    dialog_on(&mut h, 1);
    h.key(KeyCode::Right);
    h.key(KeyCode::Char(' '));
    h.type_text("ip");
    let screen = h.screen(120, 34);
    assert!(screen.contains("containing “ip”"), "{screen}");
    assert!(screen.contains("shipped") && !screen.contains("cancelled"), "{screen}");
    assert_eq!(draft(&h), "status = 'ip'");
    // `↓` into the list (only `shipped` holds `ip`), the tick replaces `ip`.
    h.key(KeyCode::Down);
    h.key(KeyCode::Char(' '));
    assert_eq!(draft(&h), "status = 'shipped'");
    assert!(h.screen(120, 34).contains("cancelled"), "the list in full again");
}

/// A value typed in quotes with a blank around it narrows the list; the value ticked takes its
/// place, as one typed bare does.
#[test]
fn a_tick_takes_the_place_of_a_value_typed_in_quotes() {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    let rows = ["x a", "x a", "y"]
        .iter()
        .enumerate()
        .map(|(i, s)| vec![Some(i.to_string()), Some(s.to_string()), Some("1".into())])
        .collect();
    answer(&mut h, rows);
    h.sent();
    h.key(KeyCode::Tab);
    dialog_on(&mut h, 1);
    h.key(KeyCode::Right);
    h.key(KeyCode::Char(' '));
    h.type_text("' a'");
    assert_eq!(draft(&h), "status = ' a'");
    h.key(KeyCode::Down);
    h.key(KeyCode::Char(' '));
    assert_eq!(draft(&h), "status = 'x a'");
}

/// MySQL bytes typed `0xab` are the value the list shows `0xAB`: ticking it keeps one `=`,
/// never a list of the same value twice nor no condition.
#[test]
fn a_tick_of_the_bytes_typed_in_another_case_keeps_one_value() {
    let mut h = mysql();
    // Run again for a page of bytes.
    h.command("filter clear");
    assert_eq!(runs(&mut h).len(), 1);
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = vec![col("id", "int", ValueKind::Integer), col("b", "varbinary", ValueKind::Bytes)];
    let rows = ["0xAB", "0xAB", "0xCD"]
        .iter()
        .enumerate()
        .map(|(i, b)| vec![Some(i.to_string()), Some(b.to_string())])
        .collect();
    h.tab_db(index, DbEvent::Page { id, columns: Some(columns), rows, more: false, elapsed: Duration::from_millis(2) });
    dialog_on(&mut h, 1);
    h.key(KeyCode::Right);
    h.key(KeyCode::Char(' '));
    h.type_text("0xab");
    let typed = draft(&h);
    assert!(typed.starts_with("b = ") && typed.to_lowercase().contains("ab"), "{typed}");
    h.key(KeyCode::Down);
    let screen = h.screen(120, 34);
    assert!(screen.contains("0xAB") && !screen.contains("0xCD"), "{screen}");
    h.key(KeyCode::Char(' '));
    assert_eq!(draft(&h), typed, "the same value, once");
}

#[test]
fn count_exactly_sends_the_columns_count_under_the_other_conditions_only() {
    let mut h = users();
    h.command("where status = 'paid' AND id > 0");
    runs(&mut h);
    answer(&mut h, rows_of(60, "paid"));
    dialog_on(&mut h, 1);
    h.keys("#");
    let sent = counts(&mut h);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(
        sent[0].1,
        r#"SELECT CASE WHEN octet_length("status") <= 4096 THEN "status" END, octet_length("status"), count(*) FROM "shop"."users" WHERE (id > 0) GROUP BY "status" ORDER BY count(*) DESC LIMIT 201"#
    );
    assert!(sent[0].2.is_some(), "an idle session's count has a time limit");
    assert!(h.screen(120, 34).contains("counting…"));
    // Busy meanwhile: applying waits for it.
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    counted(&mut h, sent[0].0, Ok(status_counts()));
    let screen = h.screen(120, 34);
    assert!(screen.contains("exact · other conditions applied"), "{screen}");
    assert!(screen.contains("refunded") && screen.contains("900"), "{screen}");
    assert!(screen.contains("long value (5000 bytes)"), "{screen}");
    assert!(screen.contains("more values than shown"), "{screen}");
    // A value the rows loaded did not hold is ticked from the count.
    to_values(&mut h, 1);
    h.key(KeyCode::Down);
    h.key(KeyCode::Char(' '));
    assert_eq!(draft(&h), "status IN ('paid', 'refunded') AND id > 0");
    // The long value is not.
    h.keys("G");
    h.key(KeyCode::Char(' '));
    assert!(h.screen(120, 34).contains("Not ticked: a long value (5000 bytes)"), "{}", h.screen(120, 34));
    assert_eq!(draft(&h), "status IN ('paid', 'refunded') AND id > 0");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (status IN ('paid', 'refunded') AND id > 0)")]]);
    // With no other condition, no `WHERE`; a number is sent as it is.
    let mut h = users();
    dialog_on(&mut h, 0);
    h.keys("#");
    let sent = counts(&mut h);
    assert_eq!(
        sent[0].1,
        r#"SELECT "id", NULL, count(*) FROM "shop"."users" GROUP BY "id" ORDER BY count(*) DESC LIMIT 201"#
    );
}

#[test]
fn a_late_answer_of_another_count_is_dropped() {
    let mut h = users();
    dialog_on(&mut h, 1);
    h.keys("#");
    let first = counts(&mut h)[0].0;
    // An answer of no count of the dialog's.
    counted(&mut h, first + 100, Ok(status_counts()));
    assert!(h.screen(120, 34).contains("counting…"));
    // `Esc` cancels the count (the dialog stays), its answer says so.
    let index = h.app.tabs.active_index();
    h.key(KeyCode::Esc);
    assert!(h.app.overlays.is_open(OverlayKind::FilterDialog));
    assert!(h.session_cancelled(index));
    counted(&mut h, first, Err(DbError::Cancelled));
    assert!(h.screen(120, 34).contains("Values count cancelled"), "{}", h.screen(120, 34));
    // Counted again: the first count's answer, late, is dropped.
    h.keys("#");
    let second = counts(&mut h)[0].0;
    assert_ne!(first, second);
    counted(&mut h, first, Ok(status_counts()));
    let screen = h.screen(120, 34);
    assert!(!screen.contains("refunded") && screen.contains("counting…"), "{screen}");
    let only = GroupCounts { values: vec![value(Some("lost"), 1)], more: false };
    counted(&mut h, second, Ok(only));
    let screen = h.screen(120, 34);
    assert!(screen.contains("lost") && screen.contains("exact · every row"), "{screen}");
    // `Esc` with no count running closes.
    h.key(KeyCode::Esc);
    assert!(!h.app.overlays.is_open(OverlayKind::FilterDialog));
}

#[test]
fn a_count_of_other_conditions_is_not_ticked_until_counted_again() {
    let mut h = users();
    h.command("where status = 'paid' AND id > 0");
    runs(&mut h);
    answer(&mut h, rows_of(60, "paid"));
    dialog_on(&mut h, 1);
    h.keys("#");
    let id = counts(&mut h)[0].0;
    counted(&mut h, id, Ok(status_counts()));
    // The other condition taken out: the count is of another scope.
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.key(KeyCode::Right);
    h.key(KeyCode::Char('x'));
    assert_eq!(draft(&h), "status = 'paid'");
    let screen = h.screen(120, 34);
    assert!(screen.contains("earlier count (other conditions changed)"), "{screen}");
    h.key(KeyCode::BackTab);
    h.key(KeyCode::BackTab);
    to_values(&mut h, 1);
    h.key(KeyCode::Down);
    h.key(KeyCode::Char(' '));
    assert_eq!(draft(&h), "status = 'paid'", "not ticked");
    assert!(h.screen(120, 34).contains("Not ticked: counted under other conditions"), "{}", h.screen(120, 34));
    // Undone, the scope is the count's again.
    h.key_mod(KeyCode::Char('z'), KeyModifiers::CONTROL);
    assert_eq!(draft(&h), "status = 'paid' AND id > 0");
    assert!(h.screen(120, 34).contains("exact · other conditions applied"));
    h.key(KeyCode::Char(' '));
    assert_eq!(draft(&h), "status IN ('paid', 'refunded') AND id > 0");
}

#[test]
fn a_refused_other_condition_refuses_the_count_and_sends_nothing() {
    let mut h = users();
    h.command("where status = 'paid' AND pg_sleep(1) IS NULL");
    assert!(runs(&mut h).is_empty(), "refused by the filter's check");
    dialog_on(&mut h, 1);
    h.keys("#");
    assert!(counts(&mut h).is_empty());
    let screen = h.screen(120, 34);
    assert!(screen.contains("Values not counted:"), "{screen}");
    assert!(screen.contains("pg_sleep"), "{screen}");
    // A column whose type is not known (no rows of it loaded) is not counted either.
    h.key(KeyCode::Esc);
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    h.sent();
    h.keys("\t");
    h.keys("f");
    h.keys("#");
    assert!(counts(&mut h).is_empty());
}

#[test]
fn a_read_only_profile_counts_reads_and_refuses_unknown_calls() {
    let mut cfg = datarig_core::config::Config {
        connections: vec![datarig_core::profile::ConnectionConfig {
            policy: Some("prod".into()),
            ..datarig_core::profile::ConnectionConfig::test_db()
        }],
        ..Default::default()
    };
    cfg.policies.insert("prod", datarig_core::policy::Policy { read_only: true, ..Default::default() });
    let mut h = Harness::connected_with(&cfg, Lang::En);
    open_users(&mut h);
    answer(&mut h, rows(12));
    h.sent();
    for _ in 0..4 {
        if h.app.key_context() == Ctx::Grid {
            break;
        }
        h.key(KeyCode::Tab);
    }
    assert_eq!(h.app.key_context(), Ctx::Grid);
    dialog_on(&mut h, 1);
    h.keys("#");
    assert_eq!(counts(&mut h).len(), 1, "a count is a read");
    let id = h.app.tab().exec.running.map(|r| r.id).expect("the count runs");
    counted(&mut h, id, Err(DbError::Cancelled));
    h.key(KeyCode::Esc);
    h.command("where status = 'paid' AND f(id) > 1");
    assert!(runs(&mut h).is_empty());
    dialog_on(&mut h, 1);
    h.keys("#");
    assert!(counts(&mut h).is_empty());
    assert!(h.screen(120, 34).contains("Values not counted:"), "{}", h.screen(120, 34));
}

#[test]
fn each_outcome_of_a_count_is_said_and_never_as_counts() {
    let cases: [(DbError, &str); 7] = [
        (DbError::TimedOut(Duration::from_secs(30)), "Values count stopped after 30s: not known"),
        (DbError::LockTimeout("waited".into()), "Values count stopped waiting for a lock"),
        (DbError::Cancelled, "Values count cancelled"),
        (DbError::LockingRead, "Values not counted: in the open transaction its reads would lock rows"),
        // A refusal said as one, with why (MySQL: a count would use up the next transaction's
        // settings), never as a failure.
        (DbError::NextTransactionSet, "Values not counted: your next transaction has settings"),
        (DbError::SchemaChanged, "Values not counted: the column's type changed"),
        (DbError::AccessDenied("boom".into()), "Values count failed:"),
    ];
    let mut h = users();
    dialog_on(&mut h, 1);
    for (error, said) in cases {
        h.keys("#");
        let id = counts(&mut h)[0].0;
        counted(&mut h, id, Err(error));
        let screen = h.screen(120, 34);
        assert!(screen.contains(said), "{said}: {screen}");
        assert!(said.contains("failed") || !screen.contains("failed"), "{said}: not said as a failure: {screen}");
        assert!(screen.contains("~ count in the 60 loaded rows"), "the loaded values stay: {screen}");
    }
}

#[test]
fn the_values_screens() {
    let mut h = users();
    keys_came(&mut h, "local-pg", &["id"]);
    dialog_on(&mut h, 1);
    to_values(&mut h, 0);
    h.key(KeyCode::Char(' '));
    h.key(KeyCode::Down);
    assert_screen!("filter_values_column_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_values_column_en_120x34", Lang::En, h.draw(120, 34));
    // Typing narrows the list.
    h.key(KeyCode::Esc);
    h.app.tab_mut().grid.col = 1;
    h.keys("f");
    h.key(KeyCode::Right);
    h.key(KeyCode::Char(' '));
    h.type_text("p");
    assert_screen!("filter_values_typing_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_values_typing_en_120x34", Lang::En, h.draw(120, 34));
    // Numbers mostly distinct: their range.
    h.key(KeyCode::Esc);
    dialog_on(&mut h, 2);
    assert_screen!("filter_values_range_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_values_range_en_120x34", Lang::En, h.draw(120, 34));
    h.key(KeyCode::Esc);
    // Widened with an earlier load.
    h.command("where status = 'paid' AND id > 0");
    runs(&mut h);
    answer(&mut h, rows_of(60, "paid"));
    dialog_on(&mut h, 1);
    to_values(&mut h, 1);
    h.key(KeyCode::Down);
    h.key(KeyCode::Char(' '));
    assert_screen!("filter_values_widen_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_values_widen_en_120x34", Lang::En, h.draw(120, 34));
    // The server's count.
    h.keys("#");
    let id = counts(&mut h)[0].0;
    counted(&mut h, id, Ok(status_counts()));
    assert_screen!("filter_values_exact_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_values_exact_en_120x34", Lang::En, h.draw(120, 34));
}

fn col(name: &str, ty: &str, kind: ValueKind) -> ColumnMeta {
    ColumnMeta::new(name.into(), ty.into(), kind, None)
}

/// A MySQL profile with `shop.users` open on it (an id and a TIMESTAMP), the session's time
/// zone `+00:00`, its rows loaded, the grid focused.
fn mysql() -> Harness {
    let driver = FakeDriver::default();
    driver.mysql.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut p = datarig_core::profile::ConnectionConfig::test_db();
    (p.name, p.driver) = ("local-my".into(), "mysql".into());
    let cfg = datarig_core::config::Config { connections: vec![p], ..Default::default() };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["shop".into()])));
    open_users(&mut h);
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Language(Language::Sql(Dialect::MySql(MySqlMode::default()))));
    h.tab_db(index, DbEvent::Connected);
    zone(&mut h, "+00:00");
    my_page(&mut h, &["2026-11-10 09:30:00", "2026-11-11 10:00:00"]);
    h.sent();
    h.key(KeyCode::Tab);
    h
}

fn zone(h: &mut Harness, zone: &str) {
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::SessionValues(SessionValues { time_zone: zone.into(), charset: "utf8mb4".into() }));
}

fn my_page(h: &mut Harness, at: &[&str]) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = vec![col("id", "int", ValueKind::Integer), col("at", "timestamp", ValueKind::TimestampTz)];
    let rows = at.iter().enumerate().map(|(i, a)| vec![Some(i.to_string()), Some(a.to_string())]).collect();
    h.tab_db(index, DbEvent::Page { id, columns: Some(columns), rows, more: false, elapsed: Duration::from_millis(2) });
}

/// `shop.users` of `columns` open on a profile of `driver` (`mysql`: a MySQL one, its session's
/// zone `+00:00`), `rows` loaded, the grid focused, nothing sent since.
fn table_of(mysql: bool, columns: Vec<ColumnMeta>, rows: Vec<Vec<Option<String>>>) -> Harness {
    let mut h = match mysql {
        true => {
            let driver = FakeDriver::default();
            driver.mysql.store(true, std::sync::atomic::Ordering::SeqCst);
            let mut p = datarig_core::profile::ConnectionConfig::test_db();
            (p.name, p.driver) = ("local-my".into(), "mysql".into());
            let cfg = datarig_core::config::Config { connections: vec![p], ..Default::default() };
            let mut h = Harness::with_driver(&cfg, Lang::En, driver);
            h.db(DbEvent::Connected);
            h.db(DbEvent::Schemas(Ok(vec!["shop".into()])));
            open_users(&mut h);
            let index = h.app.tabs.active_index();
            h.tab_db(index, DbEvent::Language(Language::Sql(Dialect::MySql(MySqlMode::default()))));
            h.tab_db(index, DbEvent::Connected);
            zone(&mut h, "+00:00");
            h
        }
        false => {
            let mut h = Harness::connected(Lang::En);
            open_users(&mut h);
            h
        }
    };
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Page { id, columns: Some(columns), rows, more: false, elapsed: Duration::from_millis(2) });
    h.sent();
    h.key(KeyCode::Tab);
    h
}

/// In the open dialog on a column: a condition with the first operator it takes, `value` typed.
fn typed(h: &mut Harness, value: &str) {
    h.key(KeyCode::Right);
    h.key(KeyCode::Char(' '));
    h.type_text(value);
}

/// MySQL reads text compared with a number as a number without a word (`'abc'` as 0, `'12abc'`
/// as 12, `'true'` as 0): a value typed on a number column that is no number is not written,
/// and its row says why; a number is, in quotes too (as the number), and TRUE or FALSE on a
/// BOOLEAN (`tinyint(1)`) only.
#[test]
fn mysql_text_typed_on_a_number_column_is_not_written_and_says_why() {
    let s = |v: &str| Some(v.to_string());
    let cols = || vec![col("id", "int", ValueKind::Integer), col("flag", "tinyint(1)", ValueKind::Integer)];
    for (column, value, want) in [
        (0, "abc", None),
        (0, "12abc", None),
        (0, "'abc'", None),
        (0, "true", None),
        (0, "12", Some("id = 12")),
        (0, "'12'", Some("id = 12")),
        (1, "true", Some("flag = TRUE")),
        (1, "False", Some("flag = FALSE")),
        (1, "1", Some("flag = 1")),
        (1, "yes", None),
    ] {
        let mut h = table_of(true, cols(), vec![vec![s("1"), s("0")]]);
        dialog_on(&mut h, column);
        typed(&mut h, value);
        let screen = h.screen(120, 34);
        match want {
            Some(w) => assert_eq!(draft(&h), w, "{value}: {screen}"),
            None => {
                assert_eq!(draft(&h), "", "{value}");
                assert!(screen.contains("not a number"), "{value}: {screen}");
            }
        }
    }
}

/// MySQL compares a number column with a string as two doubles, which a list or a range of wide
/// values does not meet exactly: numbers typed in quotes in a list or a range of a `DECIMAL` are
/// written as the numbers, mixed with numbers typed bare.
#[test]
fn mysql_numbers_typed_in_quotes_in_a_list_or_a_range_are_written_as_numbers() {
    let s = |v: &str| Some(v.to_string());
    let cols = || vec![col("n", "decimal(30,0)", ValueKind::Decimal)];
    // The operator's place in the picker of a number column (see `each_operator_sends_its_term`).
    for (down, value, second, want) in [
        (10, "'9007199254740992', 9007199254740994", None, "n IN (9007199254740992, 9007199254740994)"),
        (11, "'9007199254740992', '9007199254740994'", None, "n NOT IN (9007199254740992, 9007199254740994)"),
        (1, "'9007199254740990'", Some("'9007199254740992'"), "n BETWEEN 9007199254740990 AND 9007199254740992"),
    ] {
        let mut h = table_of(true, cols(), vec![vec![s("9007199254740993")]]);
        dialog_on(&mut h, 0);
        h.key(KeyCode::Right);
        h.key(KeyCode::Char(' '));
        h.key(KeyCode::Left);
        h.key(KeyCode::Char(' '));
        for _ in 0..down {
            h.key(KeyCode::Down);
        }
        h.key(KeyCode::Enter);
        h.type_text(value);
        if let Some(second) = second {
            h.key(KeyCode::Right);
            h.type_text(second);
        }
        assert_eq!(draft(&h), want, "{}", h.screen(120, 34));
    }
}

/// A single-precision float: on PostgreSQL (`real`) a typed or ticked value is written as a
/// string the server reads as a `real` (a bare `0.1` would match nothing); on MySQL (`FLOAT`) no
/// value equals one exactly: `=`, `≠` and the lists are dimmed with why, and a value of the rows
/// is not ticked; an order comparison is written as typed.
#[test]
fn a_single_precision_float_is_compared_as_it_is_held() {
    let s = |v: &str| Some(v.to_string());
    let cols = |ty: &str| vec![col("id", "int8", ValueKind::Integer), col("r", ty, ValueKind::Float)];
    let rows = || vec![vec![s("1"), s("0.1")], vec![s("2"), s("1.5")]];
    let mut h = table_of(false, cols("float4"), rows());
    dialog_on(&mut h, 1);
    typed(&mut h, "0.1");
    assert_eq!(draft(&h), "r = '0.1'");
    h.key(KeyCode::Esc);
    dialog_on(&mut h, 1);
    to_values(&mut h, 0);
    h.key(KeyCode::Char(' '));
    assert_eq!(draft(&h), "r = '0.1'", "{}", h.screen(120, 34));
    h.key(KeyCode::Esc);
    let mut h = table_of(true, cols("float"), rows());
    dialog_on(&mut h, 1);
    typed(&mut h, "0.1");
    assert_eq!(draft(&h), "", "the first operator it takes is a range: {}", h.screen(120, 34));
    h.key(KeyCode::Esc);
    // The operator picker: `=` dimmed, why said.
    dialog_on(&mut h, 1);
    h.key(KeyCode::Right);
    h.key(KeyCode::Char(' '));
    h.key(KeyCode::Left);
    h.key(KeyCode::Char(' '));
    h.key(KeyCode::Char('g'));
    h.key(KeyCode::Char(' '));
    let screen = h.screen(120, 34);
    assert!(screen.contains("A MySQL FLOAT is held more precisely than it is shown"), "{screen}");
}

/// The dialog's sort line says where this server puts NULLs in the order chosen for the column:
/// PostgreSQL last going up and first going down, MySQL the other way round.
#[test]
fn the_sort_line_says_where_nulls_go_on_this_server() {
    let s = |v: &str| Some(v.to_string());
    let cols = || vec![col("id", "int8", ValueKind::Integer), col("note", "text", ValueKind::Text)];
    for (mysql, dir, said) in [
        (false, "ASC", "NULLs last"),
        (false, "DESC", "NULLs first"),
        (true, "ASC", "NULLs first"),
        (true, "DESC", "NULLs last"),
    ] {
        let mut h = table_of(mysql, cols(), vec![vec![s("1"), None]]);
        h.command(&format!("orderby note {dir}"));
        let id = h.app.tab().exec.query_id;
        let index = h.app.tabs.active_index();
        let rows = vec![vec![s("1"), None]];
        h.tab_db(index, DbEvent::Page { id, columns: Some(cols()), rows, more: false, elapsed: Duration::ZERO });
        h.sent();
        dialog_on(&mut h, 1);
        let screen = h.screen(120, 34);
        let sort = screen.lines().find(|l| l.contains("(\u{2022})")).unwrap_or_default().to_string();
        assert!(sort.contains(said), "{mysql} {dir}: {screen}");
    }
}

#[test]
fn a_mysql_timestamp_is_ticked_only_in_the_zone_it_was_read_in() {
    let mut h = mysql();
    h.command("where at = '2026-11-10 09:30:00'");
    runs(&mut h);
    my_page(&mut h, &["2026-11-10 09:30:00"]);
    dialog_on(&mut h, 1);
    // In the zone of both loads: the earlier load's value widens the condition.
    to_values(&mut h, 1);
    h.key(KeyCode::Down);
    h.key(KeyCode::Char(' '));
    assert_eq!(draft(&h), "at IN ('2026-11-10 09:30:00', '2026-11-11 10:00:00+00:00')");
    h.keys("u");
    h.key(KeyCode::Esc);
    // The session's zone changed since: neither the loaded nor the earlier value is taken.
    zone(&mut h, "+09:00");
    dialog_on(&mut h, 1);
    to_values(&mut h, 1);
    h.key(KeyCode::Down);
    h.key(KeyCode::Char(' '));
    assert_eq!(draft(&h), "at = '2026-11-10 09:30:00'");
    let screen = h.screen(120, 34);
    assert!(screen.contains("Not ticked: this column was read across a change"), "{screen}");
    h.key(KeyCode::Up);
    h.key(KeyCode::Char(' '));
    assert_eq!(draft(&h), "at = '2026-11-10 09:30:00'");
}

#[test]
fn ctrl_c_cancels_the_count_as_it_does_a_run() {
    let mut h = users();
    dialog_on(&mut h, 1);
    h.keys("#");
    let id = counts(&mut h)[0].0;
    let index = h.app.tabs.active_index();
    assert!(!h.session_cancelled(index));
    h.key_mod(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(h.session_cancelled(index));
    assert!(h.app.overlays.is_open(OverlayKind::FilterDialog), "the dialog stays");
    counted(&mut h, id, Err(DbError::Cancelled));
    assert!(h.screen(120, 34).contains("Values count cancelled"));
    // A column whose values compare by no equality: `# count exactly` refused with why.
    h.key(KeyCode::Esc);
    let mut cols = columns();
    cols[2] = meta("amount", "json", false, true);
    let id = h.app.tab().exec.query_id;
    h.tab_db(
        index,
        DbEvent::Page { id, columns: Some(cols), rows: rows(3), more: false, elapsed: Duration::from_millis(2) },
    );
    dialog_on(&mut h, 2);
    h.keys("#");
    assert!(counts(&mut h).is_empty());
    let screen = h.screen(120, 34);
    assert!(screen.contains("Values not counted: The column's type has no equality"), "{screen}");
}

/// A value the server sent written otherwise than the grid writes the column (not shown) is
/// listed as that, not as a long value, and is not ticked.
#[test]
fn a_value_the_server_writes_otherwise_is_listed_as_not_shown() {
    let mut h = users();
    dialog_on(&mut h, 1);
    h.keys("#");
    let sent = counts(&mut h);
    let other = GroupValue { value: None, whole: false, bytes: Some(4), rows: 2, prefix: false, inexact: true };
    counted(&mut h, sent[0].0, Ok(GroupCounts { values: vec![value(Some("paid"), 9), other], more: false }));
    let screen = h.screen(120, 34);
    assert!(screen.contains("value not shown (4 bytes)"), "{screen}");
    assert!(!screen.contains("long value"), "{screen}");
    to_values(&mut h, 0);
    h.keys("G");
    h.key(KeyCode::Char(' '));
    assert!(h.screen(120, 34).contains("Not ticked: the server wrote this value otherwise"), "{}", h.screen(120, 34));
    assert_eq!(draft(&h), "");
}

/// A table tab's result of 5000 rows that spilled to its file (1000 kept in memory), the
/// dialog open on it.
#[test]
fn the_list_of_a_spilled_result_counts_the_rows_in_memory_and_never_reads_the_file() {
    let state = std::env::temp_dir().join(format!("datarig-flows-filter-values-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    let mut cfg = test_db_config();
    cfg.result_window_rows = 1_000;
    let mut h = Harness::connected_with(&cfg, Lang::En);
    h.app.set_paths(datarig_core::paths::Paths { data: None, state: Some(state.clone()) });
    open_users(&mut h);
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let all = rows(5_000);
    let mut columns = Some(columns());
    for (p, page) in all.chunks(500).enumerate() {
        let more = p < 9;
        let rows = page.to_vec();
        h.tab_db(index, DbEvent::Page { id, columns: columns.take(), rows, more, elapsed: Duration::from_millis(1) });
    }
    let read = |h: &Harness| match &h.app.tab().results {
        datarig_tui::app::Results::Rows(rs) => {
            assert!(rs.rows.spilled() > 0 && rs.rows.resident().len() < rs.rows.len());
            rs.rows.rows_read_through()
        }
        _ => panic!("rows"),
    };
    let before = read(&h);
    h.key(KeyCode::Tab);
    h.keys("f");
    // The list of `id`, then of `status`, then of `amount`, then back, drawn at both sizes.
    for _ in 0..2 {
        for k in ["", "j", "j", "k", "k"] {
            h.keys(k);
            h.draw(80, 24);
            h.draw(120, 34);
        }
    }
    h.keys("j");
    let s = h.screen(120, 34);
    assert_eq!(read(&h), before, "no row was read from the spill file");
    assert!(s.contains("sample: 1,000/5,000"), "{s}");
    assert!(s.contains("paid"), "{s}");
    let _ = std::fs::remove_dir_all(&state);
}

/// MySQL `INVISIBLE` columns (a number and a text), which `SELECT *` leaves out: no values are
/// loaded for them, but the server counts them as their catalogue types say, and their counted
/// values tick into conditions that are sent, written as their catalogue types say.
#[test]
fn count_exactly_counts_a_mysql_invisible_column_by_its_catalogue_type() {
    let mut h = mysql_invisible();
    // The second counted under the condition the first ticked.
    let cases =
        [(2, "rank", "", "7", "`rank` = 7"), (3, "note", " WHERE (`rank` = 7)", "x", "`rank` = 7 AND note = 'x'")];
    for (down, name, other, counted_value, sent_where) in cases {
        for _ in 0..down {
            h.key(KeyCode::Char('j'));
        }
        let screen = h.screen(120, 34);
        assert!(screen.contains("not in SELECT *"), "{screen}");
        h.keys("#");
        let sent = counts(&mut h);
        let (preview, limit) = (datarig_core::query::GROUP_PREVIEW, datarig_core::query::GROUP_VALUES + 1);
        assert_eq!(
            sent.iter().map(|s| s.1.clone()).collect::<Vec<_>>(),
            vec![format!(
                "SELECT IF(LENGTH(`{name}`) <= {preview}, `{name}`, NULL), LENGTH(`{name}`), COUNT(*) \
                 FROM `shop`.`users`{other} GROUP BY `{name}` ORDER BY COUNT(*) DESC LIMIT {limit}"
            )],
            "{name}"
        );
        counted(&mut h, sent[0].0, Ok(GroupCounts { values: vec![value(Some(counted_value), 3)], more: false }));
        to_values(&mut h, 0);
        h.key(KeyCode::Char(' '));
        assert_eq!(draft(&h), sent_where, "{}", h.screen(120, 34));
        h.key(KeyCode::Enter);
        assert_eq!(runs(&mut h), vec![vec![format!("SELECT * FROM `shop`.`users` WHERE ({sent_where})")]]);
        my_page(&mut h, &["2026-11-10 09:30:00"]);
        h.keys("f");
        // Back to the first column for the next one.
        h.keys("gg");
    }
}

/// In the open dialog, from the column list: the value field of the column's first condition,
/// the cursor at its end.
fn to_first_field(h: &mut Harness) {
    h.key(KeyCode::Right);
    h.key(KeyCode::Right);
    h.key(KeyCode::End);
    assert_eq!(h.app.key_context(), Ctx::FilterDialogText);
}

/// Values with a line feed and a carriage return, ticked into a list: another value of the list
/// edited leaves them as they are (the field holds one line).
#[test]
fn a_ticked_value_with_a_line_break_stays_when_another_is_edited() {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    let status = [("a\nb", 3), ("c\rd", 2), ("x", 1)];
    let rows: Vec<Vec<Option<String>>> = status
        .iter()
        .flat_map(|(s, n)| std::iter::repeat_n(*s, *n))
        .enumerate()
        .map(|(i, s)| vec![Some(i.to_string()), Some(s.to_string()), Some("1".into())])
        .collect();
    answer(&mut h, rows);
    h.sent();
    h.key(KeyCode::Tab);
    dialog_on(&mut h, 1);
    to_values(&mut h, 0);
    for _ in 0..3 {
        h.key(KeyCode::Char(' '));
        h.key(KeyCode::Down);
    }
    let before = draft(&h);
    assert!(before.contains("IN (") && before.contains("'x'"), "{before}");
    h.key(KeyCode::BackTab);
    to_first_field(&mut h);
    h.type_text("y");
    assert_eq!(draft(&h), before.replace("'x'", "'xy'"));
}

/// The same read back from MySQL's SQL (a text with a line feed written as bytes).
#[test]
fn a_mysql_value_with_a_line_break_read_back_stays_when_another_is_edited() {
    let mut h = mysql();
    h.command("where at IN (_utf8mb4 X'610A62', 'x')");
    runs(&mut h);
    my_page(&mut h, &["2026-11-10 09:30:00"]);
    dialog_on(&mut h, 1);
    to_first_field(&mut h);
    h.type_text("y");
    h.key(KeyCode::Enter);
    assert_eq!(
        runs(&mut h),
        vec![vec!["SELECT * FROM `shop`.`users` WHERE (at IN (_utf8mb4 X'610A62', 'xy'))".to_string()]]
    );
}

/// `shop.users` with `n` rows loaded (pages of 500) whose `status` is one of `kinds` different
/// values, in a result window of `window` rows (spilled past it, under `state`).
fn loaded(n: usize, kinds: usize, window: usize, state: &std::path::Path) -> Harness {
    let mut cfg = test_db_config();
    cfg.result_window_rows = window;
    let mut h = Harness::connected_with(&cfg, Lang::En);
    h.app.set_paths(datarig_core::paths::Paths { data: None, state: Some(state.to_path_buf()) });
    open_users(&mut h);
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let all: Vec<Vec<Option<String>>> =
        (0..n).map(|i| vec![Some((i + 1).to_string()), Some(format!("s{:02}", i % kinds)), Some("1".into())]).collect();
    let mut columns = Some(columns());
    let pages = all.chunks(500).count();
    for (p, page) in all.chunks(500).enumerate() {
        let more = p + 1 < pages;
        let rows = page.to_vec();
        h.tab_db(index, DbEvent::Page { id, columns: columns.take(), rows, more, elapsed: Duration::from_millis(1) });
    }
    h.sent();
    h.key(KeyCode::Tab);
    h
}

/// A state directory of the test's own, removed when it ends.
struct State(std::path::PathBuf);

impl State {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("datarig-flows-filter-values-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        State(dir)
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A large window in memory: the list counts a sample of its rows, and says how many.
#[test]
fn the_list_of_a_large_window_in_memory_counts_a_sample_of_it() {
    let state = State::new("window");
    let mut h = loaded(30_000, 4, 50_000, &state.0);
    match &h.app.tab().results {
        datarig_tui::app::Results::Rows(rs) => assert_eq!(rs.rows.resident().len(), 30_000, "all in memory"),
        _ => panic!("rows"),
    }
    dialog_on(&mut h, 1);
    let s = h.screen(120, 34);
    assert!(s.contains("sample: 20,000/30,000"), "{s}");
    // 20,000 rows of four values in turn: 5,000 each.
    assert!(s.contains("~5,000"), "{s}");
}

/// The sample is said at 80 columns too, with the cue of a list longer than the pane.
#[test]
fn the_sample_is_said_at_80_columns_beside_the_lists_cue() {
    let state = State::new("narrow");
    let mut h = loaded(5_000, 30, 1_000, &state.0);
    dialog_on(&mut h, 1);
    let s = h.screen(80, 24);
    let heading = s.lines().find(|l| l.contains("Values")).unwrap_or_default().to_string();
    assert!(heading.contains("sample: 1,000/5,000"), "{s}");
    assert!(heading.contains(" of 30"), "{s}");
}

/// `shop.users` on MySQL with its catalogue read: `id`, `at`, and the INVISIBLE `rank`
/// (`int unsigned`) and `note` (`varchar(20)`); the dialog open on `id`.
fn mysql_invisible() -> Harness {
    use datarig_core::driver::keys::{CatalogColumn, OrderKeys};
    let mut h = mysql();
    dialog_on(&mut h, 0);
    let column = |name: &str, ty: &str, invisible: bool| CatalogColumn {
        name: name.into(),
        type_name: ty.into(),
        not_null: name == "id",
        primary_key: name == "id",
        foreign_key: false,
        invisible,
    };
    let keys = OrderKeys {
        primary_key: vec!["id".into()],
        columns: Some(vec![
            column("id", "int", false),
            column("at", "timestamp", false),
            column("rank", "int unsigned", true),
            column("note", "varchar(20)", true),
        ]),
        ..Default::default()
    };
    keys_came_with(&mut h, "local-my", keys);
    h
}

/// A condition typed on an INVISIBLE column is written as its catalogue type says (a number
/// bare, text quoted), and once applied it reads back as that column's condition, not as an
/// expression.
#[test]
fn a_condition_typed_on_an_invisible_column_is_its_columns() {
    let mut h = mysql_invisible();
    for (down, value, term) in [(2, "7", "`rank` = 7"), (3, "abc", "note = 'abc'")] {
        for _ in 0..down {
            h.key(KeyCode::Char('j'));
        }
        h.key(KeyCode::Right);
        h.key(KeyCode::Char(' '));
        h.type_text(value);
        assert_eq!(draft(&h), term);
        h.key(KeyCode::Enter);
        assert_eq!(runs(&mut h), vec![vec![format!("SELECT * FROM `shop`.`users` WHERE ({term})")]]);
        my_page(&mut h, &["2026-11-10 09:30:00"]);
        h.keys("f");
        let screen = h.screen(120, 34);
        assert!(!screen.contains("expr"), "a column's condition: {screen}");
        // On the column again, its condition is on the column's line, its value in the field.
        for _ in 0..down {
            h.key(KeyCode::Char('j'));
        }
        let screen = h.screen(120, 34);
        let row = screen.lines().find(|l| l.contains('\u{25be}')).unwrap_or_default();
        assert!(row.contains(value), "{screen}");
        // Taken out for the next one.
        h.key(KeyCode::Right);
        h.key(KeyCode::Char('x'));
        h.key(KeyCode::Enter);
        runs(&mut h);
        my_page(&mut h, &["2026-11-10 09:30:00"]);
        h.keys("f");
    }
}

#[test]
fn a_range_of_more_numbers_than_a_list_holds_says_no_more_than_the_range() {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    // 300 amounts, each its own: more than a list holds, and no list is shown.
    answer(&mut h, rows(300));
    h.sent();
    h.key(KeyCode::Tab);
    dialog_on(&mut h, 2);
    for (w, ht) in [(80, 24), (120, 34)] {
        let screen = h.screen(w, ht);
        assert!(screen.contains("mostly distinct"), "{screen}");
        assert!(!screen.contains("more values than shown"), "nothing is listed, so nothing more: {screen}");
    }
}

/// A value with a line feed ticked into `=`: its field shows it on one line in quotes, and what
/// is typed in it keeps the line feed.
#[test]
fn a_ticked_value_with_a_line_feed_stays_when_its_field_is_edited() {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    let rows = (0..3).map(|i| vec![Some(i.to_string()), Some("a\nb".to_string()), Some("1".into())]).collect();
    answer(&mut h, rows);
    h.sent();
    h.key(KeyCode::Tab);
    dialog_on(&mut h, 1);
    to_values(&mut h, 0);
    h.key(KeyCode::Char(' '));
    let ticked = draft(&h);
    assert_eq!(ticked, "status = E'a\\nb'");
    h.key(KeyCode::BackTab);
    to_first_field(&mut h);
    // Before the closing quote.
    h.key(KeyCode::Left);
    h.type_text("c");
    assert_eq!(draft(&h), "status = E'a\\nbc'");
}

/// A value with a carriage return read back from MySQL's SQL into `<`: edited in its field, it
/// keeps the carriage return.
#[test]
fn a_mysql_value_with_a_carriage_return_read_back_stays_when_its_field_is_edited() {
    let mut h = mysql();
    h.command("where at < _utf8mb4 X'630D64'");
    runs(&mut h);
    my_page(&mut h, &["2026-11-10 09:30:00"]);
    dialog_on(&mut h, 1);
    to_first_field(&mut h);
    h.key(KeyCode::Left);
    h.type_text("e");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec!["SELECT * FROM `shop`.`users` WHERE (at < _utf8mb4 X'630D6465')".to_string()]]);
}

/// The first bound of `between` with a line feed (read back from MySQL's SQL) stays as it is
/// when the second is edited.
#[test]
fn a_between_bound_with_a_line_feed_stays_when_the_other_is_edited() {
    let mut h = mysql();
    h.command("where at BETWEEN _utf8mb4 X'610A62' AND 'x'");
    runs(&mut h);
    my_page(&mut h, &["2026-11-10 09:30:00"]);
    dialog_on(&mut h, 1);
    to_first_field(&mut h);
    h.key(KeyCode::Right);
    h.key(KeyCode::End);
    h.type_text("y");
    h.key(KeyCode::Enter);
    assert_eq!(
        runs(&mut h),
        vec![vec!["SELECT * FROM `shop`.`users` WHERE (at BETWEEN _utf8mb4 X'610A62' AND 'xy')".to_string()]]
    );
}
