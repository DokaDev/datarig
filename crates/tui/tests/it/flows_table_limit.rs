//! A table tab's `LIMIT`: the filter dialog's `Limit` field caps the rows. It is not written
//! into the statement the grid runs (on MySQL a `LIMIT` would override the cap the driver sets
//! per page): the pages stop at the cap, the last one cut, and nothing is fetched past it (the
//! portal is closed); the title says so. A count and an export of every row read the capped
//! statement. Text that is not a number of rows is refused and nothing is sent; a new limit is
//! a new run, asked about first over pending edits.

use crate::common;

use common::*;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_i18n::Lang;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::{Paging, Results};
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::KeyCode;
use std::time::Duration;

const ALL: &str = r#"SELECT * FROM "shop"."users""#;

fn lang_tag(l: Lang) -> &'static str {
    if l == Lang::En { "en" } else { "ko" }
}

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

/// `n` rows of `shop.users` from `from` on.
fn rows(from: usize, n: usize) -> Vec<Vec<Option<String>>> {
    (from..from + n).map(|i| vec![Some(i.to_string()), Some(format!("n{i}"))]).collect()
}

/// Answer the active tab's current run with a first page of `n` rows (`more`: past it).
fn first_page(h: &mut Harness, n: usize, more: bool) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
    h.tab_db(index, DbEvent::Page { id, columns, rows: rows(0, n), more, elapsed: Duration::from_millis(2) });
}

/// A later page of `n` rows of the active tab's result, after `from` rows.
fn next_page(h: &mut Harness, from: usize, n: usize, more: bool) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Page { id, columns: None, rows: rows(from, n), more, elapsed: Duration::from_millis(2) });
}

/// `shop.users` open and answered (three rows), the grid focused, nothing sent since.
fn users(lang: Lang) -> Harness {
    let mut h = Harness::connected(lang);
    open_users(&mut h);
    first_page(&mut h, 3, false);
    h.sent();
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h
}

/// Type `limit` into the line's `LIMIT` field (what it held cleared) and apply.
/// The filter dialog (opened unless it is open), its `Limit` field set to `limit`, applied.
fn set_limit(h: &mut Harness, limit: &str) {
    if !h.app.overlays.is_open(OverlayKind::FilterDialog) {
        h.keys("f");
    }
    for _ in 0..6 {
        if h.app.key_context() == Ctx::FilterDialogText {
            break;
        }
        h.key(KeyCode::Tab);
    }
    assert_eq!(h.app.key_context(), Ctx::FilterDialogText);
    h.ctrl('u');
    h.type_text(limit);
    h.key(KeyCode::Enter);
}

/// What the open filter dialog says in its hint line (and anywhere on it).
fn dialog(h: &mut Harness) -> String {
    assert!(h.app.overlays.is_open(OverlayKind::FilterDialog), "the dialog is open");
    h.screen(120, 34)
}

fn status(h: &Harness) -> String {
    h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

/// The rows the result holds, and whether it says the server has more.
fn held(h: &Harness) -> (usize, bool) {
    match &h.app.tab().results {
        Results::Rows(rs) => (rs.rows.len(), rs.more),
        _ => (0, false),
    }
}

fn closed(sent: &[DbCommand]) -> bool {
    sent.iter().any(|c| matches!(c, DbCommand::ClosePortal { .. }))
}

fn fetches(sent: &[DbCommand]) -> usize {
    sent.iter().filter(|c| matches!(c, DbCommand::FetchMore { .. } | DbCommand::Resume { .. })).count()
}

/// `shop.users` with limit `limit` applied (the statement sent is the table's own), its run
/// answered by nothing yet.
fn limited(limit: u64) -> Harness {
    let mut h = users(Lang::En);
    set_limit(&mut h, &limit.to_string());
    assert_eq!(runs(&mut h), vec![vec![ALL.to_string()]], "no LIMIT in the grid's statement");
    h
}

#[test]
fn the_limit_applies_with_the_other_fields_and_is_not_sent_in_the_grids_statement() {
    let mut h = users(Lang::En);
    h.command("where id > 1");
    runs(&mut h);
    first_page(&mut h, 3, false);
    h.command("orderby name");
    runs(&mut h);
    first_page(&mut h, 3, false);
    set_limit(&mut h, " 0100 ");
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} WHERE (id > 1) ORDER BY "name" ASC"#)]]);
    assert_eq!(h.app.tab().doc.table_query.as_ref().unwrap().applied().limit, Some(100));
    assert_eq!(h.app.key_context(), Ctx::Grid);
    // Blank: no limit.
    first_page(&mut h, 3, false);
    set_limit(&mut h, "  ");
    assert_eq!(runs(&mut h).len(), 1);
    assert_eq!(h.app.tab().doc.table_query.as_ref().unwrap().applied().limit, None);
}

