//! View as plan: the rows of a text `EXPLAIN` say under them that they can be viewed as a
//! plan; the key, the leader key and the action menu ask the statement that produced them (not
//! the editor's text) again with `FORMAT JSON`, its other options kept, through the run's own
//! checks, and the Plan tab opens. Never by itself; an `ANALYZE` asks first (Enter keeps,
//! `y` runs); a result whose tab moved since, or that changed while the question was open,
//! is refused.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbEvent, PagingMode};
use datarig_core::i18n::{Label, Lang, Msg};
use datarig_core::policy::Policy;
use datarig_core::profile::ConnectionConfig;
use datarig_tui::app::Focus;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::tabs::ResultView;
use datarig_tui::theme;
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::time::Duration;

const W: u16 = 100;
const H: u16 = 30;
const HINT: &str = "text plan · P view as plan";

/// A plan captured from PostgreSQL `version` (the core's fixtures).
fn fixture(version: u32, name: &str) -> String {
    let path = format!("{}/../datarig-core/src/sql/plan/fixtures/pg{version}/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The answer of run `id`: a text plan, one line a row, as `psql` shows it.
fn text_page(id: u64, lines: &[&str]) -> DbEvent {
    DbEvent::Page {
        id,
        columns: Some(vec![meta("QUERY PLAN", "text", false, false)]),
        rows: lines.iter().map(|l| vec![Some(l.to_string())]).collect(),
        more: false,
        elapsed: Duration::from_millis(2),
    }
}

/// The answer of run `id`: one `QUERY PLAN` row of `json`.
fn plan_page(id: u64, json: &str) -> DbEvent {
    DbEvent::Page {
        id,
        columns: Some(vec![meta("QUERY PLAN", "json", false, true)]),
        rows: vec![vec![Some(json.to_string())]],
        more: false,
        elapsed: Duration::from_millis(3),
    }
}

const TEXT_PLAN: &[&str] = &["Seq Scan on t  (cost=0.00..35.50 rows=2550 width=4)", "  Filter: (a > 1)"];

/// The runs `h` sent since last asked: each one's id and statements.
fn runs(h: &mut Harness) -> Vec<(u64, Vec<String>)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { id, statements, paging: PagingMode::NoHold } => Some((id, statements)),
            _ => None,
        })
        .collect()
}

/// The one statement of the one run `h` just sent, and the run's id.
fn sent_one(h: &mut Harness) -> (u64, String) {
    let runs = runs(h);
    let [(id, statements)] = &runs[..] else { panic!("one run: {runs:?}") };
    let [sql] = &statements[..] else { panic!("one statement: {statements:?}") };
    (*id, sql.clone())
}

/// `h` ran `sql` (its editor's text; confirmed when it asks) and got `page` for it; the
/// results focused.
fn ran(h: &mut Harness, sql: &str, page: impl FnOnce(u64) -> DbEvent) {
    h.app.tab_mut().editor = Editor::new(sql);
    h.sent();
    h.ctrl('e');
    if h.app.overlays.run_confirm().is_some() {
        h.keys("y");
    }
    let (id, sent) = sent_one(h);
    assert_eq!(sent, sql.trim_end_matches(';'));
    h.db(page(id));
    h.app.focus = Focus::Results;
}

/// A connected harness showing the text plan of `sql`.
fn text_plan(sql: &str) -> Harness {
    let mut h = Harness::connected(Lang::En);
    ran(&mut h, sql, |id| text_page(id, TEXT_PLAN));
    h
}

/// The status line says `l` (a notice of the moment, or the tab's last outcome).
fn said(h: &mut Harness, l: Label) {
    let line = h.status(W, H);
    assert!(line.contains(l.text(Lang::En)), "{l:?}: {line}");
}

fn status(h: &Harness) -> Option<Msg> {
    h.app.status.as_ref().map(|n| n.msg.clone())
}

/// Where `text` starts on the screen drawn at `W`×`H`.
fn find(h: &mut Harness, text: &str) -> Option<(u16, u16)> {
    let t = h.draw(W, H);
    let buf = t.backend().buffer();
    (0..H).find_map(|y| {
        let row = row_text(buf, y);
        row.find(text).map(|i| (datarig_tui::text::width(&row[..i]) as u16, y))
    })
}

