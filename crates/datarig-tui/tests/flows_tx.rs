//! Labels of results read inside the user's transaction: while it is open
//! they say "uncommitted (in tx)"; once it ended, "from rolled-back tx" when the app knows it
//! rolled back, else "from ended tx" (unknown is never shown as committed). A run without rows
//! (the `COMMIT` itself) keeps the rows on screen, so the label stays readable.

mod common;

use common::*;
use datarig_core::driver::{DbError, DbEvent, Outcome};
use datarig_core::i18n::Lang;
use datarig_tui::app::Results;
use datarig_tui::app::tabs::ResultView;
use datarig_tui::widgets::editor::Editor;
use datarig_tui::widgets::grid::TxMark;
use std::time::Duration;

const MS: Duration = Duration::from_millis(3);

/// Run `sql` in the active tab and answer it: rows (`Some(n)`) or a command tag.
fn answer(h: &mut Harness, sql: &str, rows: Option<usize>) {
    h.app.tab_mut().editor = Editor::new(sql);
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let ev = match rows {
        Some(n) => DbEvent::Page {
            id,
            columns: Some(vec![meta("x", "int4", true, false)]),
            rows: (0..n).map(|i| vec![Some(i.to_string())]).collect(),
            more: false,
            elapsed: MS,
        },
        None => DbEvent::Done {
            id,
            outcome: Outcome::Command(sql.split_whitespace().next().unwrap().to_uppercase()),
            elapsed: MS,
        },
    };
    h.tab_db(index, ev);
}

/// `BEGIN`, as the driver reports it: the command, then the user's block.
fn begin(h: &mut Harness) {
    answer(h, "BEGIN", None);
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
}

/// The block ends as the driver reports it after the statement that ended it.
fn end_block(h: &mut Harness) {
    h.tab_db(0, DbEvent::Block(false));
    h.tab_db(0, DbEvent::TxOpen(false));
}

fn mark(h: &Harness) -> Option<TxMark> {
    match &h.app.tab().results {
        Results::Rows(rs) => rs.tx,
        _ => None,
    }
}

fn strip(h: &mut Harness) -> String {
    let screen = h.screen(160, 45);
    screen.lines().nth(h.app.layout.strip.y as usize).unwrap_or_default().to_string()
}

#[test]
fn rows_read_in_the_users_transaction_say_so_until_it_commits() {
    let mut h = Harness::connected(Lang::En);
    answer(&mut h, "SELECT 1", Some(2));
    assert_eq!(mark(&h), None, "read outside a transaction");
    assert!(!h.app.strip_shown(), "no label, no strip");
    begin(&mut h);
    answer(&mut h, "SELECT x FROM t", Some(3));
    assert_eq!(mark(&h), Some(TxMark::InTx(1)));
    let s = strip(&mut h);
    assert!(s.contains("uncommitted (in tx)"), "{s}");
    insta::assert_snapshot!("tx_uncommitted_en_160x45", h.draw(160, 45).backend());
    insta::assert_snapshot!("tx_uncommitted_en_80x24", h.draw(80, 24).backend());
    // COMMIT: a run without rows keeps them on screen, now from an ended transaction.
    answer(&mut h, "COMMIT", None);
    end_block(&mut h);
    assert_eq!(h.app.tab().exec.view, ResultView::Messages, "Messages are shown; the rows are one tab away");
    h.key(ratatui::crossterm::event::KeyCode::Tab);
    h.keys("L");
    assert_eq!(mark(&h), Some(TxMark::Ended));
    let s = strip(&mut h);
    assert!(s.contains("from ended tx") && s.contains("earlier run"), "{s}");
    assert!(!s.contains("uncommitted"), "{s}");
}

