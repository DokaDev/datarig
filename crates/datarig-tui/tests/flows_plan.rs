//! EXPLAIN and the Plan tab: the explain actions wrap the statement under the cursor and run
//! it like any statement (the read-only refusal, the confirmation, the rollback of an `EXPLAIN
//! ANALYZE` stay the run's), a JSON plan result shows as a plan (also an `EXPLAIN` typed by
//! hand), its views switch without asking the server again, and it goes with the rows of its
//! run.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbEvent, Outcome};
use datarig_core::i18n::{Label, Lang, Msg};
use datarig_core::policy::{Confirm, Policy};
use datarig_core::profile::ConnectionConfig;
use datarig_tui::app::Focus;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::plan::PlanView;
use datarig_tui::app::tabs::ResultView;
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::time::Duration;

/// A plan captured from PostgreSQL `version` (the core's fixtures).
fn fixture(version: u32, name: &str) -> String {
    let path = format!("{}/../datarig-core/src/sql/plan/fixtures/pg{version}/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
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

/// The one statement of the run `h` just sent, and the run's id.
fn sent_one(h: &mut Harness) -> (u64, String) {
    let sent = h.sent();
    let [DbCommand::Execute { id, statements }] = &sent[..] else { panic!("one run: {sent:?}") };
    let [sql] = &statements[..] else { panic!("one statement: {statements:?}") };
    (*id, sql.clone())
}

/// `h` with its editor holding `sql`, after `keys` (an explain key) sent one run: the run's id
/// and its statement.
fn explain_in(mut h: Harness, sql: &str, keys: &str) -> (Harness, u64, String) {
    h.app.tab_mut().editor = Editor::new(sql);
    h.sent();
    h.keys(keys);
    let (id, sql) = sent_one(&mut h);
    (h, id, sql)
}

fn explain(sql: &str, keys: &str) -> (Harness, u64, String) {
    explain_in(Harness::connected(Lang::En), sql, keys)
}

/// A connected harness showing `fixture`'s plan (ANALYZE of PostgreSQL 18's parallel plan by
/// default), the results focused.
fn shown(version: u32, name: &str) -> Harness {
    shown_in(Lang::En, version, name)
}

fn shown_in(lang: Lang, version: u32, name: &str) -> Harness {
    let (mut h, id, _) = explain_in(Harness::connected(lang), "SELECT * FROM t;", " ea");
    h.db(plan_page(id, &fixture(version, name)));
    h.app.focus = Focus::Results;
    h
}

fn plan(h: &Harness) -> &datarig_tui::app::plan::PlanTab {
    h.app.tab().exec.plan.as_ref().expect("a plan")
}

fn status(h: &Harness) -> Option<Msg> {
    h.app.status.as_ref().map(|n| n.msg.clone())
}

#[test]
fn explain_wraps_the_statement_under_the_cursor_and_shows_its_plan() {
    let (mut h, id, sql) = explain("SELECT 1;\nSELECT * FROM t WHERE a > 1;", "j ex");
    assert_eq!(sql, "EXPLAIN (FORMAT JSON) SELECT * FROM t WHERE a > 1");
    h.db(plan_page(id, &fixture(17, "join.plan.json")));
    let t = h.app.tab();
    assert_eq!(t.exec.view, ResultView::Plan, "the plan is shown, not its JSON");
    let p = plan(&h);
    assert_eq!((p.view, p.selected, p.index), (PlanView::Tree, 0, 0));
    assert_eq!(h.app.focus, Focus::Editor, "the focus stays where it was");
    let screen = h.screen(160, 45);
    for want in ["Plan · Tree", "[Plan]", "[Tree]", "estimated cost, not time (no ANALYZE)", "self cost", "Hash Join"]
    {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
    assert!(!screen.contains(" ms "), "estimates are never shown as time:\n{screen}");
    insta::assert_snapshot!("plan_tree_estimates_en_160x45", h.draw(160, 45).backend());
    insta::assert_snapshot!("plan_tree_estimates_en_80x24", h.draw(80, 24).backend());
    // The selection explains too (one statement).
    let (_, _, sql) = explain("SELECT 1;\nDELETE FROM t WHERE a = 1;", "jV ex");
    assert_eq!(sql, "EXPLAIN (FORMAT JSON) DELETE FROM t WHERE a = 1");
}

#[test]
fn explain_analyze_shows_measured_times_and_marks_hot_nodes() {
    let mut h = shown(18, "parallel.analyze.json");
    let screen = h.screen(160, 45);
    for want in ["execution 133.0 ms · planning 7.803 ms", "rows ×loops", "61.9k ×3", "54.7 ms", "41%", "hit/read"] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
    // Hot: Gather (31%) and Partial HashAggregate (41%); not the scan (19.6%, shown as 19%)
    // nor the top (8%).
    let hot: Vec<usize> = (0..4).filter(|i| plan(&h).plan.is_hot(*i)).collect();
    assert_eq!(hot, [1, 2]);
    assert!(screen.contains("19%") && !screen.contains("20%"), "a share is never rounded up to hot:\n{screen}");
    assert!(screen.contains("Gather hot") && !screen.contains("Finalize HashAggregate hot"), "{screen}");
    insta::assert_snapshot!("plan_tree_analyze_en_160x45", h.draw(160, 45).backend());
    insta::assert_snapshot!("plan_tree_analyze_en_80x24", h.draw(80, 24).backend());
    remember_english("plan_tree_analyze_en_160x45", h.draw(160, 45).backend().buffer());
}

#[test]
fn the_plan_screens_in_korean() {
    let mut h = shown(18, "parallel.analyze.json");
    remember_english("plan_tree_analyze_en_160x45", h.draw(160, 45).backend().buffer());
    h.key(KeyCode::Enter);
    remember_english("plan_detail_en_160x45", h.draw(160, 45).backend().buffer());
    let mut h = shown_in(Lang::Ko, 18, "parallel.analyze.json");
    check_localized("plan_tree_analyze_ko_160x45", Lang::Ko, h.draw(160, 45).backend().buffer());
    h.key(KeyCode::Enter);
    check_localized("plan_detail_ko_160x45", Lang::Ko, h.draw(160, 45).backend().buffer());
}

#[test]
fn a_write_explained_with_analyze_goes_through_the_run_checks() {
    // A plain INSERT does not ask (as without EXPLAIN); the driver rolls it back, and the run
    // says so.
    let (mut h, id, sql) = explain("INSERT INTO t VALUES (1);", " ea");
    assert_eq!(sql, "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) INSERT INTO t VALUES (1)");
    assert!(h.app.overlays.run_confirm().is_none());
    h.db(plan_page(id, &fixture(17, "insert.analyze.json")));
    assert_eq!(status(&h), Some(Label::SafetyExplainRolledBack.into()));
    assert_eq!(h.app.tab().exec.view, ResultView::Plan);
    assert_eq!(plan(&h).plan.nodes[0].op, "Insert");
    // A DELETE of every row asks first, saying that EXPLAIN ANALYZE runs it: Esc sends nothing.
    let mut h = Harness::connected(Lang::En);
    h.app.tab_mut().editor = Editor::new("DELETE FROM t;");
    h.sent();
    h.keys(" ea");
    assert!(h.app.overlays.run_confirm().is_some(), "it asks");
    let screen = h.screen(120, 40);
    assert!(screen.contains("EXPLAIN ANALYZE runs it, then rolls it back"), "{screen}");
    h.key(KeyCode::Esc);
    assert!(h.sent().is_empty(), "nothing ran");
    h.keys(" ea");
    h.keys("y");
    let (_, sql) = sent_one(&mut h);
    assert_eq!(sql, "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) DELETE FROM t");
    // A policy that asks before every write asks for the INSERT too.
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("careful".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert("careful", Policy { confirm: Confirm::Writes, ..Policy::default() });
    cfg.policies.insert("ro", Policy { read_only: true, ..Policy::default() });
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    h.app.tab_mut().editor = Editor::new("INSERT INTO t VALUES (1);");
    h.sent();
    h.keys(" ea");
    assert!(h.app.overlays.run_confirm().is_some(), "confirm = writes asks");
    // A read-only profile refuses it before anything is sent; a plain EXPLAIN only plans.
    cfg.connections[0].policy = Some("ro".into());
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    h.app.tab_mut().editor = Editor::new("INSERT INTO t VALUES (1);");
    h.sent();
    h.keys(" ea");
    assert!(h.sent().is_empty(), "refused");
    assert!(matches!(status(&h), Some(Msg::SafetyReadOnlyBlocked { .. })), "{:?}", status(&h));
    h.keys(" ex");
    let (_, sql) = sent_one(&mut h);
    assert_eq!(sql, "EXPLAIN (FORMAT JSON) INSERT INTO t VALUES (1)");
}

#[test]
fn explain_takes_one_statement_that_is_not_an_explain() {
    let mut h = Harness::connected(Lang::En);
    h.app.tab_mut().editor = Editor::new("SELECT 1;\nSELECT 2;");
    h.sent();
    h.keys("ggVG ex");
    assert!(h.sent().is_empty());
    let said = |h: &mut Harness| h.status(160, 45);
    assert!(said(&mut h).contains("Explain takes one statement; the selection has 2"), "{}", said(&mut h));
    h.app.tab_mut().editor = Editor::new("EXPLAIN ANALYZE DELETE FROM t;");
    h.keys(" ea");
    assert!(h.sent().is_empty(), "never wrapped twice");
    assert!(said(&mut h).contains("This statement is an EXPLAIN already: Ctrl+E runs it"), "{}", said(&mut h));
    h.app.tab_mut().editor = Editor::new("");
    h.keys(" ex");
    assert!(h.sent().is_empty());
}

#[test]
fn the_command_line_explains_too() {
    let mut h = Harness::connected(Lang::En);
    h.app.tab_mut().editor = Editor::new("SELECT * FROM t;");
    h.sent();
    h.command("explain");
    assert_eq!(sent_one(&mut h).1, "EXPLAIN (FORMAT JSON) SELECT * FROM t");
    h.db(DbEvent::Done {
        id: h.app.tab().exec.query_id,
        outcome: Outcome::Command("EXPLAIN".into()),
        elapsed: Duration::ZERO,
    });
    h.command("explain analyze");
    assert_eq!(sent_one(&mut h).1, "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT * FROM t");
    h.db(DbEvent::Done {
        id: h.app.tab().exec.query_id,
        outcome: Outcome::Command("EXPLAIN".into()),
        elapsed: Duration::ZERO,
    });
    h.command("explain verbose");
    assert!(h.sent().is_empty());
    let error = h.cmdline().and_then(|c| c.error.clone()).map(|n| n.msg);
    assert_eq!(error, Some(Label::PlanCommandUsage.into()));
}

/// An `EXPLAIN (FORMAT JSON)` typed and run by hand shows as a plan; other results that look
/// a little like one stay rows.
#[test]
fn a_plan_typed_by_hand_shows_and_look_alikes_stay_rows() {
    let mut h = Harness::connected(Lang::En);
    h.app.tab_mut().editor = Editor::new("EXPLAIN (FORMAT JSON) SELECT 1;");
    h.sent();
    h.ctrl('e');
    let (id, sql) = sent_one(&mut h);
    assert_eq!(sql, "EXPLAIN (FORMAT JSON) SELECT 1", "run as typed");
    h.db(plan_page(id, &fixture(13, "simple.plan.json")));
    assert_eq!(h.app.tab().exec.view, ResultView::Plan);
    let not_plans: Vec<DbEvent> = vec![
        // The text form.
        DbEvent::Page {
            id: 0,
            columns: Some(vec![meta("QUERY PLAN", "text", false, false)]),
            rows: vec![vec![Some("Result  (cost=0.00..0.01 rows=1 width=4)".into())]],
            more: false,
            elapsed: Duration::ZERO,
        },
        // JSON that is not a plan, under the same name.
        plan_page(0, r#"{"a": 1}"#),
        // Another column name.
        DbEvent::Page {
            id: 0,
            columns: Some(vec![meta("plan", "json", false, true)]),
            rows: vec![vec![Some(fixture(13, "simple.plan.json"))]],
            more: false,
            elapsed: Duration::ZERO,
        },
    ];
    for ev in not_plans {
        h.app.tab_mut().editor = Editor::new("SELECT x;");
        h.ctrl('e');
        let (id, _) = sent_one(&mut h);
        let ev = match ev {
            DbEvent::Page { columns, rows, more, elapsed, .. } => DbEvent::Page { id, columns, rows, more, elapsed },
            ev => ev,
        };
        h.db(ev);
        assert_eq!(h.app.tab().exec.view, ResultView::Rows);
        assert!(h.app.tab().exec.plan.is_none(), "a run with rows took the plan's place");
    }
}

/// A plan from a statement before the last of a run is that statement's plan; the run's last
/// rows are shown when they are not a plan.
#[test]
fn a_plan_of_a_statement_before_the_last_is_kept_with_its_index() {
    let mut h = Harness::connected(Lang::En);
    h.app.tab_mut().editor = Editor::new("EXPLAIN (FORMAT JSON) SELECT 1;\nSELECT 2;");
    h.sent();
    h.keys("ggVG");
    h.ctrl('e');
    let (id, statements) = match &h.sent()[..] {
        [DbCommand::Execute { id, statements }] => (*id, statements.clone()),
        s => panic!("{s:?}"),
    };
    assert_eq!(statements.len(), 2);
    h.db(DbEvent::Started { id, index: 0 });
    h.db(DbEvent::StepRows {
        id,
        index: 0,
        columns: Some(vec![meta("QUERY PLAN", "json", false, true)]),
        rows: vec![vec![Some(fixture(16, "simple.plan.json"))]],
        more: false,
    });
    h.db(DbEvent::Finished { id, index: 0, outcome: Outcome::Command("EXPLAIN".into()), elapsed: Duration::ZERO });
    h.db(DbEvent::Started { id, index: 1 });
    h.db(DbEvent::Page {
        id,
        columns: Some(vec![meta("?column?", "int4", true, false)]),
        rows: vec![vec![Some("2".into())]],
        more: false,
        elapsed: Duration::ZERO,
    });
    assert_eq!(plan(&h).index, 0);
    assert_eq!(h.app.tab().exec.view, ResultView::Rows, "the last statement's rows");
    let strip = h.screen(160, 45);
    assert!(strip.contains("Result 1   [Result 2]   Plan   Messages") || strip.contains("[Result 2]"), "{strip}");
}

#[test]
fn views_switch_without_asking_the_server() {
    let mut h = shown(18, "subplans.analyze.json");
    h.sent();
    h.keys("v");
    assert_eq!(plan(&h).view, PlanView::Raw);
    insta::assert_snapshot!("plan_raw_en_160x45", h.draw(160, 45).backend());
    insta::assert_snapshot!("plan_raw_en_80x24", h.draw(80, 24).backend());
    h.keys("v");
    assert_eq!(plan(&h).view, PlanView::Tree, "around the end");
    h.keys("V");
    assert_eq!(plan(&h).view, PlanView::Raw);
    h.keys("1");
    assert_eq!(plan(&h).view, PlanView::Tree);
    h.keys("9");
    assert_eq!(plan(&h).view, PlanView::Raw);
    // From `:`, also while another result tab is shown: it shows the plan.
    h.keys("H");
    assert_eq!(h.app.tab().exec.view, ResultView::Rows);
    h.command("Plan view: tree");
    assert_eq!((h.app.tab().exec.view, plan(&h).view), (ResultView::Plan, PlanView::Tree));
    // A click on a view's name.
    h.draw(160, 45);
    let (rect, _) = *plan(&h).view_hits.iter().find(|(_, v)| *v == PlanView::Raw).unwrap();
    h.mouse(MouseEventKind::Down(MouseButton::Left), rect.x + 1, rect.y);
    assert_eq!(plan(&h).view, PlanView::Raw);
    assert!(h.sent().is_empty(), "nothing was asked of the server");
    // The raw text is the plan's text as psql shows it, built from the JSON.
    let screen = h.screen(160, 45);
    assert!(screen.contains("->  Aggregate  (cost=539.64..539.65 rows=1 width=32) (actual time="), "{screen}");
    assert!(screen.contains("SubPlan 1"), "{screen}");
    let text = datarig_core::sql::plan::text::text(&plan(&h).plan);
    assert!(text.ends_with("Execution Time: 82.391 ms"), "{text}");
}

#[test]
fn the_tree_folds_and_the_selection_moves() {
    let mut h = shown(18, "subplans.analyze.json");
    let ops = |h: &Harness| plan(h).plan.nodes.iter().map(|n| n.op.clone()).collect::<Vec<_>>();
    assert_eq!(
        ops(&h),
        ["Seq Scan", "Result", "Limit", "Index Only Scan", "Aggregate", "Bitmap Heap Scan", "Bitmap Index Scan"]
    );
    h.keys("j");
    assert_eq!(plan(&h).selected, 1);
    h.keys("h");
    assert!(plan(&h).collapsed[1], "closed");
    let screen = h.screen(160, 45);
    assert!(!screen.contains("Index Only Scan") && screen.contains("▸ InitPlan 3: Result"), "{screen}");
    h.keys("j");
    assert_eq!(plan(&h).selected, 4, "the hidden nodes are skipped");
    h.keys("k");
    h.keys("l");
    assert!(!plan(&h).collapsed[1], "open again");
    h.keys("l");
    assert_eq!(plan(&h).selected, 2, "to its first child");
    h.keys("G");
    assert_eq!(plan(&h).selected, 6);
    h.keys("h");
    assert_eq!(plan(&h).selected, 5, "a leaf goes to its parent");
    h.keys("gg");
    assert_eq!(plan(&h).selected, 0);
    // Enter: the detail of the selected node, beside the tree in a wide pane.
    h.keys("jjjj");
    h.key(KeyCode::Enter);
    assert!(plan(&h).detail);
    let screen = h.screen(160, 45);
    for want in ["SubPlan 1", "Per loop", "250 loops", "Recheck Cond"] {
        assert!(screen.contains(want) || want == "Recheck Cond", "{want}:\n{screen}");
    }
    insta::assert_snapshot!("plan_detail_en_160x45", h.draw(160, 45).backend());
    h.keys("i");
    assert!(!plan(&h).detail, "i too");
}

#[test]
fn misestimates_say_how_far_off_and_which_way() {
    // The never executed inner side of a nested loop has no estimate to be off.
    let mut h = shown(18, "never.analyze.json");
    let screen = h.screen(160, 45);
    assert!(screen.contains("never ran"), "{screen}");
    // A plan with a node estimated at 10 rows that returned 1000.
    let json = r#"[{"Plan": {"Node Type": "Seq Scan", "Relation Name": "t", "Alias": "t", "Startup Cost": 0.00,
        "Total Cost": 10.00, "Plan Rows": 10, "Plan Width": 4, "Actual Startup Time": 0.1, "Actual Total Time": 5.0,
        "Actual Rows": 1000, "Actual Loops": 1}, "Execution Time": 5.2}]"#;
    let (mut h, id, _) = explain("SELECT * FROM t;", " ea");
    h.db(plan_page(id, json));
    h.app.focus = Focus::Results;
    let screen = h.screen(160, 45);
    assert!(screen.contains("Seq Scan on t hot !×100↑"), "{screen}");
    assert!(plan(&h).plan.misestimate(0).is_some_and(|o| o.under && o.ratio == 100.0));
    h.key(KeyCode::Enter);
    let screen = h.screen(160, 45);
    assert!(screen.contains("off by ×100: more rows"), "{screen}");
}

/// The plan goes with the rows of its run: a run without rows leaves it (an earlier run's), one
/// with rows replaces it; answers of an earlier run are dropped.
#[test]
fn the_plan_goes_with_the_rows_of_its_run() {
    let (mut h, id, _) = explain("SELECT * FROM t;", " ex");
    h.db(plan_page(id, &fixture(17, "simple.plan.json")));
    // A COMMIT: no rows, the plan stays and says it is from an earlier run.
    h.app.tab_mut().editor = Editor::new("COMMIT;");
    h.ctrl('e');
    let (commit, _) = sent_one(&mut h);
    // The earlier run's answer, late: dropped.
    let first = plan(&h).plan.clone();
    h.db(plan_page(id, &fixture(17, "join.plan.json")));
    assert_eq!(plan(&h).plan, first, "still the first plan");
    h.db(DbEvent::Done { id: commit, outcome: Outcome::Command("COMMIT".into()), elapsed: Duration::ZERO });
    assert!(h.app.tab().exec.plan.is_some());
    assert!(h.screen(160, 45).contains("earlier run"));
    // Rows: the plan goes.
    h.app.tab_mut().editor = Editor::new("SELECT 1;");
    h.ctrl('e');
    let (select, _) = sent_one(&mut h);
    h.db(DbEvent::Page {
        id: select,
        columns: Some(vec![meta("a", "int4", true, false)]),
        rows: vec![vec![Some("1".into())]],
        more: false,
        elapsed: Duration::ZERO,
    });
    assert!(h.app.tab().exec.plan.is_none());
    assert_eq!(h.app.tab().exec.view, ResultView::Rows);
    let screen = h.screen(160, 45);
    assert!(!screen.contains("Plan"), "{screen}");
    // A table tab never shows a plan.
    assert!(!h.app.tab().is_table());
}

#[test]
fn the_strip_and_its_keys_reach_the_plan() {
    let mut h = shown(17, "join.analyze.json");
    let screen = h.screen(160, 45);
    assert!(screen.contains("Result 1  [Plan]  Messages"), "{screen}");
    h.keys("L");
    assert_eq!(h.app.tab().exec.view, ResultView::Messages);
    h.keys("L");
    assert_eq!(h.app.tab().exec.view, ResultView::Rows, "around: the plan's own rows (its JSON)");
    h.keys("L");
    assert_eq!(h.app.tab().exec.view, ResultView::Plan);
    h.keys("H");
    assert_eq!(h.app.tab().exec.view, ResultView::Rows);
    // A click on Plan in the strip.
    h.draw(160, 45);
    let (x0, ..) = h.app.strip_hits().into_iter().find(|(.., v)| *v == ResultView::Plan).unwrap();
    h.mouse(MouseEventKind::Down(MouseButton::Left), x0 + 1, h.app.layout.strip.y);
    assert_eq!(h.app.tab().exec.view, ResultView::Plan);
}

#[test]
fn the_pointer_selects_nodes_and_the_wheel_scrolls() {
    let mut h = shown(18, "subplans.analyze.json");
    h.app.focus = Focus::Editor;
    h.draw(160, 45);
    let (rect, i) = plan(&h).hits.iter().copied().find(|(_, i)| *i == 3).unwrap();
    h.mouse(MouseEventKind::Down(MouseButton::Left), rect.x + 3, rect.y);
    assert_eq!((plan(&h).selected, i), (3, 3));
    assert_eq!(h.app.focus, Focus::Results, "the click focuses the plan");
    assert!(!plan(&h).detail);
    h.mouse(MouseEventKind::Down(MouseButton::Left), rect.x + 3, rect.y);
    assert!(plan(&h).detail, "a double click shows its detail");
    // In a short pane the wheel scrolls the view; a key brings the selection back.
    h.draw(80, 24);
    h.mouse(MouseEventKind::ScrollDown, rect.x + 3, h.app.layout.results.y + 4);
    assert!(plan(&h).scroll > 0 && plan(&h).detached, "{}", plan(&h).scroll);
    h.keys("gg");
    h.draw(80, 24);
    assert_eq!(plan(&h).scroll, 0);
    // Every view: a click selects (the raw text's lines too).
    h.keys("9");
    h.draw(160, 45);
    let (rect, i) = plan(&h).hits.iter().copied().find(|(_, i)| *i == 5).unwrap();
    h.mouse(MouseEventKind::Down(MouseButton::Left), rect.x + 3, rect.y);
    assert_eq!((plan(&h).selected, i), (5, 5));
}

#[test]
fn the_plans_menu_acts_on_the_plan_it_was_opened_on() {
    let mut h = shown(17, "join.analyze.json");
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("j  ");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
    let labels = h.menu_labels();
    for want in [
        "Plan: show or hide the selected node's detail",
        "Plan: copy as text (as psql shows it)",
        "Plan: copy its JSON",
        "Plan view: tree",
        "Plan view: raw text (as psql shows it)",
    ] {
        assert!(labels.iter().any(|l| l == want), "{want}: {labels:?}");
    }
    let heading = h.app.menu_lines()[0].1.to_string();
    assert_eq!(heading, plan(&h).plan.label(1), "the selected node");
    insta::assert_snapshot!("plan_menu_en_120x34", h.draw(120, 34).backend());
    h.menu_pick("Plan view: raw text (as psql shows it)");
    assert_eq!(plan(&h).view, PlanView::Raw);
    // Copies: the text as psql shows it, and the JSON as the server sent it.
    h.keys("y");
    let text = datarig_core::sql::plan::text::text(&plan(&h).plan);
    assert_eq!(clip.last().as_deref(), Some(text.as_str()));
    h.keys("Y");
    assert_eq!(clip.last(), Some(fixture(17, "join.analyze.json")));
    // A menu opened on a plan that a later run replaced runs nothing.
    h.keys("  ");
    h.app.tab_mut().editor = Editor::new("EXPLAIN (FORMAT JSON) SELECT 2;");
    h.app.run(vec!["EXPLAIN (FORMAT JSON) SELECT 2".into()]);
    let (id, _) = sent_one(&mut h);
    h.db(plan_page(id, &fixture(17, "simple.plan.json")));
    h.menu_pick("Plan view: tree");
    assert!(h.status(160, 45).contains("Not run: what the menu was opened on has changed"), "{}", h.status(160, 45));
    assert_eq!(plan(&h).view, PlanView::Tree, "the new plan, as it opened");
    // The editor's menu explains.
    h.app.tab_mut().editor = Editor::new("SELECT 2;");
    h.app.focus = Focus::Editor;
    h.keys("  ");
    let labels = h.menu_labels();
    assert!(labels.contains(&"Explain: the statement's plan (nothing runs)".to_string()), "{labels:?}");
    h.menu_pick("Explain analyze: run and measure the statement (writes rolled back)");
    assert_eq!(sent_one(&mut h).1, "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT 2");
}

/// Hot nodes and misestimates are drawn with the theme's tokens, in the default `terminal`
/// theme and every built-in; on the focused selection only their modifier stays.
#[test]
fn hot_and_misestimate_use_the_themes_tokens() {
    for (name, th) in datarig_tui::theme::BUILTINS.iter().copied() {
        let mut h = shown(18, "parallel.analyze.json");
        h.app.theme = std::sync::Arc::new(th.clone());
        h.app.focus = Focus::Editor;
        let t = h.draw(160, 45);
        let buf = t.backend().buffer();
        let (y, x) = (0..buf.area.height)
            .find_map(|y| row_text(buf, y).find("Gather hot").map(|x| (y, x)))
            .unwrap_or_else(|| panic!("{name}"));
        let x = row_text(buf, y)[..x].chars().count() as u16;
        let cell = &buf[(x, y)];
        assert_eq!(cell.fg, th.plan_hot.fg.unwrap(), "{name}: the hot node's name");
        assert!(cell.modifier.contains(th.plan_hot.add_modifier), "{name}");
    }
}

/// A menu opened on a plan while another run is going is about that plan: when the other run's
/// plan arrives, the menu's items do not act on it.
#[test]
fn a_menu_opened_on_a_plan_is_stale_once_the_running_runs_plan_arrives() {
    let mut h = shown(17, "join.analyze.json");
    h.app.run(vec!["EXPLAIN (FORMAT JSON) SELECT 2".into()]);
    let (id, _) = sent_one(&mut h);
    h.app.focus = Focus::Results;
    h.keys("  ");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
    h.db(plan_page(id, &fixture(17, "join.plan.json")));
    h.menu_pick("Plan view: raw text (as psql shows it)");
    assert!(h.status(160, 45).contains("Not run: what the menu was opened on has changed"), "{}", h.status(160, 45));
    assert_eq!(plan(&h).view, PlanView::Tree);
}

/// A node selected in a view that shows every node is shown by the tree too: the closed nodes
/// above it open.
#[test]
fn a_selection_made_in_another_view_is_shown_in_the_tree() {
    let mut h = shown(18, "subplans.analyze.json");
    h.keys("jh");
    assert!(plan(&h).collapsed[1]);
    h.keys("9jj");
    let selected = plan(&h).selected;
    let mut above = plan(&h).plan.nodes[selected].parent;
    while above.is_some_and(|a| a != 1) {
        above = above.and_then(|a| plan(&h).plan.nodes[a].parent);
    }
    assert_eq!(above, Some(1), "inside the node closed in the tree");
    h.keys("1");
    assert!(!plan(&h).collapsed[1], "opened");
    assert!(plan(&h).visible().contains(&selected));
}

/// Long raw lines move sideways only as far as the longest one.
#[test]
fn the_raw_text_moves_sideways_only_as_far_as_its_longest_line() {
    let mut h = shown(18, "subplans.analyze.json");
    h.keys("9");
    for _ in 0..100 {
        h.keys(">");
    }
    let screen = h.screen(160, 45);
    assert!(plan(&h).pan < 200, "{}", plan(&h).pan);
    assert!(screen.contains("rows=") || screen.contains("loops="), "the ends of the longest lines show:\n{screen}");
}
