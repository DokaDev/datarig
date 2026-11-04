//! Asking for an Apply (`space d a`, `:apply`): the rows found by their values counted afresh
//! first, the changes' problems shown in the Changes tab instead of a question, the preview only
//! for a choice among identical rows, an unknown result or an environment that confirms every
//! write; else one line in the status bar, where only `y` applies and any other key, a click or
//! a paste cancels and does nothing else. After the Apply the Changes tab shows what was applied
//! and the statements sent, or why nothing was.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::structure::TableStructure;
use datarig_core::driver::{AppliedStep, ApplyError, ApplyFailure, ApplyTx, ColumnMeta, DbCommand, DbError, DbEvent};
use datarig_core::edit::{Expect, Step};
use datarig_core::i18n::{Label, Lang, Msg};
use datarig_core::policy::{CommitRule, Confirm, Policy};
use datarig_core::profile::ConnectionConfig;
use datarig_tui::app::edit::{EditState, PreviewLine, Target};
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::tabs::ResultView;
use datarig_tui::app::{Focus, Results};
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::{Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use std::time::Duration;

fn s(v: &str) -> Option<String> {
    Some(v.to_string())
}

fn users_columns() -> Vec<ColumnMeta> {
    vec![
        meta_from("id", "int8", true, USERS, 1),
        meta_from("email", "text", false, USERS, 2),
        meta_from("nickname", "text", false, USERS, 4),
    ]
}

fn users_rows() -> Vec<Vec<Option<String>>> {
    vec![vec![s("1"), s("a@x"), s("ann")], vec![s("2"), s("b@x"), None], vec![s("3"), s("c@x"), s("cy")]]
}

const SQL: &str = "SELECT id, email, nickname FROM shop.users";
const CHECK_1: &str = r#"SELECT 1 FROM "shop"."users" WHERE "id" = '1' AND "email" = 'a@x' FOR UPDATE NOWAIT"#;
const UPDATE_1: &str = r#"UPDATE "shop"."users" SET "email" = 'z@x' WHERE "id" = '1' AND "email" = 'a@x' RETURNING *"#;

fn structure(h: &mut Harness, st: TableStructure) {
    let name = h.app.profiles[0].name.clone();
    h.meta_db(&name, DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Ok(Box::new(st)) });
}

/// Run the users' query and let its rows come, the grid focused.
fn answer(h: &mut Harness) {
    h.app.run(vec![SQL.to_string()]);
    let id = h.app.tab().exec.query_id;
    h.db(DbEvent::Page {
        id,
        columns: Some(users_columns()),
        rows: users_rows(),
        more: false,
        elapsed: Duration::ZERO,
    });
    h.app.focus = Focus::Results;
}

/// `h` connected, the users' rows in the grid, resolved with `st`; nothing sent yet.
fn resolved(mut h: Harness, st: TableStructure) -> Harness {
    h.db(DbEvent::Keys(Ok(shop_keys())));
    answer(&mut h);
    h.keys("le");
    structure(&mut h, st);
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    h.key(KeyCode::Esc);
    h.app.transient = None;
    h.sent();
    h
}

/// `shop.users` whose `name` (NOT NULL, not in the results here) has a default.
fn named() -> TableStructure {
    let mut st = users_structure();
    st.columns[2].default = Some("''::text".into());
    st
}

/// The profile in environment `stg` with `policy`.
fn in_env(policy: Policy, lang: Lang) -> Harness {
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("stg".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert("stg", policy);
    Harness::connected_with(&cfg, lang)
}

/// The users' rows by their key.
fn ready(lang: Lang) -> Harness {
    resolved(Harness::connected(lang), named())
}

/// The users' rows by their key, in an environment that commits by hand.
fn manual(lang: Lang) -> Harness {
    resolved(in_env(Policy { commit: CommitRule::Manual, ..Policy::default() }, lang), named())
}

/// `shop.users` without a key: its rows are found by their values. `four`: only its first four
/// columns (identical rows are a choice).
fn keyless(four: bool) -> TableStructure {
    let mut st = named();
    st.primary_key = None;
    st.unique_constraints.clear();
    st.indexes.clear();
    if four {
        st.columns.truncate(4);
    }
    st
}

fn by_values(lang: Lang) -> Harness {
    resolved(Harness::connected(lang), keyless(false))
}

fn edits(h: &Harness) -> &EditState {
    match &h.app.tab().results {
        Results::Rows(rs) => &rs.edit,
        _ => panic!("no row result"),
    }
}

fn edits_mut(h: &mut Harness) -> &mut EditState {
    match &mut h.app.tab_mut().results {
        Results::Rows(rs) => &mut rs.edit,
        _ => panic!("no row result"),
    }
}

fn said(h: &mut Harness) -> Option<Msg> {
    h.app.transient.take().map(|(n, _)| n.msg)
}

/// Type `v` into the cursor's cell.
fn type_cell(h: &mut Harness, v: &str) {
    h.keys("e");
    let n = h.app.tab().cell_edit.as_ref().map_or(0, |e| e.input.text().chars().count());
    for _ in 0..n {
        h.key(KeyCode::Backspace);
    }
    h.type_text(v);
    h.key(KeyCode::Enter);
}

fn counts(h: &mut Harness) -> Vec<(u64, String)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Count { id, sql } => Some((id, sql)),
            _ => None,
        })
        .collect()
}

