//! Runs of several statements: the driver runs them one after the other in
//! the tab's session; the tab keeps what each one did, names the statement that failed or was
//! cancelled, and shows the last statement's result.

mod common;

use common::*;
use datarig_core::driver::PagingMode;
use datarig_core::driver::{DbCommand, DbError, DbEvent, Outcome};
use datarig_core::i18n::Lang;
use datarig_tui::app::Results;
use datarig_tui::app::runlog::StatementOutcome::*;
use datarig_tui::app::tabs::ResultView;
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::KeyCode;
use std::time::Duration;

const MS: Duration = Duration::from_millis(4);

/// A connected harness that ran the three statements of its editor as one run (selected with
/// `ggVG`-like keys: Visual from the top to the end); the run's id.
fn run_three() -> (Harness, u64) {
    let mut h = Harness::connected(Lang::En);
    h.app.tab_mut().editor = Editor::new("INSERT INTO t VALUES (1);\nSELECT 1/0;\nSELECT * FROM t;");
    h.keys("ggvG$");
    h.sent();
    h.ctrl('e');
    let sent = h.sent();
    let [DbCommand::Execute { id, statements, paging: PagingMode::NoHold }] = &sent[..] else { panic!("{sent:?}") };
    assert_eq!(statements, &["INSERT INTO t VALUES (1)", "SELECT 1/0", "SELECT * FROM t"], "one run, in order");
    (h, *id)
}

fn outcomes(h: &Harness) -> Vec<datarig_tui::app::runlog::StatementOutcome> {
    h.app.tab().exec.run.statements.iter().map(|s| s.outcome.clone()).collect()
}

#[test]
fn a_failure_in_the_middle_names_the_statement_and_the_rest_did_not_run() {
    let (mut h, id) = run_three();
    h.db(DbEvent::Started { id, index: 0 });
    assert!(h.status(100, 30).contains("Running statement 1 of 3"), "{}", h.status(100, 30));
    h.db(DbEvent::Finished { id, index: 0, outcome: Outcome::Affected(1), elapsed: MS });
    h.db(DbEvent::Started { id, index: 1 });
    assert!(h.status(100, 30).contains("Running statement 2 of 3"));
    h.db(DbEvent::Failed { id, error: DbError::from("ERROR: division by zero"), cancelled: false });
    assert!(h.app.tab().exec.running.is_none(), "the run ended");
    assert_eq!(outcomes(&h), [Affected(1), Failed("ERROR: division by zero".into()), NotRun]);
    let screen = h.screen(160, 45);
    assert!(
        screen.contains("Statement 2 of 3 failed (SELECT 1/0): ERROR: division by zero"),
        "status bar and results pane: {screen}"
    );
    assert!(matches!(h.app.tab().results, Results::Error(_)));
}

#[test]
fn all_well_shows_the_last_statements_result() {
    let (mut h, id) = run_three();
    for (i, outcome) in [(0, Outcome::Affected(1)), (1, Outcome::Command("SELECT".into()))] {
        h.db(DbEvent::Started { id, index: i });
        h.db(DbEvent::Finished { id, index: i, outcome, elapsed: MS });
    }
    h.db(DbEvent::Started { id, index: 2 });
    let cols = vec![meta("a", "int4", true, false)];
    h.db(DbEvent::Page { id, columns: Some(cols), rows: vec![vec![Some("1".into())]], more: false, elapsed: MS });
    assert_eq!(outcomes(&h), [Affected(1), Command("SELECT".into()), Rows { count: 1, more: false }]);
    assert!(matches!(&h.app.tab().results, Results::Rows(rs) if rs.rows.len() == 1));
    assert!(h.status(100, 30).contains("1 row"), "{}", h.status(100, 30));
}

