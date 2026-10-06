//! Paging through the real `App` event path with a fake clock (policy
//! `paging_idle_timeout`, explicit pages). The grid shows one page at a time:
//! `n`/`p` (and the title's `‹`/`›`) move between pages, pages already fetched come from the
//! result's rows without a command, the page past them is fetched while the portal is open.
//! The results title says where the pages are on its right, in parentheses. A portal outside
//! the user's transaction closes after the policy's timeout without a fetch; one inside it never
//! does. Past a closed portal the next page runs the statement again only when it is a plain
//! `SELECT` (announced), anything else is refused. Counting runs only when the user asks. No
//! real sleeps.

mod common;

use common::*;
use datarig_core::driver::PagingMode;
use datarig_core::driver::{DbCommand, DbError, DbEvent, Outcome};
use datarig_core::i18n::Lang;
use datarig_core::policy::Policy;
use datarig_tui::app::{Paging, Results};
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::time::Duration;

const SEC: Duration = Duration::from_secs(1);

fn page(from: usize, n: usize) -> Vec<Vec<Option<String>>> {
    (from..from + n).map(|i| vec![Some(i.to_string())]).collect()
}

/// Run the statement under the cursor in the active tab and answer with a first page of `n`
/// rows (`more`: the portal stays open). Returns the statement id.
fn first_page(h: &mut Harness, n: usize, more: bool) -> u64 {
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::TxOpen(true));
    let columns = Some(vec![meta("id", "int8", true, false)]);
    h.tab_db(index, DbEvent::Page { id, columns, rows: page(0, n), more, elapsed: Duration::from_millis(2) });
    id
}

/// The results panel's top border as drawn at `w`×`hh` (its title and paging state).
fn results_title(h: &mut Harness, w: u16, hh: u16) -> String {
    let t = h.draw(w, hh);
    row_text(t.backend().buffer(), h.app.layout.results.y)
}

fn close_requests(cmds: &[DbCommand]) -> Vec<u64> {
    cmds.iter()
        .filter_map(|c| match c {
            DbCommand::ClosePortal { id } => Some(*id),
            _ => None,
        })
        .collect()
}

fn fetches(cmds: &[DbCommand]) -> usize {
    cmds.iter().filter(|c| matches!(c, DbCommand::FetchMore { .. })).count()
}

fn resumes(cmds: &[DbCommand]) -> Vec<(u64, String, u64)> {
    cmds.iter()
        .filter_map(|c| match c {
            DbCommand::Resume { id, sql, skip, paging: PagingMode::NoHold } => Some((*id, sql.clone(), *skip)),
            _ => None,
        })
        .collect()
}

fn counts(cmds: &[DbCommand]) -> Vec<(u64, String)> {
    cmds.iter()
        .filter_map(|c| match c {
            DbCommand::Count { id, sql } => Some((*id, sql.clone())),
            _ => None,
        })
        .collect()
}

/// The first cell of the first row the grid shows.
fn first_shown(h: &Harness) -> Option<String> {
    let t = h.app.tab();
    let Results::Rows(rs) = &t.results else { return None };
    let start = t.grid.window(rs.rows.len()).start;
    match rs.cell(start, 0) {
        datarig_tui::widgets::grid::CellRef::Here(c) => c.clone(),
        _ => None,
    }
}