/// The Apply sent since the last call: its id, its steps and its transaction.
fn applies(h: &mut Harness) -> Option<(u64, Vec<Step>, ApplyTx)> {
    h.sent().into_iter().find_map(|c| match c {
        DbCommand::ApplyEdits { id, steps, tx } => Some((id, steps, tx)),
        _ => None,
    })
}

/// What the status bar asks.
fn asked(h: &Harness) -> Option<String> {
    h.app.status_ask_line().map(|(text, _, _)| text)
}

/// Every step of `steps` stood, affecting the rows it expected, nothing returned.
fn stood(steps: &[Step]) -> Vec<AppliedStep> {
    steps
        .iter()
        .map(|s| {
            let n = match s.expect {
                Expect::Rows(n) | Expect::Affected(n) => n,
            };
            AppliedStep { count: n, ..AppliedStep::default() }
        })
        .collect()
}

fn screen(h: &mut Harness, w: u16, hh: u16) -> String {
    buffer_text(h.draw(w, hh).backend().buffer())
}

/// An update of row 1 and a delete of row 2, asked for: the status bar asks.
fn two_asked(lang: Lang) -> Harness {
    let mut h = ready(lang);
    type_cell(&mut h, "z@x");
    h.keys("jD");
    h.keys(" da");
    h.app.transient = None;
    h
}

#[test]
fn the_apply_is_asked_on_the_status_line_and_y_sends_it() {
    let mut h = two_asked(Lang::En);
    assert!(h.app.overlays.top().is_none(), "no dialog");
    assert_eq!(h.app.key_context(), Ctx::StatusAsk);
    assert_eq!(
        asked(&h).as_deref(),
        Some("Apply 2 changes to shop.users (1 update · 1 delete)?  y apply · any other key cancels")
    );
    assert!(applies(&mut h).is_none(), "nothing sent before y");
    h.key(KeyCode::Char('y'));
    let (_, steps, tx) = applies(&mut h).expect("y sends the Apply");
    assert_eq!(tx, ApplyTx::Commit);
    assert!(steps.iter().any(|s| s.sql == UPDATE_1) && steps.iter().any(|s| s.sql == CHECK_1));
    assert!(asked(&h).is_none());
    assert!(edits(&h).applying.is_some());
    // `:apply` asks the same.
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    assert_eq!(asked(&h).as_deref(), Some("Apply 1 change to shop.users (1 update)?  y apply · any other key cancels"));
}

#[test]
fn the_question_says_how_the_apply_stands_to_a_transaction() {
    let mut h = manual(Lang::En);
    type_cell(&mut h, "z@x");
    h.keys(" da");
    assert_eq!(
        asked(&h).as_deref(),
        Some(
            "Apply 1 change to shop.users (1 update)? Opens a transaction; nothing is committed.  y apply · any other key cancels"
        )
    );
    h.key(KeyCode::Char('y'));
    let (_, _, tx) = applies(&mut h).expect("sent");
    assert!(matches!(tx, ApplyTx::Keep { .. }), "{tx:?}");
    // Inside an open block.
    let mut h = ready(Lang::En);
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    type_cell(&mut h, "z@x");
    h.keys(" da");
    let text = asked(&h).expect("asked");
    assert!(text.contains("? Runs inside the open transaction; nothing is committed.  y apply"), "{text}");
}

