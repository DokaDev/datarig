//! The status line's summary of the grid's selected range through the real `App` event path:
//! numbers (sum, average, least, greatest, NULL), other values (distinct, NULL), the pending
//! values a copy takes, whole columns of a result the server has more of, and the rows read on
//! a worker (an answer for a selection that changed is dropped).

mod common;

use common::*;
use datarig_core::driver::DbEvent;
use datarig_core::edit::Value;
use datarig_i18n::Lang;
use datarig_tui::app::{Focus, Results};
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::time::Duration;

const W: u16 = 160;
const H: u16 = 45;

fn s(v: &str) -> Option<String> {
    Some(v.to_string())
}

/// Five rows of an id, an amount (numeric, one NULL) and a name (text, one NULL); the grid
/// focused, `more` rows on the server or not.
fn amounts(lang: Lang, more: bool) -> Harness {
    let mut h = Harness::connected(lang);
    h.app.run(vec!["SELECT id, amount, name FROM t".to_string()]);
    let id = h.app.tab().exec.query_id;
    let columns = vec![
        meta("id", "int8", true, false),
        meta("amount", "numeric", true, false),
        meta("name", "text", false, false),
    ];
    let rows = vec![
        vec![s("1"), s("10.50"), s("a")],
        vec![s("2"), s("2.25"), s("b")],
        vec![s("3"), None, s("a")],
        vec![s("4"), s("1000"), None],
        vec![s("5"), s("0.25"), s("c")],
    ];
    h.db(DbEvent::Page { id, columns: Some(columns), rows, more, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
    h
}

/// The status line once the summary's job answered.
fn summary(h: &mut Harness) -> String {
    h.app.pump_grid_search(Duration::from_secs(10));
    h.status(W, H)
}

#[test]
fn numbers_are_summed_and_other_values_counted() {
    let mut h = amounts(Lang::En, false);
    // No range: no summary.
    h.keys("l");
    assert_eq!(h.app.selection_summary(), None);
    // The amounts of every row: the cursor on the amount column, then down.
    h.keys("vjjjj");
    let line = summary(&mut h);
    assert!(line.contains("5 rows × 1 column · sum 1,013.00 · avg 253.25 · min 0.25 · max 1,000 · NULL 1"), "{line}");
    // Narrow: the parts that matter least go first (least and greatest, then NULL).
    let forms = h.app.selection_summary_forms();
    assert_eq!(
        forms[1..],
        [
            "5 rows × 1 column · sum 1,013.00 · avg 253.25 · NULL 1",
            "5 rows × 1 column · sum 1,013.00 · avg 253.25",
            "5 rows × 1 column · sum 1,013.00",
        ]
    );
    let narrow = h.status(80, 24);
    assert!(narrow.contains("5 rows × 1 column · sum 1,013.00") && !narrow.contains("…"), "{narrow}");
    // With the ids: still numbers.
    h.keys("h");
    let line = summary(&mut h);
    assert!(line.contains("5 rows × 2 columns · sum 1,028.00 · avg 114.222222 · min 0.25 · max 1,000"), "{line}");
    // The amounts and the names (from the anchor's column): distinct values.
    h.keys("ll");
    let line = summary(&mut h);
    assert!(line.contains("5 rows × 2 columns · 7 distinct values · NULL 2"), "{line}");
    // Esc drops the range, and the summary with it.
    h.key(KeyCode::Esc);
    assert_eq!(h.app.selection_summary(), None);
    assert!(!h.status(W, H).contains("distinct"));
    // Not while the grid has no focus.
    h.keys("vj");
    assert!(h.app.selection_summary().is_some());
    h.app.focus = Focus::Editor;
    h.key(KeyCode::Esc);
    assert_eq!(h.app.selection_summary(), None);
}

#[test]
fn the_values_shown_are_summed_as_a_copy_takes_them() {
    let mut h = amounts(Lang::En, false);
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        let row = vec![s("4"), s("1000"), None];
        rs.edit.batch.set(&[(3, 1)], &Value::Text("1".into()), |_| row.clone());
        let row = vec![s("5"), s("0.25"), s("c")];
        rs.edit.batch.set(&[(4, 1)], &Value::Default, |_| row.clone());
    }
    h.keys("lvjjjj");
    let line = summary(&mut h);
    assert!(
        line.contains(
            "5 rows × 1 column · sum 13.75 · avg 4.583333 · min 1 · max 10.50 · NULL 1 · 1 with no value yet"
        ),
        "{line}"
    );
    // A change made while the range stays: summed again at the next input.
    let jobs = h.app.grid_summary_jobs();
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        let row = vec![s("1"), s("10.50"), s("a")];
        rs.edit.batch.set(&[(0, 1)], &Value::Text("0.50".into()), |_| row.clone());
    }
    h.keys("zz");
    assert_eq!(h.app.grid_summary_jobs(), jobs + 1);
    let line = summary(&mut h);
    assert!(line.contains("sum 3.75 · avg 1.25 · min 0.50 · max 2.25"), "{line}");
}