#[test]
fn a_limit_that_is_not_a_number_of_rows_is_refused_and_nothing_is_sent() {
    let mut h = users(Lang::En);
    for (text, why) in [
        ("ten", "a whole number of rows"),
        ("-5", "a whole number of rows"),
        ("1e3", "a whole number of rows"),
        ("0", "0 rows reads nothing"),
        ("9223372036854775808", "at most 9,223,372,036,854,775,807 rows"),
        ("99999999999999999999999", "at most"),
    ] {
        set_limit(&mut h, text);
        assert!(runs(&mut h).is_empty(), "{text}");
        let said = dialog(&mut h);
        assert!(said.contains("Limit refused: ") && said.contains(why), "{text}: {said}");
        assert_eq!(h.app.key_context(), Ctx::FilterDialogText, "the field keeps the keys");
        h.key(KeyCode::Esc);
    }
    // Esc dropped the draft: no limit applied, none to run.
    let tq = h.app.tab().doc.table_query.as_ref().unwrap();
    assert_eq!((tq.applied().limit, tq.draft.limit), (None, None));
    // The largest a server reads is let through.
    set_limit(&mut h, "9223372036854775807");
    assert_eq!(runs(&mut h).len(), 1);
}

#[test]
fn a_first_page_stops_at_a_limit_below_a_page() {
    let mut h = limited(2);
    let size = h.app.page_size;
    first_page(&mut h, size, true);
    assert_eq!(held(&h), (2, false));
    assert_eq!(h.app.tab().exec.paging, Paging::None);
    let sent = h.sent();
    assert!(closed(&sent), "what the server holds past the cap is closed: {sent:?}");
    assert!(h.screen(120, 30).contains("limit 2"), "{}", h.screen(120, 30));
    // Nothing is fetched past the cap.
    h.keys("n");
    assert_eq!(fetches(&h.sent()), 0);
}

#[test]
fn a_first_page_of_exactly_the_limit_fetches_nothing_more() {
    let size = Harness::connected(Lang::En).app.page_size;
    let mut h = limited(size as u64);
    first_page(&mut h, size, true);
    assert_eq!(held(&h), (size, false));
    assert!(closed(&h.sent()));
    h.keys("n");
    assert_eq!(fetches(&h.sent()), 0);
    // The server had no more: nothing to close.
    let mut h = limited(size as u64);
    first_page(&mut h, size, false);
    assert_eq!(held(&h), (size, false));
    assert!(!closed(&h.sent()));
}

#[test]
fn later_pages_stop_at_a_limit_past_the_first() {
    let size = Harness::connected(Lang::En).app.page_size;
    let mut h = limited(size as u64 * 2 + 3);
    first_page(&mut h, size, true);
    assert_eq!(held(&h), (size, true));
    assert!(matches!(h.app.tab().exec.paging, Paging::Open { .. }));
    assert!(!closed(&h.sent()));
    assert!(!h.screen(120, 30).contains("limit "), "not reached yet");
    h.keys("n");
    assert_eq!(fetches(&h.sent()), 1);
    next_page(&mut h, size, size, true);
    assert_eq!(held(&h), (size * 2, true));
    h.keys("n");
    assert_eq!(fetches(&h.sent()), 1);
    // The third page is cut at the cap, and the portal closed.
    next_page(&mut h, size * 2, size, true);
    assert_eq!(held(&h), (size * 2 + 3, false));
    let sent = h.sent();
    assert!(closed(&sent), "{sent:?}");
    assert_eq!(h.app.tab().exec.paging, Paging::None);
    let said = format!("limit {}", datarig_i18n::fmt_count(size as u64 * 2 + 3));
    assert!(h.screen(120, 30).contains(&said), "{}", h.screen(120, 30));
    h.keys("nn");
    assert_eq!(fetches(&h.sent()), 0, "nothing past the cap");
    // A count asks nothing: every row up to the cap is there.
    h.keys("#");
    assert!(!h.sent().iter().any(|c| matches!(c, DbCommand::Count { .. })));
}