#[test]
fn a_text_plan_offers_to_be_viewed_as_a_plan_and_opens_it() {
    let mut h = text_plan("EXPLAIN (VERBOSE, COSTS off) SELECT * FROM t WHERE a > 1;");
    assert_eq!(h.app.tab().exec.view, ResultView::Rows, "a text plan stays rows");
    // The line under the rows, dim on the surface color.
    let (x, y) = find(&mut h, HINT).unwrap_or_else(|| panic!("the hint:\n{}", h.screen(W, H)));
    let t = h.draw(W, H);
    let buf = t.backend().buffer();
    for dx in 0..HINT.chars().count() as u16 {
        assert_eq!(buf[(x + dx, y)].fg, theme::DARK.fg_dim, "column {dx}");
        assert_eq!(buf[(x + dx, y)].bg, theme::DARK.surface, "column {dx}");
    }
    insta::assert_snapshot!("text_plan_hint_en_100x30", h.draw(W, H).backend());
    remember_english("text_plan_hint_en_100x30", h.draw(W, H).backend().buffer());
    // Nothing ran by itself.
    assert!(runs(&mut h).is_empty());
    // `P`: the same statement as JSON, its options kept; its plan opens.
    h.keys("P");
    let (id, sql) = sent_one(&mut h);
    assert_eq!(sql, "EXPLAIN (VERBOSE, COSTS off, FORMAT JSON) SELECT * FROM t WHERE a > 1");
    assert!(h.app.overlays.confirm().is_none(), "a plain EXPLAIN does not ask");
    h.db(plan_page(id, &fixture(17, "join.plan.json")));
    assert_eq!(h.app.tab().exec.view, ResultView::Plan);
    assert!(h.app.tab().exec.plan.is_some());
    let screen = h.screen(W, H);
    assert!(screen.contains("Plan · Tree") && !screen.contains("view as plan"), "{screen}");
}

#[test]
fn the_hint_and_the_action_are_only_for_a_text_plan() {
    // Rows of another statement: no hint, and the key says why it does nothing.
    let mut h = Harness::connected(Lang::En);
    ran(&mut h, "SELECT a FROM t;", |id| text_page(id, &["1"]));
    assert!(!h.screen(W, H).contains("view as plan"));
    h.keys("P");
    assert!(runs(&mut h).is_empty());
    said(&mut h, Label::PlanAsPlanNotExplain);
    // A JSON plan is a plan already.
    let mut h = Harness::connected(Lang::En);
    ran(&mut h, "EXPLAIN (FORMAT JSON) SELECT * FROM t;", |id| plan_page(id, &fixture(17, "join.plan.json")));
    assert_eq!(h.app.tab().exec.view, ResultView::Plan);
    h.key(KeyCode::Char('H'));
    assert_eq!(h.app.tab().exec.view, ResultView::Rows, "its rows");
    assert!(!h.screen(W, H).contains("view as plan"));
    // An EXPLAIN that asked for JSON but whose rows are not a plan: nothing to view differently.
    let mut h = text_plan("EXPLAIN (FORMAT 'json') SELECT 1;");
    assert!(!h.screen(W, H).contains("view as plan"));
    h.keys("P");
    assert!(runs(&mut h).is_empty());
    said(&mut h, Label::PlanAsPlanAlreadyJson);
    // A text the parser does not read as the lexer does: refused, never run.
    let mut h = text_plan("EXPLAIN VERBOSE ANALYZE SELECT 1;");
    h.keys("P");
    assert!(runs(&mut h).is_empty());
    said(&mut h, Label::PlanAsPlanUnreadable);
    // The Messages of a text plan: no hint, no action.
    let mut h = text_plan("EXPLAIN SELECT 1;");
    h.key(KeyCode::Char('L'));
    assert_eq!(h.app.tab().exec.view, ResultView::Messages);
    assert!(!h.screen(W, H).contains("view as plan"));
}