#[test]
fn a_rollback_says_so() {
    use datarig_core::i18n::{I18n, Label};
    assert_eq!(I18n::new(Lang::En).label(Label::ResultsTxOpen), "uncommitted (in tx)");
    for lang in [Lang::En, Lang::Ko] {
        let (open, rolled) =
            (I18n::new(lang).label(Label::ResultsTxOpen), I18n::new(lang).label(Label::ResultsTxRolledBack));
        let (open, rolled) = (open.as_str(), rolled.as_str());
        let mut h = Harness::connected(lang);
        begin(&mut h);
        answer(&mut h, "SELECT x FROM t", Some(3));
        assert!(strip(&mut h).contains(open));
        answer(&mut h, "ROLLBACK", None);
        end_block(&mut h);
        h.key(ratatui::crossterm::event::KeyCode::Tab);
        h.keys("L");
        assert_eq!(mark(&h), Some(TxMark::RolledBack));
        assert!(strip(&mut h).contains(rolled));
        // At 80 columns: every line whole, the shown result tab and the label, from the
        // catalog.
        let screen = h.screen(80, 24);
        assert!(screen.lines().all(|l| datarig_tui::text::width(l) == 80), "{screen}");
        let line = screen.lines().nth(h.app.layout.strip.y as usize).unwrap();
        assert!(line.contains("[1]") && line.contains(rolled), "{lang:?}: {line}");
    }
}

/// `ROLLBACK TO SAVEPOINT` does not end the block; a `COMMIT` of an aborted block, and a
/// `COMMIT` that fails, roll back.
#[test]
fn the_app_tells_a_rollback_from_the_statement_that_ended_the_block() {
    let mut h = Harness::connected(Lang::En);
    begin(&mut h);
    answer(&mut h, "SELECT x FROM t", Some(3));
    answer(&mut h, "ROLLBACK TO SAVEPOINT s", None);
    assert_eq!(h.app.tab().result_tabs(), [0]);
    h.key(ratatui::crossterm::event::KeyCode::Tab);
    h.keys("L");
    assert_eq!(mark(&h), Some(TxMark::InTx(1)), "still open");
    // A failure aborts the block; its COMMIT rolls back.
    h.tab_db(0, DbEvent::TxAborted(true));
    answer(&mut h, "COMMIT", None);
    end_block(&mut h);
    h.keys("L");
    assert_eq!(mark(&h), Some(TxMark::RolledBack));

    // A COMMIT that fails (a deferred constraint) rolls back too.
    let mut h = Harness::connected(Lang::En);
    begin(&mut h);
    answer(&mut h, "SELECT x FROM t", Some(3));
    h.app.tab_mut().editor = Editor::new("COMMIT");
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::Failed { id, error: DbError::from("ERROR: violates constraint"), cancelled: false });
    end_block(&mut h);
    h.key(ratatui::crossterm::event::KeyCode::Tab);
    h.keys("L");
    assert_eq!(mark(&h), Some(TxMark::RolledBack));

    // PREPARE TRANSACTION ends it without the app knowing more: ended, never "committed".
    let mut h = Harness::connected(Lang::En);
    begin(&mut h);
    answer(&mut h, "SELECT x FROM t", Some(3));
    answer(&mut h, "PREPARE TRANSACTION 'p1'", None);
    end_block(&mut h);
    h.key(ratatui::crossterm::event::KeyCode::Tab);
    h.keys("L");
    assert_eq!(mark(&h), Some(TxMark::Ended));
}

/// A session that ends (lost, switched to another connection) takes the transaction with it:
/// the server rolls it back.
#[test]
fn a_session_that_ends_rolls_the_transaction_back() {
    let mut h = Harness::connected(Lang::En);
    begin(&mut h);
    answer(&mut h, "SELECT x FROM t", Some(3));
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    assert_eq!(mark(&h), Some(TxMark::RolledBack));
    assert!(!h.app.tab().exec.in_block);
    assert!(strip(&mut h).contains("from rolled-back tx"));

    // Disconnecting the profile.
    let mut h = Harness::connected(Lang::En);
    begin(&mut h);
    answer(&mut h, "SELECT x FROM t", Some(3));
    h.command("conn.disconnect_current");
    h.keys("y");
    assert_eq!(mark(&h), Some(TxMark::RolledBack));
}

/// Each transaction has its own number: the end of a later one does not relabel rows read in
/// an earlier one, nor rows read outside any.
#[test]
fn a_later_transaction_leaves_earlier_rows_alone() {
    let mut h = Harness::connected(Lang::En);
    begin(&mut h);
    answer(&mut h, "SELECT x FROM t", Some(3));
    answer(&mut h, "ROLLBACK", None);
    end_block(&mut h);
    begin(&mut h);
    answer(&mut h, "COMMIT", None);
    end_block(&mut h);
    h.key(ratatui::crossterm::event::KeyCode::Tab);
    h.keys("L");
    assert_eq!(mark(&h), Some(TxMark::RolledBack), "the first one's fate");
}