#[test]
fn an_idle_portal_closes_and_the_next_page_runs_a_select_again() {
    let mut h = Harness::connected(Lang::En);
    // The whole width for the grid and its footer.
    h.app.detail.visible = false;
    let id = first_page(&mut h, 500, true);
    h.sent();
    assert!(h.app.needs_tick(), "the countdown needs the tick");
    let title = results_title(&mut h, 160, 45);
    assert!(title.contains("Results · rows 1–500") && title.contains("‹ (page 1 / ? · 30s) › ╮"), "{title}");
    assert!(!h.status(160, 45).contains("paging"), "no countdown in the status bar");
    h.advance(Duration::from_millis(6_500));
    assert!(results_title(&mut h, 160, 45).contains("(page 1 / ? · 24s)"));

    // Scrolling to the end of the page fetches nothing; it says which key shows the next.
    h.key(KeyCode::Tab); // results
    h.keys("G");
    assert_eq!(fetches(&h.sent()), 0, "no infinite scroll");
    assert_eq!(h.app.tab().grid.row, 499);
    h.keys("j");
    assert!(h.status(160, 45).contains("End of the page: n shows the next one"));
    // `n` fetches the next page; it shows once it arrives.
    h.keys("n");
    assert_eq!(fetches(&h.sent()), 1);
    h.advance(40 * SEC);
    assert!(close_requests(&h.sent()).is_empty(), "nothing closes while a fetch runs");
    h.tab_db(0, DbEvent::Page { id, columns: None, rows: page(500, 500), more: true, elapsed: SEC / 100 });
    assert_eq!(first_shown(&h).as_deref(), Some("500"), "the new page is shown");
    h.advance(29 * SEC);
    assert!(close_requests(&h.sent()).is_empty(), "the new page restarted the count");
    let title = results_title(&mut h, 160, 45);
    assert!(title.contains("Results · rows 501–1,000") && title.contains("(page 2 / ? · 1s)"), "{title}");

    // 30s without a fetch: the app asks the driver to close the portal, once.
    h.advance(SEC);
    assert_eq!(close_requests(&h.sent()), [id]);
    assert_eq!(h.app.tab().exec.paging, Paging::ClosedIdle);
    h.advance(60 * SEC);
    assert!(h.sent().is_empty(), "asked once");
    let screen = h.screen(160, 45);
    assert!(
        screen.contains("Showing 1,000 rows · paging closed — n runs the SELECT again for the next page"),
        "{screen}"
    );
    assert!(results_title(&mut h, 160, 45).contains("(page 2 / ? · paging closed) › ╮"));
    // Unknown is not absent: the server may have more rows.
    assert!(matches!(&h.app.tab().results, Results::Rows(rs) if rs.more));
    // Autocommit: the driver ended the implicit transaction.
    h.tab_db(0, DbEvent::TxOpen(false));
    assert!(!h.status(160, 45).contains("TX open"));
    // (The page-end hint flashed above, on the real clock.)
    h.app.transient = None;
    assert!(!h.app.needs_tick());

    // Pages already fetched come back without a command.
    h.keys("p");
    assert_eq!(first_shown(&h).as_deref(), Some("0"));
    h.keys("p");
    assert!(h.status(160, 45).contains("First page"));
    h.keys("n");
    assert_eq!(first_shown(&h).as_deref(), Some("500"));
    assert!(h.sent().is_empty(), "served from the fetched rows");

    // Past them: the SELECT runs again, skipping the rows the result has, and it says so.
    h.keys("n");
    let sent = h.sent();
    let [(rid, sql, skip)] = &resumes(&sent)[..] else { panic!("{sent:?}") };
    assert_eq!((sql.as_str(), *skip), ("SELECT * FROM shop.users WHERE id <= 8", 1000));
    assert_ne!(*rid, id);
    // Said in the tab's status (the status bar shows the fetch while it runs).
    let status = h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default();
    assert!(
        status.contains("Paging had closed: ran the SELECT again to fetch rows 1,001–1,500; without ORDER BY"),
        "{status}"
    );
    assert!(
        h.app
            .tab()
            .exec
            .run
            .notes
            .iter()
            .any(|n| matches!(n.msg, datarig_core::i18n::Msg::ResultsPageResumedUnordered { .. }))
    );
    let columns = Some(vec![meta("id", "int8", true, false)]);
    h.tab_db(0, DbEvent::Page { id: *rid, columns, rows: page(1000, 120), more: false, elapsed: SEC / 100 });
    assert_eq!(first_shown(&h).as_deref(), Some("1000"));
    assert!(matches!(&h.app.tab().results, Results::Rows(rs) if rs.rows.len() == 1120 && !rs.more));
    // The announcement stays in the status bar with the rows it brought.
    h.app.transient = None;
    assert!(h.status(220, 45).contains("Paging had closed: ran the SELECT again"), "{}", h.status(220, 45));
    let title = results_title(&mut h, 160, 45);
    assert!(title.contains("Results · rows 1,001–1,120") && title.contains("(page 3 / 3) ›"), "{title}");
    h.keys("n");
    assert!(h.status(160, 45).contains("Last page"));
    assert!(h.sent().is_empty());

    // Running again gives a fresh result with its own countdown.
    h.key(KeyCode::Tab); // explorer
    h.key(KeyCode::Tab); // editor
    let id2 = first_page(&mut h, 500, true);
    assert_ne!(id2, id);
    assert!(matches!(h.app.tab().exec.paging, Paging::Open { .. }));
    assert!(!h.screen(160, 45).contains("paging closed"));
    assert!(results_title(&mut h, 160, 45).contains("(page 1 / ? · 30s)"));
}

/// A portal read inside the user's own transaction is never closed for being
/// idle (the owner reads uncommitted changes there); the transaction indicator stays and
/// quitting still asks.
#[test]
fn inside_the_users_transaction_the_portal_stays_open() {
    let mut h = Harness::connected(Lang::En);
    h.tab_db(0, DbEvent::Block(true));
    first_page(&mut h, 500, true);
    h.sent();
    assert!(matches!(h.app.tab().exec.paging, Paging::Open { in_block: true, .. }));
    assert!(results_title(&mut h, 160, 45).contains("(page 1 / ? · in tx) ›"));
    assert!(!h.app.needs_tick(), "no countdown, no timer");
    assert_eq!(h.app.next_tick(h.clock.now()), None);
    h.advance(3600 * SEC);
    assert!(close_requests(&h.sent()).is_empty(), "never closed for being idle");
    // It still pages.
    h.key(KeyCode::Tab);
    h.keys("n");
    assert_eq!(fetches(&h.sent()), 1);
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::Page { id, columns: None, rows: page(500, 500), more: true, elapsed: SEC });
    assert!(matches!(h.app.tab().exec.paging, Paging::Open { in_block: true, .. }), "still in the transaction");
    assert!(h.app.tab().exec.tx_open);
    assert!(h.status(160, 45).contains("TX open"));
    h.ctrl('q');
    assert!(h.screen(160, 45).contains("A transaction is open"), "quitting still asks");
}

#[test]
fn a_result_that_fits_in_one_page_never_counts_down() {
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 8, false);
    h.tab_db(0, DbEvent::TxOpen(false));
    h.sent();
    assert_eq!(h.app.tab().exec.paging, Paging::None);
    let title = results_title(&mut h, 160, 45);
    assert!(title.contains("Results · 8 rows") && !title.contains("page") && !title.contains('‹'), "{title}");
    h.advance(3600 * SEC);
    assert!(close_requests(&h.sent()).is_empty());
}

#[test]
fn background_tabs_close_too_and_off_disables_it() {
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 500, true);
    h.ctrl('t');
    h.sent();
    h.advance(30 * SEC);
    assert_eq!(close_requests(&h.sent()).len(), 1, "the first tab's portal, while another tab is active");

    // A policy with `paging_idle_timeout = "off"`.
    let mut cfg = test_db_config();
    cfg.connections[0].policy = Some("local".into());
    cfg.policies.insert("local", Policy { paging_idle_timeout: None, ..Policy::default() });
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    first_page(&mut h, 500, true);
    h.sent();
    let title = results_title(&mut h, 160, 45);
    assert!(title.contains("(page 1 / ? · paging) › ╮") && !title.contains("s)"), "open, no countdown: {title}");
    h.advance(3600 * SEC);
    assert!(close_requests(&h.sent()).is_empty());
    assert!(matches!(h.app.tab().exec.paging, Paging::Open { .. }));
}