/// Cancel while the second statement runs: the session is asked to cancel; its answer says
/// the run was cancelled there (the driver also stops between two statements, see the
/// driver's tests), and the rest did not run.
#[test]
fn cancel_between_statements_stops_the_rest() {
    let (mut h, id) = run_three();
    h.db(DbEvent::Started { id, index: 0 });
    h.db(DbEvent::Finished { id, index: 0, outcome: Outcome::Affected(1), elapsed: MS });
    h.ctrl('c');
    assert!(h.session_cancelled(1), "the tab's session was asked to cancel");
    assert!(h.status(100, 30).contains("Cancelling"));
    // The driver saw the cancel before the second statement started: that one never ran, and
    // is not named as the one cancelled.
    h.db(DbEvent::Failed { id, error: DbError::Cancelled, cancelled: true });
    assert_eq!(outcomes(&h), [Affected(1), NotRun, NotRun]);
    let status = h.status(160, 45);
    assert!(status.contains("Cancelled; statements 2–3 of 3 did not run"), "{status}");
    assert!(matches!(h.app.tab().results, Results::Cancelled));
    assert!(h.app.tab().exec.running.is_none());

    // Cancelled while the second statement runs: that one is named.
    let (mut h, id) = run_three();
    h.db(DbEvent::Started { id, index: 0 });
    h.db(DbEvent::Finished { id, index: 0, outcome: Outcome::Affected(1), elapsed: MS });
    h.db(DbEvent::Started { id, index: 1 });
    h.ctrl('c');
    h.db(DbEvent::Failed {
        id,
        error: DbError::from("ERROR: canceling statement due to user request"),
        cancelled: true,
    });
    assert_eq!(outcomes(&h), [Affected(1), Cancelled, NotRun]);
    let status = h.status(160, 45);
    assert!(status.contains("Cancelled at statement 2 of 3 (SELECT 1/0); the rest did not run"), "{status}");

    // Before the last statement: only it did not run.
    let (mut h, id) = run_three();
    for i in 0..2 {
        h.db(DbEvent::Started { id, index: i });
        h.db(DbEvent::Finished { id, index: i, outcome: Outcome::Affected(1), elapsed: MS });
    }
    h.ctrl('c');
    h.db(DbEvent::Failed { id, error: DbError::Cancelled, cancelled: true });
    assert_eq!(outcomes(&h), [Affected(1), Affected(1), NotRun]);
    let status = h.status(160, 45);
    assert!(status.contains("Cancelled; statement 3 of 3 (SELECT * FROM t) did not run"), "{status}");
}

/// It is one run: a second Ctrl+E while it runs is refused, and a new run starts a new log.
#[test]
fn a_run_of_several_statements_is_one_run() {
    let (mut h, id) = run_three();
    h.db(DbEvent::Started { id, index: 0 });
    h.key(KeyCode::Esc);
    h.ctrl('e');
    assert!(h.sent().is_empty(), "busy: nothing more is sent");
    assert!(h.status(100, 30).contains("already running"));
    h.db(DbEvent::Failed { id, error: DbError::from("ERROR: x"), cancelled: false });
    h.ctrl('e');
    assert_eq!(h.app.tab().exec.run.len(), 1, "one statement under the cursor now");
    assert_eq!(outcomes(&h), [Waiting]);
    // Events of the earlier run are ignored.
    h.db(DbEvent::Started { id, index: 2 });
    assert_eq!(outcomes(&h), [Waiting]);
}

