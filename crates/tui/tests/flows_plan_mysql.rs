//! Plans on a MySQL profile: the explain actions ask for MySQL's tree (`EXPLAIN FORMAT=TREE`,
//! `EXPLAIN ANALYZE FORMAT=TREE`) through the run's own checks, a tree result (also one typed by
//! hand) shows as a plan without running anything again, its raw view and copies are the
//! server's text as it came, a cost the server did not give is marked as such, a table
//! `EXPLAIN` is viewed as a plan by asking for the tree, and a server that gives no plan the
//! app reads (MariaDB) offers no plan action.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbEvent, ExplainFormat};
use datarig_core::i18n::{Label, Lang, Msg};
use datarig_core::policy::Policy;
use datarig_tui::app::Focus;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::plan::PlanView;
use datarig_tui::app::tabs::ResultView;
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::KeyCode;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// A tree captured from MySQL 8.4 (the core's fixtures).
fn fixture(name: &str) -> String {
    let path = format!("{}/../core/src/sql/plan/fixtures/mysql/8.4/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

const JOIN: &str = "SELECT c.city, SUM(o.amount) FROM c JOIN o ON o.c_id=c.id WHERE c.city IN ('city1','city2') \
                    GROUP BY c.city ORDER BY 2 DESC";

/// A MySQL profile (`local-my`, with `policy` when given), connected.
fn mysql_with(policy: Option<Policy>) -> Harness {
    let driver = FakeDriver::default();
    driver.mysql.store(true, Ordering::SeqCst);
    let mut p = datarig_core::profile::ConnectionConfig::test_db();
    p.name = "local-my".into();
    p.driver = "mysql".into();
    let mut cfg = Config::default();
    if let Some(policy) = policy {
        p.policy = Some("custom".into());
        cfg.policies.insert("custom", policy);
    }
    cfg.connections = vec![p];
    let mut h = Harness::with_driver(&cfg, Lang::En, driver);
    h.db(DbEvent::Connected);
    h
}

fn mysql() -> Harness {
    mysql_with(None)
}

/// The editor of `h`'s tab holds `sql`, read in the tab's language.
fn edit(h: &mut Harness, sql: &str) {
    let mut editor = Editor::new(sql);
    editor.set_language(h.app.tab_language(h.app.tab().id));
    h.app.tab_mut().editor = editor;
}

/// The answer of run `id`: MySQL's one `EXPLAIN` cell holding `tree`.
fn tree_page(id: u64, tree: &str) -> DbEvent {
    DbEvent::Page {
        id,
        columns: Some(vec![meta("EXPLAIN", "TEXT", false, false)]),
        rows: vec![vec![Some(tree.to_string())]],
        more: false,
        elapsed: Duration::from_millis(3),
    }
}

/// The answer of run `id`: a table `EXPLAIN`, one row.
fn table_page(id: u64) -> DbEvent {
    let cols = ["id", "select_type", "table", "type", "key", "rows", "Extra"];
    DbEvent::Page {
        id,
        columns: Some(cols.iter().map(|c| meta(c, "VARCHAR", false, false)).collect()),
        rows: vec![
            ["1", "SIMPLE", "c", "range", "PRIMARY", "4", "Using where"].iter().map(|v| Some(v.to_string())).collect(),
        ],
        more: false,
        elapsed: Duration::from_millis(2),
    }
}

/// The runs `h` sent since last asked: each one's id and statements.
fn runs(h: &mut Harness) -> Vec<(u64, Vec<String>)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { id, statements, .. } => Some((id, statements)),
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

/// `h` with its editor holding `sql`, after `keys`: the one run sent, its id and statement.
fn keyed(h: &mut Harness, sql: &str, keys: &str) -> (u64, String) {
    edit(h, sql);
    h.sent();
    h.keys(keys);
    sent_one(h)
}

/// A MySQL profile showing the plan `EXPLAIN` gave for the join (its tree fixture `name`).
fn shown(name: &str, keys: &str) -> Harness {
    let mut h = mysql();
    let (id, _) = keyed(&mut h, &format!("{JOIN};"), keys);
    h.db(tree_page(id, &fixture(name)));
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
fn explain_asks_mysql_for_its_tree_and_marks_a_cost_it_did_not_give() {
    let mut h = mysql();
    let (id, sql) = keyed(&mut h, &format!("SELECT 1;\n{JOIN};"), "j ex");
    assert_eq!(sql, format!("EXPLAIN FORMAT=TREE {JOIN}"));
    h.db(tree_page(id, &fixture("join_group_sort.tree.txt")));
    assert_eq!(h.app.tab().exec.view, ResultView::Plan, "the tree is shown as a plan");
    let p = plan(&h);
    assert_eq!((p.view, p.format), (PlanView::Tree, ExplainFormat::MySqlTree));
    assert!(p.plan.nodes[0].cost_derived, "the sort has no cost of its own");
    let screen = h.screen(120, 40);
    for want in [
        "estimated cost, not time (no ANALYZE)",
        "≈ = sum of its children",
        "self cost",
        "≈273",
        "Stream results",
        "Nested loop inner join",
    ] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
    // The server's own costs are shown as they are.
    assert!(screen.contains(" 215") && !screen.contains("≈215"), "{screen}");
    insta::assert_snapshot!("plan_mysql_tree_en_120x40", h.draw(120, 40).backend());
    insta::assert_snapshot!("plan_mysql_tree_en_80x24", h.draw(80, 24).backend());
    // The detail of the sort says where its number comes from.
    h.app.focus = Focus::Results;
    h.key(KeyCode::Enter);
    let screen = h.screen(120, 40);
    assert!(
        screen.contains("≈273: the sum of its children (the server gives this node no cost)"),
        "the detail:\n{screen}"
    );
    assert!(!screen.contains("0.00..273"), "never as the server's numbers:\n{screen}");
}

#[test]
fn explain_analyze_asks_for_the_measured_tree_through_the_run_checks() {
    let mut h = mysql();
    let (id, sql) = keyed(&mut h, &format!("{JOIN};"), " ea");
    assert_eq!(sql, format!("EXPLAIN ANALYZE FORMAT=TREE {JOIN}"));
    assert!(h.app.overlays.run_confirm().is_none(), "a read does not ask");
    h.db(tree_page(id, &fixture("join_group_sort.analyze.txt")));
    let p = plan(&h);
    assert!(p.plan.analyzed);
    let screen = h.screen(120, 40);
    for want in ["rows ×loops", "10 ×58", "measured 1.420 ms (the server reports no statement time)"] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
    // MySQL reports no planning or execution time of its own: none is shown as unknown.
    assert!(!screen.contains("planning") && !screen.contains('?'), "{screen}");
    // No buffer counts in MySQL's plans: no column for them, and no card saying they are
    // missing.
    assert!(!screen.contains("hit/read"), "{screen}");
    h.app.focus = Focus::Results;
    h.keys("2");
    let screen = h.screen(160, 45);
    assert!(screen.contains("Slowest node") && !screen.contains("Disk reads"), "{screen}");
    assert!(screen.contains("Measured time") && !screen.contains("planning"), "{screen}");
    insta::assert_snapshot!("plan_mysql_analyze_summary_en_160x45", h.draw(160, 45).backend());
    // A DELETE of every row asks first: Esc sends nothing, `y` runs it under EXPLAIN ANALYZE.
    let mut h = mysql();
    edit(&mut h, "DELETE FROM o;");
    h.sent();
    h.keys(" ea");
    assert!(h.app.overlays.run_confirm().is_some(), "it asks");
    h.key(KeyCode::Esc);
    assert!(runs(&mut h).is_empty(), "nothing ran");
    h.keys(" ea");
    h.keys("y");
    assert_eq!(sent_one(&mut h).1, "EXPLAIN ANALYZE FORMAT=TREE DELETE FROM o");
    // A read-only profile refuses it before anything is sent.
    let mut h = mysql_with(Some(Policy { read_only: true, ..Policy::default() }));
    edit(&mut h, "UPDATE o SET amount = 1 WHERE id = 3;");
    h.sent();
    h.keys(" ea");
    assert!(runs(&mut h).is_empty(), "refused");
    assert!(matches!(status(&h), Some(Msg::SafetyReadOnlyBlocked { .. })), "{:?}", status(&h));
}

#[test]
fn a_tree_typed_by_hand_shows_as_a_plan_without_running_again() {
    for typed in [format!("EXPLAIN ANALYZE {JOIN};"), format!("explain format = tree {JOIN};")] {
        let mut h = mysql();
        edit(&mut h, &typed);
        h.sent();
        h.ctrl('e');
        let (id, sql) = sent_one(&mut h);
        assert_eq!(sql, typed.trim_end_matches(';'), "run as typed");
        let name = if typed.starts_with("EXPLAIN ANALYZE") { "analyze" } else { "tree" };
        h.db(tree_page(id, &fixture(&format!("join_group_sort.{name}.txt"))));
        assert_eq!(h.app.tab().exec.view, ResultView::Plan, "{typed}");
        assert_eq!(plan(&h).plan.analyzed, name == "analyze");
        assert!(h.sent().is_empty(), "nothing asked again: {typed}");
    }
    // Text that only starts like a tree, or is not one the server could give, stays rows.
    let mut h = mysql();
    edit(&mut h, "SELECT '-> hi' AS `EXPLAIN`;");
    h.sent();
    h.ctrl('e');
    let (id, _) = sent_one(&mut h);
    h.db(tree_page(id, "-> hi"));
    assert_eq!(h.app.tab().exec.view, ResultView::Rows);
    assert!(h.app.tab().exec.plan.is_none());
}

#[test]
fn the_raw_view_and_the_copies_are_the_servers_text() {
    let mut h = shown("join_group_sort.tree.txt", " ex");
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("9");
    assert_eq!(plan(&h).view, PlanView::Raw);
    let tree = fixture("join_group_sort.tree.txt");
    let screen = h.screen(220, 40);
    for line in tree.lines().filter(|l| !l.trim().is_empty()) {
        assert!(screen.contains(line), "verbatim: {line}\n{screen}");
    }
    assert!(
        !screen.lines().any(|l| l.contains("->") && l.contains('≈')),
        "the raw view is the server's alone:\n{screen}"
    );
    insta::assert_snapshot!("plan_mysql_raw_en_120x40", h.draw(120, 40).backend());
    insta::assert_snapshot!("plan_mysql_raw_en_80x24", h.draw(80, 24).backend());
    // Selecting a node marks its own line.
    h.keys("j");
    let raw = h.app.tab_mut().exec.plan.as_mut().expect("a plan").raw();
    assert_eq!((raw.node_line(0), raw.node_line(1), raw.node_line(6)), (0, 1, 6), "a node on its own line");
    assert_eq!(raw.lines[1].1, Some(1));
    // `y` and `Y` both copy the text as the server sent it.
    h.keys("y");
    assert_eq!(clip.last().as_deref(), Some(tree.as_str()));
    assert!(matches!(status(&h), Some(Msg::PlanCopied { .. })), "{:?}", status(&h));
    h.keys("Y");
    assert_eq!(clip.last().as_deref(), Some(tree.as_str()));
    assert!(matches!(status(&h), Some(Msg::PlanCopied { .. })), "said as text, not JSON: {:?}", status(&h));
}

/// The result of a table `EXPLAIN` (`sql`), shown on a MySQL profile.
fn table_plan(sql: &str) -> Harness {
    let mut h = mysql();
    edit(&mut h, sql);
    h.sent();
    h.ctrl('e');
    if h.app.overlays.run_confirm().is_some() {
        h.keys("y");
    }
    let (id, sent) = sent_one(&mut h);
    assert_eq!(sent, sql.trim_end_matches(';'));
    h.db(table_page(id));
    h.app.focus = Focus::Results;
    h
}

#[test]
fn a_table_explain_is_viewed_as_a_plan_by_asking_for_the_tree() {
    let mut h = table_plan("EXPLAIN FORMAT=TRADITIONAL SELECT * FROM c WHERE id < 5;");
    assert_eq!(h.app.tab().exec.view, ResultView::Rows);
    let screen = h.screen(100, 30);
    assert!(screen.contains("text plan · P view as plan"), "the hint:\n{screen}");
    h.keys("P");
    // On the allowlist: the server is asked first, then it runs without a question.
    let sent = h.sent();
    let Some(DbCommand::CheckRepeat { id, sql }) =
        sent.iter().find(|c| matches!(c, DbCommand::CheckRepeat { .. })).cloned()
    else {
        panic!("the server is asked first: {sent:?}")
    };
    assert_eq!(sql, "SELECT * FROM c WHERE id < 5");
    h.db(DbEvent::RepeatChecked { id, result: Ok(()) });
    let (id, sql) = sent_one(&mut h);
    assert_eq!(sql, "EXPLAIN FORMAT=TREE SELECT * FROM c WHERE id < 5", "never ANALYZE");
    h.db(tree_page(id, &fixture("scalar_sub.tree.txt")));
    assert_eq!(h.app.tab().exec.view, ResultView::Plan);
    // Off the allowlist (a function of the user's, a write): it asks, and Enter keeps it.
    for sql in ["EXPLAIN SELECT f(id) FROM c;", "EXPLAIN DELETE FROM o WHERE id = 1;"] {
        let mut h = table_plan(sql);
        h.keys("P");
        assert!(h.app.overlays.confirm().is_some_and(|c| c.action == ConfirmAction::ExplainAgain), "{sql}: it asks");
        h.key(KeyCode::Enter);
        assert!(runs(&mut h).is_empty(), "{sql}: nothing ran");
    }
}

#[test]
fn what_cannot_be_asked_for_as_a_tree_is_refused_with_a_reason() {
    for (sql, why) in [
        ("DESC c;", Label::PlanAsPlanNotExplain),
        ("EXPLAIN /*!80000 FORMAT=JSON */ SELECT 1;", Label::PlanAsPlanUnreadable),
        ("EXPLAIN FOR CONNECTION 12;", Label::PlanAsPlanUnreadable),
    ] {
        let mut h = table_plan(sql);
        h.keys("P");
        assert!(runs(&mut h).is_empty(), "{sql}: nothing ran");
        assert_ne!(h.overlay_kind(), Some(OverlayKind::Confirm), "{sql}: no question");
        let line = h.status(220, 30);
        assert!(line.contains(why.text(Lang::En)), "{sql}: {why:?}: {line}");
    }
}

#[test]
fn a_server_without_a_plan_the_app_reads_offers_no_plan_action() {
    let explain = Label::ActionQueryExplain.text(Lang::En).to_string();
    let analyze = Label::ActionQueryExplainAnalyze.text(Lang::En).to_string();
    let as_plan = Label::ActionResultsViewAsPlan.text(Lang::En).to_string();
    let mut h = table_plan("EXPLAIN SELECT * FROM c WHERE id < 5;");
    let menu = |h: &mut Harness| {
        h.key_mod(KeyCode::F(10), ratatui::crossterm::event::KeyModifiers::SHIFT);
        assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu), "a menu opened");
        let labels = h.menu_labels();
        h.key(KeyCode::Esc);
        labels
    };
    let commands = |h: &mut Harness| {
        h.ctrl('k');
        h.type_text("explain");
        let rows: Vec<String> =
            h.app.command_rows().iter().flat_map(|r| [r.name.to_string(), r.label.to_string()]).collect();
        h.key(KeyCode::Esc);
        rows
    };
    assert!(menu(&mut h).contains(&as_plan), "offered on MySQL");
    // The session says its server gives none (MariaDB).
    h.db(DbEvent::Explain(None));
    let before = status(&h);
    assert!(!h.screen(100, 30).contains("view as plan"), "no hint");
    assert!(!menu(&mut h).contains(&as_plan));
    h.keys("P");
    h.keys(" ep");
    assert!(h.sent().is_empty());
    assert_eq!(h.app.tab().exec.view, ResultView::Rows);
    assert_eq!(status(&h), before, "nothing said");
    h.app.focus = Focus::Editor;
    edit(&mut h, "SELECT * FROM c;");
    let menu_now = menu(&mut h);
    assert!(!menu_now.contains(&explain) && !menu_now.contains(&analyze), "{menu_now:?}");
    let rows = commands(&mut h);
    assert!(!rows.iter().any(|r| r.starts_with(":explain") || *r == explain || *r == analyze), "{rows:?}");
    h.keys(" ex");
    h.keys(" ea");
    h.command("explain");
    assert!(h.sent().is_empty());
    // Its plan-shaped text is not read as a plan either.
    h.ctrl('e');
    let (id, _) = sent_one(&mut h);
    h.db(tree_page(id, &fixture("scan.tree.txt")));
    assert_eq!(h.app.tab().exec.view, ResultView::Rows);
    // The metadata session's word counts too: a server that gives the tree offers it all again.
    h.meta_db("local-my", DbEvent::Explain(Some(ExplainFormat::MySqlTree)));
    let menu_now = menu(&mut h);
    assert!(menu_now.contains(&explain) && menu_now.contains(&analyze), "{menu_now:?}");
    h.keys(" ex");
    assert_eq!(sent_one(&mut h).1, "EXPLAIN FORMAT=TREE SELECT * FROM c");
}

/// A format that reports the statement's own times keeps saying one it lacks as unknown
/// (PostgreSQL without its execution time), never the roots' total in its place.
#[test]
fn a_postgres_plan_without_its_execution_time_still_says_it_is_unknown() {
    let path = format!("{}/../core/src/sql/plan/fixtures/pg18/parallel.analyze.json", env!("CARGO_MANIFEST_DIR"));
    let json = std::fs::read_to_string(&path).unwrap().replace(",\n    \"Execution Time\": 133.009", "");
    assert!(!json.contains("Execution Time"), "the fixture's time is taken out");
    let mut h = Harness::connected(Lang::En);
    edit(&mut h, "SELECT * FROM t;");
    h.sent();
    h.keys(" ea");
    let (id, _) = sent_one(&mut h);
    h.db(DbEvent::Page {
        id,
        columns: Some(vec![meta("QUERY PLAN", "json", false, true)]),
        rows: vec![vec![Some(json)]],
        more: false,
        elapsed: Duration::from_millis(3),
    });
    let screen = h.screen(160, 45);
    assert!(screen.contains("execution ? · planning 7.803 ms"), "{screen}");
    assert!(!screen.contains("measured"), "{screen}");
}
