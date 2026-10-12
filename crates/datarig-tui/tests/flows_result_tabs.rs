//! The results pane's tab strip from the first frame on: Result, Chart and Messages in a
//! console and in a table tab (a Plan next to the result for an `EXPLAIN`). The Chart tab is
//! only named until it is viewed (nothing is read for it before), and says why when there is
//! nothing to draw. `H`/`L`, a click and `c` move between the tabs.

mod common;

use common::*;
use datarig_core::driver::{ColumnMeta, DbCommand, DbEvent, Outcome, PagingMode};
use datarig_core::i18n::{I18n, Label, Lang, Msg};
use datarig_tui::app::Focus;
use datarig_tui::app::hover::PointerOn;
use datarig_tui::app::tabs::ResultView;
use datarig_tui::widgets::chart::take_work;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::time::Duration;

const W: u16 = 120;
const H: u16 = 30;

fn lang_tag(l: Lang) -> &'static str {
    if l == Lang::En { "en" } else { "ko" }
}

fn s(v: &str) -> Option<String> {
    Some(v.to_string())
}

/// Run `sql` in `h`'s console and answer it with `cols` and `rows`.
fn answer(h: &mut Harness, sql: &str, cols: Vec<ColumnMeta>, rows: Vec<Vec<Option<String>>>) {
    h.sent();
    h.app.run(vec![sql.into()]);
    let sent = h.sent();
    let [DbCommand::Execute { id, paging: PagingMode::NoHold, .. }] = &sent[..] else { panic!("one run: {sent:?}") };
    h.db(DbEvent::Page { id: *id, columns: Some(cols), rows, more: false, elapsed: Duration::from_millis(3) });
}

/// Orders per day: a date and a number, `n` rows.
fn per_day(h: &mut Harness, n: usize) {
    let rows = (1..=n).map(|d| vec![s(&format!("2026-09-{d:02}")), s(&(d * 7).to_string())]).collect();
    let cols = vec![meta("day", "date", false, false), meta("orders", "int8", true, false)];
    answer(h, "SELECT day, orders FROM daily", cols, rows);
}

/// Run a statement that returns no rows (`SET`).
fn set(h: &mut Harness) {
    h.sent();
    h.app.run(vec!["SET a = 1".into()]);
    let sent = h.sent();
    let [DbCommand::Execute { id, .. }] = &sent[..] else { panic!("one run: {sent:?}") };
    h.db(DbEvent::Done { id: *id, outcome: Outcome::Command("SET".into()), elapsed: Duration::from_millis(1) });
}

/// The strip's line as drawn `W`×`H`.
fn strip(h: &mut Harness) -> String {
    let screen = h.screen(W, H);
    assert_eq!(h.app.layout.strip.height, 1, "the strip is shown:\n{screen}");
    screen.lines().nth(h.app.layout.strip.y as usize).unwrap().to_string()
}

fn view(h: &Harness) -> ResultView {
    h.app.tab().exec.view
}

/// `[Result 1]  Chart  Messages` in `lang`: the shown one in brackets, two blanks between any
/// two of them.
fn tabs(lang: Lang, result: &str, on: ResultView) -> String {
    let i18n = I18n::new(lang);
    let label = |l: &str, v: ResultView| if v == on { format!("[{l}]") } else { l.to_string() };
    let chart = i18n.label(Label::ResultsTabChart).to_string();
    let messages = i18n.label(Label::ResultsTabMessages).to_string();
    format!(
        "{}  {}  {}",
        label(result, ResultView::Rows),
        label(&chart, ResultView::Chart),
        label(&messages, ResultView::Messages)
    )
}

/// From the editor, open `shop.users` in the explorer (the schema's objects answered).
fn open_users(h: &mut Harness) {
    h.key(KeyCode::BackTab);
    assert_eq!(h.app.focus, Focus::Tree);
    h.keys("jjjj"); // profile -> its database -> analytics -> public -> shop
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects {
        schema: "shop".into(),
        result: Ok((vec!["orders".to_string(), "users".to_string()], Vec::new()).into()),
    });
    h.keys("jjj"); // Tables, orders, users
    h.key(KeyCode::Enter);
    assert!(h.app.tab().is_table());
}

