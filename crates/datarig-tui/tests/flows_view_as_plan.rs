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
    let line = h.status(220, H);
    assert!(line.contains(l.text(Lang::En)), "{l:?}: {line}");
}

/// The server is asked the allowlist's question first (`CheckRepeat`): it answers that it has
/// nothing against the statement.
fn allowed(h: &mut Harness) {
    let sent = h.sent();
    let Some(DbCommand::CheckRepeat { id, .. }) =
        sent.iter().find(|c| matches!(c, DbCommand::CheckRepeat { .. })).cloned()
    else {
        panic!("the server is asked first: {sent:?}")
    };
    h.db(DbEvent::RepeatChecked { id, result: Ok(()) });
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
    // `P`: the same statement as JSON, its options kept, once the server allows it; its plan
    // opens.
    h.keys("P");
    allowed(&mut h);
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
    allowed(&mut h);
    assert_eq!(sent_one(&mut h).1, "EXPLAIN (FORMAT JSON) SELECT * FROM t");
    // The results' action menu lists it first among the pane's actions.
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    h.keys("  ");
    let label = Label::ActionResultsViewAsPlan.text(Lang::En);
    assert!(h.menu_labels().iter().any(|l| l == label), "{:?}", h.menu_labels());
    h.menu_pick(label);
    allowed(&mut h);
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
    // A plain EXPLAIN of a write only plans, but it is off the allowlist of what runs again
    // unasked: it asks, and runs on read-only.
    let mut cfg = cfg.clone();
    cfg.connections[0].policy = Some("ro".into());
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    ran(&mut h, "EXPLAIN DELETE FROM t WHERE a = 1;", |id| text_page(id, TEXT_PLAN));
    h.keys("P");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    h.keys("y");
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

#[test]
fn an_execute_is_asked_about_since_its_parameters_run_again() {
    // Without ANALYZE too: the server evaluates the parameters to plan it.
    let mut h = text_plan("EXPLAIN EXECUTE p(nextval('s'));");
    h.keys("P");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), "it asks");
    assert!(runs(&mut h).is_empty(), "nothing runs before the answer");
    let screen = h.screen(W, H);
    assert!(screen.contains("Run this EXPLAIN again?"), "{screen}");
    assert!(screen.contains("not rolled back"), "{screen}");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty(), "Enter keeps");
}

#[test]
fn the_hint_names_the_key_of_the_focused_pane() {
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    h.app.focus = Focus::Editor;
    let screen = h.screen(W, H);
    assert!(screen.contains("text plan · Space e p view as plan"), "{screen}");
    h.app.focus = Focus::Results;
    assert!(h.screen(W, H).contains(HINT));
}

#[test]
fn a_result_with_statements_run_since_or_beside_it_is_refused() {
    // A later run without rows changed the session (a setting): the rows stay, from an
    // earlier run, and the plan now could differ.
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    h.app.tab_mut().editor = Editor::new("SET enable_seqscan = off;");
    h.app.focus = Focus::Editor;
    h.sent();
    h.ctrl('e');
    let (id, _) = sent_one(&mut h);
    h.db(DbEvent::Done {
        id,
        outcome: datarig_core::driver::Outcome::Command("SET".into()),
        elapsed: Duration::from_millis(1),
    });
    h.app.tab_mut().show_result(0);
    h.app.focus = Focus::Results;
    assert!(!h.screen(W, H).contains("view as plan"), "no offer for rows of an earlier run");
    h.keys("P");
    assert!(runs(&mut h).is_empty());
    said(&mut h, Label::PlanAsPlanSince);
    // A result of a run of several statements: what ran beside it is not run again.
    let mut h = Harness::connected(Lang::En);
    h.app.tab_mut().editor = Editor::new("SET work_mem = '1GB';\nEXPLAIN SELECT * FROM t;");
    h.sent();
    h.keys("ggVG");
    h.ctrl('e');
    let (id, _) = runs(&mut h).pop().expect("a run");
    h.db(text_page(id, TEXT_PLAN));
    h.app.focus = Focus::Results;
    assert!(!h.screen(W, H).contains("view as plan"), "no offer for a result of several");
    h.keys("P");
    assert!(runs(&mut h).is_empty());
    said(&mut h, Label::PlanAsPlanBatch);
    // The session the result came from is gone (disconnected): its settings went with it.
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    h.keys(" cx");
    h.sent();
    h.app.focus = Focus::Results;
    h.keys("P");
    assert!(runs(&mut h).is_empty());
    said(&mut h, Label::PlanAsPlanMoved);
}