#[test]
fn whole_columns_say_when_the_server_has_more_and_korean_reads_from_the_catalog() {
    let mut h = amounts(Lang::En, true);
    h.draw(W, H);
    let g = h.app.tab().grid.clone();
    let (x, y) = (g.hit_cols[1].0 + 1, g.header_y);
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    let line = summary(&mut h);
    assert!(line.contains("5 rows × 1 column · sum 1,013.00") && line.contains("fetched rows only"), "{line}");
    let mut h = amounts(Lang::Ko, false);
    h.keys("lvj");
    let line = summary(&mut h);
    let want = "2\u{D589} \u{D7} 1\u{C5F4} \u{B7} \u{D569}\u{ACC4} 12.75 \u{B7} \u{D3C9}\u{ADE0} 6.375";
    assert!(line.contains(want), "{line}");
}

/// A key straight to the app: the answers of the jobs it starts stay out until pumped.
fn press(h: &mut Harness, c: char) {
    use ratatui::crossterm::event::{Event, KeyEvent, KeyModifiers};
    h.app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
}

#[test]
fn the_rows_are_read_on_a_worker_and_a_changed_selection_drops_the_answer() {
    let mut h = amounts(Lang::En, false);
    h.keys("l");
    let started = h.app.grid_summary_jobs();
    press(&mut h, 'v');
    press(&mut h, 'j');
    // Before an answer: the size, and that it is being summed (the input never waits).
    let line = h.status(W, H);
    assert!(line.contains("2 rows × 1 column · summing…"), "{line}");
    // The selection grew at its end: the job counting its first row goes on, the next row is
    // added after it.
    assert_eq!(h.app.grid_summary_jobs(), started + 1, "the job goes on");
    press(&mut h, 'j');
    let line = summary(&mut h);
    assert!(line.contains("3 rows × 1 column · sum 12.75"), "{line}");
    assert_eq!(h.app.grid_summary_rows_read(), 3, "each row read once");
    // A selection that is not the one before grown (one row up): counted again, the answer of
    // the job before dropped.
    let jobs = h.app.grid_summary_jobs();
    press(&mut h, 'k');
    press(&mut h, 'k');
    assert_eq!(h.app.grid_summary_jobs(), jobs + 2, "one job per selection, the one before stopped");
    let line = summary(&mut h);
    assert!(line.contains("1 row × 1 column · sum 10.50"), "{line}");
    // Input that leaves the selection as it is starts no job.
    let jobs = h.app.grid_summary_jobs();
    h.draw(W, H);
    h.keys("zz");
    assert_eq!(h.app.grid_summary_jobs(), jobs);
}