#[test]
fn the_leader_key_and_the_menu_view_it_as_a_plan_too() {
    // From the editor: `Space e p`.
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    h.app.focus = Focus::Editor;
    h.keys(" ep");
    assert_eq!(sent_one(&mut h).1, "EXPLAIN (FORMAT JSON) SELECT * FROM t");
    // The results' action menu lists it first among the pane's actions.
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    h.keys("  ");
    let label = Label::ActionResultsViewAsPlan.text(Lang::En);
    assert!(h.menu_labels().iter().any(|l| l == label), "{:?}", h.menu_labels());
    h.menu_pick(label);
    assert_eq!(sent_one(&mut h).1, "EXPLAIN (FORMAT JSON) SELECT * FROM t");
    // Another result's menu does not offer it.
    let mut h = Harness::connected(Lang::En);
    ran(&mut h, "SELECT a FROM t;", |id| text_page(id, &["1"]));
    h.keys("  ");
    assert!(!h.menu_labels().iter().any(|l| l == label), "{:?}", h.menu_labels());
}

#[test]
fn it_runs_the_statement_of_the_result_not_the_editors_text() {
    let mut h = text_plan("EXPLAIN ANALYSE VERBOSE SELECT * FROM t;");
    h.app.tab_mut().editor = Editor::new("EXPLAIN SELECT 2;");
    h.keys("Py");
    assert_eq!(sent_one(&mut h).1, "EXPLAIN (ANALYZE, VERBOSE, FORMAT JSON) SELECT * FROM t");
}

#[test]
fn analyze_asks_first_enter_keeps_and_y_runs() {
    let mut h = text_plan("EXPLAIN ANALYZE SELECT * FROM t;");
    h.keys("P");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    assert_eq!(h.app.overlays.confirm().map(|c| c.action), Some(ConfirmAction::ExplainAgain));
    assert!(runs(&mut h).is_empty(), "nothing runs before the answer");
    let screen = h.screen(W, H);
    for want in ["Run EXPLAIN ANALYZE again?", "runs the statement again", "y run · n/Enter cancel", "Cancel", "Run"] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
    insta::assert_snapshot!("text_plan_analyze_confirm_en_100x30", h.draw(W, H).backend());
    remember_english("text_plan_analyze_confirm_en_100x30", h.draw(W, H).backend().buffer());
    // Enter keeps: nothing runs.
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), None);
    assert!(runs(&mut h).is_empty());
    // Esc and n too.
    for k in [KeyCode::Esc, KeyCode::Char('n')] {
        h.keys("P");
        h.key(k);
        assert_eq!(h.overlay_kind(), None);
        assert!(runs(&mut h).is_empty(), "{k:?}");
    }
    // y runs it, and its plan opens.
    h.keys("Py");
    let (id, sql) = sent_one(&mut h);
    assert_eq!(sql, "EXPLAIN (ANALYZE, FORMAT JSON) SELECT * FROM t");
    h.db(plan_page(id, &fixture(18, "parallel.analyze.json")));
    assert_eq!(h.app.tab().exec.view, ResultView::Plan);
}

#[test]
fn the_question_takes_the_mouse() {
    let mut h = text_plan("EXPLAIN ANALYZE SELECT * FROM t;");
    let click = |h: &mut Harness, button: usize| {
        h.draw(W, H);
        let r = h.app.overlays.confirm().expect("the question").buttons.rects[button];
        h.advance(Duration::from_millis(500));
        h.mouse(MouseEventKind::Down(MouseButton::Left), r.x + 1, r.y);
        h.mouse(MouseEventKind::Up(MouseButton::Left), r.x + 1, r.y);
    };
    h.keys("P");
    click(&mut h, 0);
    assert_eq!(h.overlay_kind(), None, "Cancel");
    assert!(runs(&mut h).is_empty());
    h.keys("P");
    click(&mut h, 1);
    assert_eq!(sent_one(&mut h).1, "EXPLAIN (ANALYZE, FORMAT JSON) SELECT * FROM t");
}