/// A statement before the last runs to its end and its rows come too (the driver fetches them
/// all): the tab keeps them as that statement's own result, and a new run starts afresh.
#[test]
fn the_rows_of_a_statement_before_the_last_are_kept_as_its_own_result() {
    let (mut h, id) = run_three();
    h.db(DbEvent::Started { id, index: 0 });
    let cols = vec![meta("nextval", "int8", true, false)];
    let page = |from: u64, n: u64| (from..from + n).map(|v| vec![Some(v.to_string())]).collect::<Vec<_>>();
    h.db(DbEvent::StepRows { id, index: 0, columns: Some(cols), rows: page(1, 10), more: true });
    assert_eq!(outcomes(&h)[0], Rows { count: 10, more: true });
    h.db(DbEvent::StepRows { id, index: 0, columns: None, rows: page(11, 10), more: true });
    h.db(DbEvent::StepRows { id, index: 0, columns: None, rows: page(21, 5), more: false });
    h.db(DbEvent::Finished { id, index: 0, outcome: Outcome::Command("SELECT".into()), elapsed: MS });
    assert_eq!(outcomes(&h)[0], Rows { count: 25, more: false }, "the count stays after it finished");
    let steps = &h.app.tab().exec.steps;
    assert_eq!(steps.keys().copied().collect::<Vec<_>>(), [0]);
    assert_eq!(steps[&0].rs.rows.len(), 25);
    assert!(!steps[&0].rs.more);
    // Rows of another run are not taken.
    h.db(DbEvent::StepRows { id: id + 7, index: 0, columns: None, rows: page(26, 5), more: false });
    assert_eq!(h.app.tab().exec.steps[&0].rs.rows.len(), 25);
    h.db(DbEvent::Failed { id, error: DbError::from("ERROR: x"), cancelled: false });
    // The run failed: its Messages are shown; the first statement's rows are its result tab.
    assert_eq!(h.app.tab().exec.view, ResultView::Messages);
    assert_eq!(h.app.tab().result_tabs(), [0]);
    h.key(KeyCode::Esc);
    h.ctrl('e');
    // A new run keeps them until it delivers rows of its own ...
    assert_eq!(h.app.tab().result_tabs(), [0], "kept while the new run has no rows");
    assert!(h.app.tab().exec.kept_log.is_some());
    // ... which replace them.
    let id2 = h.app.tab().exec.query_id;
    let cols = vec![meta("a", "int4", true, false)];
    h.db(DbEvent::Page { id: id2, columns: Some(cols), rows: page(1, 2), more: false, elapsed: MS });
    assert!(h.app.tab().exec.steps.is_empty(), "the earlier run's rows are gone");
    assert_eq!(h.app.tab().result_tabs(), [0]);
    assert!(h.app.tab().exec.kept_log.is_none());
    assert!(matches!(&h.app.tab().results, Results::Rows(rs) if rs.rows.len() == 2));
}

/// An aborted transaction (a statement in it failed) says so in words until it ends: only
/// ROLLBACK works there.
#[test]
fn an_aborted_transaction_says_rollback_is_required() {
    for (lang, open, aborted) in [
        (Lang::En, "TX open", "TX aborted — ROLLBACK required"),
        (Lang::Ko, ko(datarig_core::i18n::Label::StatusTxOpen), ko(datarig_core::i18n::Label::StatusTxAborted)),
    ] {
        let mut h = Harness::connected(lang);
        h.db(DbEvent::Block(true));
        h.db(DbEvent::TxOpen(true));
        assert!(h.status(160, 45).contains(open));
        h.db(DbEvent::TxAborted(true));
        let status = h.status(160, 45);
        assert!(status.contains(aborted) && !status.contains(open), "{status}");
        h.db(DbEvent::TxAborted(false));
        assert!(h.status(160, 45).contains(open), "repaired (ROLLBACK TO SAVEPOINT)");
        h.db(DbEvent::TxAborted(true));
        h.db(DbEvent::TxOpen(false));
        let status = h.status(160, 45);
        assert!(!status.contains(aborted) && !status.contains(open), "ended: {status}");
        assert!(!h.app.tab().exec.tx_aborted);
    }
}

// ── one result tab per row result, and Messages ─────────────────────────

/// `SELECT; UPDATE; SELECT` as one run, answered: two result tabs (statements 1 and 3), the
/// last one shown, then Messages.
fn run_two_results(lang: Lang) -> (Harness, u64) {
    let mut h = Harness::connected(lang);
    h.app.tab_mut().editor = Editor::new("SELECT a FROM t;\nUPDATE t SET a = a WHERE a > 0;\nSELECT b FROM u;");
    h.keys("ggvG$");
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    assert_eq!(h.app.tab().exec.run.len(), 3, "one run of three statements");
    let page = |from: u64, n: u64| (from..from + n).map(|v| vec![Some(v.to_string())]).collect::<Vec<_>>();
    h.db(DbEvent::Started { id, index: 0 });
    h.db(DbEvent::StepRows {
        id,
        index: 0,
        columns: Some(vec![meta("a", "int4", true, false)]),
        rows: page(1, 3),
        more: false,
    });
    h.db(DbEvent::Finished { id, index: 0, outcome: Outcome::Command("SELECT".into()), elapsed: MS });
    h.db(DbEvent::Started { id, index: 1 });
    h.db(DbEvent::Finished { id, index: 1, outcome: Outcome::Affected(3), elapsed: MS });
    h.db(DbEvent::Started { id, index: 2 });
    let cols = Some(vec![meta("b", "int4", true, false)]);
    h.db(DbEvent::Page { id, columns: cols, rows: page(100, 7), more: false, elapsed: MS });
    (h, id)
}