/// A console's first result: Result, Chart and Messages, before any chart was asked for.
#[test]
fn a_console_shows_result_chart_and_messages_from_its_first_result() {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = Harness::connected(lang);
        per_day(&mut h, 5);
        h.app.focus = Focus::Results;
        let result = I18n::new(lang).msg(&Msg::ResultsTabResult { n: "1".into() }).to_string();
        let line = strip(&mut h);
        assert!(line.contains(&tabs(lang, &result, ResultView::Rows)), "{line}");
        assert!(line.contains("H/L"), "the keys that move between them: {line}");
        assert!(h.app.tab().exec.chart.is_none(), "named, not made");
        assert_screen!(format!("result_tabs_console_{}_{W}x{H}", lang_tag(lang)), lang, h.draw(W, H));
    }
}

/// A console's first run, before any result came: the pane shows with the three tabs, the
/// result tab named for what is coming; Messages and the Chart (nothing to draw yet) are there.
#[test]
fn a_running_first_query_shows_the_three_tabs() {
    for lang in [Lang::En, Lang::Ko] {
        let i18n = I18n::new(lang);
        let mut h = Harness::connected(lang);
        h.sent();
        h.app.run(vec!["SELECT pg_sleep(10)".into()]);
        assert!(h.app.tab().exec.running.is_some(), "still running");
        h.app.focus = Focus::Results;
        // The result tab says the run goes on, not how to start one.
        let screen = h.screen(W, H);
        assert!(screen.contains(&i18n.label(Label::ResultsRunning).to_string()), "{screen}");
        let empty = i18n.msg(&Msg::ResultsEmpty { run: h.app.run_key().into() }).to_string();
        assert!(!screen.contains(&empty), "{screen}");
        let pending = i18n.label(Label::ResultsTabRows).to_string();
        assert!(strip(&mut h).contains(&tabs(lang, &pending, ResultView::Rows)), "{}", strip(&mut h));
        // (No snapshot: the status bar counts the run's real time.)
        h.keys("L");
        assert_eq!(view(&h), ResultView::Chart);
        let screen = h.screen(W, H);
        assert!(screen.contains(&i18n.label(Label::ChartUnsuitableNoResult).to_string()), "{screen}");
        h.keys("L");
        assert_eq!(view(&h), ResultView::Messages);
        assert!(h.screen(W, H).contains("SELECT pg_sleep(10)"), "{}", h.screen(W, H));
        assert!(h.app.tab().exec.chart.is_none(), "nothing made for the chart");
    }
}