#[test]
fn a_short_policy_timeout_and_korean_title() {
    let mut cfg = test_db_config();
    cfg.connections[0].policy = Some("careful".into());
    cfg.policies.insert("careful", Policy { paging_idle_timeout: Some(10 * SEC), ..Policy::default() });
    let mut h = Harness::with_config(&cfg, Lang::Ko);
    h.db(DbEvent::Connected);
    first_page(&mut h, 500, true);
    h.sent();
    let title = results_title(&mut h, 160, 45);
    let ko = datarig_core::i18n::I18n::new(Lang::Ko);
    use datarig_core::i18n::Msg;
    let rows = ko.msg(&Msg::PaneResultsRange { from: "1".into(), to: "500".into() }).to_string();
    let page = ko.msg(&Msg::ResultsTitlePage { page: "1".into(), pages: "?".into() }).to_string();
    assert!(title.contains(&rows) && title.contains(&format!("({page} · 10s)")), "{title}");
    h.advance(10 * SEC);
    assert_eq!(close_requests(&h.sent()).len(), 1);
    let screen = h.screen(160, 45);
    let footer = ko.msg(&Msg::ResultsPagingClosedRerun { count: 500, key: "n".into() }).to_string();
    assert!(screen.contains(&footer), "{screen}");
}

/// At 80 columns the title keeps the paging state on its right (the short form); the line
/// stays whole (no wide character is split).
#[test]
fn a_narrow_title_keeps_the_paging_state() {
    for (lang, state) in [(Lang::En, "‹ (1/? · 30s) › ╮"), (Lang::Ko, "‹ (1/? · 30s) › ╮")] {
        let mut h = Harness::connected(lang);
        first_page(&mut h, 500, true);
        let title = results_title(&mut h, 80, 24);
        assert!(title.contains(state), "{title}");
        assert_eq!(datarig_tui::text::width(&title), 80, "{title}");
    }
}

/// The title's `‹` and `›` are the previous and next page for the mouse.
#[test]
fn the_title_arrows_page_with_the_mouse() {
    let mut h = Harness::connected(Lang::En);
    let id = first_page(&mut h, 500, true);
    h.sent();
    h.draw(160, 45);
    let next = h.app.layout.page_next;
    assert_eq!(next.width, 1);
    h.mouse(MouseEventKind::Down(MouseButton::Left), next.x, next.y);
    assert_eq!(fetches(&h.sent()), 1);
    h.tab_db(0, DbEvent::Page { id, columns: None, rows: page(500, 500), more: true, elapsed: SEC / 100 });
    assert_eq!(first_shown(&h).as_deref(), Some("500"));
    h.draw(160, 45);
    let prev = h.app.layout.page_prev;
    h.mouse(MouseEventKind::Down(MouseButton::Left), prev.x, prev.y);
    assert_eq!(first_shown(&h).as_deref(), Some("0"));
    assert!(h.sent().is_empty(), "the first page came from the fetched rows");
    // Icons on: Nerd Font carets.
    h.app.icons = datarig_core::config::IconsSetting::On;
    let title = results_title(&mut h, 160, 45);
    assert!(title.contains("\u{f0d9} (page 1 / ? · ") && title.contains(") \u{f0da}"), "{title}");
}

/// Past a closed portal nothing but a plain `SELECT` is run again: anything else is refused
/// with the reason and nothing is sent.
#[test]
fn a_statement_off_the_allowlist_is_never_run_again() {
    for (sql, why) in [
        ("SELECT nextval('s') FROM generate_series(1, 2000)", "it calls nextval"),
        ("SELECT * FROM t FOR UPDATE", "it may write, lock rows or act on the server"),
        ("WITH d AS (DELETE FROM t WHERE id > 0 RETURNING *) SELECT * FROM d", "it may write"),
        ("SELECT myschema.f(x) FROM t", "a function that is not a built-in"),
    ] {
        let mut h = Harness::connected(Lang::En);
        h.app.tab_mut().editor = Editor::new(sql);
        let id = first_page(&mut h, 500, true);
        if h.overlay_kind().is_some() {
            // The data-modifying WITH asks first: confirmed.
            h.keys("y");
            let index = h.app.tabs.active_index();
            let columns = Some(vec![meta("id", "int8", true, false)]);
            let id = h.app.tab().exec.query_id;
            h.tab_db(index, DbEvent::Page { id, columns, rows: page(0, 500), more: true, elapsed: SEC });
        }
        let _ = id;
        h.sent();
        h.advance(30 * SEC);
        assert_eq!(close_requests(&h.sent()).len(), 1, "{sql}");
        assert!(!h.screen(160, 45).contains("runs the SELECT again"), "{sql}");
        h.key(KeyCode::Tab);
        h.keys("n");
        assert!(resumes(&h.sent()).is_empty(), "{sql}: nothing is run again");
        let status = h.status(220, 45);
        assert!(
            status.contains("Paging closed and this statement is not run again for you") && status.contains(why),
            "{sql}: {status}"
        );
    }
}

/// A result whose columns changed when it ran again gets nothing added; its portal goes.
#[test]
fn rows_run_again_with_other_columns_are_not_added() {
    let mut h = Harness::connected(Lang::En);
    let id = first_page(&mut h, 500, true);
    h.advance(30 * SEC);
    h.sent();
    h.key(KeyCode::Tab);
    h.keys("n");
    let (rid, _, _) = resumes(&h.sent())[0].clone();
    assert_ne!(rid, id);
    let columns = Some(vec![meta("id", "text", false, false)]);
    h.tab_db(0, DbEvent::Page { id: rid, columns, rows: page(500, 500), more: true, elapsed: SEC });
    assert!(matches!(&h.app.tab().results, Results::Rows(rs) if rs.rows.len() == 500));
    assert_eq!(close_requests(&h.sent()), [rid]);
    assert!(h.status(160, 45).contains("Not added: the statement's columns changed"));
    assert_eq!(first_shown(&h).as_deref(), Some("0"));
}