fn shown_first_cell(h: &Harness) -> Option<String> {
    match &h.app.tab().results {
        Results::Rows(rs) => match rs.cell(0, 0) {
            datarig_tui::widgets::grid::CellRef::Here(c) => c.clone(),
            _ => None,
        },
        _ => None,
    }
}

#[test]
fn every_row_result_of_a_run_has_a_tab_and_messages_list_the_run() {
    let (mut h, _) = run_two_results(Lang::En);
    assert_eq!(h.app.tab().result_tabs(), [0, 2]);
    assert_eq!(h.app.tab().exec.view, ResultView::Rows);
    assert_eq!(shown_first_cell(&h).as_deref(), Some("100"), "the last statement's rows are shown");
    let screen = h.screen(160, 45);
    let strip = screen.lines().nth(h.app.layout.strip.y as usize).unwrap().to_string();
    assert!(strip.contains(" Result 1 ") && strip.contains("[Result 3]") && strip.contains(" Messages "), "{strip}");
    insta::assert_snapshot!("result_tabs_en_160x45", h.draw(160, 45).backend());
    // H and L switch; each result keeps its own place in the grid.
    h.key(KeyCode::Tab); // results
    h.keys("j");
    h.keys("H");
    assert_eq!(shown_first_cell(&h).as_deref(), Some("1"));
    assert_eq!(h.app.tab().grid.row, 0, "its own cursor");
    h.keys("L");
    assert_eq!(shown_first_cell(&h).as_deref(), Some("100"));
    assert_eq!(h.app.tab().grid.row, 1, "the cursor it had");
    // Then Messages: every statement, what it did; no inspector, nothing to copy there.
    h.keys("L");
    assert_eq!(h.app.tab().exec.view, ResultView::Messages);
    let screen = h.screen(160, 45);
    for want in [
        "1  SELECT a FROM t",
        "3 rows · ",
        "2  UPDATE t SET a = a WHERE a > 0",
        "3 rows affected · ",
        "3  SELECT b FROM u",
        "7 rows · ",
    ] {
        assert!(screen.contains(want), "{want}: {screen}");
    }
    assert!(!h.app.inspector_shown());
    let spec = datarig_tui::app::action::by_id("results.copy.all.csv").unwrap();
    assert!(!(spec.when)(&h.app), "nothing to copy in Messages");
    insta::assert_snapshot!("result_tabs_messages_en_80x24", h.draw(80, 24).backend());
    // A click on a result tab of the strip shows it.
    h.draw(160, 45);
    let (x0, ..) = h.app.strip_hits().into_iter().find(|(_, _, i, _)| *i == Some(0)).unwrap();
    h.mouse(
        ratatui::crossterm::event::MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left),
        x0 + 1,
        h.app.layout.strip.y,
    );
    assert_eq!(shown_first_cell(&h).as_deref(), Some("1"));
    // Wrapping around: from the first result back to Messages.
    h.keys("H");
    assert_eq!(h.app.tab().exec.view, ResultView::Messages);
    // Korean at 80 columns, from the catalog: every line whole, both result tabs and Messages
    // with the same labels as in English, the shown one in brackets.
    let mut ko = run_two_results(Lang::Ko).0;
    let i18n = datarig_core::i18n::I18n::new(Lang::Ko);
    let result = |n: &str| i18n.msg(&datarig_core::i18n::Msg::ResultsTabResult { n: n.into() }).to_string();
    let messages = i18n.label(datarig_core::i18n::Label::ResultsTabMessages).to_string();
    let screen = ko.screen(80, 24);
    assert!(screen.lines().all(|l| datarig_tui::text::width(l) == 80), "{screen}");
    let strip = screen.lines().nth(ko.app.layout.strip.y as usize).unwrap().to_string();
    assert!(
        strip.contains(&format!(" {} ", result("1")))
            && strip.contains(&format!("[{}]", result("3")))
            && strip.contains(&format!(" {messages} ")),
        "{strip}"
    );
    let title = i18n.msg(&datarig_core::i18n::Msg::PaneResultsRows { count: 7 }).to_string();
    assert!(screen.lines().nth(ko.app.layout.results.y as usize).unwrap().contains(&title), "{screen}");
    assert!(screen.contains("1 │  100") && screen.contains("7 │  106"), "{screen}");
}