#[test]
fn a_narrow_window_asks_in_short_naming_the_deletes() {
    let mut h = two_asked(Lang::En);
    let status = h.status(80, 24);
    assert!(status.contains("Apply 2 changes (1 delete)? y/n"), "{status}");
    let status = h.status(120, 40);
    assert!(status.contains("Apply 2 changes to shop.users (1 update · 1 delete)?"), "{status}");
    let mut h = ready(Lang::En);
    h.keys("D");
    h.keys(" da");
    assert_eq!(h.app.status_ask_line().map(|(_, short, _)| short).as_deref(), Some("Apply 1 change (1 delete)? y/n"));
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.keys(" da");
    assert_eq!(h.app.status_ask_line().map(|(_, short, _)| short).as_deref(), Some("Apply 1 change? y/n"));
}

/// Where the grid is and what is pending: nothing of it may move while the question cancels.
fn grid_state(h: &Harness) -> (usize, usize, Option<(usize, usize)>, u64, ResultView, Focus) {
    let t = h.app.tab();
    (t.grid.row, t.grid.col, t.grid.anchor, edits(h).batch.revision(), t.exec.view, h.app.focus)
}

#[test]
fn every_other_key_a_click_or_a_paste_cancels_and_does_nothing_else() {
    let keys: Vec<(KeyCode, KeyModifiers)> = vec![
        (KeyCode::Enter, KeyModifiers::NONE),
        (KeyCode::Esc, KeyModifiers::NONE),
        (KeyCode::Char('n'), KeyModifiers::NONE),
        (KeyCode::Char('j'), KeyModifiers::NONE),
        (KeyCode::Char('D'), KeyModifiers::SHIFT),
        (KeyCode::Char('u'), KeyModifiers::NONE),
        (KeyCode::Char('v'), KeyModifiers::NONE),
        (KeyCode::Char(':'), KeyModifiers::NONE),
        (KeyCode::Char(' '), KeyModifiers::NONE),
        (KeyCode::Char('Y'), KeyModifiers::SHIFT),
        (KeyCode::Char('y'), KeyModifiers::CONTROL),
        (KeyCode::Down, KeyModifiers::NONE),
        (KeyCode::Tab, KeyModifiers::NONE),
    ];
    for (code, mods) in keys {
        let mut h = two_asked(Lang::En);
        let before = grid_state(&h);
        h.key_mod(code, mods);
        assert!(asked(&h).is_none(), "{code:?} cancels");
        assert_eq!(said(&mut h), Some(Label::EditAskCancelled.into()), "{code:?}");
        assert_eq!(grid_state(&h), before, "{code:?} did nothing else");
        assert!(h.app.overlays.top().is_none(), "{code:?} opened nothing");
        assert!(applies(&mut h).is_none(), "{code:?}: nothing sent");
        assert_eq!(h.app.key_context(), Ctx::Grid);
    }
    // A click on another row of the grid, the wheel, a right click.
    for kind in
        [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Down(MouseButton::Right), MouseEventKind::ScrollDown]
    {
        let mut h = two_asked(Lang::En);
        let _ = h.draw(120, 40);
        let g = h.app.layout.results;
        let before = grid_state(&h);
        h.mouse(kind, g.x + 8, g.y + 5);
        assert!(asked(&h).is_none(), "{kind:?} cancels");
        assert_eq!(said(&mut h), Some(Label::EditAskCancelled.into()), "{kind:?}");
        assert_eq!(grid_state(&h), before, "{kind:?} did nothing else");
        assert!(h.app.overlays.top().is_none(), "{kind:?} opened nothing");
        assert!(applies(&mut h).is_none());
    }
    // A paste.
    let mut h = two_asked(Lang::En);
    let before = grid_state(&h);
    h.event(Event::Paste("y".into()));
    assert!(asked(&h).is_none());
    assert_eq!(said(&mut h), Some(Label::EditAskCancelled.into()));
    assert_eq!(grid_state(&h), before);
    assert!(applies(&mut h).is_none(), "a pasted y is not a y");
}