#[test]
fn a_page_read_again_past_a_closed_portal_stops_at_the_limit() {
    let size = Harness::connected(Lang::En).app.page_size;
    let mut h = limited(size as u64 + 2);
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Released { id });
    first_page(&mut h, size, true);
    assert_eq!(h.app.tab().exec.paging, Paging::Released);
    assert!(!closed(&h.sent()), "nothing is held");
    h.keys("n");
    let sent = h.sent();
    let resumed: Vec<&String> = sent
        .iter()
        .filter_map(|c| match c {
            DbCommand::Resume { sql, .. } => Some(sql),
            _ => None,
        })
        .collect();
    assert_eq!(resumed, vec![ALL], "run again as it ran, without a LIMIT");
    // Its page: cut at the cap, its portal closed, nothing read past it.
    let id = h.app.tab().exec.query_id;
    let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
    h.tab_db(
        index,
        DbEvent::Page { id, columns, rows: rows(size, size), more: true, elapsed: Duration::from_millis(2) },
    );
    assert_eq!(held(&h), (size + 2, false));
    assert!(closed(&h.sent()));
    h.keys("n");
    assert_eq!(fetches(&h.sent()), 0);
}

#[test]
fn a_count_and_an_export_of_every_row_read_the_capped_statement() {
    let size = Harness::connected(Lang::En).app.page_size;
    let mut h = users(Lang::En);
    h.command("where id > 0");
    runs(&mut h);
    first_page(&mut h, 3, false);
    set_limit(&mut h, "700");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 0)")]]);
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Released { id });
    first_page(&mut h, size, true);
    h.sent();
    exact_count(&mut h);
    let counts: Vec<String> = h
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Count { sql, .. } => Some(sql),
            _ => None,
        })
        .collect();
    assert!(counts.len() == 1 && counts[0].contains(&format!("{ALL} WHERE (id > 0) LIMIT 700")), "{counts:?}");
    let qid = h.app.tab().exec.query_id;
    h.tab_db(index, DbEvent::Counted { id: qid, result: Ok(700), snapshot: false });
    // Every row: the statement read once, capped; never the table by its key.
    let dir = std::env::temp_dir().join(format!("datarig-table-limit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    h.command(&format!("export {}", dir.join("all.csv").display()));
    if h.app.overlays.is_open(datarig_tui::app::overlay::OverlayKind::Confirm) {
        h.key(KeyCode::Char('y'));
    }
    let start = std::time::Instant::now();
    let mut sent = Vec::new();
    while h.app.exporting(h.app.tab().id) && start.elapsed() < Duration::from_secs(10) {
        h.app.pump_exports(Duration::from_millis(50));
        sent.extend(h.sent());
        if sent.iter().any(|c| matches!(c, DbCommand::Export { .. })) {
            break;
        }
    }
    let exported: Vec<&String> = sent
        .iter()
        .filter_map(|c| match c {
            DbCommand::Export { sql, .. } => Some(sql),
            _ => None,
        })
        .collect();
    assert!(!sent.iter().any(|c| matches!(c, DbCommand::LoadStructure { .. })), "{sent:?}");
    assert_eq!(exported, vec![&format!("{ALL} WHERE (id > 0) LIMIT 700")], "{sent:?}");
    // Without a filter the capped statement is read once too, not by the table's key.
    let mut h = users(Lang::En);
    set_limit(&mut h, "700");
    h.sent();
    let (id, index) = (h.app.tab().exec.query_id, h.app.tabs.active_index());
    h.tab_db(index, DbEvent::Released { id });
    first_page(&mut h, size, true);
    h.sent();
    h.command(&format!("export {}", dir.join("plain.csv").display()));
    let sent = h.sent();
    assert!(!sent.iter().any(|c| matches!(c, DbCommand::LoadStructure { .. })), "{sent:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_new_limit_over_pending_edits_asks_first_and_enter_keeps_them() {
    let mut h = users(Lang::En);
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        assert!(rs.edit.batch.delete(&[0], |_| Vec::new()));
    }
    set_limit(&mut h, "5");
    assert_eq!(
        h.app.overlays.confirm().map(|c| c.action),
        Some(ConfirmAction::LeaveEdits(datarig_tui::app::edit::Leave::Run(h.app.tab().id)))
    );
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty(), "Enter keeps the edits: nothing runs");
    assert!(matches!(&h.app.tab().results, Results::Rows(rs) if !rs.edit.batch.is_empty()));
    // `y` drops them: a new run, never a page read again.
    set_limit(&mut h, "5");
    h.keys("y");
    let sent = h.sent();
    assert!(
        sent.iter().any(|c| matches!(c, DbCommand::Execute { statements, .. } if statements == &vec![ALL.to_string()]))
    );
    assert_eq!(fetches(&sent), 0);
}