#[test]
fn an_unavailable_view_as_plan_says_why() {
    // The text plan is not the shown view: the keys that show it are named.
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    h.key(KeyCode::Char('L'));
    assert_eq!(h.app.tab().exec.view, ResultView::Messages);
    h.keys(" ep");
    assert!(runs(&mut h).is_empty());
    let line = h.status(W, H);
    assert!(line.contains("show it first (H/L)"), "{line}");
    // A JSON plan's rows: a plan already.
    let mut h = Harness::connected(Lang::En);
    ran(&mut h, "EXPLAIN (FORMAT JSON) SELECT * FROM t;", |id| plan_page(id, &fixture(17, "join.plan.json")));
    h.key(KeyCode::Char('H'));
    h.keys("P");
    said(&mut h, Label::PlanAsPlanAlreadyJson);
    // The results pane is hidden.
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    h.app.focus = Focus::Editor;
    h.keys(" rh");
    h.keys(" ep");
    assert!(runs(&mut h).is_empty());
    let line = h.status(W, H);
    assert!(line.contains("show it first"), "{line}");
}

#[test]
fn the_hint_follows_the_result_shown_from_run_to_run() {
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    assert!(h.screen(W, H).contains(HINT));
    // While the next run waits, its rows replace these in the same place: the hint follows.
    h.app.focus = Focus::Editor;
    h.app.tab_mut().editor = Editor::new("SELECT a FROM t;");
    h.ctrl('e');
    let (id, _) = sent_one(&mut h);
    assert!(!h.screen(W, H).contains("view as plan"), "the earlier rows, while the run waits");
    h.db(text_page(id, &["1"]));
    assert!(!h.screen(W, H).contains("view as plan"), "{}", h.screen(W, H));
    ran(&mut h, "EXPLAIN SELECT a FROM t;", |id| text_page(id, TEXT_PLAN));
    assert!(h.screen(W, H).contains(HINT));
}

#[test]
fn in_insert_mode_the_hint_names_no_key_that_would_type() {
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    h.app.focus = Focus::Editor;
    h.keys("i");
    let screen = h.screen(W, H);
    assert!(!screen.contains("Space e p"), "{screen}");
    assert!(screen.contains("text plan · Ctrl+K “view as plan”"), "{screen}");
}

#[test]
fn a_plain_explain_runs_again_only_once_the_server_allows_it() {
    use datarig_core::driver::DbError;
    use datarig_core::sql::risk::repeat::NotRepeatable;
    // The text is on the allowlist: the server is asked first (views, overloads), not the
    // statement.
    let check = |h: &mut Harness| -> u64 {
        let sent = h.sent();
        assert!(!sent.iter().any(|c| matches!(c, DbCommand::Execute { .. })), "nothing runs yet: {sent:?}");
        let Some(DbCommand::CheckRepeat { id, sql }) =
            sent.into_iter().find(|c| matches!(c, DbCommand::CheckRepeat { .. }))
        else {
            panic!("the server is asked first")
        };
        assert_eq!(sql, "SELECT * FROM t");
        id
    };
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    h.keys("P");
    let id = check(&mut h);
    assert_eq!(h.overlay_kind(), None);
    h.db(DbEvent::RepeatChecked { id, result: Ok(()) });
    assert_eq!(sent_one(&mut h).1, "EXPLAIN (FORMAT JSON) SELECT * FROM t");
    // The server says no (a view, a shadowed name): it asks, Enter keeps, y runs.
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    h.keys("P");
    let id = check(&mut h);
    h.db(DbEvent::RepeatChecked { id, result: Err(DbError::NotRepeatable(NotRepeatable::NotATable("t".into()))) });
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    assert!(h.screen(W, H).contains("Run this EXPLAIN again?"));
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    // An answer for a result that changed since is dropped.
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    h.keys("P");
    let id = check(&mut h);
    ran(&mut h, "SELECT a FROM t;", |q| text_page(q, &["1"]));
    h.db(DbEvent::RepeatChecked { id, result: Ok(()) });
    assert!(runs(&mut h).is_empty() && h.overlay_kind().is_none());
}