/// A trigger that moves what the question is about drops it, said; `y` then does nothing.
#[test]
fn the_question_is_dropped_when_what_it_asks_about_moves() {
    let moved = Some(Label::EditAskMoved.into());
    // An edit (undone under it).
    let mut h = two_asked(Lang::En);
    edits_mut(&mut h).batch.undo();
    h.db(DbEvent::Counted { id: 999_999, result: Ok(1), snapshot: false });
    assert!(asked(&h).is_none());
    assert_eq!(said(&mut h), moved);
    // The tab opened a transaction.
    let mut h = two_asked(Lang::En);
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    assert!(asked(&h).is_none());
    assert_eq!(said(&mut h), moved);
    // A run started in the tab.
    let mut h = two_asked(Lang::En);
    h.app.run(vec!["SELECT 1".into()]);
    h.db(DbEvent::Counted { id: 999_999, result: Ok(1), snapshot: false });
    assert!(asked(&h).is_none());
    // Another tab.
    let mut h = two_asked(Lang::En);
    h.app.dispatch(datarig_tui::app::action::Action::NewTab);
    h.db(DbEvent::Counted { id: 999_999, result: Ok(1), snapshot: false });
    assert!(asked(&h).is_none());
    assert_eq!(said(&mut h), moved);
    // The session lost.
    let mut h = two_asked(Lang::En);
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    assert!(asked(&h).is_none());
    // The statements moved without an edit (the table's structure read again).
    let mut h = two_asked(Lang::En);
    if let Target::Ready(t) = &mut edits_mut(&mut h).target {
        t.table.schema = "other".into();
    }
    h.db(DbEvent::Counted { id: 999_999, result: Ok(1), snapshot: false });
    assert!(asked(&h).is_none(), "the statements are not those asked about");
    assert_eq!(said(&mut h), moved);
    h.key(KeyCode::Char('y'));
    assert!(applies(&mut h).is_none(), "nothing sent");
}

#[test]
fn the_dialog_opens_for_an_environment_that_confirms_writes_in_the_sql_view() {
    let mut h = resolved(in_env(Policy { confirm: Confirm::Writes, ..Policy::default() }, Lang::En), named());
    type_cell(&mut h, "z@x");
    h.keys(" da");
    assert!(asked(&h).is_none());
    assert_eq!(h.overlay_kind(), Some(OverlayKind::EditPreview));
    let v = h.app.edit_preview_view().expect("open");
    assert!(v.lines.iter().any(|l| matches!(l, PreviewLine::Step { sql, .. } if sql == UPDATE_1)), "{:?}", v.lines);
    h.key(KeyCode::Char('y'));
    assert!(applies(&mut h).is_some(), "y in the dialog applies");
}

#[test]
fn the_dialog_opens_for_an_apply_whose_result_is_unknown() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    edits_mut(&mut h).apply_unknown = true;
    h.keys(" da");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::EditPreview));
    h.key(KeyCode::Char('y'));
    assert!(applies(&mut h).is_none(), "the first y warns");
    h.key(KeyCode::Char('y'));
    assert!(applies(&mut h).is_some(), "the second applies");
}