/// A selection whose rows cannot be read from the spill file: the status line says why, not a
/// summary in its place.
#[test]
fn rows_that_cannot_be_read_are_said_not_summed() {
    use datarig_core::paths::Paths;
    let state = std::env::temp_dir().join(format!("datarig-flows-grid-summary-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    let mut cfg = test_db_config();
    cfg.result_window_rows = 1_000;
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.app.set_paths(Paths { data: None, state: Some(state.clone()) });
    h.db(DbEvent::Connected);
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let mut columns = Some(vec![meta("n", "int8", true, false)]);
    for p in 0..10 {
        let rows = (p * 500..p * 500 + 500).map(|i| vec![Some(i.to_string())]).collect();
        let page = DbEvent::Page { id, columns: columns.take(), rows, more: false, elapsed: Duration::from_millis(1) };
        h.tab_db(0, page);
    }
    let Results::Rows(rs) = &h.app.tab().results else { panic!("rows") };
    assert!(rs.rows.spilled() > 0);
    let path = rs.rows.spill_path().expect("spilled").to_path_buf();
    std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(0).unwrap();
    h.key(KeyCode::Tab);
    h.keys("vG");
    h.app.pump_grid_search(Duration::from_secs(10));
    // Past the "End of the page" notice `G` flashed.
    h.app.transient = None;
    let line = h.status(W, H);
    assert!(line.contains("Could not read the result's rows"), "{line}");
    assert!(!line.contains(" × "), "no summary over it: {line}");
    assert_eq!(h.app.selection_summary(), None);
    let _ = std::fs::remove_dir_all(&state);
}

/// Whole columns while every row is fetched, page after page: each page's rows are added to
/// what was counted (never read again), the summary so far stays on the status line meanwhile,
/// and the sum is exact.
#[test]
fn whole_columns_add_each_streamed_page_to_the_count_so_far() {
    let mut h = Harness::connected(Lang::En);
    let _clip = FakeClipboard::attach(&mut h, false, &[]);
    h.app.run(vec!["SELECT amount FROM t".to_string()]);
    let id = h.app.tab().exec.query_id;
    let page = |range: std::ops::Range<i64>| -> Vec<Vec<Option<String>>> {
        range.map(|i| vec![if i % 100 == 7 { None } else { Some(format!("{i}.25")) }]).collect()
    };
    let columns = Some(vec![meta("amount", "numeric", true, false)]);
    h.db(DbEvent::Page { id, columns, rows: page(0..500), more: true, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
    h.draw(W, H);
    let g = h.app.tab().grid.clone();
    let (x, y) = (g.hit_cols[0].0 + 1, g.header_y);
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    let line = summary(&mut h);
    // 0..500 without the five NULLs (7, 107, …): 124,750 − 1,035 + 495 × 0.25.
    assert!(line.contains("500 rows × 1 column · sum 123,838.75"), "{line}");
    assert!(line.contains("NULL 5") && line.contains("fetched rows only"), "{line}");
    let (jobs, read) = (h.app.grid_summary_jobs(), h.app.grid_summary_rows_read());
    // Every row fetched, then copied: the column stays selected while the pages come.
    h.keys("  ");
    h.type_text("fetch every");
    h.key(KeyCode::Enter);
    h.keys("c");
    h.keys("y");
    h.db(DbEvent::Page { id, columns: None, rows: page(500..1000), more: true, elapsed: Duration::ZERO });
    // Before the new rows' job answers: the sum so far, said to be summing.
    let line = h.status(W, H);
    assert!(line.contains("1,000 rows × 1 column · sum 123,838.75") && line.contains("summing…"), "{line}");
    // Every form that shows the sum so far says it is still summing.
    let forms = h.app.selection_summary_forms();
    assert!(forms.iter().all(|f| !f.contains("sum ") || f.contains("summing…")), "{forms:?}");
    let line = summary(&mut h);
    // 0..1000 without the ten NULLs: 499,500 − 4,570 + 990 × 0.25.
    assert!(line.contains("1,000 rows × 1 column · sum 495,177.50"), "{line}");
    assert!(line.contains("NULL 10") && line.contains("fetched rows only"), "{line}");
    h.db(DbEvent::Page { id, columns: None, rows: page(1000..1500), more: true, elapsed: Duration::ZERO });
    let line = summary(&mut h);
    // 0..1500 without the fifteen NULLs: 1,124,250 − 10,605 + 1,485 × 0.25.
    assert!(line.contains("1,500 rows × 1 column · sum 1,114,016.25 · avg"), "{line}");
    assert_eq!(h.app.grid_summary_rows_read() - read, 1000, "only the new rows were read");
    assert_eq!(h.app.grid_summary_jobs() - jobs, 2, "a job per page");
}