/// Paging past a closed portal happens only on the session and binding the result came from:
/// after the tab's connection was lost (or switched), the rows stay and it says to run again.
#[test]
fn a_result_of_another_session_is_not_run_again() {
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 500, true);
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    h.sent();
    h.key(KeyCode::Tab);
    h.keys("n");
    let sent = h.sent();
    assert!(resumes(&sent).is_empty() && fetches(&sent) == 0, "{sent:?}");
    assert!(h.status(200, 45).contains("This result's session is gone"));
    h.keys("#");
    assert!(counts(&h.sent()).is_empty());
}

/// `#` counts the rows only when asked: the count query of the statement, announced, answered
/// once; the title shows the pages then. Off the allowlist it is refused; a complete result is
/// counted already; a cancel says so.
#[test]
fn counting_is_asked_for_announced_and_shown() {
    let mut h = Harness::connected(Lang::En);
    let id = first_page(&mut h, 500, true);
    h.sent();
    h.key(KeyCode::Tab);
    h.keys("#");
    let sent = h.sent();
    let [(cid, sql)] = &counts(&sent)[..] else { panic!("{sent:?}") };
    assert_eq!(*cid, id);
    assert_eq!(sql, "SELECT count(*) FROM (\nSELECT * FROM shop.users WHERE id <= 8\n) AS datarig_count");
    // Announced (the status bar says it is counting while it runs).
    let note = h.app.tab().exec.run.notes.last().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default();
    assert!(note.starts_with("Counting rows, sent for you: SELECT count(*) FROM ("), "{note}");
    assert!(h.status(200, 45).contains("counting…"));
    assert!(results_title(&mut h, 160, 45).contains("counting…"));
    assert!(h.app.tab_busy(h.app.tab().id), "nothing else runs meanwhile");
    // Outside the user's block: of the rows committed now.
    h.tab_db(0, DbEvent::Counted { id, result: Ok(1234), snapshot: false });
    assert!(h.app.tab().exec.running.is_none());
    let title = results_title(&mut h, 160, 45);
    assert!(title.contains("(page 1 / 3 · 30s · 1,234 rows now)"), "{title}");
    assert!(h.status(160, 45).contains("Counted now: 1,234 rows (may differ from the pages)"));
    // Messages keep what was sent for the user.
    h.keys("L");
    let screen = h.screen(200, 45);
    assert!(screen.contains("Counting rows, sent for you") && screen.contains("Counted now: 1,234 rows"), "{screen}");
    h.keys("L");
    // A cancel.
    h.keys("#");
    assert_eq!(counts(&h.sent()).len(), 1);
    h.ctrl('c');
    assert!(h.session_cancelled(1));
    h.tab_db(0, DbEvent::Counted { id, result: Err(DbError::Cancelled), snapshot: false });
    assert!(h.status(160, 45).contains("Count cancelled"));
    // A late or stale answer changes nothing.
    h.tab_db(0, DbEvent::Counted { id: id + 9, result: Ok(1), snapshot: true });
    assert!(results_title(&mut h, 160, 45).contains("1,234 rows now"));

    // Off the allowlist: refused with the reason, nothing sent.
    let mut h = Harness::connected(Lang::En);
    h.app.tab_mut().editor = Editor::new("SELECT random() FROM generate_series(1, 5000)");
    first_page(&mut h, 500, true);
    h.sent();
    h.key(KeyCode::Tab);
    h.keys("#");
    assert!(counts(&h.sent()).is_empty());
    assert!(h.status(200, 45).contains("Rows are not counted for you: it calls random"));
    // Complete: nothing to count.
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 8, false);
    h.sent();
    h.key(KeyCode::Tab);
    h.keys("#");
    assert!(counts(&h.sent()).is_empty());
    assert!(h.status(160, 45).contains("Every row is fetched already: 8 rows"));
}

/// The checks of a run apply to what the app sends for the user: a policy that became read-only
/// after the tab's session opened refuses the count and the re-run (reconnect first).
#[test]
fn a_read_only_policy_applies_to_counts_and_re_runs() {
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 500, true);
    h.sent();
    h.app.policies.insert("default", Policy { read_only: true, ..Policy::default() });
    h.key(KeyCode::Tab);
    h.keys("#");
    assert!(counts(&h.sent()).is_empty());
    let status = h.status(220, 45);
    assert!(status.contains("read-only") || status.contains("connect"), "{status}");
    h.advance(30 * SEC);
    h.sent();
    h.keys("n");
    assert!(resumes(&h.sent()).is_empty());
}

/// Another statement run in the tab ends the portal; its rows stay when it returned none, and the next page past them runs the SELECT again.
#[test]
fn a_run_without_rows_closes_the_portal_but_keeps_the_rows() {
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 500, true);
    h.app.tab_mut().editor = Editor::new("SET work_mem = '8MB'");
    h.sent();
    h.ctrl('e');
    let qid = h.app.tab().exec.query_id;
    assert_eq!(h.app.tab().exec.paging, Paging::Replaced);
    h.tab_db(0, DbEvent::Done { id: qid, outcome: Outcome::Command("SET".into()), elapsed: SEC });
    h.key(KeyCode::Tab);
    h.keys("L"); // back from Messages to the kept rows
    assert_eq!(first_shown(&h).as_deref(), Some("0"));
    let screen = h.screen(160, 45);
    assert!(screen.contains("paging closed — n runs the SELECT again"), "{screen}");
    h.sent();
    h.keys("n");
    let r = resumes(&h.sent());
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].2, 500);
}