/// Run `sql` in the active tab and answer a first page whose portal stays open outside the
/// user's block, as the driver reports it: the transaction that holds the portal, then the page.
fn paging(h: &mut Harness, sql: &str) {
    h.app.tab_mut().editor = Editor::new(sql);
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    h.tab_db(0, DbEvent::TxOpen(true));
    let rows = (0..500).map(|i| vec![Some(i.to_string())]).collect();
    let columns = Some(vec![meta("x", "int4", true, false)]);
    h.tab_db(0, DbEvent::Page { id, columns, rows, more: true, elapsed: MS });
}

fn state(h: &Harness) -> datarig_tui::widgets::tabbar::State {
    datarig_tui::widgets::tabbar::state(&h.app, h.app.tab())
}

fn quit_asks(h: &mut Harness) -> bool {
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    let asked = h.app.overlays.confirm().is_some();
    assert_ne!(asked, h.app.quit, "either it asks or it quits");
    asked
}

/// The transaction the app opens to page a result outside the user's block is not
/// the user's: the tab is connected (`●`), the status bar says no "TX open", and quitting (or
/// closing the tab) asks nothing, since closing the portal of a plain read loses nothing.
#[test]
fn the_apps_paging_transaction_is_not_the_users() {
    use datarig_tui::widgets::tabbar::State;
    let mut h = Harness::connected(Lang::En);
    paging(&mut h, r#"SELECT * FROM "shop"."users""#);
    assert!(h.app.tab().exec.tx_open, "the server has a transaction open");
    assert_eq!(state(&h), State::Connected);
    let bar = h.screen(160, 45).lines().next().unwrap().to_string();
    assert!(bar.contains("console 1 ● ×") && !bar.contains('◆'), "{bar}");
    let status = h.status(160, 45);
    assert!(!status.contains("TX open"), "{status}");
    h.app.dispatch(datarig_tui::app::action::Action::CloseTab);
    assert!(h.app.overlays.confirm().is_none(), "closing the tab asks nothing");
    assert!(h.app.tabs.is_empty(), "closed at once");
    let mut h = Harness::connected(Lang::En);
    paging(&mut h, r#"SELECT * FROM "shop"."users""#);
    assert!(!quit_asks(&mut h), "quitting asks nothing");
}

/// A paging transaction whose statement may have written (anything but a plain `SELECT` the
/// app could run again) still asks before it is rolled back, though it is not marked `◆`.
#[test]
fn a_paging_transaction_that_may_have_written_still_asks() {
    use datarig_tui::widgets::tabbar::State;
    let mut h = Harness::connected(Lang::En);
    paging(&mut h, "INSERT INTO t SELECT g FROM generate_series(1, 500) g RETURNING x");
    assert_eq!(state(&h), State::Connected);
    assert!(!h.status(160, 45).contains("TX open"));
    assert!(quit_asks(&mut h), "its rows are not committed yet");
}

/// The user's block: `◆` and "TX open", and quitting asks; a failure in it: `!` and "ROLLBACK
/// required".
#[test]
fn the_users_block_is_marked_and_asked_about() {
    use datarig_tui::widgets::tabbar::State;
    let mut h = Harness::connected(Lang::En);
    begin(&mut h);
    assert_eq!(state(&h), State::TxOpen);
    assert!(h.screen(160, 45).lines().next().unwrap().contains("console 1 ◆ ×"));
    assert!(h.status(160, 45).contains("TX open"));
    // A result paged inside the block: still the user's transaction.
    paging(&mut h, "SELECT x FROM t");
    assert_eq!(state(&h), State::TxOpen);
    h.tab_db(0, DbEvent::TxAborted(true));
    assert_eq!(state(&h), State::Trouble);
    assert!(h.status(160, 45).contains("ROLLBACK required"));
    assert!(quit_asks(&mut h));
}