/// A page that arrives while another result tab is shown goes to the run's last statement.
#[test]
fn pages_go_to_the_last_statement_wherever_it_is() {
    let mut h = Harness::connected(Lang::En);
    h.app.tab_mut().editor = Editor::new("SELECT a FROM t;\nSELECT b FROM u;");
    h.keys("ggvG$");
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let page = |from: u64, n: u64| (from..from + n).map(|v| vec![Some(v.to_string())]).collect::<Vec<_>>();
    h.db(DbEvent::Started { id, index: 0 });
    h.db(DbEvent::StepRows {
        id,
        index: 0,
        columns: Some(vec![meta("a", "int4", true, false)]),
        rows: page(1, 2),
        more: false,
    });
    h.db(DbEvent::Finished { id, index: 0, outcome: Outcome::Command("SELECT".into()), elapsed: MS });
    h.db(DbEvent::Started { id, index: 1 });
    h.db(DbEvent::Page {
        id,
        columns: Some(vec![meta("b", "int4", true, false)]),
        rows: page(10, 500),
        more: true,
        elapsed: MS,
    });
    h.key(KeyCode::Tab);
    h.keys("H");
    assert_eq!(shown_first_cell(&h).as_deref(), Some("1"));
    h.db(DbEvent::Page { id, columns: None, rows: page(510, 500), more: false, elapsed: MS });
    assert!(matches!(&h.app.tab().results, Results::Rows(rs) if rs.rows.len() == 2), "the shown one is untouched");
    assert_eq!(h.app.tab().exec.steps[&1].rs.rows.len(), 1000, "the last statement got the page");
}

/// A run without rows (a COMMIT) keeps the rows of the run before on screen, says they are from
/// an earlier run and shows its own Messages; the next run with rows replaces them.
#[test]
fn a_run_without_rows_keeps_the_rows_of_the_run_before() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    h.db(DbEvent::Page {
        id,
        columns: Some(vec![meta("a", "int4", true, false)]),
        rows: vec![vec![Some("7".into())]],
        more: false,
        elapsed: MS,
    });
    h.app.tab_mut().editor = Editor::new("COMMIT");
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    assert_eq!(shown_first_cell(&h).as_deref(), Some("7"), "kept while it runs");
    h.db(DbEvent::Done { id, outcome: Outcome::Command("COMMIT".into()), elapsed: MS });
    assert_eq!(h.app.tab().exec.view, ResultView::Messages);
    assert_eq!(h.app.tab().result_tabs(), [0]);
    assert_eq!(h.app.tab().shown_sql(), "SELECT * FROM shop.users WHERE id <= 8", "the rows' statement");
    let screen = h.screen(160, 45);
    assert!(
        screen.contains("· earlier run") && screen.contains("1  COMMIT") && screen.contains("COMMIT · "),
        "{screen}"
    );
    insta::assert_snapshot!("earlier_run_en_160x45", h.draw(160, 45).backend());
    h.key(KeyCode::Tab);
    h.keys("L");
    assert_eq!(shown_first_cell(&h).as_deref(), Some("7"));
    // A new run with rows replaces them.
    h.app.tab_mut().editor = Editor::new("SELECT 2");
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    h.db(DbEvent::Page {
        id,
        columns: Some(vec![meta("b", "int4", true, false)]),
        rows: vec![vec![Some("2".into())]],
        more: false,
        elapsed: MS,
    });
    assert_eq!(shown_first_cell(&h).as_deref(), Some("2"));
    assert!(h.app.tab().exec.kept_log.is_none());
    assert!(!h.screen(160, 45).contains("earlier run"));
}

/// A run of `n` `SELECT`s read inside the user's transaction, answered: `n` result tabs, the
/// last one shown, then Messages, and the strip's transaction label.
fn run_selects(lang: Lang, n: usize) -> Harness {
    let mut h = Harness::connected(lang);
    let text: Vec<String> = (1..=n).map(|i| format!("SELECT {i} AS c{i};")).collect();
    h.app.tab_mut().editor = Editor::new(&text.join("\n"));
    h.keys("ggvG$");
    h.db(DbEvent::Block(true));
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    for index in 0..n {
        h.db(DbEvent::Started { id, index });
        let columns = Some(vec![meta(&format!("c{}", index + 1), "int4", true, false)]);
        let rows = vec![vec![Some((index + 1).to_string())]];
        if index + 1 < n {
            h.db(DbEvent::StepRows { id, index, columns, rows, more: false });
            h.db(DbEvent::Finished { id, index, outcome: Outcome::Command("SELECT".into()), elapsed: MS });
        } else {
            h.db(DbEvent::Page { id, columns, rows, more: false, elapsed: MS });
        }
    }
    h
}