#[test]
fn a_write_under_analyze_keeps_the_run_checks() {
    // A DELETE of some rows: the question, then the run, which the driver rolls back.
    let mut h = text_plan("EXPLAIN ANALYZE DELETE FROM t WHERE a = 1;");
    h.keys("Py");
    let (id, sql) = sent_one(&mut h);
    assert_eq!(sql, "EXPLAIN (ANALYZE, FORMAT JSON) DELETE FROM t WHERE a = 1");
    h.db(plan_page(id, &fixture(17, "insert.analyze.json")));
    assert_eq!(status(&h), Some(Label::SafetyExplainRolledBack.into()));
    // A DELETE of every row asks as before, after the question of its own.
    let mut h = text_plan("EXPLAIN ANALYZE DELETE FROM t;");
    h.keys("Py");
    assert!(h.app.overlays.run_confirm().is_some(), "the run's confirmation");
    assert!(runs(&mut h).is_empty());
    h.keys("y");
    assert_eq!(sent_one(&mut h).1, "EXPLAIN (ANALYZE, FORMAT JSON) DELETE FROM t");
}

#[test]
fn a_read_only_profile_refuses_analyze_of_a_write_before_asking() {
    let mut cfg = Config { connections: vec![ConnectionConfig::test_db()], ..Config::default() };
    cfg.policies.insert("ro", Policy { read_only: true, ..Policy::default() });
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    ran(&mut h, "EXPLAIN ANALYZE DELETE FROM t WHERE a = 1;", |id| text_page(id, TEXT_PLAN));
    // The profile became read-only since: its session is not, so nothing runs on it.
    h.app.profiles[0].policy = Some("ro".into());
    h.keys("P");
    assert_eq!(h.overlay_kind(), None, "no question for what is refused");
    assert!(runs(&mut h).is_empty());
    assert!(matches!(status(&h), Some(Msg::SafetyReadOnlyReconnect { .. })), "{:?}", status(&h));
    // On a read-only session the write is refused as any other run of it.
    h.app.tab_mut().exec.read_only = true;
    h.keys("P");
    assert_eq!(h.overlay_kind(), None);
    assert!(runs(&mut h).is_empty());
    assert!(matches!(status(&h), Some(Msg::SafetyReadOnlyBlocked { .. })), "{:?}", status(&h));
    // A plain EXPLAIN of a write only plans: it runs.
    let mut cfg = cfg.clone();
    cfg.connections[0].policy = Some("ro".into());
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    ran(&mut h, "EXPLAIN DELETE FROM t WHERE a = 1;", |id| text_page(id, TEXT_PLAN));
    h.keys("P");
    assert_eq!(sent_one(&mut h).1, "EXPLAIN (FORMAT JSON) DELETE FROM t WHERE a = 1");
}

#[test]
fn a_result_of_another_binding_or_one_that_changed_is_refused() {
    // The tab moved to another schema since the result: its plan there may differ.
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    h.command("use .shop");
    h.sent();
    h.app.focus = Focus::Results;
    h.keys("P");
    assert!(runs(&mut h).is_empty());
    said(&mut h, Label::PlanAsPlanMoved);
    // The result changed while the question was open (the tab was bound again from elsewhere).
    let mut h = text_plan("EXPLAIN ANALYZE SELECT * FROM t;");
    h.keys("P");
    let (id, profile) = (h.app.tab().id, h.app.tab().profile);
    h.app.tabs.bind(id, profile);
    h.keys("y");
    assert!(runs(&mut h).is_empty());
    said(&mut h, Label::PlanAsPlanStale);
}

#[test]
fn the_text_plan_screens_in_korean() {
    let mut h = text_plan("EXPLAIN ANALYZE SELECT * FROM t;");
    remember_english("text_plan_hint_en_100x30", h.draw(W, H).backend().buffer());
    h.keys("P");
    remember_english("text_plan_analyze_confirm_en_100x30", h.draw(W, H).backend().buffer());
    let mut h = Harness::connected(Lang::Ko);
    ran(&mut h, "EXPLAIN ANALYZE SELECT * FROM t;", |id| text_page(id, TEXT_PLAN));
    let screen = h.screen(W, H);
    let hint = ko_msg(&Msg::PlanAsPlanHint { key: "P".into() });
    assert!(screen.contains(&hint), "{hint}:\n{screen}");
    check_localized("text_plan_hint_ko_100x30", Lang::Ko, h.draw(W, H).backend().buffer());
    h.keys("P");
    check_localized("text_plan_analyze_confirm_ko_100x30", Lang::Ko, h.draw(W, H).backend().buffer());
}