/// Unknown is not absent: a fetch of the next page that is cancelled (`Ctrl+C`), fails, or whose
/// cancel is never answered leaves the rows fetched, and the result never claims to be complete
/// (no "last page", no "every row is fetched", `?` pages): the next page goes the way of a
/// closed portal (run again for a plain `SELECT`), and `#` counts.
#[test]
fn a_stopped_fetch_keeps_the_result_incomplete() {
    for cancelled in [true, false] {
        let mut h = Harness::connected(Lang::En);
        h.app.detail.visible = false;
        let id = first_page(&mut h, 500, true);
        h.sent();
        h.key(KeyCode::Tab); // results
        h.keys("n");
        assert_eq!(fetches(&h.sent()), 1);
        let error = if cancelled {
            h.ctrl('c');
            assert!(h.session_cancelled(1));
            DbError::Server("ERROR: canceling statement due to user request".into())
        } else {
            DbError::Server("ERROR: division by zero".into())
        };
        h.tab_db(0, DbEvent::Failed { id, error, cancelled });
        assert!(h.app.tab().exec.running.is_none());
        assert!(matches!(&h.app.tab().results, Results::Rows(rs) if rs.rows.len() == 500 && rs.more), "{cancelled}");
        assert!(h.app.tab().exec.paging.closed(), "the portal is gone, the rows may go on");
        let title = results_title(&mut h, 160, 45);
        assert!(
            title.contains("Results · rows 1–500") && title.contains("(page 1 / ? · paging closed)"),
            "{cancelled}: {title}"
        );
        let screen = h.screen(160, 45);
        assert!(screen.contains("Showing 500 rows · paging closed — n runs the SELECT again"), "{screen}");
        // `#` counts: the result is not complete.
        h.app.transient = None;
        h.keys("#");
        let sent = h.sent();
        assert_eq!(counts(&sent).len(), 1, "{cancelled}: {sent:?}");
        assert!(!h.status(200, 45).contains("Every row is fetched already"));
        h.tab_db(0, DbEvent::Counted { id, result: Ok(1600), snapshot: false });
        assert!(results_title(&mut h, 160, 45).contains("(page 1 / 4 · paging closed · 1,600 rows now)"));
        // `n` runs the SELECT again past the rows it has (never "last page").
        h.keys("n");
        let sent = h.sent();
        let [(rid, _, skip)] = &resumes(&sent)[..] else { panic!("{cancelled}: {sent:?}") };
        assert_eq!(*skip, 500);
        assert!(!h.status(200, 45).contains("Last page"));
        let columns = Some(vec![meta("id", "int8", true, false)]);
        h.tab_db(0, DbEvent::Page { id: *rid, columns, rows: page(500, 500), more: true, elapsed: SEC });
        assert_eq!(first_shown(&h).as_deref(), Some("500"));
    }

    // Off the allowlist: the next page is refused with the reason, not "last page".
    let mut h = Harness::connected(Lang::En);
    h.app.tab_mut().editor = Editor::new("SELECT random() FROM generate_series(1, 5000)");
    let id = first_page(&mut h, 500, true);
    h.sent();
    h.key(KeyCode::Tab);
    h.keys("n");
    h.ctrl('c');
    h.tab_db(0, DbEvent::Failed { id, error: DbError::Server("canceled".into()), cancelled: true });
    let screen = h.screen(200, 45);
    assert!(screen.contains("Showing 500 rows · fetching the next page stopped (failed or cancelled)"), "{screen}");
    h.app.transient = None;
    h.keys("n");
    assert!(resumes(&h.sent()).is_empty());
    let status = h.status(220, 45);
    assert!(status.contains("not run again for you (it calls random,") && !status.contains("Last page"), "{status}");

    // A cancel the session never answers: the session is closed, the rows stay, and the result
    // is still not complete (its session is gone, so nothing is fetched for it).
    let mut h = Harness::connected(Lang::En);
    first_page(&mut h, 500, true);
    h.sent();
    h.key(KeyCode::Tab);
    h.keys("n");
    h.ctrl('c');
    h.advance(60 * SEC);
    assert!(h.app.tab().exec.running.is_none());
    assert!(matches!(&h.app.tab().results, Results::Rows(rs) if rs.more));
    assert!(results_title(&mut h, 160, 45).contains("(page 1 / ?"));
    h.app.transient = None;
    h.keys("n");
    let status = h.status(220, 45);
    assert!(status.contains("This result's session is gone") && !status.contains("Last page"), "{status}");
    h.keys("#");
    assert!(!h.status(220, 45).contains("Every row is fetched already"));
}

/// The server's side of the allowlist (asked by the driver right before) refuses a re-run or a
/// count: said with the reason, nothing is added, and the line below the rows no longer offers
/// to run the SELECT again.
#[test]
fn a_refusal_of_the_server_side_of_the_allowlist_is_said() {
    use datarig_core::sql::risk::repeat::NotRepeatable;
    let mut h = Harness::connected(Lang::En);
    h.app.detail.visible = false;
    let id = first_page(&mut h, 500, true);
    h.sent();
    h.key(KeyCode::Tab);
    h.keys("#");
    let why = NotRepeatable::Shadowed("abs".into());
    h.tab_db(0, DbEvent::Counted { id, result: Err(DbError::NotRepeatable(why)), snapshot: false });
    let status = h.status(220, 45);
    assert!(
        status.contains("Rows are not counted for you: a function or operator of the database's own is also named abs"),
        "{status}"
    );
    h.advance(30 * SEC);
    h.sent();
    assert!(h.screen(160, 45).contains("n runs the SELECT again"));
    h.keys("n");
    let (rid, _, _) = resumes(&h.sent())[0].clone();
    let error = DbError::NotRepeatable(NotRepeatable::NotATable("zz_v".into()));
    h.tab_db(0, DbEvent::Failed { id: rid, error, cancelled: false });
    assert!(matches!(&h.app.tab().results, Results::Rows(rs) if rs.rows.len() == 500 && rs.more));
    let status = h.status(220, 45);
    assert!(status.contains("not run again for you (it reads zz_v, which is not a table"), "{status}");
    let screen = h.screen(160, 45);
    assert!(screen.contains("paging closed after idle — re-run to load more"), "{screen}");
}