/// A table tab has the same tabs, while it loads too; its Messages list its query.
#[test]
fn a_table_tab_shows_result_chart_and_messages() {
    for lang in [Lang::En, Lang::Ko] {
        let i18n = I18n::new(lang);
        let mut h = Harness::connected(lang);
        open_users(&mut h);
        // Loading: one result tab for what is coming and saying so, the Chart says there is
        // nothing yet.
        let screen = h.screen(W, H);
        assert!(screen.contains(&i18n.label(Label::ResultsRunning).to_string()), "{screen}");
        let pending = i18n.label(Label::ResultsTabRows).to_string();
        assert!(strip(&mut h).contains(&tabs(lang, &pending, ResultView::Rows)), "{}", strip(&mut h));
        h.key(KeyCode::Tab);
        assert_eq!(h.app.focus, Focus::Results);
        h.keys("L");
        assert_eq!(view(&h), ResultView::Chart);
        let screen = h.screen(W, H);
        assert!(screen.contains(&i18n.label(Label::ChartUnsuitableNoResult).to_string()), "{screen}");
        assert!(h.app.tab().exec.chart.is_none());
        h.keys("H");
        // The rows: the Chart tab shown again after them (no chart yet).
        let (cols, rows) = edge_rows();
        let id = h.app.tab().exec.query_id;
        h.tab_db(1, DbEvent::Page { id, columns: Some(cols), rows, more: false, elapsed: Duration::from_millis(3) });
        let result = i18n.msg(&Msg::ResultsTabResult { n: "1".into() }).to_string();
        assert!(strip(&mut h).contains(&tabs(lang, &result, ResultView::Rows)), "{}", strip(&mut h));
        assert!(h.app.tab().exec.chart.is_none(), "named, not made");
        assert_screen!(format!("result_tabs_table_{}_{W}x{H}", lang_tag(lang)), lang, h.draw(W, H));
        // `H`/`L` go round them; the Messages list the table's query.
        h.keys("L");
        assert_eq!(view(&h), ResultView::Chart);
        h.keys("L");
        assert_eq!(view(&h), ResultView::Messages);
        assert!(h.screen(W, H).contains(r#"SELECT * FROM "shop"."users""#), "{}", h.screen(W, H));
        h.keys("L");
        assert_eq!(view(&h), ResultView::Rows);
        h.keys("H");
        assert_eq!(view(&h), ResultView::Messages);
        // `c` shows the chart and the rows again.
        h.keys("H");
        assert_eq!(view(&h), ResultView::Chart);
        h.keys("c");
        assert_eq!(view(&h), ResultView::Rows);
        h.keys("c");
        assert_eq!(view(&h), ResultView::Chart);
        assert!(h.app.tab().exec.chart.is_some(), "made once viewed");
        // A reload keeps the Chart tab: it draws the new rows.
        h.ctrl('e');
        let id = h.app.tab().exec.query_id;
        let (cols, rows) = edge_rows();
        h.tab_db(1, DbEvent::Page { id, columns: Some(cols), rows, more: false, elapsed: Duration::from_millis(3) });
        assert_eq!(view(&h), ResultView::Chart);
    }
}

/// Showing the Chart tab's name reads no row: the chart is made, and its rows read, only when
/// the tab is viewed.
#[test]
fn the_chart_is_made_only_when_its_tab_is_viewed() {
    let mut h = Harness::connected(Lang::En);
    per_day(&mut h, 30);
    h.app.focus = Focus::Results;
    take_work();
    for keys in ["", "jj", "l", "n"] {
        h.keys(keys);
        h.draw(W, H);
    }
    // The pointer over the Chart tab only lights it.
    let (x0, ..) = h.app.strip_hits().into_iter().find(|(.., v)| *v == ResultView::Chart).expect("Chart in the strip");
    h.mouse(MouseEventKind::Moved, x0 + 1, h.app.layout.strip.y);
    assert_eq!(h.app.pointer_on, Some(PointerOn::Strip(None, ResultView::Chart)));
    h.draw(W, H);
    assert!(h.app.tab().exec.chart.is_none(), "not made");
    assert_eq!(take_work(), 0, "no row read for a chart");
    // Viewed: made, its rows read and drawn.
    h.mouse(MouseEventKind::Down(MouseButton::Left), x0 + 1, h.app.layout.strip.y);
    assert_eq!(view(&h), ResultView::Chart);
    h.draw(W, H);
    let made = h.app.tab().exec.chart.as_ref().expect("made once viewed");
    assert_eq!(made.model().map(|m| m.rows), Some(30));
    assert!(take_work() >= 30, "the rows were read");
}

/// The Chart tab says why it draws nothing, with the reasons the chart gives: no row result,
/// no rows, no numbers.
#[test]
fn the_chart_tab_says_why_it_has_nothing_to_draw() {
    for lang in [Lang::En, Lang::Ko] {
        let i18n = I18n::new(lang);
        let shows = |h: &mut Harness, l: Label| {
            let screen = h.screen(W, H);
            // The first sentence: a long reason wraps.
            let text = i18n.label(l).to_string();
            let first = text.split(". ").next().unwrap().to_string();
            assert!(screen.contains(&first), "{first}:\n{screen}");
        };
        // A run without rows: its Messages are shown; the Chart tab is one key away.
        let mut h = Harness::connected(lang);
        set(&mut h);
        h.app.focus = Focus::Results;
        assert_eq!(view(&h), ResultView::Messages);
        let pending = i18n.label(Label::ResultsTabRows).to_string();
        assert!(strip(&mut h).contains(&tabs(lang, &pending, ResultView::Messages)), "{}", strip(&mut h));
        h.keys("H");
        assert_eq!(view(&h), ResultView::Chart);
        shows(&mut h, Label::ChartUnsuitableNoResult);
        assert_screen!(format!("chart_tab_no_result_{}_{W}x{H}", lang_tag(lang)), lang, h.draw(W, H));
        // `c` there shows the result tab, and `c` the Chart tab again.
        h.keys("c");
        assert_eq!(view(&h), ResultView::Rows);
        h.keys("c");
        assert_eq!(view(&h), ResultView::Chart);
        // No rows.
        let cols = vec![meta("k", "text", false, false), meta("n", "int4", true, false)];
        answer(&mut h, "SELECT k, n FROM t WHERE false", cols, vec![]);
        assert_eq!(view(&h), ResultView::Chart, "the Chart tab stays");
        shows(&mut h, Label::ChartUnsuitableNoRows);
        assert_screen!(format!("chart_tab_no_rows_{}_{W}x{H}", lang_tag(lang)), lang, h.draw(W, H));
        // No column of numbers.
        let rows = vec![vec![s("a")], vec![s("b")]];
        answer(&mut h, "SELECT name FROM t", vec![meta("name", "text", false, false)], rows);
        shows(&mut h, Label::ChartUnsuitableNoNumber);
    }
}

/// `H`/`L` go round the tabs in the strip's order, both ways, and `c` keeps switching between
/// the rows and the chart.
#[test]
fn h_and_l_go_round_the_tabs_and_c_switches_to_the_chart() {
    let mut h = Harness::connected(Lang::En);
    per_day(&mut h, 5);
    h.app.focus = Focus::Results;
    for (key, want) in [
        ("L", ResultView::Chart),
        ("L", ResultView::Messages),
        ("L", ResultView::Rows),
        ("H", ResultView::Messages),
        ("H", ResultView::Chart),
        ("H", ResultView::Rows),
    ] {
        h.keys(key);
        assert_eq!(view(&h), want, "{key}");
        h.draw(W, H);
    }
    h.keys("c");
    assert_eq!(view(&h), ResultView::Chart);
    let line = strip(&mut h);
    assert!(line.contains(&tabs(Lang::En, "Result 1", ResultView::Chart)), "{line}");
    h.keys("c");
    assert_eq!(view(&h), ResultView::Rows);
}

/// A run of several statements while the Chart tab is shown: its first rows replace the shown
/// result's, so until the run's answer comes the Chart tab has no row result, says so, and the
/// chart of the rows before is neither drawn nor acted on (copy, the point's row, a click).
#[test]
fn a_run_of_several_statements_leaves_no_stale_chart() {
    let mut h = Harness::connected(Lang::En);
    per_day(&mut h, 5);
    h.app.focus = Focus::Results;
    h.keys("c");
    h.draw(W, H);
    assert_eq!(h.app.tab().exec.chart.as_ref().and_then(|c| c.model()).map(|m| m.rows), Some(5));
    let plot = h.app.tab().exec.chart.as_ref().unwrap().plot;
    // Two statements: the first one's rows come, the second one still runs.
    h.sent();
    h.app.run(vec!["SELECT 1 AS a".into(), "SELECT 2 AS b".into()]);
    let id = h.app.tab().exec.query_id;
    let columns = Some(vec![meta("a", "int4", true, false)]);
    h.db(DbEvent::StepRows { id, index: 0, columns, rows: vec![vec![s("1")]], more: false });
    assert_eq!(view(&h), ResultView::Chart);
    let screen = h.screen(W, H);
    assert!(screen.contains("Nothing to chart: there is no row result to draw."), "{screen}");
    assert!(!screen.contains("2026-09") && !screen.contains("fetched rows"), "the chart before: {screen}");
    // Nothing acts on the chart before.
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("y");
    assert_eq!(clip.last(), None, "nothing copied");
    h.key(KeyCode::Enter);
    assert_eq!(view(&h), ResultView::Chart, "no row of the rows before");
    let cursor = h.app.tab().exec.chart.as_ref().map(|c| c.cursor);
    h.mouse(MouseEventKind::Down(MouseButton::Left), plot.x + plot.width - 2, plot.y + plot.height / 2);
    h.mouse(MouseEventKind::Up(MouseButton::Left), plot.x + plot.width - 2, plot.y + plot.height / 2);
    assert_eq!(h.app.tab().exec.chart.as_ref().map(|c| c.cursor), cursor);
    h.keys("  ");
    assert!(!h.menu_labels().iter().any(|l| l.contains("Chart kind")), "{:?}", h.menu_labels());
    h.key(KeyCode::Esc);
    // Nor the keys on it (a column list, the cursor, the kind) or the wheel.
    let spec = h.app.tab().exec.chart.as_ref().map(|c| c.spec.clone());
    h.keys("x");
    assert_eq!(h.overlay_kind(), None, "no list of the columns before");
    h.keys("l1");
    h.mouse(MouseEventKind::ScrollDown, plot.x + 2, plot.y + 1);
    assert_eq!(h.app.tab().exec.chart.as_ref().map(|c| c.cursor), cursor);
    assert_eq!(h.app.tab().exec.chart.as_ref().map(|c| c.spec.clone()), spec);
    // The answer: the Chart tab draws it, with the columns chosen afresh.
    let columns = Some(vec![meta("k", "text", false, false), meta("n", "int4", true, false)]);
    let rows = vec![vec![s("x"), s("1")], vec![s("y"), s("2")]];
    h.db(DbEvent::Page { id, columns, rows, more: false, elapsed: Duration::from_millis(1) });
    h.draw(W, H);
    assert_eq!(h.app.tab().exec.chart.as_ref().and_then(|c| c.model()).map(|m| m.rows), Some(2));
}