#[test]
fn identical_rows_open_the_dialog_and_the_tab_says_the_apply_asks() {
    let columns = vec![
        meta_from("id", "int8", true, USERS, 1),
        meta_from("email", "text", false, USERS, 2),
        meta_from("name", "text", false, USERS, 3),
        meta_from("nickname", "text", false, USERS, 4),
    ];
    let rows = vec![vec![s("1"), s("a@x"), s("Ann"), s("ann")], vec![s("1"), s("a@x"), s("Ann"), s("ann")]];
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    h.app.run(vec!["SELECT id, email, name, nickname FROM shop.users".into()]);
    let id = h.app.tab().exec.query_id;
    h.db(DbEvent::Page { id, columns: Some(columns), rows, more: false, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
    h.keys("le");
    structure(&mut h, keyless(true));
    h.key(KeyCode::Esc);
    type_cell(&mut h, "z@x");
    h.sent();
    h.keys(" da");
    let asked_counts = counts(&mut h);
    h.db(DbEvent::Counted { id: asked_counts[0].0, result: Ok(2), snapshot: false });
    assert_eq!(h.overlay_kind(), Some(OverlayKind::EditPreview), "the choice is made in the dialog");
    let v = h.app.edit_preview_view().unwrap();
    assert!(matches!(&v.lines[..], [PreviewLine::Choose { keys, .. }] if keys == "[1] just one   [a] all 2"));
    // The Changes tab names no key that does nothing there.
    h.key(KeyCode::Esc);
    h.command("changes");
    h.keys("s");
    let s = screen(&mut h, 120, 40);
    assert!(s.contains("The Apply asks which") && !s.contains("[1] just one"), "{s}");
    // In the dialog, `a` answers and `y` applies.
    h.keys(" da");
    let asked_counts = counts(&mut h);
    h.db(DbEvent::Counted { id: asked_counts[0].0, result: Ok(2), snapshot: false });
    h.key(KeyCode::Char('a'));
    h.key(KeyCode::Char('y'));
    assert!(applies(&mut h).is_some());
}

#[test]
fn problems_block_the_apply_and_the_changes_tab_shows_the_first() {
    // `name` is NOT NULL, has no default and is not in the result: a new row cannot be added.
    let mut h = resolved(Harness::connected(Lang::En), users_structure());
    type_cell(&mut h, "z@x");
    h.keys("ol");
    type_cell(&mut h, "n@x");
    h.command("changes");
    h.keys("G");
    h.keys("H");
    assert_eq!(h.app.tab().exec.view, ResultView::Rows);
    h.keys(" da");
    assert!(asked(&h).is_none() && h.app.overlays.top().is_none(), "nothing asked");
    assert_eq!(said(&mut h), Some(Msg::EditAskProblems { count: 1 }));
    assert_eq!(h.app.key_context(), Ctx::Changes);
    assert_eq!(edits(&h).cursor.line, 0, "the first problem selected");
    let s = screen(&mut h, 120, 40);
    assert!(s.contains("! Row 4: name is NOT NULL, has no default and is not in the result"), "{s}");
    assert!(applies(&mut h).is_none());
}

#[test]
fn the_rows_are_counted_again_on_every_apply_and_an_edit_drops_the_wait() {
    let mut h = by_values(Lang::En);
    type_cell(&mut h, "z@x");
    // The Changes tab counts them once the changes settle.
    h.command("changes");
    h.advance(Duration::from_millis(600));
    let tab = counts(&mut h);
    assert_eq!(tab.len(), 1, "the tab counted");
    h.db(DbEvent::Counted { id: tab[0].0, result: Ok(1), snapshot: false });
    // The Apply counts them again, whatever the tab has.
    h.keys(" da");
    let asked_counts = counts(&mut h);
    assert_eq!(asked_counts.len(), 1, "counted again");
    assert!(asked(&h).is_none());
    assert!(h.status(120, 40).contains("Checking 1 row…"));
    // Esc stops waiting.
    h.key(KeyCode::Esc);
    assert_eq!(said(&mut h), Some(Label::EditAskCancelled.into()));
    assert!(h.app.apply_wait.is_none());
    h.db(DbEvent::Counted { id: asked_counts[0].0, result: Ok(1), snapshot: false });
    assert!(asked(&h).is_none(), "an answer after Esc asks nothing");
    // An edit before the answer drops the Apply.
    h.keys("H");
    h.keys(" da");
    let asked_counts = counts(&mut h);
    h.keys("j");
    type_cell(&mut h, "y@x");
    assert!(h.app.apply_wait.is_none());
    assert_eq!(said(&mut h), Some(Msg::EditAskChanged { keys: "Space d a".into() }));
    h.db(DbEvent::Counted { id: asked_counts[0].0, result: Ok(1), snapshot: false });
    assert!(asked(&h).is_none() && h.app.overlays.top().is_none(), "the stale answer asks nothing");
    // Counted for the changes as they are: asked.
    h.keys(" da");
    let asked_counts = counts(&mut h);
    assert_eq!(asked_counts.len(), 2);
    for (id, _) in asked_counts {
        h.db(DbEvent::Counted { id, result: Ok(1), snapshot: false });
    }
    assert!(asked(&h).is_some_and(|t| t.starts_with("Apply 2 changes to shop.users")), "{:?}", asked(&h));
}

/// The update and delete of `two_asked`, applied and answered.
fn two_applied(mut h: Harness) -> (Harness, Vec<Step>) {
    h.key(KeyCode::Char('y'));
    let (id, steps, _) = applies(&mut h).expect("sent");
    h.db(DbEvent::Applied { id, result: Ok(stood(&steps)) });
    h.app.transient = None;
    (h, steps)
}

#[test]
fn the_changes_tab_shows_what_was_applied_until_the_query_runs_again() {
    let (mut h, steps) = two_applied(two_asked(Lang::En));
    assert!(edits(&h).batch.is_empty());
    h.command("changes");
    assert_eq!(h.app.key_context(), Ctx::Changes, "the tab stays after the Apply");
    let s = screen(&mut h, 120, 40);
    assert!(s.contains("Applied · re-run to refresh"), "{s}");
    assert!(s.contains("- id=2") && s.contains("~ id=1") && s.contains("'a@x' → 'z@x'"), "{s}");
    // The statements sent, copied whole.
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("c");
    let sent: String = steps.iter().map(|s| format!("{};\n", s.sql)).collect();
    assert_eq!(clip.last(), Some(sent));
    h.keys("s");
    let s = screen(&mut h, 120, 40);
    assert!(s.contains("UPDATE \"shop\".\"users\" SET \"email\" = 'z@x'"), "{s}");
    // Run again: the tab goes.
    answer(&mut h);
    assert!(!h.app.tab().has_changes());
    assert_eq!(h.app.tab().exec.view, ResultView::Rows);
}

#[test]
fn a_rollback_takes_the_applied_changes_and_the_tab_says_so() {
    let mut h = manual(Lang::En);
    type_cell(&mut h, "z@x");
    h.keys(" da");
    h.key(KeyCode::Char('y'));
    let (id, steps, _) = applies(&mut h).expect("sent");
    h.db(DbEvent::Applied { id, result: Ok(stood(&steps)) });
    h.tab_db(0, DbEvent::ManualBegun { id, index: 0 });
    h.db(DbEvent::Block(true));
    h.db(DbEvent::TxOpen(true));
    h.command("changes");
    assert!(screen(&mut h, 120, 40).contains("Applied · not committed"));
    h.command("rollback");
    let end = h.sent().into_iter().find_map(|c| match c {
        DbCommand::EndTx { id, commit: false } => Some(id),
        _ => None,
    });
    let outcome = datarig_core::driver::TxOutcome::RolledBack;
    h.tab_db(0, DbEvent::TxEnded { id: end.expect("a rollback sent"), outcome });
    h.db(DbEvent::Block(false));
    h.db(DbEvent::TxOpen(false));
    assert!(edits(&h).applied.is_none());
    assert!(h.app.tab().has_changes(), "the tab stays, to say so");
    let s = screen(&mut h, 120, 40);
    assert!(s.contains("The transaction was rolled back; the applied changes are gone"), "{s}");
    assert!(s.contains("[Changes]"), "{s}");
}

#[test]
fn a_failure_marks_its_statement_and_the_changes_stay() {
    let mut h = two_asked(Lang::En);
    h.key(KeyCode::Char('y'));
    let (id, steps, _) = applies(&mut h).expect("sent");
    let at = steps.iter().position(|s| s.sql.starts_with("DELETE ")).expect("the delete");
    h.db(DbEvent::Applied { id, result: Err(ApplyFailure { step: Some(at), why: ApplyError::Count { got: 0 } }) });
    assert_eq!(edits(&h).batch.counts().total(), 2, "the changes stay");
    h.command("changes");
    h.keys("s");
    let s = screen(&mut h, 80, 24);
    assert!(s.contains("Row 2 (id = 2) was changed or deleted since it was read"), "{s}");
    assert!(s.contains("! DELETE FROM \"shop\".\"users\""), "the failing statement on screen, marked: {s}");
    assert!(edits(&h).cursor.sql_scroll > 0, "scrolled to it");
    // Asked again, it may be applied again.
    h.keys(" da");
    assert!(asked(&h).is_some());
}

#[test]
fn the_action_menu_offers_show_changes_and_apply() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.keys("  ");
    let labels = h.menu_labels();
    assert!(labels.iter().any(|l| l == "Show changes"), "{labels:?}");
    h.menu_pick("Apply the pending changes (asked first)");
    assert!(asked(&h).is_some(), "the menu asks as the key does");
    h.key(KeyCode::Esc);
    h.keys("  ");
    h.menu_pick("Show changes");
    assert_eq!(h.app.key_context(), Ctx::Changes);
}

#[test]
fn the_question_and_the_applied_view_on_screen() {
    for (w, hh) in [(120, 40), (80, 24)] {
        let mut h = two_asked(Lang::En);
        assert_screen!(format!("edit_ask_apply_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
        let (mut h, _) = two_applied(h);
        h.command("changes");
        h.app.transient = None;
        assert_screen!(format!("edit_ask_applied_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
    }
    for lang in [Lang::Ko] {
        let mut h = two_asked(lang);
        assert_screen!("edit_ask_apply_ko_120x40", lang, h.draw(120, 40));
    }
}