/// At 80 columns a closed portal's title keeps the page number (the arrows go first, then the
/// state, which the line below the rows says too), in both languages.
#[test]
fn a_narrow_closed_title_keeps_the_page_number() {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = Harness::connected(lang);
        let id = first_page(&mut h, 500, true);
        h.key(KeyCode::Tab);
        h.keys("n");
        h.tab_db(0, DbEvent::Page { id, columns: None, rows: page(500, 500), more: true, elapsed: SEC / 100 });
        h.advance(30 * SEC);
        assert_eq!(h.app.tab().exec.paging, Paging::ClosedIdle);
        let title = results_title(&mut h, 80, 24);
        assert!(title.contains("(2/?"), "{lang:?}: {title}");
        assert_eq!(datarig_tui::text::width(&title), 80, "{title}");
        // Wide: the whole state.
        let wide = results_title(&mut h, 160, 45);
        let i18n = datarig_core::i18n::I18n::new(lang);
        let closed = i18n.label(datarig_core::i18n::Label::ResultsTitlePagingClosed).to_string();
        assert!(wide.contains(&closed), "{lang:?}: {wide}");
    }
}

/// A count or a fetch of the next page that a connection switch stops ends with a note of its
/// own in the status bar and the Messages (one terminal note per request): "Counting rows, sent
/// for you" is never the last word.
#[test]
fn a_connection_switch_ends_a_count_or_a_fetch_with_a_note() {
    for (key, want) in [
        ("#", "Count stopped: the tab switched to another connection"),
        ("n", "Fetching the next page stopped: the tab switched to another connection"),
    ] {
        let mut h = Harness::connected(Lang::En);
        first_page(&mut h, 500, true);
        h.key(KeyCode::Tab);
        h.keys(key);
        assert!(h.app.tab().exec.running.is_some(), "{key}");
        h.keys(" cs");
        h.keys("y");
        h.type_text("local");
        h.key(KeyCode::Enter);
        assert!(h.app.tab().exec.running.is_none(), "{key}");
        h.app.transient = None;
        let status = h.status(220, 45);
        assert!(status.contains(want), "{key}: {status}");
        let last = h.app.tab().exec.run.notes.last().map(|n| n.render(&h.app.i18n).to_string());
        assert_eq!(last.as_deref(), Some(want), "{key}");
        assert!(matches!(&h.app.tab().results, Results::Rows(rs) if rs.more), "{key}: still not complete");
    }
}

/// A count says whether it is of the rows being paged. It is only when the
/// driver counted in a transaction with one snapshot (`snapshot`) that is the user's block every
/// page was read in. Otherwise it is of the rows committed when it ran, and says so: "Counted
/// now: … (may differ from the pages)", and "rows now" in the title; also outside the user's
/// block while the portal is open (the app pages as psql does, at the session's isolation).
#[test]
fn a_count_says_when_it_may_differ_from_the_pages() {
    let now = "Counted now: 1,100 rows (may differ from the pages)";
    // (i) The portal's own transaction, outside the user's block: counted now, whatever the
    // isolation the server said.
    for snapshot in [false, true] {
        let mut h = Harness::connected(Lang::En);
        let id = first_page(&mut h, 500, true);
        h.key(KeyCode::Tab);
        h.keys("#");
        h.tab_db(0, DbEvent::Counted { id, result: Ok(1100), snapshot });
        assert!(h.status(200, 45).contains(now), "{snapshot}: {}", h.status(200, 45));
        assert!(results_title(&mut h, 160, 45).contains("· 1,100 rows now)"));
    }
    // (ii) The portal in the user's block at READ COMMITTED: counted now.
    let mut h = Harness::connected(Lang::En);
    h.tab_db(0, DbEvent::Block(true));
    let id = first_page(&mut h, 500, true);
    h.key(KeyCode::Tab);
    h.keys("#");
    h.tab_db(0, DbEvent::Counted { id, result: Ok(1100), snapshot: false });
    assert!(h.status(200, 45).contains(now), "{}", h.status(200, 45));
    assert!(results_title(&mut h, 160, 45).contains("· 1,100 rows now)"));
    let note = h.app.tab().exec.run.notes.last().map(|n| n.render(&h.app.i18n).to_string());
    assert_eq!(note.as_deref(), Some(now), "said in the Messages too");
    // (iii) The user's block at REPEATABLE READ: the pages' count, also once the portal ended
    // (every page was read in that block).
    for open in [true, false] {
        let mut h = Harness::connected(Lang::En);
        h.tab_db(0, DbEvent::Block(true));
        let id = first_page(&mut h, 500, true);
        if !open {
            h.app.tab_mut().exec.paging = Paging::Replaced;
        }
        h.key(KeyCode::Tab);
        h.keys("#");
        h.tab_db(0, DbEvent::Counted { id, result: Ok(1000), snapshot: true });
        assert!(h.status(200, 45).contains("Counted: 1,000 rows"), "{open}: {}", h.status(200, 45));
    }
    // After the portal closed, outside a transaction: counted now.
    let mut h = Harness::connected(Lang::En);
    let id = first_page(&mut h, 500, true);
    h.advance(30 * SEC);
    h.key(KeyCode::Tab);
    h.keys("#");
    h.tab_db(0, DbEvent::Counted { id, result: Ok(1100), snapshot: false });
    assert!(h.status(200, 45).contains(now));
    // Pages read outside the block, counted in a REPEATABLE READ block later: not the pages'.
    let mut h = Harness::connected(Lang::En);
    let id = first_page(&mut h, 500, true);
    h.advance(30 * SEC);
    h.tab_db(0, DbEvent::Block(true));
    h.key(KeyCode::Tab);
    h.keys("#");
    h.tab_db(0, DbEvent::Counted { id, result: Ok(1100), snapshot: true });
    assert!(h.status(200, 45).contains(now));
    // Korean, from the catalog.
    let mut h = Harness::connected(Lang::Ko);
    let id = first_page(&mut h, 500, true);
    h.advance(30 * SEC);
    h.key(KeyCode::Tab);
    h.keys("#");
    h.tab_db(0, DbEvent::Counted { id, result: Ok(1100), snapshot: false });
    let ko = datarig_core::i18n::I18n::new(Lang::Ko)
        .msg(&datarig_core::i18n::Msg::ResultsCountDoneNow { count: 1100 })
        .to_string();
    assert!(h.status(200, 45).contains(&ko));
}