#[test]
fn the_limit_screens() {
    for lang in [Lang::En, Lang::Ko] {
        let tag = lang_tag(lang);
        // Applied with a condition and an order: the title says the limit.
        let mut h = users(lang);
        keys_came(&mut h, "local-pg", &["id"]);
        h.keys("i");
        h.command("where id > 1");
        first_page(&mut h, 3, false);
        h.command("orderby name desc");
        first_page(&mut h, 3, false);
        set_limit(&mut h, "100");
        first_page(&mut h, 3, false);
        assert_screen!(format!("table_limit_applied_{tag}_80x24"), lang, h.draw(80, 24));
        assert_screen!(format!("table_limit_applied_{tag}_120x30"), lang, h.draw(120, 30));
        // Refused: the field underlined, the reason in the dialog's hint line.
        set_limit(&mut h, "lots");
        assert_screen!(format!("table_limit_refused_{tag}_80x24"), lang, h.draw(80, 24));
        assert_screen!(format!("table_limit_refused_{tag}_120x34"), lang, h.draw(120, 34));
        // The rows stopped at the cap: the title says it.
        let mut h = users(lang);
        set_limit(&mut h, "2");
        let size = h.app.page_size;
        first_page(&mut h, size, true);
        assert_screen!(format!("table_limit_reached_{tag}_80x24"), lang, h.draw(80, 24));
        assert_screen!(format!("table_limit_reached_{tag}_120x30"), lang, h.draw(120, 30));
    }
}

/// The cap goes to the driver with the statement, so it reads no row past it: the run carries
/// the limit, a page read again what the limit leaves past the rows held; a run without a
/// limit carries none.
#[test]
fn the_runs_carry_what_the_cap_leaves_to_the_driver() {
    let mut h = users(Lang::En);
    set_limit(&mut h, "3");
    let caps: Vec<Option<u64>> = h
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { max_rows, .. } => Some(max_rows),
            _ => None,
        })
        .collect();
    assert_eq!(caps, vec![Some(3)]);
    // Read again past a closed portal: what is left of the cap.
    let size = Harness::connected(Lang::En).app.page_size;
    let mut h = limited(size as u64 + 2);
    let (id, index) = (h.app.tab().exec.query_id, h.app.tabs.active_index());
    h.tab_db(index, DbEvent::Released { id });
    first_page(&mut h, size, true);
    h.keys("n");
    let resumed: Vec<(u64, Option<u64>)> = h
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Resume { skip, max_rows, .. } => Some((skip, max_rows)),
            _ => None,
        })
        .collect();
    assert_eq!(resumed, vec![(size as u64, Some(2))]);
    // No limit: no cap.
    let mut h = users(Lang::En);
    h.ctrl('e');
    let caps: Vec<Option<u64>> = h
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { max_rows, .. } => Some(max_rows),
            _ => None,
        })
        .collect();
    assert_eq!(caps, vec![None]);
}

