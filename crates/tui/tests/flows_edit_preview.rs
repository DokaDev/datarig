//! The preview of a result's pending changes (`space d a`, `:apply`):
//! the statements that apply them, each with the count it must give; rows found by their values
//! counted first on the tab's session; why nothing can be applied yet; the transaction the Apply
//! runs in. `space d x` / `:discard` drops the changes once the user says so.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::structure::{ColumnFill, TableStructure};
use datarig_core::driver::{
    AppliedStep, ApplyTx, ColumnMeta, DbCommand, DbError, DbEvent, EditFormat, MySqlEdit, ReadIn, ValueKind,
};
use datarig_core::edit::{Expect, Step, Value};
use datarig_core::i18n::{Label, Lang, Msg};
use datarig_core::policy::{CommitRule, Policy};
use datarig_core::profile::ConnectionConfig;
use datarig_tui::app::edit::{Applied, ChangesModel, EditState, ListEntry, PreviewLine, RowAt, Target};
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::{Focus, Results};
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::KeyCode;
use std::time::Duration;

/// The profile's environment confirms every write: an Apply shows the preview before `y`.
fn writes_config() -> Config {
    use datarig_core::policy::Confirm;
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("stg".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert("stg", Policy { confirm: Confirm::Writes, ..Policy::default() });
    cfg
}

/// Connected under [`writes_config`].
fn connected(lang: Lang) -> Harness {
    Harness::connected_with(&writes_config(), lang)
}

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

/// Run `sql` and let `columns` and `rows` come back as its result, the grid focused.
fn answer(h: &mut Harness, sql: &str, columns: Vec<ColumnMeta>, rows: Vec<Vec<Option<String>>>) {
    h.app.run(vec![sql.to_string()]);
    let id = h.app.tab().exec.query_id;
    h.db(DbEvent::Page { id, columns: Some(columns), rows, more: false, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
}

fn structure(h: &mut Harness, st: TableStructure) {
    let name = h.app.profiles[0].name.clone();
    h.meta_db(&name, DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Ok(Box::new(st)) });
}

/// `h` connected, `sql`'s result in the grid, resolved with `st` (`e` on the first row's second
/// column, then the editor closed); nothing sent yet.
fn resolved(
    mut h: Harness,
    sql: &str,
    columns: Vec<ColumnMeta>,
    rows: Vec<Vec<Option<String>>>,
    st: TableStructure,
) -> Harness {
    h.db(DbEvent::Keys(Ok(shop_keys())));
    answer(&mut h, sql, columns, rows);
    h.keys("le");
    structure(&mut h, st);
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    h.key(KeyCode::Esc);
    h.app.transient = None;
    h.sent();
    h
}

/// `shop.users` whose `name` (NOT NULL, not in the results here) has a default: a new row may
/// leave it out.
fn named() -> TableStructure {
    let mut st = users_structure();
    st.columns[2].default = Some("''::text".into());
    st
}

/// The users' rows by their key.
fn ready(lang: Lang) -> Harness {
    resolved(connected(lang), "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows(), named())
}

/// `shop.users` without any key: its rows are found by their values. `four`: only its id,
/// email, name and nickname (every one shown and exact: identical rows are a choice).
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

/// The users' id, email and nickname, found by their values (identical rows are refused).
fn by_values(lang: Lang) -> Harness {
    resolved(
        connected(lang),
        "SELECT id, email, nickname FROM shop.users",
        users_columns(),
        users_rows(),
        keyless(false),
    )
}

/// Every column of a keyless `shop.users`: identical rows are a choice.
fn identical(lang: Lang) -> Harness {
    let columns = vec![
        meta_from("id", "int8", true, USERS, 1),
        meta_from("email", "text", false, USERS, 2),
        meta_from("name", "text", false, USERS, 3),
        meta_from("nickname", "text", false, USERS, 4),
    ];
    let rows = vec![vec![s("1"), s("a@x"), s("Ann"), s("ann")], vec![s("1"), s("a@x"), s("Ann"), s("ann")]];
    resolved(connected(lang), "SELECT id, email, name, nickname FROM shop.users", columns, rows, keyless(true))
}

fn edits(h: &Harness) -> &EditState {
    match &h.app.tab().results {
        Results::Rows(rs) => &rs.edit,
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

fn lines(h: &Harness) -> Vec<PreviewLine> {
    h.app.edit_preview_view().expect("the preview is open").lines
}

/// An Apply refused for the changes' problems: nothing asked, the Changes tab shows them with
/// the focus, the first selected, and the status says how many; their texts.
fn blocked(h: &mut Harness) -> Vec<String> {
    assert!(h.app.overlays.top().is_none() && h.app.ask.is_none(), "nothing asked");
    assert_eq!(h.app.key_context(), Ctx::Changes);
    let m = h.app.shown_changes().expect("a model");
    // A row's problems are listed under its lines.
    let first = m.entries.iter().position(|e| matches!(e, ListEntry::Problem(_)));
    assert_eq!(Some(edits(h).cursor.line), first, "the first problem selected");
    assert_eq!(said(h), Some(Msg::EditAskProblems { count: m.problems.len() as u64 }));
    assert!(!h.sent().iter().any(|c| matches!(c, DbCommand::ApplyEdits { .. })), "nothing applied");
    m.problems.iter().map(|p| p.text.clone()).collect()
}

fn step(sql: &str, n: u64, risky: bool) -> PreviewLine {
    PreviewLine::Step { sql: sql.into(), expect: format!("expect {n}"), risky }
}

fn head(text: &str) -> PreviewLine {
    PreviewLine::Head(text.into())
}

/// The counts the preview sent, with their ids.
fn counts(h: &mut Harness) -> Vec<(u64, String)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Count { id, sql } => Some((id, sql)),
            _ => None,
        })
        .collect()
}

const CHECK_1: &str = r#"SELECT 1 FROM "shop"."users" WHERE "id" = '1' AND "email" = 'a@x' FOR UPDATE NOWAIT"#;
const UPDATE_1: &str = r#"UPDATE "shop"."users" SET "email" = 'z@x' WHERE "id" = '1' AND "email" = 'a@x' RETURNING *"#;

#[test]
fn a_keyed_change_shows_its_check_and_its_write_and_cancel_is_the_default() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.keys(" da");
    assert_eq!(h.app.key_context(), Ctx::EditPreview);
    assert!(counts(&mut h).is_empty(), "a row found by its key is not counted");
    let v = h.app.edit_preview_view().unwrap();
    assert_eq!(v.title.to_string(), "Apply changes: shop.users");
    assert_eq!(v.summary, "1 update");
    assert_eq!(v.footer, "Runs in one transaction; commits at the end.");
    assert!(v.apply);
    assert_eq!(
        v.lines,
        [head("Row checks (lock, no waiting)"), step(CHECK_1, 1, false), head("Writes"), step(UPDATE_1, 1, false)]
    );
    // Enter cancels (Cancel has the focus), as Esc does; the changes stay.
    h.key(KeyCode::Enter);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h.keys(" da");
    assert_eq!(h.app.key_context(), Ctx::EditPreview, "space d a opens the same dialog again");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert_eq!(edits(&h).batch.counts().total(), 1);
    h.command("apply");
    assert_eq!(h.app.key_context(), Ctx::EditPreview, ":apply");
    h.key(KeyCode::Char('q'));
    assert!(h.sent().iter().all(|c| !matches!(c, DbCommand::ApplyEdits { .. })), "nothing applied");
}

#[test]
fn deletes_and_new_rows_come_after_the_checks_in_the_risk_tone_for_deletes() {
    let mut h = ready(Lang::En);
    h.keys("jD");
    h.keys("ol");
    type_cell(&mut h, "n@x");
    h.command("apply");
    let v = h.app.edit_preview_view().unwrap();
    assert_eq!(v.summary, "1 insert · 1 delete");
    assert_eq!(
        v.lines,
        [
            head("Row checks (lock, no waiting)"),
            step(
                r#"SELECT 1 FROM "shop"."users" WHERE "id" = '2' AND "email" = 'b@x' AND "nickname" IS NULL FOR UPDATE NOWAIT"#,
                1,
                false
            ),
            head("Writes"),
            step(r#"DELETE FROM "shop"."users" WHERE "id" = '2' AND "email" = 'b@x' AND "nickname" IS NULL"#, 1, true),
            step(r#"INSERT INTO "shop"."users" ("email") VALUES ('n@x') RETURNING *"#, 1, false),
        ]
    );
}

/// A new row's cells left to their default show what the server puts there (the id
/// `<generated>`, the nickname's literal), but the Apply still leaves them out: the server
/// applies its own defaults.
#[test]
fn the_defaults_a_new_row_shows_are_not_sent() {
    let mut st = named();
    st.columns[3].default = Some("'anon'::text".into());
    let sql = "SELECT id, email, nickname FROM shop.users";
    let mut h = resolved(connected(Lang::En), sql, users_columns(), users_rows(), st);
    h.keys("ol");
    type_cell(&mut h, "n@x");
    let screen = h.screen(120, 40);
    assert!(screen.contains("<generated>") && screen.contains("\u{2502} anon "), "{screen}");
    h.command("apply");
    let v = h.app.edit_preview_view().unwrap();
    assert_eq!(
        v.lines.last(),
        Some(&step(r#"INSERT INTO "shop"."users" ("email") VALUES ('n@x') RETURNING *"#, 1, false))
    );
}

#[test]
fn nothing_pending_and_a_read_only_environment_are_refused_before_the_dialog() {
    let mut h = ready(Lang::En);
    h.keys(" dp");
    assert_eq!(said(&mut h), Some(Label::EditNothingPending.into()));
    h.command("apply");
    assert_eq!(said(&mut h), Some(Label::EditNothingPending.into()));
    assert!(h.app.overlays.top().is_none());
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("prod".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert("prod", Policy { read_only: true, ..Policy::default() });
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows());
    h.command("changes");
    let reason = "the connection's environment is read-only".to_string();
    assert_eq!(said(&mut h), Some(Msg::EditReadOnly { reason }));
    assert!(h.app.overlays.top().is_none());
}

#[test]
fn rows_found_by_their_values_are_counted_on_the_tab_first() {
    let mut h = by_values(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    let asked = counts(&mut h);
    let condition = r#""id" = '1' AND "email" = 'a@x' AND "nickname" = 'ann'"#;
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].1, format!(r#"SELECT count(*) FROM "shop"."users" WHERE {condition}"#));
    // The dialog waits for the count; the status bar says so.
    assert!(h.app.overlays.edit_preview().is_none());
    assert_eq!(h.app.apply_wait_text().as_deref(), Some("Checking 1 row…"));
    // Another count's answer is not the Apply's.
    h.db(DbEvent::Counted { id: asked[0].0 + 100, result: Ok(5), snapshot: false });
    assert!(h.app.overlays.edit_preview().is_none());
    h.db(DbEvent::Counted { id: asked[0].0, result: Ok(1), snapshot: false });
    assert!(h.app.apply_wait_text().is_none());
    assert_eq!(
        lines(&h),
        [
            head("Row checks (lock, no waiting)"),
            step(&format!(r#"SELECT 1 FROM "shop"."users" WHERE {condition} FOR UPDATE NOWAIT"#), 1, false),
            head("Writes"),
            step(&format!(r#"UPDATE "shop"."users" SET "email" = 'z@x' WHERE {condition} RETURNING *"#), 1, false),
        ]
    );
    assert!(h.app.edit_preview_view().unwrap().apply);
}

/// What the preview shows is made once for the changes and what was counted for them, not for
/// every frame: again only when an answer comes or the changes move.
#[test]
fn the_changes_are_planned_once_per_revision_not_for_every_frame() {
    let mut h = by_values(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    let asked = counts(&mut h);
    let builds = |h: &Harness| edits(h).changes.builds();
    let _ = screen(&mut h, 120, 40);
    let at = builds(&h);
    for _ in 0..3 {
        let _ = screen(&mut h, 120, 40);
    }
    assert_eq!(builds(&h), at, "kept for three frames");
    h.db(DbEvent::Counted { id: asked[0].0, result: Ok(1), snapshot: false });
    for _ in 0..3 {
        let _ = screen(&mut h, 120, 40);
    }
    assert!(h.app.edit_preview_view().unwrap().apply);
    assert_eq!(builds(&h), at + 1, "once more for the answer");
    let before = builds(&h);
    h.key(KeyCode::Esc);
    type_cell(&mut h, "y@x");
    h.command("apply");
    let asked = counts(&mut h);
    h.db(DbEvent::Counted { id: asked[0].0, result: Ok(1), snapshot: false });
    let _ = screen(&mut h, 80, 24);
    let at = builds(&h);
    assert!(at > before, "made again for the changes as they are now");
    for _ in 0..3 {
        let _ = screen(&mut h, 80, 24);
    }
    assert_eq!(builds(&h), at, "and kept");
}

/// A count that comes for changes that moved since it was asked (the preview closed, a cell
/// edited) is dropped: nothing is recorded for it, and the changes as they are now are not
/// counted.
#[test]
fn a_count_for_changes_that_moved_is_dropped_and_the_changes_are_not_counted() {
    let mut h = by_values(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    let asked = counts(&mut h);
    assert_eq!(asked.len(), 1);
    h.key(KeyCode::Esc);
    type_cell(&mut h, "y@x");
    let _ = h.app.shown_changes().expect("a model");
    let at = edits(&h).changes.builds();
    h.db(DbEvent::Counted { id: asked[0].0, result: Ok(1), snapshot: false });
    assert!(edits(&h).checks.matches().is_empty(), "nothing recorded for the old changes");
    let m = h.app.shown_changes().expect("a model");
    assert_eq!(edits(&h).changes.builds(), at, "what was counted did not move");
    assert_eq!(m.sql, [PreviewLine::Problem("row 1 \u{B7} was not counted".into())]);
}

/// `mysql_note_previewed`'s first row's note set and counted, its changes shown: how many times
/// they were made, kept while nothing they read moves.
fn kept(h: &mut Harness) -> u64 {
    let _ = h.app.shown_changes().expect("a model");
    let at = edits(h).changes.builds();
    let _ = h.app.shown_changes();
    assert_eq!(edits(h).changes.builds(), at, "kept while nothing moves");
    at
}

/// The changes shown again were made again (`at`: how many times before) once `what` moved,
/// though neither the changes nor what was counted for them did.
fn made_again(h: &mut Harness, at: u64, what: &str) -> std::sync::Arc<ChangesModel> {
    let m = h.app.shown_changes().expect("a model");
    assert_eq!(edits(h).changes.builds(), at + 1, "made again when {what} moved");
    m
}

fn noted() -> Harness {
    mysql_note_previewed(Lang::En, people("Ann", "old"), "new")
}

fn rows_mut(h: &mut Harness) -> &mut datarig_tui::widgets::grid::ResultSet {
    match &mut h.app.tab_mut().results {
        Results::Rows(rs) => rs,
        _ => panic!("no row result"),
    }
}

/// The session's values change between two pages of the rows: a text as read is no longer known
/// to be shown whole, so the plan is refused, though the changes did not move.
#[test]
fn the_changes_are_made_again_when_the_values_the_rows_were_read_in_move() {
    let mut h = noted();
    let m = h.app.shown_changes().expect("a model");
    let ready = m.sql.iter().any(|l| matches!(l, PreviewLine::Step { .. }));
    assert!(m.problems.is_empty() && ready, "{:?}", m.sql);
    let at = kept(&mut h);
    rows_mut(&mut h).read_in = ReadIn::Changed;
    let m = made_again(&mut h, at, "the values the rows were read in");
    let found: Vec<_> = m.problems.iter().map(|p| (p.text.as_str(), p.at)).collect();
    let text = "row 1 \u{B7} name is not shown whole, so the row can't be found again";
    // The value typed for `note` is no longer known to be stored as typed either: it is invalid
    // now, listed first.
    let note = "row 1 · note: note was read across a change of the session's time zone or character set \
                (run it again to copy it), so its values can't be edited as shown";
    assert_eq!(found, [(note, Some((RowAt::Read(0), Some(2)))), (text, Some((RowAt::Read(0), Some(1))))]);
    assert_eq!(m.sql, [PreviewLine::Problem(note.into())]);
}

#[test]
fn the_changes_are_made_again_when_the_target_moves() {
    let mut h = noted();
    let at = kept(&mut h);
    let Target::Ready(t) = &mut rows_mut(&mut h).edit.target else { panic!("resolved") };
    t.all_columns_shown = !t.all_columns_shown;
    made_again(&mut h, at, "the target");
}

#[test]
fn the_changes_are_made_again_when_the_language_moves() {
    let mut h = noted();
    let at = kept(&mut h);
    h.app.i18n.set_lang(Lang::Ko);
    made_again(&mut h, at, "the language");
}

#[test]
fn the_changes_are_made_again_when_the_servers_time_zone_moves() {
    use datarig_core::driver::keys::{KeyCatalog, NameRule};
    let mut h = noted();
    let at = kept(&mut h);
    let mut k = KeyCatalog::default();
    k.set_names(NameRule::MySql { tables_ignore_case: false });
    k.set_server_zone("+02:00");
    k.add_table(
        1,
        "shop",
        "people",
        ["id", "name", "note"].iter().enumerate().map(|(i, n)| (i as i16 + 1, n.to_string())),
    );
    h.db(DbEvent::Keys(Ok(k)));
    made_again(&mut h, at, "the server's time zone");
}

#[test]
fn the_changes_are_made_again_when_the_rows_before_the_new_rows_move() {
    let mut h = noted();
    let at = kept(&mut h);
    rows_mut(&mut h).edit.applied = Some(Box::new(Applied { new: vec![Vec::new()], ..Applied::default() }));
    made_again(&mut h, at, "the rows an Apply added");
}

#[test]
fn the_changes_are_made_again_when_the_format_moves() {
    let mut h = noted();
    h.db(DbEvent::Edit(Some(EditFormat::MySql(MySqlEdit { nowait: false }))));
    let at = kept(&mut h);
    h.db(DbEvent::Edit(Some(EditFormat::MySql(MySqlEdit { nowait: true }))));
    made_again(&mut h, at, "the format");
}

#[test]
fn a_row_gone_ambiguous_or_not_counted_is_said_and_nothing_can_be_applied() {
    let mut h = by_values(Lang::En);
    type_cell(&mut h, "z@x");
    for (answer, want) in [
        (Ok(0), "row 1 \u{B7} matches no row of the table now: it was changed or deleted. Re-run the query."),
        (Ok(2), "row 1 \u{B7} matches 2 rows; add the key to the SELECT to edit it"),
        (Err(DbError::Server("canceling statement".into())), "Couldn't count the matching rows: canceling statement"),
    ] {
        h.command("apply");
        let asked = counts(&mut h);
        h.db(DbEvent::Counted { id: asked[0].0, result: answer, snapshot: false });
        match &blocked(&mut h)[..] {
            [text] => assert!(text.starts_with(want), "{text}"),
            other => panic!("{other:?}"),
        }
        assert!(h.app.overlays.edit_preview().is_none(), "no dialog");
    }
}

/// The changes hold every row's problem, each with its row; the Apply shows them all.
#[test]
fn every_rows_problem_is_in_the_changes_with_its_row_and_the_apply_shows_them() {
    let mut h = by_values(Lang::En);
    type_cell(&mut h, "z@x");
    h.keys("j");
    type_cell(&mut h, "y@x");
    h.command("apply");
    let asked = counts(&mut h);
    assert_eq!(asked.len(), 2);
    h.db(DbEvent::Counted { id: asked[0].0, result: Ok(0), snapshot: false });
    h.db(DbEvent::Counted { id: asked[1].0, result: Ok(2), snapshot: false });
    let m = h.app.shown_changes().expect("a model");
    let found: Vec<_> = m.problems.iter().map(|p| (p.text.as_str(), p.at)).collect();
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].0.starts_with("row 1 \u{B7} matches no row of the table now"), "{found:?}");
    assert!(found[1].0.starts_with("row 2 \u{B7} matches 2 rows"), "{found:?}");
    assert_eq!((found[0].1, found[1].1), (Some((RowAt::Read(0), None)), Some((RowAt::Read(1), None))));
    assert_eq!(blocked(&mut h).len(), 2);
    let cells: Vec<_> = m.list.iter().map(|l| (l.at, l.col, l.new.as_deref())).collect();
    assert_eq!(cells, [(RowAt::Read(0), Some(1), Some("'z@x'")), (RowAt::Read(1), Some(1), Some("'y@x'"))]);
}

#[test]
fn identical_rows_wait_for_one_or_all() {
    let mut h = identical(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    let asked = counts(&mut h);
    h.db(DbEvent::Counted { id: asked[0].0, result: Ok(3), snapshot: false });
    assert_eq!(
        lines(&h),
        [PreviewLine::Choose {
            text: "row 1 \u{B7} matches 3 identical rows".into(),
            keys: "[1] just one   [a] all 3".into()
        }]
    );
    h.key(KeyCode::Char('y'));
    assert!(matches!(said(&mut h), Some(Msg::EditPreviewCantApply { .. })));
    h.key(KeyCode::Char('a'));
    let all = lines(&h);
    assert!(matches!(&all[1], PreviewLine::Step { expect, risky: true, .. } if expect == "expect 3"), "{all:?}");
    assert!(matches!(&all[3], PreviewLine::Step { expect, risky: true, .. } if expect == "expect 3"), "{all:?}");
    h.key(KeyCode::Esc);
    // Asked again when opened again: the count and the answer.
    h.command("apply");
    let asked = counts(&mut h);
    h.db(DbEvent::Counted { id: asked[0].0, result: Ok(3), snapshot: false });
    h.key(KeyCode::Char('1'));
    let one = lines(&h);
    assert!(matches!(&one[1], PreviewLine::Step { expect, risky: true, .. } if expect == "expect 3"), "{one:?}");
    match &one[3] {
        PreviewLine::Step { sql, expect, risky: false } => {
            assert_eq!(expect, "expect 1");
            assert!(sql.contains("ctid"), "{sql}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_new_row_leaving_a_not_null_column_without_a_default_is_listed_and_not_counted() {
    let mut h = by_values(Lang::En);
    h.keys("o");
    h.keys("jo");
    h.command("apply");
    assert!(counts(&mut h).is_empty(), "nothing is counted for changes that cannot be made");
    let want = |n: u32| format!("new {n} \u{B7} email is NOT NULL and has no default; give it a value");
    assert_eq!(blocked(&mut h), [want(1), want(2)]);
    h.keys("H");
    h.keys("Gk");
    h.keys("l");
    type_cell(&mut h, "n@x");
    h.keys("j");
    type_cell(&mut h, "m@x");
    h.command("apply");
    let v = h.app.edit_preview_view().unwrap();
    assert!(v.apply);
    assert_eq!(v.summary, "2 inserts");
}

#[test]
fn c_copies_every_statement_through_the_clipboard_setting() {
    let mut h = ready(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    type_cell(&mut h, "z@x");
    h.command("apply");
    h.key(KeyCode::Char('c'));
    assert_eq!(clip.last(), Some(format!("{CHECK_1};\n{UPDATE_1};\n")));
    assert!(matches!(said(&mut h), Some(Msg::EditPreviewCopied { count: 2, .. })));
    assert_eq!(h.app.key_context(), Ctx::EditPreview, "the dialog stays");
    // While the rows are counted there is no dialog to copy from yet.
    let mut h = by_values(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    assert!(h.app.overlays.edit_preview().is_none(), "the dialog waits for the counts");
}

#[test]
fn the_footer_says_how_the_apply_stands_to_a_transaction() {
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("stg".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert(
        "stg",
        Policy { commit: CommitRule::Manual, confirm: datarig_core::policy::Confirm::Writes, ..Policy::default() },
    );
    let mut h = resolved(
        {
            let mut h = Harness::with_config(&cfg, Lang::En);
            h.db(DbEvent::Connected);
            h
        },
        "SELECT id, email, nickname FROM shop.users",
        users_columns(),
        users_rows(),
        users_structure(),
    );
    type_cell(&mut h, "z@x");
    h.command("apply");
    assert_eq!(h.app.edit_preview_view().unwrap().footer, "Opens a transaction; nothing is committed.");
    h.key(KeyCode::Esc);
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    h.command("apply");
    assert_eq!(h.app.edit_preview_view().unwrap().footer, "Runs inside the open transaction; nothing is committed.");
}

#[test]
fn the_preview_closes_when_its_result_is_replaced_or_its_session_lost() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows());
    assert!(h.app.overlays.edit_preview().is_none());
    assert_eq!(said(&mut h), Some(Label::EditPreviewClosed.into()));
    // The tab's session lost while it counts: no answer comes there any more.
    let mut h = by_values(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    assert_eq!(counts(&mut h).len(), 1);
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    assert!(h.app.overlays.edit_preview().is_none());
}

#[test]
fn the_hint_line_offers_apply_and_undo_while_changes_are_pending() {
    let mut h = ready(Lang::En);
    let has = |h: &Harness, label: &str| h.app.hints().iter().any(|(_, l)| &**l == label);
    assert!(!has(&h, "apply"));
    type_cell(&mut h, "z@x");
    let hints = h.app.hints();
    assert_eq!((hints[0].0.as_str(), hints[0].1.to_string().as_str()), ("Space d a", "apply"), "{hints:?}");
    assert_eq!(hints[1].1.to_string(), "undo");
}

#[test]
fn discard_asks_and_enter_keeps_the_changes() {
    let mut h = ready(Lang::En);
    h.keys(" dx");
    assert_eq!(said(&mut h), Some(Label::EditNothingPending.into()));
    type_cell(&mut h, "z@x");
    h.keys("jD");
    h.keys(" dx");
    let c = h.app.overlays.confirm().expect("asked");
    assert!(matches!(c.action, ConfirmAction::DiscardEdits { .. }));
    assert_eq!(c.text, Msg::EditDiscardText { count: 2 });
    h.key(KeyCode::Enter);
    assert!(h.app.overlays.top().is_none());
    assert_eq!(edits(&h).batch.counts().total(), 2, "Enter keeps them");
    h.command("discard");
    assert_eq!(h.app.overlays.top().map(|o| o.kind()), Some(OverlayKind::Confirm));
    h.key(KeyCode::Char('y'));
    assert!(edits(&h).batch.is_empty());
    assert_eq!(said(&mut h), Some(Msg::EditDiscarded { count: 2 }));
    // One change: undone, they come back.
    h.keys("u");
    assert_eq!(edits(&h).batch.counts().total(), 2);
}

/// A MySQL profile (connected, the keys of `shop.products` read) whose tab's first run read
/// `shop.products` as its session said its values when it connected; resolved, nothing sent.
fn mysql_products() -> Harness {
    use datarig_core::driver::keys::{KeyCatalog, KeyKind, NameRule};
    use datarig_core::driver::{ColumnOrigin, SessionValues};
    let driver = FakeDriver::default();
    driver.mysql.store(true, std::sync::atomic::Ordering::SeqCst);
    let cfg = Config {
        connections: vec![ConnectionConfig {
            driver: "mysql".into(),
            policy: Some("stg".into()),
            ..ConnectionConfig::test_db()
        }],
        ..writes_config()
    };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver);
    h.db(DbEvent::Connected);
    let mut k = KeyCatalog::default();
    k.set_names(NameRule::MySql { tables_ignore_case: false });
    k.set_server_zone("UTC");
    let names = ["id", "name", "price"];
    k.add_table(1, "shop", "products", names.iter().enumerate().map(|(i, n)| (i as i16 + 1, n.to_string())));
    k.mark(1, &[1], KeyKind::Primary);
    h.db(DbEvent::Keys(Ok(k)));
    let col = |name: &str, ty: &str, kind: ValueKind| {
        let origin = ColumnOrigin::Named { schema: "shop".into(), table: "products".into(), column: name.into() };
        ColumnMeta::new(name.into(), ty.into(), kind, Some(origin))
    };
    let columns = vec![
        col("id", "bigint", ValueKind::Integer),
        col("name", "varchar", ValueKind::Text),
        col("price", "decimal", ValueKind::Decimal),
    ];
    h.app.run(vec!["SELECT id, name, price FROM shop.products".to_string()]);
    h.db(DbEvent::SessionValues(SessionValues { time_zone: "SYSTEM".into(), charset: "utf8mb4".into() }));
    let id = h.app.tab().exec.query_id;
    let rows = vec![vec![s("1"), s("pen"), s("1.50")]];
    h.db(DbEvent::Page { id, columns: Some(columns), rows, more: false, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
    let mut st = users_structure();
    st.columns.truncate(3);
    st.columns[0].type_name = "bigint".into();
    st.columns[0].default = None;
    st.columns[0].fill = ColumnFill::AutoIncrement;
    st.columns[1] = datarig_core::driver::structure::StructureColumn {
        name: "name".into(),
        type_name: "varchar(100)".into(),
        ..st.columns[1].clone()
    };
    st.columns[2] = datarig_core::driver::structure::StructureColumn {
        name: "price".into(),
        type_name: "decimal(12,2)".into(),
        not_null: true,
        default: None,
        ..st.columns[2].clone()
    };
    st.unique_constraints.clear();
    st.indexes.retain(|x| x.primary);
    h.keys("le");
    let name = h.app.profiles[0].name.clone();
    h.meta_db(&name, DbEvent::Structure { schema: "shop".into(), table: "products".into(), result: Ok(Box::new(st)) });
    assert_eq!(h.app.key_context(), Ctx::CellEdit, "{:?}", said(&mut h));
    h.key(KeyCode::Esc);
    h.app.transient = None;
    h.sent();
    h
}

/// QA saw a MySQL new row's `price` (DECIMAL) refused as read across a change of the session's
/// time zone or character set: a DECIMAL is under no such rule, and it is not refused.
#[test]
fn a_mysql_new_rows_decimal_is_typed_and_its_not_null_columns_must_be_given() {
    let mut h = mysql_products();
    h.keys("o");
    h.command("apply");
    let want = |c: &str| format!("new 1 \u{B7} {c} is NOT NULL and has no default; give it a value");
    assert_eq!(blocked(&mut h), [want("name"), want("price")]);
    h.keys("H");
    h.keys("$");
    type_cell(&mut h, "9.99");
    assert_eq!(said(&mut h), None, "not refused");
    h.keys("h");
    type_cell(&mut h, "ink");
    let new = &edits(&h).batch.drafts()[0].cells;
    assert_eq!(new[1..], [Value::Text("ink".into()), Value::Text("9.99".into())]);
    h.command("apply");
    let v = h.app.edit_preview_view().unwrap();
    assert!(v.apply);
    assert_eq!(v.lines[1], step("INSERT INTO `shop`.`products` (`name`, `price`) VALUES ('ink', 9.99)", 1, false));
}

/// The screens of `setup` (built in each language) at 120x40 and 80x24, as `<name>_<lang>_<size>`:
/// English ones pinned, Korean ones checked to show their catalog's texts.
fn screens(name: &str, setup: impl Fn(Lang) -> Harness) {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = setup(lang);
        let tag = format!("{lang:?}").to_lowercase();
        for (w, hh) in [(120, 40), (80, 24)] {
            assert_screen!(format!("{name}_{tag}_{w}x{hh}"), lang, h.draw(w, hh));
        }
    }
}

/// An update, a delete and a new row, previewed in `h`.
fn three_changes(mut h: Harness) -> Harness {
    type_cell(&mut h, "z@x");
    h.keys("jD");
    h.keys("ol");
    type_cell(&mut h, "n@x");
    h.command("apply");
    h.app.transient = None;
    h
}

/// Connected to the test profile in an environment that commits by hand.
fn manual(lang: Lang) -> Harness {
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("stg".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert(
        "stg",
        Policy { commit: CommitRule::Manual, confirm: datarig_core::policy::Confirm::Writes, ..Policy::default() },
    );
    let mut h = Harness::with_config(&cfg, lang);
    h.db(DbEvent::Connected);
    resolved(h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows(), named())
}

#[test]
fn the_preview_on_screen() {
    screens("edit_preview_auto", |lang| three_changes(ready(lang)));
    screens("edit_preview_manual_none", |lang| three_changes(manual(lang)));
    screens("edit_preview_manual_open", |lang| {
        let mut h = manual(lang);
        h.tab_db(0, DbEvent::Block(true));
        h.tab_db(0, DbEvent::TxOpen(true));
        three_changes(h)
    });
    screens("edit_preview_choose", |lang| {
        let mut h = identical(lang);
        type_cell(&mut h, "z@x");
        h.command("apply");
        let asked = counts(&mut h);
        h.db(DbEvent::Counted { id: asked[0].0, result: Ok(3), snapshot: false });
        h
    });
    // The Apply waits for the count, the status bar says so (no dialog yet).
    let mut h = by_values(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    for (w, hh) in [(120, 40), (80, 24)] {
        assert_screen!(format!("edit_apply_checking_en_{w}x{hh}"), Lang::En, h.draw(w, hh));
    }
    // Problems: the Changes tab shows them instead of a dialog.
    screens("edit_apply_count_failed", |lang| {
        let mut h = by_values(lang);
        type_cell(&mut h, "z@x");
        h.command("apply");
        let asked = counts(&mut h);
        let result = Err(DbError::Server("canceling statement due to lock timeout".into()));
        h.db(DbEvent::Counted { id: asked[0].0, result, snapshot: false });
        h
    });
    screens("edit_apply_no_default", |lang| {
        let mut h = ready(lang);
        h.keys("o");
        h.command("apply");
        h
    });
}

/// A new row leaves `name` (NOT NULL, no default) to a default it lacks when the result does
/// not show it: the server would refuse the row, or (MySQL, not strict) store an empty name.
/// Listed, nothing counted, `y` refused, nothing sent.
#[test]
fn a_new_row_is_refused_while_the_table_has_a_required_column_the_result_does_not_show() {
    let mut h = resolved(
        connected(Lang::En),
        "SELECT id, email, nickname FROM shop.users",
        users_columns(),
        users_rows(),
        users_structure(),
    );
    h.keys("ol");
    type_cell(&mut h, "n@x");
    h.command("apply");
    let want = "new 1 \u{B7} name is NOT NULL, has no default and is not in the result; add it to the SELECT";
    assert!(h.sent().iter().all(|c| !matches!(c, DbCommand::Count { .. })), "nothing counted");
    assert_eq!(blocked(&mut h), [want.to_string()]);
    // Changes of the rows read leave the table's other columns as they are.
    h.keys("H");
    h.keys("G");
    h.keys("D");
    h.keys("gg");
    type_cell(&mut h, "z@x");
    h.command("apply");
    assert!(h.app.edit_preview_view().unwrap().apply, "{:?}", lines(&h));
}

// The Apply: sent on `y`, answered by `DbEvent::Applied`.

/// The Apply sent since the last call: its id, its steps and its transaction.
fn applies(h: &mut Harness) -> (u64, Vec<Step>, ApplyTx) {
    let mut sent: Vec<(u64, Vec<Step>, ApplyTx)> = h
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::ApplyEdits { id, steps, tx } => Some((id, steps, tx)),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 1, "one Apply sent");
    sent.remove(0)
}

fn no_apply_sent(h: &mut Harness) -> bool {
    h.sent().iter().all(|c| !matches!(c, DbCommand::ApplyEdits { .. }))
}

/// Every step of `steps` stood: each gave its count, nothing returned.
fn stood(steps: &[Step]) -> Vec<AppliedStep> {
    let n = |s: &Step| match s.expect {
        Expect::Rows(n) | Expect::Affected(n) => n,
    };
    steps.iter().map(|s| AppliedStep { count: n(s), ..AppliedStep::default() }).collect()
}

/// The columns of `shop.users` as `RETURNING *` gives them.
type Returned = Option<(Vec<ColumnMeta>, Vec<Vec<Option<String>>>)>;

fn users_returned(row: [Option<&str>; 5]) -> Returned {
    let cols = ["id", "email", "name", "nickname", "profile"].map(|n| meta_from(n, "text", false, USERS, 1)).to_vec();
    Some((cols, vec![row.iter().map(|v| v.map(str::to_string)).collect()]))
}

fn text(v: &str) -> Value {
    Value::Text(v.into())
}

fn applied(h: &Harness) -> &Applied {
    edits(h).applied.as_deref().expect("an applied layer")
}

fn screen(h: &mut Harness, w: u16, hh: u16) -> String {
    buffer_text(h.draw(w, hh).backend().buffer())
}

/// An update, a delete and a new row, applied (`y` pressed, nothing answered yet).
fn three_applying(mut h: Harness) -> (Harness, u64, Vec<Step>) {
    type_cell(&mut h, "z@x");
    h.keys("jD");
    h.keys("ol");
    type_cell(&mut h, "n@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, steps, _) = applies(&mut h);
    h.app.transient = None;
    (h, id, steps)
}

/// The answer to `three_applying`'s Apply: every step stood, the update's and the insert's rows
/// returned (a trigger changed the nickname; the table numbered the new row).
fn three_done(steps: &[Step]) -> Vec<AppliedStep> {
    let mut done = stood(steps);
    for (s, d) in steps.iter().zip(done.iter_mut()) {
        if s.sql.starts_with("UPDATE ") {
            d.returned = users_returned([Some("1"), Some("z@x"), Some("Ann"), Some("ann!"), Some("{}")]);
        } else if s.sql.starts_with("INSERT ") {
            d.returned = users_returned([Some("4"), Some("n@x"), Some(""), None, Some("{}")]);
        }
    }
    done
}

#[test]
fn y_sends_the_apply_on_the_tabs_session_and_edits_wait_for_its_answer() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (_, steps, tx) = applies(&mut h);
    assert_eq!(steps.iter().map(|s| s.sql.as_str()).collect::<Vec<_>>(), [CHECK_1, UPDATE_1]);
    assert_eq!(tx, ApplyTx::Commit);
    assert_eq!(h.app.key_context(), Ctx::EditPreview, "the dialog stays");
    let v = h.app.edit_preview_view().unwrap();
    assert_eq!(v.lines[0], PreviewLine::Note("Applying the changes…".into()));
    assert!(!v.apply);
    assert_eq!(h.app.edit_preview_buttons().len(), 2, "no Apply button");
    h.key(KeyCode::Char('y'));
    let reason = "Applying the changes…".to_string();
    assert_eq!(said(&mut h), Some(Msg::EditPreviewCantApply { reason }));
    assert!(no_apply_sent(&mut h), "sent once");
    assert!(screen(&mut h, 120, 40).contains("Applying the changes…"), "the status bar says it");
    // Nothing changes the changes while they are applied.
    h.key(KeyCode::Esc);
    let refused = Some(Msg::EditRefused { reason: "the pending changes are being applied".into() });
    for keys in ["u", "D", "e", " dx", " da"] {
        h.keys(keys);
        assert_eq!(said(&mut h), refused, "{keys}");
        assert!(h.app.overlays.top().is_none(), "{keys}");
    }
    assert_eq!(edits(&h).batch.counts().total(), 1);
    assert!(screen(&mut h, 120, 40).contains("applying…"), "the strip says it");
}

/// The environment confirms every write: the dialog is that confirmation, nothing asks again.
#[test]
fn the_dialog_is_the_confirmation_of_an_environment_that_confirms_writes() {
    use datarig_core::policy::Confirm;
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("stg".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert("stg", Policy { confirm: Confirm::Writes, ..Policy::default() });
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.db(DbEvent::Connected);
    let mut h = resolved(h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows(), named());
    h.keys("D");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    applies(&mut h);
    assert_eq!(h.app.overlays.top().map(|o| o.kind()), Some(OverlayKind::EditPreview), "no second dialog");
}

#[test]
fn applied_changes_show_over_the_rows_and_the_changes_are_cleared() {
    let (mut h, id, steps) = three_applying(ready(Lang::En));
    h.db(DbEvent::Applied { id, result: Ok(three_done(&steps)) });
    assert!(h.app.overlays.edit_preview().is_none(), "the dialog closed");
    assert_eq!(said(&mut h), Some(Msg::EditApplied { count: 3 }));
    let e = edits(&h);
    assert!(e.batch.is_empty() && !e.batch.can_undo(), "the changes and their history are cleared");
    assert!(e.applying.is_none());
    let a = applied(&h);
    // The row as the server returned it: the nickname a trigger set too.
    assert_eq!(a.set[&0], [(1, text("z@x")), (2, text("ann!"))].into());
    assert_eq!(a.deleted, [1].into());
    assert_eq!(a.new, [vec![text("4"), text("n@x"), Value::Null]], "the new row as stored");
    assert_eq!(a.tx, None);
    let s = screen(&mut h, 120, 40);
    assert!(s.contains("]  Applied  Chart") && s.contains("applied"), "the strip and the badge: {s}");
    assert!(s.contains("ann!") && s.contains("n@x"), "{s}");
    assert!(h.app.tab().exec.running.is_none(), "the tab is free");
    // The rows are read again before they are edited again.
    h.keys("e");
    let reason = "changes were applied to these rows; re-run the query to edit them".to_string();
    assert_eq!(said(&mut h), Some(Msg::EditRefused { reason }));
    // Running the query again shows the table as it is.
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows());
    assert!(edits(&h).applied.is_none());
}

/// Rows an Apply added are numbered after the rows read, as new rows are: the gutter's numbers
/// run on.
#[test]
fn rows_an_apply_added_keep_the_row_numbers_running() {
    let (mut h, id, steps) = three_applying(ready(Lang::En));
    h.db(DbEvent::Applied { id, result: Ok(three_done(&steps)) });
    let s = screen(&mut h, 120, 40);
    let line = s.lines().find(|l| l.contains("n@x")).unwrap_or_default().to_string();
    assert!(line.contains("\u{2502} 4 \u{2502}"), "the added row: {line:?}\n{s}");
}

/// A MySQL insert returns no row: the number its `AUTO_INCREMENT` column took fills that column;
/// a column left to its default stays `default`.
#[test]
fn a_mysql_new_rows_number_comes_from_the_insert_id() {
    let mut h = mysql_products();
    h.keys("o");
    h.keys("l");
    type_cell(&mut h, "ink");
    h.keys("l");
    type_cell(&mut h, "9.99");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, steps, tx) = applies(&mut h);
    assert_eq!(tx, ApplyTx::Commit);
    let mut done = stood(&steps);
    done.last_mut().unwrap().insert_id = Some(7);
    h.db(DbEvent::Applied { id, result: Ok(done) });
    assert_eq!(applied(&h).new, [vec![text("7"), text("ink"), text("9.99")]]);
}

#[test]
fn each_failure_says_why_and_the_changes_stay() {
    use datarig_core::driver::{ApplyError, ApplyFailure};
    let update = r#"UPDATE "shop"."users" SET "email" = 'z@x' WHERE "id" = '1' AND "email" = 'a@x' RETURNING *"#;
    let cases: Vec<(Option<usize>, ApplyError, String)> = vec![
        (
            Some(0),
            ApplyError::Count { got: 0 },
            "id=1 \u{B7} was changed or deleted since it was read. Nothing was applied. Re-run the query to see the current values.".into(),
        ),
        (Some(1), ApplyError::Count { got: 4 }, "id=1 \u{B7} now matches 4 rows, not 1. Nothing was applied.".into()),
        (
            Some(0),
            ApplyError::Db(DbError::LockBusy("could not obtain lock".into())),
            "id=1 \u{B7} is locked by another session. Nothing was applied.".into(),
        ),
        (
            Some(1),
            ApplyError::Db(DbError::Server("value too long".into())),
            format!("id=1 \u{B7} value too long. Nothing was applied. Statement: {update}"),
        ),
        (None, ApplyError::Db(DbError::Server("could not serialize access".into())), "could not serialize access. Nothing was applied.".into()),
        (Some(0), ApplyError::Db(DbError::Cancelled), "Apply cancelled; nothing was applied.".into()),
        (
            None,
            ApplyError::Db(DbError::ReadWriteRefused),
            "not sent: the session is read-only and this statement asks for read-write. Nothing was applied.".into(),
        ),
    ];
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    for (step, why, want) in cases {
        h.command("apply");
        h.key(KeyCode::Char('y'));
        let (id, _, _) = applies(&mut h);
        h.db(DbEvent::Applied { id, result: Err(ApplyFailure { step, why }) });
        assert_eq!(said(&mut h).map(|m| h.app.i18n.msg(&m).to_string()), Some(want.clone()));
        let v = h.app.edit_preview_view().expect("the dialog stays");
        assert_eq!(v.lines[0], PreviewLine::Problem(want.clone()));
        assert!(v.apply, "it may be applied again: {want}");
        assert_eq!(edits(&h).batch.counts().total(), 1, "the changes stay: {want}");
        assert!(edits(&h).applied.is_none() && edits(&h).applying.is_none());
        let notes = &h.app.tab().exec.run.notes;
        assert_eq!(notes.last().map(|n| h.app.i18n.msg(&n.msg).to_string()), Some(want), "in the Messages");
        h.key(KeyCode::Esc);
    }
}

/// A failure of a new row's insert names it as the Changes tab does.
#[test]
fn a_failure_names_a_new_row_as_the_list_does() {
    use datarig_core::driver::{ApplyError, ApplyFailure};
    let mut h = mysql_products();
    h.keys("o");
    h.keys("l");
    type_cell(&mut h, "ink");
    h.keys("l");
    type_cell(&mut h, "9.99");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, steps, _) = applies(&mut h);
    let step = steps.iter().position(|s| s.sql.starts_with("INSERT "));
    let why = ApplyError::Db(DbError::Server("duplicate key".into()));
    h.db(DbEvent::Applied { id, result: Err(ApplyFailure { step, why }) });
    let text = said(&mut h).map(|m| h.app.i18n.msg(&m).to_string()).expect("said");
    assert!(text.starts_with("new 1 \u{B7} duplicate key. Nothing was applied."), "{text}");
}

/// The connection ended while committing: whether the changes are in the table is not known.
/// They are applied again only once the user says so again; a run reads the rows again.
#[test]
fn an_apply_whose_result_is_unknown_is_applied_again_only_when_asked() {
    use datarig_core::driver::{ApplyError, ApplyFailure};
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, _, _) = applies(&mut h);
    let why = ApplyError::Db(DbError::CommitUnknown);
    h.db(DbEvent::Applied { id, result: Err(ApplyFailure { step: None, why }) });
    let unknown =
        "Apply result unknown: the connection was lost before the server confirmed. Re-run the query to check.";
    assert_eq!(said(&mut h).map(|m| h.app.i18n.msg(&m).to_string()).as_deref(), Some(unknown));
    let v = h.app.edit_preview_view().unwrap();
    assert_eq!(v.lines[0], PreviewLine::Problem(unknown.into()));
    assert!(!v.apply, "Apply is disabled");
    h.key(KeyCode::Esc);
    h.command("apply");
    assert_eq!(h.app.edit_preview_view().unwrap().lines[0], PreviewLine::Problem(unknown.into()), "still");
    h.key(KeyCode::Char('y'));
    assert!(no_apply_sent(&mut h), "warned first");
    let again = "The last Apply's result is unknown: its changes may be in the table already. y applies them again; Enter or Esc keeps them as they are.";
    let v = h.app.edit_preview_view().unwrap();
    assert_eq!(v.lines[1], PreviewLine::Problem(again.into()));
    assert!(v.apply);
    h.key(KeyCode::Enter);
    assert!(no_apply_sent(&mut h), "Enter keeps them as they are");
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h.command("apply");
    assert!(!h.app.edit_preview_view().unwrap().apply, "warned again");
    h.key(KeyCode::Char('y'));
    h.key(KeyCode::Char('y'));
    applies(&mut h);
    // A run reads the rows again: the new result has no such doubt.
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, _, _) = applies(&mut h);
    let why = ApplyError::Db(DbError::CommitUnknown);
    h.db(DbEvent::Applied { id, result: Err(ApplyFailure { step: None, why }) });
    h.key(KeyCode::Esc);
    answer(&mut h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows());
    assert!(!edits(&h).apply_unknown);
}

/// An answer to another Apply is not this one's; an answer that comes after the session ended
/// is never read, and the Apply's result is not known.
#[test]
fn a_stale_answer_is_dropped() {
    let (mut h, id, steps) = three_applying(ready(Lang::En));
    h.db(DbEvent::Applied { id: id + 100, result: Ok(three_done(&steps)) });
    assert!(edits(&h).applying.is_some() && edits(&h).applied.is_none(), "still applying");
    assert_eq!(edits(&h).batch.counts().total(), 3);
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    assert!(edits(&h).applying.is_none());
    assert!(edits(&h).apply_unknown);
    assert!(h.app.tab().exec.run.notes.iter().any(|n| n.msg == Label::EditFailedUnknown.into()));
    h.db(DbEvent::Applied { id, result: Ok(three_done(&steps)) });
    assert!(edits(&h).applied.is_none(), "the lost session's answer is not read");
    assert_eq!(edits(&h).batch.counts().total(), 3, "the changes stay");
}

/// The query's cancel key cancels the Apply; nothing was applied.
#[test]
fn the_cancel_key_cancels_the_apply() {
    use datarig_core::driver::{ApplyError, ApplyFailure};
    let (mut h, id, _) = three_applying(ready(Lang::En));
    h.ctrl('c');
    assert!(h.cancelled.load(std::sync::atomic::Ordering::SeqCst), "the session was asked to cancel");
    h.db(DbEvent::Applied { id, result: Err(ApplyFailure { step: Some(2), why: ApplyError::Db(DbError::Cancelled) }) });
    assert_eq!(said(&mut h), Some(Label::EditFailedCancelled.into()));
    assert_eq!(edits(&h).batch.counts().total(), 3);
    assert!(h.app.tab().exec.running.is_none());
}

/// Under manual commit the Apply keeps its transaction open: the changes are not committed
/// until it ends. A commit keeps them; a rollback (the user's, the server's, a lost
/// connection) drops them, and says so.
#[test]
fn under_manual_commit_the_transactions_end_decides_the_applied_changes() {
    let open = |h: &mut Harness| {
        let (id, steps, tx) = applies(h);
        assert!(matches!(tx, ApplyTx::Keep { .. }), "{tx:?}");
        h.db(DbEvent::Applied { id, result: Ok(three_done(&steps)) });
        assert_eq!(said(h), Some(Msg::EditAppliedKept { count: 3 }));
        // The transaction it opened follows its answer.
        h.tab_db(0, DbEvent::ManualBegun { id, index: 0 });
        h.db(DbEvent::Block(true));
        h.db(DbEvent::TxOpen(true));
        assert!(applied(h).tx.is_some());
        let s = screen(h, 120, 40);
        assert!(s.contains("applied · not committed"), "{s}");
    };
    let gone = "The transaction was rolled back; the applied changes are gone";
    // COMMIT.
    let (mut h, _, _) = {
        let mut h = manual(Lang::En);
        type_cell(&mut h, "z@x");
        h.keys("jD");
        h.keys("ol");
        type_cell(&mut h, "n@x");
        h.command("apply");
        h.key(KeyCode::Char('y'));
        (h, 0, ())
    };
    open(&mut h);
    h.command("commit");
    let end = h.sent().into_iter().find_map(|c| match c {
        DbCommand::EndTx { id, commit: true } => Some(id),
        _ => None,
    });
    let end = end.expect("a commit sent");
    h.tab_db(0, DbEvent::TxEnded { id: end, outcome: datarig_core::driver::TxOutcome::Committed });
    h.db(DbEvent::Block(false));
    h.db(DbEvent::TxOpen(false));
    assert_eq!(applied(&h).tx, None, "committed");
    h.command("changes");
    assert!(screen(&mut h, 120, 40).contains("Applied · re-run to refresh"));
    // ROLLBACK, the server's rollback, a lost connection.
    for how in ["rollback", "server", "lost"] {
        let mut h = manual(Lang::En);
        type_cell(&mut h, "z@x");
        h.command("apply");
        h.key(KeyCode::Char('y'));
        let (id, steps, _) = applies(&mut h);
        h.db(DbEvent::Applied { id, result: Ok(stood(&steps)) });
        h.tab_db(0, DbEvent::ManualBegun { id, index: 0 });
        h.db(DbEvent::Block(true));
        h.db(DbEvent::TxOpen(true));
        h.app.transient = None;
        match how {
            "rollback" => {
                h.command("rollback");
                let end = h.sent().into_iter().find_map(|c| match c {
                    DbCommand::EndTx { id, commit: false } => Some(id),
                    _ => None,
                });
                let outcome = datarig_core::driver::TxOutcome::RolledBack;
                h.tab_db(0, DbEvent::TxEnded { id: end.expect("a rollback sent"), outcome });
                h.db(DbEvent::Block(false));
                h.db(DbEvent::TxOpen(false));
            }
            "server" => {
                h.tab_db(0, DbEvent::TxClosedBy { id, how: datarig_core::driver::ServerEnd::RolledBack });
                h.db(DbEvent::Block(false));
                h.db(DbEvent::TxOpen(false));
            }
            _ => h.tab_db(0, DbEvent::Lost { error: DbError::Closed }),
        }
        assert!(edits(&h).applied.is_none(), "{how}: the original values show again");
        let status = h.status(120, 40);
        assert!(status.contains(gone), "{how}: {status}");
        assert!(h.app.tab().exec.run.notes.iter().any(|n| n.msg == Label::EditAppliedGone.into()), "{how}");
    }
}

/// Inside the user's open block the driver applies under a savepoint and commits nothing: the
/// changes are not committed until the block ends.
#[test]
fn inside_an_open_block_the_applied_changes_wait_for_its_end() {
    for rolled_back in [false, true] {
        let mut h = ready(Lang::En);
        h.db(DbEvent::Block(true));
        h.db(DbEvent::TxOpen(true));
        type_cell(&mut h, "z@x");
        h.command("apply");
        assert_eq!(
            h.app.edit_preview_view().unwrap().footer,
            "Runs inside the open transaction; nothing is committed."
        );
        h.key(KeyCode::Char('y'));
        let (id, steps, tx) = applies(&mut h);
        assert_eq!(tx, ApplyTx::Commit, "the driver uses a savepoint by itself");
        h.db(DbEvent::Applied { id, result: Ok(stood(&steps)) });
        assert_eq!(said(&mut h), Some(Msg::EditAppliedKept { count: 1 }));
        assert!(applied(&h).tx.is_some());
        if rolled_back {
            h.tab_db(0, DbEvent::TxClosedBy { id, how: datarig_core::driver::ServerEnd::RolledBack });
        } else {
            ran(&mut h, "COMMIT", "COMMIT");
        }
        h.db(DbEvent::Block(false));
        h.db(DbEvent::TxOpen(false));
        match rolled_back {
            true => assert!(edits(&h).applied.is_none()),
            false => assert_eq!(applied(&h).tx, None),
        }
    }
}

/// In a dark and a light theme: a value an Apply wrote in `edit_applied`, a row it deleted still
/// crossed out, a value the table filled in for a new row dim.
#[test]
fn applied_values_take_their_token() {
    use datarig_tui::theme;
    use ratatui::style::Modifier;
    for th in [theme::DARK, theme::LIGHT] {
        let (mut h, id, steps) = three_applying(ready(Lang::En));
        h.app.theme = std::sync::Arc::new(th.clone());
        let mut done = three_done(&steps);
        // The insert returned nothing: what the table filled in is not known.
        if let Some(d) = done.iter_mut().zip(&steps).find(|(_, s)| s.sql.starts_with("INSERT ")).map(|(d, _)| d) {
            d.returned = None;
        }
        h.db(DbEvent::Applied { id, result: Ok(done) });
        h.keys("G");
        let t = h.draw(120, 40);
        let buf = t.backend().buffer();
        let g = &h.app.tab().grid;
        let at = |r: usize, c: usize| {
            let x = g.cell_rect(r, c, 3).expect("on screen");
            buf[(x.x + 1, x.y)].clone()
        };
        let written = at(0, 1);
        assert_eq!((written.fg, written.bg), (th.edit_applied.fg.unwrap(), th.bg), "a value written");
        assert!(written.modifier.contains(Modifier::ITALIC) && !written.modifier.contains(Modifier::BOLD));
        let gone = at(1, 1);
        assert_eq!(gone.fg, th.edit_deleted.fg.unwrap(), "a row deleted");
        assert!(gone.modifier.contains(Modifier::CROSSED_OUT));
        let added = at(3, 1);
        assert_eq!(added.fg, th.edit_applied.fg.unwrap(), "a new row's value");
        let filled = at(3, 0);
        assert!(filled.fg == th.fg_muted && filled.modifier.contains(Modifier::DIM), "filled by the table: {filled:?}");
        assert_eq!(at(2, 1).fg, th.fg, "a row as read");
    }
}

#[test]
fn the_apply_on_screen() {
    use datarig_core::driver::{ApplyError, ApplyFailure};
    screens("edit_apply_applying", |lang| {
        let mut h = three_applying(ready(lang)).0;
        // The status bar's spinner by the wall clock: its first frame.
        let started = std::time::Instant::now() + Duration::from_secs(3600);
        h.app.tab_mut().exec.running.as_mut().expect("applying").started = started;
        h
    });
    screens("edit_apply_applied", |lang| {
        let (mut h, id, steps) = three_applying(ready(lang));
        h.db(DbEvent::Applied { id, result: Ok(three_done(&steps)) });
        h.app.transient = None;
        h
    });
    screens("edit_apply_not_committed", |lang| {
        let (mut h, id, steps) = three_applying(manual(lang));
        h.db(DbEvent::Applied { id, result: Ok(three_done(&steps)) });
        h.tab_db(0, DbEvent::ManualBegun { id, index: 0 });
        h.db(DbEvent::Block(true));
        h.db(DbEvent::TxOpen(true));
        h.app.transient = None;
        h
    });
    screens("edit_apply_failed", |lang| {
        let (mut h, id, _) = three_applying(ready(lang));
        let why = ApplyError::Count { got: 0 };
        h.db(DbEvent::Applied { id, result: Err(ApplyFailure { step: Some(0), why }) });
        h.app.transient = None;
        h
    });
}

fn edits_mut(h: &mut Harness) -> &mut EditState {
    match &mut h.app.tab_mut().results {
        Results::Rows(rs) => &mut rs.edit,
        _ => panic!("no row result"),
    }
}

/// Run `sql` in the tab and let the server say it was done (`tag`).
fn ran(h: &mut Harness, sql: &str, tag: &str) {
    use datarig_core::driver::Outcome;
    h.app.run(vec![sql.to_string()]);
    let id = h.app.tab().exec.query_id;
    h.db(DbEvent::Done { id, outcome: Outcome::Command(tag.into()), elapsed: Duration::ZERO });
}

/// `y` is refused while the tab waits for a commit or a rollback (the app's own for an idle
/// transaction too): the Apply would land in a transaction that is ending.
#[test]
fn y_waits_while_the_tab_ends_its_transaction() {
    use datarig_tui::app::tx::Ending;
    let mut h = manual(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    h.app.tab_mut().exec.ending = Some(Ending { id: 99, commit: false, idle: None });
    h.key(KeyCode::Char('y'));
    assert!(no_apply_sent(&mut h), "nothing sent into a transaction that is ending");
    assert!(edits(&h).applying.is_none());
    assert!(h.app.tab().exec.running.is_none(), "the tab is not left busy");
}

/// The answer came for the result's Apply, but its changes are no longer the ones sent: what
/// it did is not known, and applying them again needs the user's word.
#[test]
fn an_answer_to_changes_no_longer_pending_makes_the_result_unknown() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, steps, _) = applies(&mut h);
    edits_mut(&mut h).batch.discard();
    h.db(DbEvent::Applied { id, result: Ok(stood(&steps)) });
    assert!(edits(&h).applying.is_none());
    assert!(edits(&h).apply_unknown, "unknown, not dropped");
    let notes = &h.app.tab().exec.run.notes;
    assert_eq!(notes.last().map(|n| n.msg.clone()), Some(Label::EditFailedUnknown.into()), "said");
}

/// A question asked before the Apply was sent (drop the changes?) answered while it runs:
/// nothing changes the changes being applied.
#[test]
fn the_changes_are_not_dropped_while_they_are_applied() {
    use datarig_tui::app::overlay::{Buttons, Confirm, Overlay};
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    applies(&mut h);
    h.key(KeyCode::Esc);
    let (tab, result) = match &h.app.tab().results {
        Results::Rows(rs) => (h.app.tab().id, rs.id),
        _ => unreachable!(),
    };
    h.app.overlays.push(Overlay::Confirm(Confirm {
        title: Label::EditDiscardTitle,
        text: Msg::EditDiscardText { count: 1 },
        details: Vec::new(),
        keys: Label::EditDiscardKeys,
        action: ConfirmAction::DiscardEdits { tab, result },
        folder: None,
        path: None,
        tx: None,
        buttons: Buttons::default(),
    }));
    h.key(KeyCode::Char('y'));
    assert_eq!(edits(&h).batch.counts().total(), 1, "the changes stay");
    let refused = Some(Msg::EditRefused { reason: "the pending changes are being applied".into() });
    assert_eq!(said(&mut h), refused);
}

/// Under manual commit the session was lost after the Apply's answer and before the block it
/// opened was reported: the server rolled it back with the session.
#[test]
fn a_session_lost_before_the_kept_transaction_was_reported_drops_the_applied_changes() {
    let mut h = manual(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, steps, tx) = applies(&mut h);
    assert!(matches!(tx, ApplyTx::Keep { .. }));
    h.db(DbEvent::Applied { id, result: Ok(stood(&steps)) });
    assert!(applied(&h).tx.is_some());
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    assert!(edits(&h).applied.is_none(), "the original values show again");
    assert!(h.app.tab().exec.run.notes.iter().any(|n| n.msg == Label::EditAppliedGone.into()));
}

/// The block an Apply ran in ended without the app knowing how (`PREPARE TRANSACTION`, a
/// procedure that ended it): the changes are not shown as committed.
#[test]
fn a_transaction_that_ended_unknown_drops_the_applied_changes() {
    let mut h = ready(Lang::En);
    h.db(DbEvent::Block(true));
    h.db(DbEvent::TxOpen(true));
    type_cell(&mut h, "z@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, steps, _) = applies(&mut h);
    h.db(DbEvent::Applied { id, result: Ok(stood(&steps)) });
    assert!(applied(&h).tx.is_some());
    ran(&mut h, "PREPARE TRANSACTION 'x'", "PREPARE TRANSACTION");
    h.db(DbEvent::Block(false));
    h.db(DbEvent::TxOpen(false));
    assert!(edits(&h).applied.is_none(), "not shown as committed");
    let text = "The transaction ended without the app knowing how; re-run the query to see the table";
    let notes = &h.app.tab().exec.run.notes;
    assert!(notes.iter().any(|n| h.app.i18n.msg(&n.msg) == text), "{notes:?}");
}

/// A savepoint statement the user ran in the block an Apply's changes are in: what the table
/// holds is no longer known from them. Only once the server confirmed it; a committed layer
/// stays.
#[test]
fn a_savepoint_statement_drops_the_uncommitted_applied_changes() {
    use datarig_core::driver::DbEvent as E;
    let text = "A savepoint statement ran; re-run the query to see the table";
    for (sql, tag) in [
        ("SAVEPOINT a", "SAVEPOINT"),
        ("RELEASE SAVEPOINT a", "RELEASE"),
        ("rollback to savepoint a", "ROLLBACK"),
        ("ROLLBACK WORK TO a", "ROLLBACK"),
    ] {
        let mut h = ready(Lang::En);
        h.db(E::Block(true));
        h.db(E::TxOpen(true));
        type_cell(&mut h, "z@x");
        h.command("apply");
        h.key(KeyCode::Char('y'));
        let (id, steps, _) = applies(&mut h);
        h.db(E::Applied { id, result: Ok(stood(&steps)) });
        ran(&mut h, sql, tag);
        assert!(edits(&h).applied.is_none(), "{sql}");
        let notes = &h.app.tab().exec.run.notes;
        assert!(notes.iter().any(|n| h.app.i18n.msg(&n.msg) == text), "{sql}: {notes:?}");
    }
    // Refused by the server: nothing changed.
    let mut h = ready(Lang::En);
    h.db(E::Block(true));
    h.db(E::TxOpen(true));
    type_cell(&mut h, "z@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, steps, _) = applies(&mut h);
    h.db(E::Applied { id, result: Ok(stood(&steps)) });
    h.app.run(vec!["ROLLBACK TO SAVEPOINT nope".to_string()]);
    let q = h.app.tab().exec.query_id;
    h.db(E::Failed { id: q, error: DbError::Server("no such savepoint".into()), cancelled: false });
    assert!(edits(&h).applied.is_some(), "a failed one changes nothing");
    // Committed already: the table holds them whatever the savepoints do.
    let (mut h, id, steps) = three_applying(ready(Lang::En));
    h.db(E::Applied { id, result: Ok(three_done(&steps)) });
    ran(&mut h, "SAVEPOINT a", "SAVEPOINT");
    assert!(edits(&h).applied.is_some());
}

/// Only the values the Apply wrote (and every value of a row it added) take the applied tone:
/// a value the server returned as it was read, NULL included, and one a trigger changed, do
/// not.
#[test]
fn only_the_values_written_take_the_applied_tone() {
    use datarig_tui::theme;
    let th = theme::DARK;
    let (mut h, id, steps) = three_applying(ready(Lang::En));
    h.app.theme = std::sync::Arc::new(th.clone());
    let mut done = three_done(&steps);
    for (s, d) in steps.iter().zip(done.iter_mut()) {
        if s.sql.starts_with("UPDATE ") {
            // Row 1 as read (`1`, `a@x`, `ann`) with the email written; the nickname changed by
            // a trigger.
            d.returned = users_returned([Some("1"), Some("z@x"), Some("Ann"), Some("ann!"), Some("{}")]);
        }
    }
    h.db(DbEvent::Applied { id, result: Ok(done) });
    let t = h.draw(120, 40);
    let buf = t.backend().buffer();
    let g = &h.app.tab().grid;
    let at = |r: usize, c: usize| {
        let x = g.cell_rect(r, c, 3).expect("on screen");
        buf[(x.x + 1, x.y)].clone()
    };
    assert_eq!(at(0, 1).fg, th.edit_applied.fg.unwrap(), "the email written");
    assert_ne!(at(0, 0).fg, th.edit_applied.fg.unwrap(), "the id as read");
    assert_ne!(at(0, 2).fg, th.edit_applied.fg.unwrap(), "the nickname a trigger changed");
    assert_eq!(at(3, 2).fg, th.edit_applied.fg.unwrap(), "a new row's NULL");
    // A row whose NULL came back as it was read.
    let mut h = ready(Lang::En);
    h.app.theme = std::sync::Arc::new(th.clone());
    h.keys("j");
    type_cell(&mut h, "y@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, steps, _) = applies(&mut h);
    let mut done = stood(&steps);
    done.last_mut().unwrap().returned = users_returned([Some("2"), Some("y@x"), Some(""), None, Some("{}")]);
    h.db(DbEvent::Applied { id, result: Ok(done) });
    let t = h.draw(120, 40);
    let buf = t.backend().buffer();
    let g = &h.app.tab().grid;
    let x = g.cell_rect(1, 2, 3).expect("on screen");
    assert_ne!(buf[(x.x + 1, x.y)].fg, th.edit_applied.fg.unwrap(), "NULL as read");
    let x = g.cell_rect(1, 1, 3).expect("on screen");
    assert_eq!(buf[(x.x + 1, x.y)].fg, th.edit_applied.fg.unwrap(), "the email written");
}

/// A result read inside the open transaction: once an Apply changed it, the strip says the
/// changes are not committed (not only that the rows were read in a transaction).
#[test]
fn the_applied_badge_wins_over_the_transactions_mark() {
    let mut h = connected(Lang::En);
    h.db(DbEvent::Block(true));
    h.db(DbEvent::TxOpen(true));
    let mut h = resolved(h, "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows(), named());
    assert!(screen(&mut h, 120, 40).contains("uncommitted (in tx)"), "read in the transaction");
    type_cell(&mut h, "z@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, steps, _) = applies(&mut h);
    h.db(DbEvent::Applied { id, result: Ok(stood(&steps)) });
    h.app.transient = None;
    let s = screen(&mut h, 120, 40);
    assert!(s.contains("applied · not committed"), "{s}");
    assert!(!s.contains("uncommitted (in tx)"), "{s}");
}

/// At 80x24 the strip has room for short labels only: its Changes tab says the rows were
/// applied, and that tab whether that is committed.
#[test]
fn the_applied_badge_fits_a_small_screen() {
    for lang in [Lang::En, Lang::Ko] {
        let (mut h, id, steps) = three_applying(ready(lang));
        h.db(DbEvent::Applied { id, result: Ok(three_done(&steps)) });
        h.app.transient = None;
        let want = h.app.i18n.label(Label::ResultsTabAppliedShort).to_string();
        let s = screen(&mut h, 80, 24);
        let chart = h.app.i18n.label(Label::ResultsTabChart).to_string();
        let strip = s.lines().find(|l| l.contains(&chart)).unwrap_or_default().to_string();
        assert!(strip.contains(&want), "{lang:?}: {strip}");
        let (mut h, id, steps) = three_applying(manual(lang));
        h.db(DbEvent::Applied { id, result: Ok(three_done(&steps)) });
        h.tab_db(0, DbEvent::ManualBegun { id, index: 0 });
        h.db(DbEvent::Block(true));
        h.db(DbEvent::TxOpen(true));
        h.app.transient = None;
        h.command("changes");
        let want = h.app.i18n.label(Label::ChangesAppliedKept).to_string();
        let s = screen(&mut h, 80, 24);
        assert!(s.contains(&want), "{lang:?}: {s}");
    }
}

/// A value the server returned in another form than the result shows (PostgreSQL's text of a
/// time stamp): written, it shows dim `re-run to see` rather than another spelling; not
/// written, as read.
#[test]
fn a_value_returned_in_another_form_is_seen_by_a_run() {
    use datarig_tui::theme;
    use ratatui::style::Modifier;
    let th = theme::DARK;
    let (mut h, id, steps) = three_applying(ready(Lang::En));
    h.app.theme = std::sync::Arc::new(th.clone());
    let mut done = three_done(&steps);
    for (s, d) in steps.iter().zip(done.iter_mut()) {
        if s.sql.starts_with("UPDATE ") {
            d.returned = users_returned([Some("1"), Some("Z@X"), Some("Ann"), Some("ANN"), Some("{}")]);
            d.unshown = vec![1, 3];
        } else if s.sql.starts_with("INSERT ") {
            d.unshown = vec![0];
        }
    }
    h.db(DbEvent::Applied { id, result: Ok(done) });
    let a = applied(&h);
    assert_eq!(a.rerun, [(0, 1), (0, 2)].into(), "the email written, the nickname not written");
    assert_eq!(a.rerun_new, [(0, 0)].into(), "the new row's id");
    let t = h.draw(120, 40);
    let buf = t.backend().buffer();
    let g = &h.app.tab().grid;
    let text = |r: usize, c: usize| {
        let x = g.cell_rect(r, c, 3).expect("on screen");
        (0..x.width).map(|i| buf[(x.x + i, x.y)].symbol().to_string()).collect::<String>().trim().to_string()
    };
    let at = |r: usize, c: usize| {
        let x = g.cell_rect(r, c, 3).expect("on screen");
        buf[(x.x + 1, x.y)].clone()
    };
    assert!(text(0, 1).starts_with("re-run"), "{}", text(0, 1));
    assert!(at(0, 1).modifier.contains(Modifier::DIM), "dim");
    assert!(text(0, 2).starts_with("re-run"), "not written, it may have changed: {}", text(0, 2));
    assert!(text(3, 0).starts_with("re-r"), "{}", text(3, 0));
    assert_eq!(text(3, 1), "n@x");
}

// Leaving pending changes; pending and applied values in what the grid's other views show.

/// What the last flash said, in English.
fn said_text(h: &mut Harness) -> String {
    said(h).map(|m| h.app.i18n.msg(&m).to_string()).unwrap_or_default()
}

/// The text of the question asked, if one is.
fn asked(h: &Harness) -> Option<String> {
    h.app.overlays.confirm().map(|c| h.app.i18n.msg(&c.text).to_string())
}

fn executed(h: &mut Harness) -> Vec<Vec<String>> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { statements, .. } => Some(statements),
            _ => None,
        })
        .collect()
}

/// Running a statement in the tab, closing the tab and quitting ask first while changes are
/// pending: `Enter` keeps them (nothing runs, closes or quits), `y` drops them and goes on.
#[test]
fn leaving_pending_changes_asks_first() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.app.run(vec!["SELECT 1".into()]);
    let run = "Running a statement here replaces this tab's results: its 1 pending change is dropped.";
    assert_eq!(asked(&h).as_deref(), Some(run), "asks before the run");
    assert!(executed(&mut h).is_empty());
    h.key(KeyCode::Enter);
    assert!(h.app.overlays.confirm().is_none());
    assert!(executed(&mut h).is_empty(), "kept: nothing runs");
    assert_eq!(edits(&h).batch.counts().total(), 1);
    h.app.run(vec!["SELECT 1".into()]);
    h.key(KeyCode::Char('y'));
    assert_eq!(executed(&mut h), [vec!["SELECT 1".to_string()]], "dropped: it runs");
    assert!(edits(&h).batch.is_empty());

    // Closing the tab.
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("tabnew");
    assert_eq!(h.app.tabs.len(), 2);
    h.app.tabs.activate(0);
    h.command("tabclose");
    assert_eq!(asked(&h).as_deref(), Some("Closing the tab drops the 1 pending change of its results."));
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tabs.len(), 2, "kept: the tab stays");
    h.command("tabclose");
    h.key(KeyCode::Char('y'));
    assert_eq!(h.app.tabs.len(), 1, "dropped: the tab closes");

    // Quitting.
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("q");
    assert_eq!(asked(&h).as_deref(), Some("Quitting drops 1 pending change that was not applied."));
    h.key(KeyCode::Enter);
    assert!(!h.app.quit);
    h.command("q");
    h.key(KeyCode::Char('y'));
    assert!(h.app.quit, "dropped: the app quits");

    // Applied changes are no pending ones: nothing asks.
    let (mut h, id, steps) = three_applying(ready(Lang::En));
    h.db(DbEvent::Applied { id, result: Ok(three_done(&steps)) });
    h.app.run(vec!["SELECT 1".into()]);
    assert_eq!(asked(&h), None);
    assert_eq!(executed(&mut h), [vec!["SELECT 1".to_string()]]);
}

/// A copy takes the values the grid shows: pending ones and applied ones, and says so when
/// some are pending; a row to be deleted is copied as shown and counted in the message; a cell
/// shown as `re-run to see` is not copied.
#[test]
fn a_copy_takes_the_values_shown() {
    let mut h = ready(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    type_cell(&mut h, "z@x");
    h.keys("y");
    assert_eq!(clip.last().as_deref(), Some("z@x"), "the pending value");
    let s = said_text(&mut h);
    assert!(s.ends_with(" (with pending changes)"), "{s}");
    h.keys("Y");
    assert_eq!(clip.last().as_deref(), Some("1\tz@x\tann"));
    h.keys("jD");
    h.command("copy tsv all");
    let all = clip.last().unwrap_or_default();
    assert!(all.contains("z@x") && all.contains("b@x") && !all.contains("a@x"), "{all}");
    let s = said_text(&mut h);
    assert!(s.ends_with(" (with pending changes; 1 row to be deleted)"), "{s}");
    // Applied values; one returned in another form is refused.
    let (mut h, id, steps) = three_applying(ready(Lang::En));
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    let mut done = three_done(&steps);
    for (s, d) in steps.iter().zip(done.iter_mut()) {
        if s.sql.starts_with("UPDATE ") {
            d.returned = users_returned([Some("1"), Some("z@x"), Some("Ann"), Some("ann!"), Some("{}")]);
        }
    }
    h.db(DbEvent::Applied { id, result: Ok(done) });
    h.app.transient = None;
    h.keys("gg");
    h.keys("Y");
    assert_eq!(clip.last().as_deref(), Some("1\tz@x\tann!"), "the values applied");
    let s = said_text(&mut h);
    assert!(!s.contains("pending"), "{s}");
    let (mut h, id, steps) = three_applying(ready(Lang::En));
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    let mut done = three_done(&steps);
    for (s, d) in steps.iter().zip(done.iter_mut()) {
        if s.sql.starts_with("UPDATE ") {
            d.unshown = vec![1];
        }
    }
    h.db(DbEvent::Applied { id, result: Ok(done) });
    h.app.transient = None;
    h.keys("gg");
    h.keys("y");
    assert_eq!(clip.last(), None, "not copied");
    assert_eq!(said_text(&mut h), "Not copied: email shows “re-run to see”; run the query again to copy it");
}

/// A pending DEFAULT has no value yet: a text copy refuses it; an SQL INSERT writes `DEFAULT`
/// (its VALUES take it); every other SQL form refuses it.
#[test]
fn a_pending_default_is_copied_only_into_an_insert() {
    let mut st = named();
    st.columns[3].default = Some("'anon'::text".into());
    let mut h =
        resolved(connected(Lang::En), "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows(), st);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("l");
    h.keys(" dD");
    assert_eq!(edits(&h).batch.value(0, 2), Some(&Value::Default));
    h.app.transient = None;
    let refused = "Not copied: nickname has a pending DEFAULT, which has no value yet";
    h.keys("y");
    assert_eq!(said_text(&mut h), refused);
    assert_eq!(clip.last(), None);
    h.command("copy csv all");
    assert_eq!(said_text(&mut h), refused);
    h.command("copy update all");
    assert_eq!(said_text(&mut h), refused);
    assert_eq!(clip.last(), None);
    h.command("copy insert all");
    let text = clip.last().unwrap_or_default();
    let first = text.lines().next().unwrap_or_default().to_string();
    assert!(first.ends_with("'a@x', DEFAULT);"), "{text}");
    assert!(text.lines().nth(1).is_some_and(|l| l.ends_with("'b@x', NULL);")), "{text}");
    let s = said_text(&mut h);
    assert!(s.ends_with(" (with pending changes)"), "{s}");
}

/// The grid's search looks at the values shown: a pending value is found, the value it
/// replaces is not.
#[test]
fn the_grid_search_finds_the_values_shown() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.keys("jjh");
    h.keys("/");
    h.type_text("z@x");
    h.key(KeyCode::Enter);
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (0, 1), "the pending value is found");
    h.key(KeyCode::Esc);
    h.keys("jj");
    h.keys("/");
    h.type_text("a@x");
    h.key(KeyCode::Enter);
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (2, 1), "the value replaced is not");
}

/// The cell viewer and the inspector show the value the grid shows.
#[test]
fn the_viewer_and_the_inspector_show_the_values_shown() {
    use datarig_tui::app::overlay::Overlay;
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.key(KeyCode::Enter);
    let text = match h.app.overlays.top() {
        Some(Overlay::CellViewer(v)) => v.text.clone(),
        _ => panic!("no viewer"),
    };
    assert_eq!(text, "z@x");
    h.key(KeyCode::Esc);
    // The inspector is shown beside a wide enough grid.
    let t = h.draw(160, 45);
    let d = h.app.layout.detail;
    let buf = t.backend().buffer();
    let inside: String = (d.y..d.y + d.height)
        .map(|y| (d.x..d.x + d.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(inside.contains("z@x") && !inside.contains("a@x"), "{inside}");
}

/// An unwritten column whose returned value is in another form than the result shows: it may
/// have changed (a trigger), so a run shows it.
#[test]
fn a_value_not_written_returned_in_another_form_is_seen_by_a_run() {
    let (mut h, id, steps) = three_applying(ready(Lang::En));
    let mut done = three_done(&steps);
    for (s, d) in steps.iter().zip(done.iter_mut()) {
        if s.sql.starts_with("UPDATE ") {
            d.unshown = vec![3];
        }
    }
    h.db(DbEvent::Applied { id, result: Ok(done) });
    assert!(applied(&h).rerun.contains(&(0, 2)), "{:?}", applied(&h).rerun);
    let s = screen(&mut h, 120, 40);
    let row = s.lines().find(|l| l.contains("z@x")).unwrap_or_default().to_string();
    assert!(row.contains("re-run") && !row.contains("ann"), "{row}");
}

/// An Apply that stays in the open transaction counts there: its writes in the status bar's
/// changes, one entry in the transaction's summary, and the commit says them.
#[test]
fn a_kept_apply_counts_in_the_open_transaction() {
    let mut h = manual(Lang::En);
    type_cell(&mut h, "z@x");
    h.keys("jD");
    h.keys("ol");
    type_cell(&mut h, "n@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, steps, _) = applies(&mut h);
    h.db(DbEvent::Applied { id, result: Ok(three_done(&steps)) });
    h.tab_db(0, DbEvent::ManualBegun { id, index: 0 });
    h.db(DbEvent::Block(true));
    h.db(DbEvent::TxOpen(true));
    let status = h.app.tx_status().unwrap_or_default();
    assert!(status.contains("3 changes · 3 rows"), "{status}");
    let log: Vec<String> =
        h.app.tab().exec.tx.as_ref().map(|t| t.log.iter().map(|e| e.sql.clone()).collect()).unwrap_or_default();
    assert_eq!(log, ["Apply (3 changes)"]);
    h.app.transient = None;
    h.command("commit");
    let end = h.sent().into_iter().find_map(|c| match c {
        DbCommand::EndTx { id, commit: true } => Some(id),
        _ => None,
    });
    h.tab_db(0, DbEvent::TxEnded { id: end.expect("a commit"), outcome: datarig_core::driver::TxOutcome::Committed });
    let notes = &h.app.tab().exec.run.notes;
    assert!(notes.iter().any(|n| matches!(n.msg, Msg::TxCommitted { count: 3, .. })), "{notes:?}");
    // Inside the user's own block: counted at once.
    let mut h = ready(Lang::En);
    h.db(DbEvent::Block(true));
    h.db(DbEvent::TxOpen(true));
    type_cell(&mut h, "z@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, steps, _) = applies(&mut h);
    h.db(DbEvent::Applied { id, result: Ok(stood(&steps)) });
    let status = h.app.tx_status().unwrap_or_default();
    assert!(status.contains("1 change · 1 row"), "{status}");
}

/// Paging past the rows read after an Apply would run the statement again from an offset the
/// Apply moved (deleted rows shift the rest up): refused, the query is run again instead.
#[test]
fn paging_on_after_an_apply_is_refused() {
    let mut h = connected(Lang::En);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    h.app.run(vec!["SELECT id, email, nickname FROM shop.users".into()]);
    let qid = h.app.tab().exec.query_id;
    h.db(DbEvent::Page {
        id: qid,
        columns: Some(users_columns()),
        rows: users_rows(),
        more: true,
        elapsed: Duration::ZERO,
    });
    h.app.focus = Focus::Results;
    h.keys("le");
    structure(&mut h, named());
    h.key(KeyCode::Esc);
    h.app.transient = None;
    h.keys("jD");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, steps, _) = applies(&mut h);
    h.db(DbEvent::Applied { id, result: Ok(stood(&steps)) });
    h.app.transient = None;
    h.keys("n");
    let sent = h.sent();
    assert!(!sent.iter().any(|c| matches!(c, DbCommand::Resume { .. })), "{sent:?}");
    assert_eq!(said_text(&mut h), "Changes were applied to these rows: re-run the query to see the rest");
}

/// `Enter` views a cell of a row read; on a new row it does nothing, and the hint line does not
/// offer it.
#[test]
fn the_hint_line_offers_no_view_on_a_new_row() {
    let mut h = ready(Lang::En);
    let view = |h: &Harness| h.app.hints().iter().any(|(_, l)| &**l == "view cell");
    assert!(view(&h));
    h.keys("o");
    assert!(h.app.tab().grid.row >= 3);
    assert!(!view(&h), "{:?}", h.app.hints());
    h.keys("k");
    assert!(view(&h));
}

/// An export of the rows held would write them as read, not as the grid shows them: refused
/// while some are pending or applied, in every scope that writes them and every format.
#[test]
fn an_export_of_the_rows_held_is_refused_while_they_are_edited() {
    let dir = std::env::temp_dir().join(format!("datarig-edit-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let refused =
        "Not exported: this result has pending or applied changes; apply or discard them, or run the query again";
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    let (pending, id, steps) = three_applying(ready(Lang::En));
    let mut applied_h = pending;
    applied_h.db(DbEvent::Applied { id, result: Ok(three_done(&steps)) });
    applied_h.app.transient = None;
    let pending_refused = "Not exported: this result has pending changes; apply or discard them, or run the query \
                           again (see Changes)";
    for (n, h) in [&mut h, &mut applied_h].into_iter().enumerate() {
        let refused = if n == 0 { pending_refused } else { refused };
        for (i, (format, scope)) in
            [("csv", "fetched"), ("json", "selection"), ("xlsx", "all"), ("insert", "fetched")].into_iter().enumerate()
        {
            let path = dir.join(format!("{n}-{i}.out"));
            if scope == "selection" {
                h.keys("ggvj");
            }
            h.command(&format!("export {format} {scope} {}", path.display()));
            assert_eq!(said_text(h), refused, "{format} {scope}");
            assert!(!h.app.exporting(h.app.tab().id), "{format} {scope}");
            assert!(!path.exists(), "{format} {scope}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every row read again from the server: allowed, but it says the pending changes are not in
/// the file.
#[test]
fn an_export_of_every_row_says_the_pending_changes_are_left_out() {
    let mut h = connected(Lang::En);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    h.app.run(vec!["SELECT id, email, nickname FROM shop.users".into()]);
    let qid = h.app.tab().exec.query_id;
    h.db(DbEvent::Page {
        id: qid,
        columns: Some(users_columns()),
        rows: users_rows(),
        more: true,
        elapsed: Duration::ZERO,
    });
    h.app.focus = Focus::Results;
    h.keys("le");
    structure(&mut h, named());
    h.key(KeyCode::Esc);
    h.app.transient = None;
    type_cell(&mut h, "z@x");
    h.app.transient = None;
    let path = std::env::temp_dir().join(format!("datarig-edit-export-all-{}.csv", std::process::id()));
    h.command(&format!("export csv all {}", path.display()));
    let note = "The pending changes are not in the file: every row is read again from the server";
    let details: Vec<String> = h
        .app
        .overlays
        .confirm()
        .map(|c| c.details.iter().map(|d| h.app.i18n.msg(d).to_string()).collect())
        .unwrap_or_default();
    let s = said_text(&mut h);
    assert!(s == note || details.iter().any(|d| d == note), "{s} / {details:?}");
    assert!(h.app.exporting(h.app.tab().id), "the export goes on");
    let _ = std::fs::remove_file(&path);
}

/// The chart would draw the values as read: while some are pending or applied it says why it
/// draws nothing.
#[test]
fn the_chart_draws_nothing_while_the_rows_are_edited() {
    let why = "Not drawn: this result has pending or applied changes. Run the query again to chart the edited values.";
    let pending = "Not drawn: this result has pending changes. Apply or discard them, or run the query again (see \
                   Changes)";
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.keys("c");
    let s = screen(&mut h, 160, 40);
    assert!(s.contains(pending), "{s}");
    let (mut h, id, steps) = three_applying(ready(Lang::En));
    h.db(DbEvent::Applied { id, result: Ok(three_done(&steps)) });
    h.app.transient = None;
    h.keys("c");
    let s = screen(&mut h, 160, 40);
    assert!(s.contains(why), "{s}");
    // Without changes it draws again.
    let mut h = ready(Lang::En);
    h.keys("c");
    let s = screen(&mut h, 160, 40);
    assert!(!s.contains(why), "{s}");
}

/// Deleting the profile closes its tabs: the question says how many pending changes go.
#[test]
fn deleting_the_profile_says_the_pending_changes_it_drops() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.keys("jD");
    let name = h.app.profiles[0].name.clone();
    h.explore(&name);
    h.keys("d");
    let details: Vec<String> = h
        .app
        .overlays
        .confirm()
        .map(|c| c.details.iter().map(|d| h.app.i18n.msg(d).to_string()).collect())
        .unwrap_or_default();
    assert!(details.iter().any(|d| d == "2 pending changes of the results are dropped."), "{details:?}");
}

/// A value an Apply wrote as DEFAULT is shown `default`: a copy says that, not `re-run to see`.
#[test]
fn a_copy_of_an_applied_default_says_default() {
    let mut st = named();
    st.columns[3].default = Some("'anon'::text".into());
    let mut h =
        resolved(connected(Lang::En), "SELECT id, email, nickname FROM shop.users", users_columns(), users_rows(), st);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("l");
    h.keys(" dD");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    let (id, steps, _) = applies(&mut h);
    h.db(DbEvent::Applied { id, result: Ok(stood(&steps)) });
    h.app.transient = None;
    h.keys("y");
    assert_eq!(clip.last(), None, "not copied");
    assert_eq!(said_text(&mut h), "Not copied: nickname shows “default”; run the query again to copy its value");
}

/// A MySQL profile whose tab shows `shop.people` (no key: its rows are found by their values),
/// a text `name` and `note` in `utf8mb4`, with `rows`; resolved, nothing sent.
fn mysql_people(lang: Lang, rows: Vec<Vec<Option<String>>>) -> Harness {
    use datarig_core::driver::keys::{KeyCatalog, NameRule};
    use datarig_core::driver::structure::{Collation, StructureColumn};
    use datarig_core::driver::{ColumnOrigin, SessionValues};
    let driver = FakeDriver::default();
    driver.mysql.store(true, std::sync::atomic::Ordering::SeqCst);
    let cfg = Config {
        connections: vec![ConnectionConfig {
            driver: "mysql".into(),
            policy: Some("stg".into()),
            ..ConnectionConfig::test_db()
        }],
        ..writes_config()
    };
    let mut h = Harness::with_driver(&cfg, lang, driver);
    h.db(DbEvent::Connected);
    let mut k = KeyCatalog::default();
    k.set_names(NameRule::MySql { tables_ignore_case: false });
    k.set_server_zone("UTC");
    let names = ["id", "name", "note"];
    k.add_table(1, "shop", "people", names.iter().enumerate().map(|(i, n)| (i as i16 + 1, n.to_string())));
    h.db(DbEvent::Keys(Ok(k)));
    let col = |name: &str, ty: &str, kind: ValueKind| {
        let origin = ColumnOrigin::Named { schema: "shop".into(), table: "people".into(), column: name.into() };
        ColumnMeta::new(name.into(), ty.into(), kind, Some(origin))
    };
    let columns = vec![
        col("id", "int", ValueKind::Integer),
        col("name", "varchar", ValueKind::Text),
        col("note", "text", ValueKind::Text),
    ];
    h.app.run(vec!["SELECT id, name, note FROM shop.people".to_string()]);
    h.db(DbEvent::SessionValues(SessionValues { time_zone: "SYSTEM".into(), charset: "utf8mb4".into() }));
    let id = h.app.tab().exec.query_id;
    h.db(DbEvent::Page { id, columns: Some(columns), rows, more: false, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
    let utf8 =
        Some(Collation { name: "utf8mb4_0900_ai_ci".into(), charset: Some("utf8mb4".into()), deterministic: true });
    let column = |name: &str, ty: &str, collation: Option<Collation>| StructureColumn {
        name: name.into(),
        type_name: ty.into(),
        not_null: false,
        default: None,
        fill: ColumnFill::Default,
        enum_labels: Vec::new(),
        collation,
    };
    let mut st = TableStructure::new(datarig_core::driver::structure::RelationKind::Table);
    st.columns =
        vec![column("id", "int", None), column("name", "varchar(200)", utf8.clone()), column("note", "text", utf8)];
    h.keys("le");
    let name = h.app.profiles[0].name.clone();
    h.meta_db(&name, DbEvent::Structure { schema: "shop".into(), table: "people".into(), result: Ok(Box::new(st)) });
    assert_eq!(h.app.key_context(), Ctx::CellEdit, "{:?}", said(&mut h));
    h.key(KeyCode::Esc);
    h.app.transient = None;
    h.sent();
    h
}

/// The first row's note of `mysql_people` set to `note` (a backslash makes it a literal of its
/// bytes), previewed, its row counted once.
fn mysql_note_previewed(lang: Lang, rows: Vec<Vec<Option<String>>>, note: &str) -> Harness {
    let mut h = mysql_people(lang, rows);
    h.keys("l");
    type_cell(&mut h, note);
    h.command("apply");
    let asked = counts(&mut h);
    let why = || format!("{:.400?} {:.400?}", h.app.edit_preview_view().map(|v| v.lines), h.app.transient);
    assert_eq!(asked.len(), 1, "the row is counted: {}", why());
    h.db(DbEvent::Counted { id: asked[0].0, result: Ok(1), snapshot: false });
    h.app.transient = None;
    h
}

fn people(name: &str, note: &str) -> Vec<Vec<Option<String>>> {
    vec![vec![s("1"), s(name), s(note)]]
}

/// MySQL compares text by its UTF-8 bytes (`_utf8mb4 X'…'`): each statement that carries such
/// literals is followed by a line, not SQL, that reads them back as `column = 'text'` in order:
/// the set value, then the compared ones (once each), Hangul included.
#[test]
fn a_mysql_statements_text_bytes_are_read_back_under_it() {
    // A Korean name (escapes: no Hangul in source).
    let name = "\u{AE40}\u{B3D9}\u{D718}";
    let mut h = mysql_note_previewed(Lang::En, people(name, "old"), "new \\ note");
    let v = h.app.edit_preview_view().expect("open");
    assert!(v.apply);
    let hex = |t: &str| t.bytes().map(|b| format!("{b:02X}")).collect::<String>();
    let check = format!(
        "SELECT 1 FROM `shop`.`people` WHERE `id` <=> 1 AND `name` = _utf8mb4 X'{n}' AND CONVERT(`name` USING utf8mb4) \
         COLLATE utf8mb4_0900_bin <=> _utf8mb4 X'{n}' AND `note` = _utf8mb4 X'{o}' AND CONVERT(`note` USING utf8mb4) \
         COLLATE utf8mb4_0900_bin <=> _utf8mb4 X'{o}' FOR UPDATE",
        n = hex(name),
        o = hex("old"),
    );
    let read = format!("name = '{name}', note = 'old'");
    assert_eq!(v.lines[1], step(&check, 1, false));
    assert_eq!(v.lines[2], PreviewLine::Values(format!("where {read}")));
    assert_eq!(v.lines[3], head("Writes"));
    let PreviewLine::Step { sql, .. } = &v.lines[4] else { panic!("the update: {:?}", v.lines[4]) };
    assert!(sql.starts_with(&format!("UPDATE `shop`.`people` SET `note` = _utf8mb4 X'{}' WHERE ", hex("new \\ note"))));
    // The column written and found by, told apart.
    assert_eq!(v.lines[5], PreviewLine::Values(format!("set note = 'new \\ note' \u{B7} where {read}")));
    assert_eq!(v.lines.len(), 6);
    // Shown under each statement, marked.
    let screen = screen(&mut h, 120, 40);
    assert_eq!(screen.matches(&format!("\u{21B3} where {read}")).count(), 1, "{screen}");
    assert!(screen.contains("\u{21B3} set note = 'new \\ note' \u{B7} where name = "), "{screen}");
}

/// The read-back values are display only: Copy SQL copies exactly the statements that are sent,
/// each ended with `;`, and the Apply sends them as planned.
#[test]
fn copy_sql_copies_what_is_sent_and_not_the_values_read_back() {
    let mut h = mysql_note_previewed(Lang::En, people("Ann", "old"), "a\\b");
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    let v = h.app.edit_preview_view().expect("open");
    assert!(v.lines.iter().any(|l| matches!(l, PreviewLine::Values(_))));
    h.key(KeyCode::Char('c'));
    h.key(KeyCode::Char('y'));
    let (_, steps, _) = applies(&mut h);
    let sent: String = steps.iter().map(|s| format!("{};\n", s.sql)).collect();
    let copied = clip.last().expect("copied");
    assert_eq!(copied, sent);
    assert!(!copied.contains('\u{21B3}') && !copied.contains("name = 'Ann'"), "{copied}");
    let shown: Vec<&str> = v
        .lines
        .iter()
        .filter_map(|l| if let PreviewLine::Step { sql, .. } = l { Some(sql.as_str()) } else { None })
        .collect();
    assert_eq!(shown, steps.iter().map(|s| s.sql.as_str()).collect::<Vec<_>>(), "shown whole, as sent");
}

/// The text of row `y` of `buf` from column `from` to `to` (both included).
fn cols(buf: &ratatui::buffer::Buffer, y: u16, from: u16, to: u16) -> String {
    (from..=to).map(|x| buf[(x, y)].symbol()).collect()
}

/// The preview's box on `buf`: its top and bottom rows, its left and right columns.
fn dialog_box(buf: &ratatui::buffer::Buffer) -> (u16, u16, u16, u16) {
    let area = buf.area;
    let corner = (0..area.height).flat_map(|y| (0..area.width).map(move |x| (x, y))).find(|&(x, y)| {
        buf[(x, y)].symbol() == "\u{256D}" && cols(buf, y, x + 1, (x + 14).min(area.width - 1)) == " Apply changes"
    });
    let (left, top) = corner.expect("the dialog");
    let right = (left + 1..area.width).find(|&x| buf[(x, top)].symbol() == "\u{256E}").expect("its corner");
    let bottom = (top + 1..area.height).find(|&y| buf[(left, y)].symbol() == "\u{2570}").expect("its bottom");
    (top, bottom, left, right)
}

/// A statement wider than the dialog is wrapped over as many rows as it takes, every character
/// shown (nothing cut with `…`): its count at the end of its first row, the rest under it.
#[test]
fn a_long_statement_wraps_whole_with_its_count_on_its_first_row() {
    let mut h = mysql_note_previewed(Lang::En, people("plain name", "old"), "a\\b");
    let v = h.app.edit_preview_view().expect("open");
    for (w, hh) in [(120, 40), (80, 24)] {
        let t = h.draw(w, hh);
        let buf = t.backend().buffer();
        let (top, bottom, left, right) = dialog_box(buf);
        let rows: Vec<String> = (top..=bottom).map(|y| cols(buf, y, left + 1, right - 1)).collect();
        let first = rows.iter().position(|r| r.contains("SELECT 1 FROM `shop`.`people`")).expect("the check");
        assert!(rows[first].trim_end().ends_with("expect 1"), "{}", rows[first]);
        // The check's rows, joined back, are the statement.
        let inner = |r: &str| r.trim().trim_end_matches("expect 1").trim().to_string();
        let mut joined = inner(&rows[first]);
        let mut y = first + 1;
        while !rows[y].contains('\u{21B3}') {
            joined.push(' ');
            joined.push_str(&inner(&rows[y]));
            y += 1;
        }
        assert!(y > first + 1, "wrapped at {w}x{hh}");
        let PreviewLine::Step { sql, .. } = &v.lines[1] else { panic!() };
        assert_eq!(&joined, sql, "whole at {w}x{hh}");
        assert!(!rows[first..y].iter().any(|r| r.contains('\u{2026}')), "nothing cut at {w}x{hh}");
    }
}

/// The statements take the editor's syntax colors on the dialog's surface: a keyword as the
/// editor draws one, a quoted name, a string.
#[test]
fn the_statements_take_the_editors_syntax_colors() {
    use datarig_tui::theme;
    for th in [theme::DARK, theme::LIGHT] {
        let mut h = ready(Lang::En);
        h.app.theme = std::sync::Arc::new(th.clone());
        type_cell(&mut h, "z@x");
        h.command("apply");
        let t = h.draw(120, 40);
        let buf = t.backend().buffer();
        let (x, y) = (0..40u16)
            .find_map(|y| {
                row_text(buf, y).find("UPDATE \"shop\"").map(|i| (row_text(buf, y)[..i].chars().count() as u16, y))
            })
            .expect("the update on screen");
        let at = |dx: u16| buf[(x + dx, y)].clone();
        let keyword = th.syntax(datarig_core::sql::lexer::Tok::Keyword, false);
        assert_eq!((at(0).fg, at(0).bg), (keyword.fg.unwrap(), th.surface), "UPDATE");
        assert_eq!(at(0).modifier, keyword.add_modifier, "UPDATE");
        let quoted = th.syntax(datarig_core::sql::lexer::Tok::QuotedIdent, false);
        assert_eq!(at(7).fg, quoted.fg.unwrap(), "\"shop\"");
        let row = row_text(buf, y);
        let lit = row.find("'z@x'").expect("the value");
        let string = th.syntax(datarig_core::sql::lexer::Tok::Str, false);
        assert_eq!(buf[(row[..lit].chars().count() as u16, y)].fg, string.fg.unwrap(), "'z@x'");
        assert_ne!(keyword.fg, string.fg);
    }
}

/// The dialog clears its whole box: the editor's text, on screen under it before it opened, is
/// not seen through it anywhere, at either size.
#[test]
fn the_preview_clears_what_is_under_it() {
    let lines: Vec<String> = (0..60).map(|i| format!("-- BLEED{i:02} {}", "BLEED ".repeat(30))).collect();
    let mut h = mysql_people(Lang::En, people("plain name", "old"));
    h.app.tab_mut().editor = datarig_tui::widgets::editor::Editor::new(&lines.join("\n"));
    let sizes = [(120u16, 40u16), (80, 24)];
    let before: Vec<_> = sizes.iter().map(|&(w, hh)| h.draw(w, hh)).collect();
    h.keys("l");
    type_cell(&mut h, "a\\b");
    h.command("apply");
    let asked = counts(&mut h);
    h.db(DbEvent::Counted { id: asked[0].0, result: Ok(1), snapshot: false });
    for (&(w, hh), before) in sizes.iter().zip(&before) {
        let after = h.draw(w, hh);
        let buf = after.backend().buffer();
        let (top, bottom, left, right) = dialog_box(buf);
        let under = (top..=bottom).map(|y| cols(before.backend().buffer(), y, left, right));
        assert!(under.filter(|r| r.contains("BLEED")).count() > 3, "the editor under it at {w}x{hh}");
        for y in top..=bottom {
            let r = cols(buf, y, left, right);
            assert!(!r.contains("BLEED"), "seen through at {w}x{hh}: {r}\n{}", buffer_text(buf));
        }
    }
}

#[test]
fn the_mysql_preview_on_screen() {
    screens("edit_preview_mysql", |lang| mysql_note_previewed(lang, people("plain name", "old"), "a\\b"));
}

/// The keys on the bottom border end a border cell before its corner, as wide as the dialog is.
#[test]
fn the_keys_leave_the_border_before_the_corner() {
    let mut h = mysql_note_previewed(Lang::En, people("plain name", "old"), "a\\b");
    for (w, hh) in [(120u16, 40u16), (80, 24)] {
        let t = h.draw(w, hh);
        let buf = t.backend().buffer();
        let (_, bottom, left, right) = dialog_box(buf);
        assert_eq!(buf[(right, bottom)].symbol(), "\u{256F}");
        assert_eq!(buf[(right - 1, bottom)].symbol(), "\u{2500}", "at {w}x{hh}: {}", cols(buf, bottom, left, right));
        assert!(cols(buf, bottom, left, right).ends_with("Enter/Esc cancel \u{2500}\u{256F}"), "at {w}x{hh}");
    }
}

/// A long text in a value (48 KiB: its statements stay under the size the safety checks read):
/// its literals are shortened on screen (their start, how many
/// bytes are not shown, their end), so the body takes a few rows; it is laid out once per
/// width, not for every frame; what is copied and sent is the whole statement.
#[test]
fn a_long_value_is_shown_shortened_and_laid_out_once_per_width() {
    // The name (not the note typed into) holds it.
    let old = "lorem ipsum ".repeat(1 << 12);
    let mut h = mysql_note_previewed(Lang::En, people(&old, "old"), "a\\b");
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    let builds = |h: &Harness| h.app.overlays.edit_preview().expect("open").body_builds;
    let screen1 = screen(&mut h, 120, 40);
    assert_eq!(builds(&h), 1);
    let rows = h.app.overlays.edit_preview().unwrap().body_rows().unwrap();
    assert!(rows < 30, "{rows} rows:\n{screen1}");
    assert!(screen1.contains("more characters\u{2026}"), "{screen1}");
    assert!(screen1.contains("\u{21B3} where name = 'lorem ipsum lorem ipsum "), "{screen1}");
    assert!(screen1.contains("\u{2026}', note = 'old'"), "{screen1}");
    h.keys("j");
    let _ = screen(&mut h, 120, 40);
    assert_eq!(builds(&h), 1, "the same width: not laid out again");
    let _ = screen(&mut h, 80, 24);
    assert_eq!(builds(&h), 2, "another width");
    h.key(KeyCode::Char('c'));
    let hex: String = old.bytes().map(|b| format!("{b:02X}")).collect();
    let copied = clip.last().expect("copied");
    assert_eq!(copied.matches(&hex).count(), 4, "every literal whole");
    assert!(!copied.contains("more characters"));
}

/// A table whose name has a control character: the Apply's question and the dialog's title
/// spell it out.
#[test]
fn a_table_name_s_control_character_is_spelled_out_in_the_apply() {
    use datarig_core::driver::keys::{KeyCatalog, KeyKind};
    let table = "t\nx";
    let names = ["id", "email", "name", "nickname", "profile"];
    let keys = || {
        let mut k = KeyCatalog::default();
        k.add_table(USERS, "shop", table, names.iter().enumerate().map(|(i, n)| (i as i16 + 1, n.to_string())));
        k.mark(USERS, &[1], KeyKind::Primary);
        k
    };
    let resolve = |mut h: Harness| {
        h.db(DbEvent::Keys(Ok(keys())));
        answer(&mut h, "SELECT id, email, nickname FROM shop.\"t\nx\"", users_columns(), users_rows());
        h.keys("le");
        let name = h.app.profiles[0].name.clone();
        let st = Box::new(named());
        h.meta_db(&name, DbEvent::Structure { schema: "shop".into(), table: table.into(), result: Ok(st) });
        h.key(KeyCode::Esc);
        h.app.transient = None;
        h
    };
    // The grid's badge names the table so too.
    let mut h = resolve(Harness::connected(Lang::En));
    let screen = h.screen(160, 45);
    assert!(screen.contains(r"shop.t\nx"), "{screen}");
    // The question, where nothing confirms writes.
    type_cell(&mut h, "z@x");
    h.keys(" da");
    let question = h.app.status_ask_line().and_then(|(forms, _)| forms.first().cloned()).expect("asked");
    assert!(question.contains(r"shop.t\nx") && !question.contains('\n'), "{question}");
    // The dialog, where every write is confirmed.
    let mut h = resolve(connected(Lang::En));
    type_cell(&mut h, "z@x");
    h.keys(" da");
    let v = h.app.edit_preview_view().expect("the dialog");
    assert_eq!(v.title.to_string(), r"Apply changes: shop.t\nx");
}