/// The wheel scrolls the grid's view (three rows a notch), as
/// DataGrip and a browser do: the selected cell stays on its data row, also off screen, the view
/// stops at the ends of the page and fetches nothing; the next key brings the view back to the
/// selection. Shift+wheel and a sideways wheel scroll the columns.
#[test]
fn the_wheel_scrolls_the_view_not_the_selection() {
    let mut h = Harness::connected(Lang::En);
    let wide: Vec<_> = (0..30).map(|i| meta(&format!("column_{i:02}"), "text", false, false)).collect();
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let rows: Vec<Vec<Option<String>>> =
        (0..500).map(|r| (0..30).map(|c| Some(format!("r{r}c{c}"))).collect()).collect();
    h.tab_db(0, DbEvent::TxOpen(true));
    h.tab_db(0, DbEvent::Page { id, columns: Some(wide), rows, more: true, elapsed: Duration::from_millis(2) });
    h.sent();
    h.draw(120, 40);
    h.app.focus = datarig_tui::app::Focus::Results;
    let r = h.app.layout.results;
    let (x, y) = (r.x + r.width / 2, r.y + r.height / 2);
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.top), (0, 0));
    for _ in 0..3 {
        h.mouse(MouseEventKind::ScrollDown, x, y);
    }
    h.draw(120, 40);
    let g = &h.app.tab().grid;
    assert_eq!((g.row, g.top), (0, 9), "three rows a notch; the selection stays on row 1");
    assert!(!h.screen(120, 40).contains("r3c0") && h.screen(120, 40).contains("r9c0"), "rows 1-9 are off screen");
    // Up past the top stops at the first row.
    for _ in 0..10 {
        h.mouse(MouseEventKind::ScrollUp, x, y);
    }
    h.draw(120, 40);
    assert_eq!(h.app.tab().grid.top, 0);
    // Down past the end stops at the page's last screen; nothing is fetched.
    for _ in 0..500 {
        h.mouse(MouseEventKind::ScrollDown, x, y);
    }
    h.draw(120, 40);
    let g = &h.app.tab().grid;
    assert_eq!(g.top, 500 - g.page_rows, "clamped at the end of the page");
    assert_eq!(g.row, 0);
    assert_eq!(fetches(&h.sent()), 0, "the wheel never fetches");
    // A key brings the view back to the selection.
    h.keys("j");
    h.draw(120, 40);
    let g = &h.app.tab().grid;
    assert_eq!((g.row, g.top), (1, 1));
    // Shift+wheel and the sideways wheel scroll the columns; the selected column stays.
    use ratatui::crossterm::event::{KeyModifiers, MouseEvent};
    h.app.handle_event(ratatui::crossterm::event::Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: x,
        row: y,
        modifiers: KeyModifiers::SHIFT,
    }));
    h.mouse(MouseEventKind::ScrollRight, x, y);
    h.draw(120, 40);
    let g = &h.app.tab().grid;
    assert_eq!((g.col, g.left), (0, 2));
    let t = h.draw(120, 40);
    let header = row_text(t.backend().buffer(), h.app.tab().grid.data_y - 2);
    assert!(!header.contains("│ column_00 │") && header.contains("│ column_02 │"), "{header}");
    h.mouse(MouseEventKind::ScrollLeft, x, y);
    h.draw(120, 40);
    assert_eq!(h.app.tab().grid.left, 1);
    h.keys("l");
    h.draw(120, 40);
    let g = &h.app.tab().grid;
    assert_eq!((g.col, g.left), (1, 1), "a key shows the selected column again");
}

/// After the tab switched its database or schema, `n` is refused (the
/// result's session is gone), so the footer no longer says "n runs the SELECT again".
#[test]
fn the_run_again_hint_goes_with_the_session() {
    let mut h = Harness::connected(Lang::En);
    h.app.detail.visible = false;
    first_page(&mut h, 500, true);
    h.advance(31 * SEC);
    assert_eq!(h.app.tab().exec.paging, Paging::ClosedIdle);
    h.tab_db(0, DbEvent::TxOpen(false));
    assert!(h.screen(160, 45).contains("paging closed — n runs the SELECT again"));
    h.command("use .shop");
    let screen = h.screen(160, 45);
    assert!(h.status(160, 45).contains("Showing 500 rows · paging closed"), "{screen}");
    assert!(!screen.contains("runs the SELECT again"), "{screen}");
    h.sent();
    h.key(KeyCode::Tab);
    h.keys("n");
    assert!(resumes(&h.sent()).is_empty(), "refused");
    assert!(h.status(200, 45).contains("This result's session is gone"));
}

/// Run the statement under the cursor and answer as the driver does when nothing is held
/// (`paging = "no_hold"`, the default): `Released`, then a first page of `n` rows with more after
/// it, and no transaction reported open. Returns the statement id.
fn first_page_released(h: &mut Harness, n: usize) -> u64 {
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = Some(vec![meta("id", "int8", true, false)]);
    h.tab_db(index, DbEvent::Released { id });
    h.tab_db(index, DbEvent::Page { id, columns, rows: page(0, n), more: true, elapsed: Duration::from_millis(2) });
    id
}