/// The `CheckRepeat` just sent, if one was.
fn check_sent(h: &mut Harness) -> Option<u64> {
    h.sent().into_iter().find_map(|c| match c {
        DbCommand::CheckRepeat { id, .. } => Some(id),
        _ => None,
    })
}

#[test]
fn a_wait_for_the_server_ends_with_its_session_or_a_cancel() {
    use datarig_core::driver::DbError;
    // The session is lost while the server is asked: the wait ends, said once, and the next
    // press asks again (not "a query is running").
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    h.keys("P");
    assert!(check_sent(&mut h).is_some());
    h.tab_db(0, DbEvent::Lost { error: DbError::Server("gone".into()) });
    said(&mut h, Label::PlanAsPlanWaitEnded);
    ran(&mut h, "EXPLAIN SELECT a FROM t;", |id| text_page(id, TEXT_PLAN));
    h.keys("P");
    assert!(check_sent(&mut h).is_some(), "asked again");
    // Ctrl+C on the tab ends the wait too, and a late answer does nothing.
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    h.keys("P");
    let id = check_sent(&mut h).expect("asked");
    h.ctrl('c');
    said(&mut h, Label::PlanAsPlanWaitEnded);
    h.db(DbEvent::RepeatChecked { id, result: Ok(()) });
    assert!(runs(&mut h).is_empty() && h.overlay_kind().is_none());
    h.keys("P");
    assert!(check_sent(&mut h).is_some(), "asked again");
}

#[test]
fn the_wait_is_shown_and_a_late_question_never_lands_on_what_the_user_does() {
    use datarig_core::driver::DbError;
    use datarig_core::sql::risk::repeat::NotRepeatable;
    let refused = || Err(DbError::NotRepeatable(NotRepeatable::NotATable("v".into())));
    let mut h = text_plan("EXPLAIN SELECT * FROM v;");
    h.keys("P");
    let id = check_sent(&mut h).expect("asked");
    said(&mut h, Label::PlanAsPlanChecking);
    // A second press says it waits, it does not claim a query runs.
    h.keys("P");
    said(&mut h, Label::PlanAsPlanChecking);
    // The command line is open when the refusal comes: no question over it, a notice instead.
    h.keys(":");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Commands));
    h.db(DbEvent::RepeatChecked { id, result: refused() });
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Commands));
    let line = h.status(220, H);
    assert!(line.contains("press P again"), "{line}");
    h.key(KeyCode::Esc);
    // Asked again with nothing on top: the question opens, and takes no `y` before it is armed.
    h.keys("P");
    let id = check_sent(&mut h).expect("asked again");
    h.db(DbEvent::RepeatChecked { id, result: refused() });
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    h.draw(W, H);
    h.keys("y");
    assert!(runs(&mut h).is_empty(), "a `y` typed as it opened does not run it");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    h.advance(Duration::from_millis(500));
    h.keys("y");
    assert_eq!(sent_one(&mut h).1, "EXPLAIN (FORMAT JSON) SELECT * FROM v");
}

#[test]
fn a_press_on_another_tab_ends_the_first_wait_with_a_word() {
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    let first = h.app.tab().id;
    h.keys("P");
    let id = check_sent(&mut h).expect("asked");
    h.app.focus = Focus::Editor;
    h.ctrl('t');
    h.sent();
    ran(&mut h, "EXPLAIN SELECT a FROM t;", |q| text_page(q, TEXT_PLAN));
    h.keys("P");
    assert!(check_sent(&mut h).is_some());
    let t = h.app.tabs.get(first).expect("the first tab");
    assert_eq!(t.status.as_ref().map(|n| n.msg.clone()), Some(Label::PlanAsPlanStale.into()));
    // Its late answer does nothing.
    h.db(DbEvent::RepeatChecked { id, result: Ok(()) });
    assert!(runs(&mut h).is_empty());
}