/// A count and an export of every row read the statement the rows were read with, only capped:
/// keys read since (a tie-breaker that would change which rows tie at the cap) do not change
/// its order.
#[test]
fn the_capped_statement_keeps_the_order_the_rows_were_read_in() {
    let size = Harness::connected(Lang::En).app.page_size;
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    let keys = h.sent().iter().find_map(|c| match c {
        DbCommand::LoadTableKeys { id, .. } => Some(*id),
        _ => None,
    });
    let keys = keys.expect("the keys are asked for");
    first_page(&mut h, 3, false);
    h.key(KeyCode::Tab);
    h.command("orderby name");
    runs(&mut h);
    first_page(&mut h, 3, false);
    set_limit(&mut h, "700");
    let ordered = format!(r#"{ALL} ORDER BY "name" ASC"#);
    assert_eq!(runs(&mut h), vec![vec![ordered.clone()]], "no keys yet: no tie-breaker");
    let (id, index) = (h.app.tab().exec.query_id, h.app.tabs.active_index());
    h.tab_db(index, DbEvent::Released { id });
    first_page(&mut h, size, true);
    let pk = datarig_core::driver::keys::OrderKeys { primary_key: vec!["id".into()], ..Default::default() };
    h.meta_db("local-pg", DbEvent::TableKeys { id: keys, result: Ok(pk) });
    h.app.focus = datarig_tui::app::Focus::Results;
    h.sent();
    h.keys("#");
    let counts: Vec<String> = h
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Count { sql, .. } => Some(sql),
            _ => None,
        })
        .collect();
    assert!(counts.len() == 1 && counts[0].contains(&format!("{ordered} LIMIT 700")), "{counts:?}");
    assert!(!counts[0].contains(r#""id" ASC"#), "{counts:?}");
}

/// Under a cap, what is said of the pages counts to the cap: a page read again says the rows
/// it may bring up to the cap, and the next page at the cap says the limit ended the rows.
#[test]
fn the_page_notes_count_to_the_cap() {
    let size = Harness::connected(Lang::En).app.page_size;
    let mut h = limited(size as u64 + 150);
    let (id, index) = (h.app.tab().exec.query_id, h.app.tabs.active_index());
    h.tab_db(index, DbEvent::Released { id });
    first_page(&mut h, size, true);
    h.keys("n");
    let to = datarig_i18n::fmt_count(size as u64 + 150);
    let said = status(&h);
    assert!(said.contains(&format!("–{to} (nothing is held open")), "{said}");
    let id = h.app.tab().exec.query_id;
    let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
    h.tab_db(
        index,
        DbEvent::Page { id, columns, rows: rows(size, 150), more: false, elapsed: Duration::from_millis(2) },
    );
    assert_eq!(held(&h), (size + 150, false));
    h.keys("nn");
    let said = h.status(200, 45);
    assert!(said.contains(&format!("Last page: the limit of {to} rows is reached")), "{said}");
}

/// `Space r v` opens the statement the rows were read with, capped, so running it reads the
/// rows the tab shows.
#[test]
fn the_statement_opened_in_a_console_has_the_cap() {
    let mut h = limited(700);
    first_page(&mut h, 3, false);
    h.app.focus = datarig_tui::app::Focus::Results;
    h.keys(" rv");
    let text = h.app.tab().editor.text();
    assert_eq!(text.trim(), format!("{ALL} LIMIT 700"), "{text}");
}

/// `#` on filtered rows, then the exact count of the choice it opens.
fn exact_count(h: &mut Harness) {
    h.keys("#");
    assert!(h.app.overlays.is_open(datarig_tui::app::overlay::OverlayKind::ContextMenu), "the choice opens");
    h.keys("exact");
    h.key(KeyCode::Enter);
}