/// By default the app asks for nothing to be held, and a result whose portal the driver
/// released says so: no countdown, no timer, no close request ever; the title says the next
/// page runs again and the footer says nothing is held. `n` runs the SELECT again for the next
/// page (announced as such), whose answer is released too and continues the result.
#[test]
fn a_result_not_held_pages_by_running_its_select_again() {
    let mut h = Harness::connected(Lang::En);
    h.app.detail.visible = false;
    let id = first_page_released(&mut h, 500);
    let sent = h.sent();
    assert!(
        sent.iter().any(|c| matches!(c, DbCommand::Execute { id: i, paging: PagingMode::NoHold, .. } if *i == id)),
        "{sent:?}"
    );
    assert_eq!(h.app.tab().exec.paging, Paging::Released);
    assert!(!h.app.tab().exec.tx_open);
    assert!(!h.app.needs_tick(), "nothing counts down");
    assert_eq!(h.app.next_tick(h.clock.now()), None);
    let title = results_title(&mut h, 160, 45);
    assert!(title.contains("Results · rows 1–500") && title.contains("(page 1 / ? · next page re-runs) ›"), "{title}");
    let screen = h.screen(160, 45);
    assert!(
        screen.contains(
            "Showing 500 rows · nothing is held open on the server — n runs the SELECT again for the next page"
        ),
        "{screen}"
    );
    h.advance(3600 * SEC);
    assert!(h.sent().is_empty(), "no close request, no fetch: nothing is open");

    h.key(KeyCode::Tab); // results
    h.keys("n");
    let sent = h.sent();
    assert_eq!(fetches(&sent), 0, "nothing to fetch from");
    let [(rid, sql, skip)] = &resumes(&sent)[..] else { panic!("{sent:?}") };
    assert_eq!((sql.as_str(), *skip), ("SELECT * FROM shop.users WHERE id <= 8", 500));
    let status = h.app.tab().status.as_ref().map(|n| n.render(&h.app.i18n).to_string()).unwrap_or_default();
    assert!(
        status
            .contains("Ran the SELECT again for rows 501–1,000 (nothing is held open between pages); without ORDER BY"),
        "{status}"
    );
    assert!(
        h.app
            .tab()
            .exec
            .run
            .notes
            .iter()
            .any(|n| matches!(n.msg, datarig_core::i18n::Msg::ResultsPageRerunUnordered { .. }))
    );
    let columns = Some(vec![meta("id", "int8", true, false)]);
    h.tab_db(0, DbEvent::Released { id: *rid });
    h.tab_db(0, DbEvent::Page { id: *rid, columns, rows: page(500, 500), more: true, elapsed: SEC / 100 });
    assert_eq!(first_shown(&h).as_deref(), Some("500"));
    assert!(matches!(&h.app.tab().results, Results::Rows(rs) if rs.rows.len() == 1000 && rs.more));
    assert_eq!(h.app.tab().exec.paging, Paging::Released);
    let title = results_title(&mut h, 160, 45);
    assert!(
        title.contains("Results · rows 501–1,000") && title.contains("(page 2 / ? · next page re-runs)"),
        "{title}"
    );
    // Nothing is offered that would need the portal: the rest cannot be fetched for a copy.
    let (x, y) = (h.app.tab().grid.hit_cols[0].0 + 2, h.app.tab().grid.data_y);
    h.mouse(MouseEventKind::Down(MouseButton::Right), x, y);
    let screen = h.screen(160, 45);
    assert!(screen.contains("(the server has more)") && !screen.contains("Fetch every row"), "{screen}");
    h.key(KeyCode::Esc);
    h.advance(3600 * SEC);
    assert!(close_requests(&h.sent()).is_empty());
}

/// A statement that is not run again for the user, not held: its first page is all there is,
/// and the app says why and what to do (LIMIT/OFFSET, or the hold policy), in the status bar,
/// in Messages, in the title and below the rows; the next page is refused with the same advice
/// and nothing is sent.
#[test]
fn a_result_not_held_that_is_not_run_again_shows_its_first_page_only() {
    let mut h = Harness::connected(Lang::En);
    h.app.detail.visible = false;
    h.app.tab_mut().editor = Editor::new("SELECT random() FROM generate_series(1, 5000)");
    first_page_released(&mut h, 500);
    h.sent();
    let status = h.status(400, 45);
    assert!(
        status.contains("First 500 rows only: nothing is held open on the server after the first page")
            && status.contains("it calls random")
            && status.contains("LIMIT/OFFSET, or set paging = \"hold\" in the profile's policy"),
        "{status}"
    );
    assert!(
        h.app
            .tab()
            .exec
            .run
            .notes
            .iter()
            .any(|n| matches!(n.msg, datarig_core::i18n::Msg::ResultsFirstPageOnly { .. }))
    );
    assert!(results_title(&mut h, 160, 45).contains("(page 1 / ? · first page only)"));
    let screen = h.screen(200, 45);
    assert!(screen.contains("Showing the first 500 rows only · this statement is not run again for you"), "{screen}");
    h.key(KeyCode::Tab);
    h.keys("n");
    assert!(h.sent().is_empty(), "nothing is sent");
    let status = h.status(400, 45);
    assert!(
        status.contains("Only the first page was read and this statement is not run again for you")
            && status.contains("paging = \"hold\""),
        "{status}"
    );
}

/// `paging = "hold"`: the app asks the driver to hold the portal (today's behaviour before
/// no-hold became the default), and a page with more rows counts down to the idle close.
#[test]
fn a_hold_policy_asks_for_the_portal_to_be_held() {
    let mut cfg = test_db_config();
    cfg.connections[0].policy = Some("browse".into());
    cfg.policies.insert("browse", Policy { paging: PagingMode::Hold, ..Policy::default() });
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    let id = first_page(&mut h, 500, true);
    let sent = h.sent();
    assert!(
        sent.iter().any(|c| matches!(c, DbCommand::Execute { id: i, paging: PagingMode::Hold, .. } if *i == id)),
        "{sent:?}"
    );
    assert!(matches!(h.app.tab().exec.paging, Paging::Open { in_block: false, .. }));
    assert!(results_title(&mut h, 160, 45).contains("(page 1 / ? · 30s)"));
    h.advance(30 * SEC);
    assert_eq!(close_requests(&h.sent()), [id]);
    // Past the closed portal the statement runs again, held again.
    h.key(KeyCode::Tab);
    h.keys("n");
    let sent = h.sent();
    assert!(sent.iter().any(|c| matches!(c, DbCommand::Resume { paging: PagingMode::Hold, .. })), "{sent:?}");
}