#[test]
fn a_question_on_another_tab_ends_the_first_wait_too() {
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    let first = h.app.tab().id;
    h.keys("P");
    let id = check_sent(&mut h).expect("asked");
    h.app.focus = Focus::Editor;
    h.ctrl('t');
    h.sent();
    ran(&mut h, "EXPLAIN ANALYZE SELECT a FROM t;", |q| text_page(q, TEXT_PLAN));
    h.keys("P");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), "the ANALYZE question");
    let t = h.app.tabs.get(first).expect("the first tab");
    assert_eq!(t.status.as_ref().map(|n| n.msg.clone()), Some(Label::PlanAsPlanStale.into()));
    h.key(KeyCode::Esc);
    h.db(DbEvent::RepeatChecked { id, result: Ok(()) });
    assert!(runs(&mut h).is_empty());
}

#[test]
fn the_asking_status_goes_with_the_wait() {
    use datarig_core::driver::DbError;
    use datarig_core::sql::risk::repeat::NotRepeatable;
    let asking = Label::PlanAsPlanChecking.text(Lang::En);
    // Ctrl+C ends the wait: once the notice of the moment is gone, the bar says it ended.
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    h.keys("P");
    h.ctrl('c');
    h.app.transient = None;
    let line = h.status(220, H);
    assert!(!line.contains(asking) && line.contains(Label::PlanAsPlanWaitEnded.text(Lang::En)), "{line}");
    // The self-opened question declined: not "asking" any more either.
    let mut h = text_plan("EXPLAIN SELECT * FROM v;");
    h.keys("P");
    let id = check_sent(&mut h).expect("asked");
    h.db(DbEvent::RepeatChecked { id, result: Err(DbError::NotRepeatable(NotRepeatable::NotATable("v".into()))) });
    let line = h.status(220, H);
    assert!(!line.contains(asking), "the question replaces it: {line}");
    h.draw(W, H);
    h.advance(Duration::from_millis(500));
    h.key(KeyCode::Esc);
    h.app.transient = None;
    let line = h.status(220, H);
    assert!(!line.contains(asking) && line.contains(Label::SafetyConfirmCancelled.text(Lang::En)), "{line}");
}

#[test]
fn a_self_opened_question_takes_no_key_before_it_is_armed() {
    use datarig_core::driver::DbError;
    use datarig_core::sql::risk::repeat::NotRepeatable;
    // The focus moved to the editor while the server was asked: an `n`, Esc or Enter typed as
    // the question comes up does not dismiss it unseen.
    let mut h = text_plan("EXPLAIN SELECT * FROM v;");
    h.keys("P");
    let id = check_sent(&mut h).expect("asked");
    h.app.focus = Focus::Editor;
    h.db(DbEvent::RepeatChecked { id, result: Err(DbError::NotRepeatable(NotRepeatable::NotATable("v".into()))) });
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    h.draw(W, H);
    for k in [KeyCode::Char('n'), KeyCode::Esc, KeyCode::Enter, KeyCode::Char('y')] {
        h.key(k);
        assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), "{k:?} before it is armed");
    }
    assert!(runs(&mut h).is_empty());
    // Armed: it answers as any question does.
    h.advance(Duration::from_millis(500));
    h.key(KeyCode::Char('n'));
    assert_eq!(h.overlay_kind(), None);
    assert!(runs(&mut h).is_empty());
}

#[test]
fn a_session_closed_under_the_wait_ends_it_once() {
    // A disconnect closes the tab's session (`Tab::session_closed`): the wait was no run, so
    // only its own end is said, once; the closed session's answer is never read.
    let mut h = text_plan("EXPLAIN SELECT * FROM t;");
    let tab = h.app.tab().id;
    h.keys("P");
    let id = check_sent(&mut h).expect("asked");
    h.app.focus = Focus::Editor;
    h.keys(" cx");
    let ended = |h: &Harness| {
        let t = h.app.tabs.get(tab).expect("the tab");
        t.status.as_ref().map(|n| n.msg.clone()) == Some(Label::PlanAsPlanWaitEnded.into())
    };
    assert!(ended(&h));
    // The disconnect's own notice of the moment goes first; then the bar says the wait ended.
    h.app.transient = None;
    said(&mut h, Label::PlanAsPlanWaitEnded);
    let notes = h.app.tab().exec.run.notes.len();
    h.db(DbEvent::RepeatChecked { id, result: Ok(()) });
    h.keys("j");
    assert!(runs(&mut h).is_empty() && h.overlay_kind().is_none());
    assert!(ended(&h), "said once, nothing after");
    assert_eq!(h.app.tab().exec.run.notes.len(), notes, "no note of a cancelled run");
}