/// At 80 columns the strip keeps the shown result tab and Messages whole, in both languages
/// with the same labels ("Result n"), leaving out others with `…` (`H`/`L` reach them); a
/// click on a shown tab still shows it.
#[test]
fn a_narrow_strip_keeps_the_shown_tab_and_messages() {
    use datarig_core::i18n::{I18n, Label, Msg};
    for lang in [Lang::En, Lang::Ko] {
        let i18n = I18n::new(lang);
        let result = |n: usize| i18n.msg(&Msg::ResultsTabResult { n: n.to_string() }).to_string();
        let messages = i18n.label(Label::ResultsTabMessages).to_string();
        let tx = i18n.label(Label::ResultsTxOpen).to_string();
        let mut h = run_selects(lang, 6);
        h.app.detail.visible = false;
        let screen = h.screen(80, 24);
        let strip = screen.lines().nth(h.app.layout.strip.y as usize).unwrap().to_string();
        assert!(strip.contains(&format!("[{}]", result(6))), "{lang:?}: {strip}");
        assert!(strip.contains(&format!(" {messages} ")), "{lang:?}: {strip}");
        assert!(strip.contains(&tx), "{lang:?}: {strip}");
        assert!(strip.contains('…'), "{lang:?}: some are left out: {strip}");
        assert!(!strip.contains(&format!(" {} ", result(1))), "{lang:?}: {strip}");
        // H goes to the one before; the strip follows it.
        h.key(KeyCode::Tab); // results
        h.keys("HHH");
        let screen = h.screen(80, 24);
        let strip = screen.lines().nth(h.app.layout.strip.y as usize).unwrap().to_string();
        assert!(strip.contains(&format!("[{}]", result(3))) && strip.contains(&format!(" {messages} ")), "{strip}");
        // Every shown tab is where the strip says (for the mouse).
        h.draw(80, 24);
        let hits = h.app.strip_hits();
        assert!(
            hits.iter().any(|(_, _, i, _)| *i == Some(2)) && hits.iter().any(|(_, _, i, _)| i.is_none()),
            "{hits:?}"
        );
        // With the inspector next to the grid (a narrower strip, the transaction first) the
        // shown one still shows.
        h.app.detail.visible = true;
        let screen = h.screen(80, 24);
        let strip = screen.lines().nth(h.app.layout.strip.y as usize).unwrap().to_string();
        assert!(strip.contains("[3]") && strip.contains(&tx), "{lang:?}: {strip}");
    }
}

/// Move the pointer to (x, y); `true` when the move needs a frame.
fn hover(h: &mut Harness, x: u16, y: u16) -> bool {
    h.mouse(ratatui::crossterm::event::MouseEventKind::Moved, x, y);
    !h.app.take_idle_event()
}

/// The result tab strip's entries light up under the pointer.
#[test]
fn the_pointer_lights_up_a_result_tab() {
    let (mut h, _) = run_two_results(Lang::En);
    h.draw(160, 45);
    let hits = h.app.strip_hits();
    assert!(hits.len() >= 2, "{hits:?}");
    let (a, b, i, view) = hits[1];
    let y = h.app.layout.strip.y;
    assert!(hover(&mut h, a, y));
    assert_eq!(h.app.pointer_on, Some(datarig_tui::app::hover::PointerOn::Strip(i, view)));
    assert!(!hover(&mut h, b - 1, y), "along the same entry: no frame");
    let t = h.draw(160, 45);
    assert_eq!(t.backend().buffer()[(a + 1, y)].bg, datarig_tui::theme::DARK.selection.bg.unwrap());
    assert!(hover(&mut h, b + 3, y + 4), "off the strip");
    let t = h.draw(160, 45);
    assert_ne!(t.backend().buffer()[(a + 1, y)].bg, datarig_tui::theme::DARK.selection.bg.unwrap());
}
