//! The Changes tab of a result: in the strip while the shown result has pending changes, one
//! line per changed cell (every reason they can't be applied first), or `s` the statements
//! that apply them; `Enter` shows a line's cell in the grid, `c` copies the statements as they
//! would be sent. Rows found by their values are counted for it once the changes settle, once
//! for each state of the changes.

mod common;

use common::*;
use datarig_core::config::ChangesView;
use datarig_core::driver::structure::TableStructure;
use datarig_core::driver::{ColumnMeta, DbCommand, DbEvent};
use datarig_core::i18n::{Label, Lang, Msg};
use datarig_tui::app::Focus;
use datarig_tui::app::Results;
use datarig_tui::app::edit::{COUNT_SETTLE, EditState};
use datarig_tui::app::tabs::ResultView;
use datarig_tui::keymap::Ctx;
use datarig_tui::widgets::editor::Editor;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
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

fn structure(h: &mut Harness, st: TableStructure) {
    let name = h.app.profiles[0].name.clone();
    h.meta_db(&name, DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Ok(Box::new(st)) });
}

/// `h` connected, the users' rows in the grid, resolved with `st` (`e` on the first row's
/// second column, then the editor closed); nothing sent yet.
fn resolved(h: Harness, st: TableStructure) -> Harness {
    resolved_rows(h, st, users_rows())
}

/// [`resolved`] with `rows` in the grid.
fn resolved_rows(mut h: Harness, st: TableStructure, rows: Vec<Vec<Option<String>>>) -> Harness {
    h.db(DbEvent::Keys(Ok(shop_keys())));
    h.app.run(vec![SQL.to_string()]);
    let id = h.app.tab().exec.query_id;
    h.db(DbEvent::Page { id, columns: Some(users_columns()), rows, more: false, elapsed: Duration::ZERO });
    h.app.focus = Focus::Results;
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

/// The users' rows by their key.
fn ready(lang: Lang) -> Harness {
    resolved(Harness::connected(lang), named())
}

/// The users' rows found by their values (no key).
fn by_values(lang: Lang) -> Harness {
    let mut st = named();
    st.primary_key = None;
    st.unique_constraints.clear();
    st.indexes.clear();
    resolved(Harness::connected(lang), st)
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

/// The counts sent since the last call, with their ids.
fn counts(h: &mut Harness) -> Vec<(u64, String)> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Count { id, sql } => Some((id, sql)),
            _ => None,
        })
        .collect()
}

fn view(h: &Harness) -> ResultView {
    h.app.tab().exec.view
}

fn screen(h: &mut Harness, w: u16, hh: u16) -> String {
    buffer_text(h.draw(w, hh).backend().buffer())
}

/// The strip's line as drawn `w`×30.
fn strip(h: &mut Harness, w: u16) -> String {
    let screen = screen(h, w, 30);
    assert_eq!(h.app.layout.strip.height, 1, "the strip is shown:\n{screen}");
    screen.lines().nth(h.app.layout.strip.y as usize).unwrap().to_string()
}

/// The lines of the results pane's body (under the strip, inside the border) as drawn
/// `w`×`hh`, without their trailing blanks.
fn pane(h: &mut Harness, w: u16, hh: u16) -> Vec<String> {
    let t = h.draw(w, hh);
    let buf = t.backend().buffer();
    let (r, strip) = (h.app.layout.results, h.app.layout.strip);
    (strip.y + 1..r.y + r.height - 1)
        .map(|y| {
            let mut line = String::new();
            let mut x = r.x + 1;
            while x < r.x + r.width - 1 {
                let sym = buf[(x, y)].symbol();
                line.push_str(sym);
                x += datarig_tui::text::width(sym).max(1) as u16;
            }
            line.trim_end().to_string()
        })
        .collect()
}

/// An update, a NULL, a delete and a new row in `h`, the grid at the first row.
fn four_changes(mut h: Harness) -> Harness {
    type_cell(&mut h, "z@x");
    h.keys("jjl");
    h.keys(" dn");
    h.keys("khD");
    h.keys("Go");
    h.keys("l");
    type_cell(&mut h, "n@x");
    h.keys("gg0");
    h.app.transient = None;
    h
}

const CHECK_1: &str = r#"SELECT 1 FROM "shop"."users" WHERE "id" = '1' AND "email" = 'a@x' FOR UPDATE NOWAIT"#;
const UPDATE_1: &str = r#"UPDATE "shop"."users" SET "email" = 'z@x' WHERE "id" = '1' AND "email" = 'a@x' RETURNING *"#;

#[test]
fn the_tab_comes_with_the_first_change_and_goes_with_the_last() {
    let mut h = ready(Lang::En);
    assert!(!strip(&mut h, 120).contains("Changes"), "nothing pending: no tab");
    type_cell(&mut h, "z@x");
    assert!(strip(&mut h, 120).contains("Changes 1"), "{}", strip(&mut h, 120));
    // `space d p` and `:changes` show it, with the focus.
    h.keys(" dp");
    assert_eq!(view(&h), ResultView::Changes);
    assert_eq!(h.app.key_context(), Ctx::Changes);
    assert!(strip(&mut h, 120).contains("[Changes 1]"));
    h.keys("H");
    assert_eq!(view(&h), ResultView::Rows);
    // From the editor too.
    h.app.focus = Focus::Editor;
    h.command("changes");
    assert_eq!(view(&h), ResultView::Changes);
    assert_eq!(h.app.focus, Focus::Results);
    assert!(h.app.overlays.top().is_none(), "no dialog");
    // The last change undone there: the rows again.
    h.keys("u");
    assert!(edits(&h).batch.is_empty());
    assert_eq!(view(&h), ResultView::Rows);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert!(!strip(&mut h, 120).contains("Changes"));
    // Nothing pending: said, as before.
    h.keys(" dp");
    assert_eq!(said(&mut h), Some(Label::EditNothingPending.into()));
    assert_eq!(view(&h), ResultView::Rows);
}

#[test]
fn h_and_l_cycle_through_it_and_the_strip_keeps_it_when_narrow() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.keys("jD");
    h.keys("L");
    assert_eq!(view(&h), ResultView::Changes, "next to the result");
    h.keys("L");
    assert_eq!(view(&h), ResultView::Chart);
    h.keys("H");
    assert_eq!(view(&h), ResultView::Changes);
    h.keys(" r[");
    assert_eq!(view(&h), ResultView::Rows);
    h.keys(" r]");
    assert_eq!(view(&h), ResultView::Changes);
    let whole = strip(&mut h, 160);
    assert!(whole.contains("Result 1  [Changes 2]  Chart  Messages"), "{whole}");
    // Narrower (a wider explorer): the short labels.
    let mut short = None;
    for _ in 0..40 {
        h.keys(" >");
        let line = strip(&mut h, 80);
        if !line.contains("Changes 2") {
            short = Some(line);
            break;
        }
    }
    let short = short.expect("short labels");
    assert!(short.contains("1  [Chg2]  Chart  Msgs"), "{short}");
    // The rows with the inspector open: the shown one, the Changes and Messages stay.
    h.keys("H");
    h.keys("i");
    let narrow = strip(&mut h, 80);
    assert!(narrow.contains("[1]") && narrow.contains("Chg2") && narrow.contains("Msgs"), "{narrow}");
}

#[test]
fn the_list_has_a_line_for_each_changed_cell_and_names_its_row() {
    let mut h = four_changes(ready(Lang::En));
    h.command("changes");
    let body = pane(&mut h, 120, 40);
    assert!(body[0].starts_with(" Changes — Result 1 · shop.users"), "{body:#?}");
    assert!(body[0].ends_with("2 updates · 1 insert · 1 delete"), "{body:#?}");
    let rows = |mark: &str| body.iter().filter(|l| l.starts_with(mark)).count();
    assert_eq!((rows(" - "), rows(" ~ "), rows(" + ")), (1, 2, 1), "{body:#?}");
    let has = |text: &str| assert!(body.iter().any(|l| l.contains(text)), "{text}: {body:#?}");
    has(" - id=2   delete");
    has(" ~ id=1   email     'a@x' → 'z@x'");
    has(" ~ id=3   nickname  'cy' → NULL");
    // A new row: the values set, the columns left to their default counted.
    has(" + new 1  email='n@x' · DEFAULT ×2");
    // The keys under it: the tab's own alone where the whole do not fit.
    has("Runs in one transaction; commits at the end.");
    has("s c Enter");
    let wide = pane(&mut h, 200, 40).join("\n");
    assert!(wide.contains("s SQL · c copy SQL · Enter go to · Space d a apply"), "{wide}");
}

#[test]
fn rows_found_by_their_values_are_named_by_their_row_in_the_grid() {
    let mut h = by_values(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("changes");
    let lines = pane(&mut h, 120, 40);
    assert!(lines.iter().any(|l| l.contains("~ row 1  email  'a@x' → 'z@x'  (located by its values)")), "{lines:#?}");
}

#[test]
fn enter_shows_the_line_s_row_and_column_in_the_grid() {
    let mut h = four_changes(ready(Lang::En));
    h.command("changes");
    // Deletes first, then the updates: the NULL of row 3's nickname is the third line.
    h.keys("jj");
    h.key(KeyCode::Enter);
    assert_eq!(view(&h), ResultView::Rows);
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (2, 2));
    // A whole row keeps the column.
    h.command("changes");
    h.keys("gg");
    h.key(KeyCode::Enter);
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (1, 2));
    // The new row, the last line.
    h.command("changes");
    h.keys("G");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().grid.row, 3);
    // A problem points at its row too: the new row lacks a column the table requires.
    let mut h = resolved(Harness::connected(Lang::En), users_structure());
    h.keys("o");
    h.command("changes");
    let lines = pane(&mut h, 120, 40);
    assert!(lines[0].ends_with("1 insert · 2 problems"), "{lines:#?}");
    assert_eq!(lines[1], " + new 1  DEFAULT ×3", "{lines:#?}");
    assert_eq!(lines[2], "   ! email is NOT NULL and has no default; give it a value", "{lines:#?}");
    assert!(lines[3].starts_with("   ! name is NOT NULL, has no default"), "{lines:#?}");
    h.keys("gg0");
    h.command("changes");
    h.keys("j");
    h.key(KeyCode::Enter);
    assert_eq!((view(&h), h.app.tab().grid.row, h.app.tab().grid.col), (ResultView::Rows, 3, 1));
}

/// A problem is listed under its row's line, without the row's name, which the line above
/// says; elsewhere (the statements, the Apply) it names the row as the list does. Every line
/// is selected and shown in the grid as it is listed.
#[test]
fn a_problem_is_listed_under_its_row_and_named_as_the_list_names_it() {
    let mut h = resolved(Harness::connected(Lang::En), users_structure());
    type_cell(&mut h, "z@x");
    h.keys("o");
    h.command("changes");
    let lines = pane(&mut h, 120, 40);
    assert!(lines[0].ends_with("1 update · 1 insert · 2 problems"), "{lines:#?}");
    assert_eq!(
        lines[1..5],
        [
            " ~ id=1   email  'a@x' → 'z@x'",
            " + new 1  DEFAULT ×3",
            "   ! email is NOT NULL and has no default; give it a value",
            "   ! name is NOT NULL, has no default and is not in the result; add it to the SELECT",
        ],
        "{lines:#?}"
    );
    // Narrow: a problem wraps under itself.
    let narrow = pane(&mut h, 80, 24);
    let at = narrow.iter().position(|l| l.starts_with("   ! name is NOT NULL")).expect("listed");
    assert!(!narrow[at].ends_with("SELECT"), "{narrow:#?}");
    assert!(narrow[at + 1].starts_with("     ") && narrow[at + 1].ends_with("the SELECT"), "{narrow:#?}");
    // Each line is the line under the cursor: the update, the new row, its two problems.
    let goto = |h: &mut Harness, down: &str| {
        h.command("changes");
        h.keys("gg");
        h.keys(down);
        h.key(KeyCode::Enter);
        (view(h), h.app.tab().grid.row, h.app.tab().grid.col)
    };
    assert_eq!(goto(&mut h, ""), (ResultView::Rows, 0, 1));
    h.keys("l");
    assert_eq!(goto(&mut h, "j"), (ResultView::Rows, 3, 2), "a whole row keeps the column");
    assert_eq!(goto(&mut h, "jj"), (ResultView::Rows, 3, 1), "the email problem: its cell");
    assert_eq!(goto(&mut h, "jjj"), (ResultView::Rows, 3, 1), "the row's problem: its row");
    h.command("changes");
    h.keys("G");
    assert_eq!(edits(&h).cursor.line, 3, "four lines");
    // The statements name the row as the list does.
    h.keys("s");
    let text = pane(&mut h, 160, 40).join("\n");
    assert!(text.contains("new 1 · email is NOT NULL and has no default; give it a value"), "{text}");
    assert!(!text.contains("Row 4"), "{text}");
    h.keys("s");
    // An Apply: the first problem selected, the reason named the same way.
    h.keys("H");
    h.keys(" da");
    assert_eq!(said(&mut h), Some(Msg::EditAskProblems { count: 2 }));
    assert_eq!((view(&h), edits(&h).cursor.line), (ResultView::Changes, 2), "the first problem");
}

/// Moving up into a row's problems keeps the row's own line on screen when it fits.
#[test]
fn a_row_s_line_stays_on_screen_over_its_selected_problem() {
    let mut h = resolved(Harness::connected(Lang::En), users_structure());
    for _ in 0..5 {
        h.keys("o");
    }
    h.command("changes");
    // Five new rows, each with two problems: fifteen lines, more than the pane shows.
    h.keys("G");
    pane(&mut h, 120, 24);
    h.keys(&"k".repeat(12));
    assert_eq!(edits(&h).cursor.line, 2, "new 1's second problem");
    let lines = pane(&mut h, 120, 24);
    assert!(!lines.iter().any(|l| l.starts_with(" + new 5")), "more than the pane shows: {lines:#?}");
    assert!(lines.iter().any(|l| l.starts_with(" + new 1")), "{lines:#?}");
}

/// Control characters of values are shown escaped in the list (a backslash doubled where it
/// would read as an escape); the statements copied carry them as they are.
#[test]
fn control_characters_are_shown_escaped_and_copied_as_they_are() {
    let mut rows = users_rows();
    rows[2][2] = s("c\u{7}y\r\n\tz \\n \\q");
    let mut h = resolved_rows(Harness::connected(Lang::En), named(), rows);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("jjl");
    h.keys(" dn");
    h.command("changes");
    let lines = pane(&mut h, 120, 40);
    assert_eq!(lines[1], r" ~ id=3  nickname  'c\x07y\r\n\tz \\n \q' → NULL", "{lines:#?}");
    h.keys("c");
    let copied = clip.last().expect("copied");
    // The control characters as they are, in a literal that escapes its backslashes.
    let value = "E'c\u{7}y\r\n\tz \\\\n \\\\q'";
    assert_eq!(copied.matches(value).count(), 2, "the check and the update find it: {copied:?}");
    assert!(!copied.contains("\\x07") && !copied.contains('\u{21B5}'), "{copied:?}");
}

#[test]
fn an_edit_key_in_the_tab_edits_the_selected_line_s_cell_in_the_grid() {
    let mut h = four_changes(ready(Lang::En));
    h.command("changes");
    h.keys("j");
    h.keys(" de");
    assert_eq!(view(&h), ResultView::Rows);
    assert_eq!(h.app.key_context(), Ctx::CellEdit, "the cell editor at id=1's email");
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (0, 1));
}

#[test]
fn a_click_in_the_tab_leaves_the_grid_behind_it_alone() {
    let mut h = four_changes(ready(Lang::En));
    h.command("changes");
    h.draw(120, 40);
    let r = h.app.layout.results;
    let before = (h.app.tab().grid.row, h.app.tab().grid.col);
    for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
        h.mouse(kind, r.x + 10, r.y + 4);
    }
    assert_eq!(view(&h), ResultView::Changes);
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), before);
    assert!(h.app.overlays.top().is_none(), "no cell viewer");
    assert!(h.app.tab().grid.anchor.is_none(), "no selection");
}

#[test]
fn s_shows_the_statements_and_the_setting_picks_the_first_view() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("changes");
    h.keys("s");
    assert!(h.app.changes_sql());
    let text = pane(&mut h, 160, 40).join("\n");
    assert!(text.contains("Row checks (lock, no waiting)") && text.contains("Writes"), "{text}");
    assert!(text.contains(r#"SELECT 1 FROM "shop"."users""#) && text.contains("expect 1"), "{text}");
    assert!(text.contains("s list · c copy SQL"), "{text}");
    h.keys("s");
    assert!(!h.app.changes_sql());
    // The setting: saved, and the view of this run gives way to it.
    h.keys("s");
    h.command("set ui.changes_view=list");
    assert_eq!(h.app.prefs.changes_view, ChangesView::List);
    assert!(!h.app.changes_sql());
    h.command("set ui.changes_view=sql");
    assert_eq!(h.app.prefs.changes_view, ChangesView::Sql);
    h.command("changes");
    assert!(h.app.changes_sql());
    assert!(pane(&mut h, 160, 40).join("\n").contains("Writes"));
}

#[test]
fn c_copies_the_exact_statements_once_they_are_final() {
    let mut h = ready(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    type_cell(&mut h, "z@x");
    h.command("changes");
    h.keys("c");
    assert_eq!(clip.last(), Some(format!("{CHECK_1};\n{UPDATE_1};\n")));
    assert!(matches!(said(&mut h), Some(Msg::EditPreviewCopied { count: 2, .. })));
    assert_eq!(h.app.key_context(), Ctx::Changes);
    // Rows still to be counted: not final.
    let mut h = by_values(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    type_cell(&mut h, "z@x");
    h.command("changes");
    h.keys("c");
    assert_eq!(said(&mut h), Some(Label::ChangesCopyCounting.into()));
    h.advance(COUNT_SETTLE);
    let asked = counts(&mut h);
    assert_eq!(asked.len(), 1);
    h.keys("c");
    assert_eq!(said(&mut h), Some(Label::ChangesCopyCounting.into()), "asked, not answered");
    assert_eq!(clip.last(), None);
    h.db(DbEvent::Counted { id: asked[0].0, result: Ok(1), snapshot: false });
    h.keys("c");
    let copied = clip.last().unwrap();
    assert!(
        copied.starts_with("SELECT 1 FROM ") && copied.contains("\nUPDATE ") && copied.ends_with(";\n"),
        "{copied}"
    );
    // Problems: not final either.
    let mut h = resolved(Harness::connected(Lang::En), users_structure());
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("o");
    h.command("changes");
    h.keys("c");
    assert_eq!(said(&mut h), Some(Msg::ChangesCopyNotFinal { count: 2 }));
    assert_eq!(clip.last(), None);
}

#[test]
fn the_counts_wait_for_the_changes_to_settle_and_are_for_their_revision() {
    let mut h = by_values(Lang::En);
    type_cell(&mut h, "z@x");
    h.keys("jl");
    type_cell(&mut h, "bob");
    // Not while the tab is not shown.
    h.advance(Duration::from_secs(2));
    assert!(counts(&mut h).is_empty(), "the grid counts nothing");
    h.command("changes");
    h.app.transient = None;
    // Shown long after the last change: counted at once.
    h.advance(Duration::from_millis(1));
    let asked = counts(&mut h);
    assert_eq!(asked.len(), 2, "one per row found by its values");
    let text = pane(&mut h, 120, 40).join("\n");
    assert!(!text.contains("problem"), "rows being counted are no problem: {text}");
    // A change before the answers: they are for other changes, dropped.
    h.keys("u");
    assert_eq!(view(&h), ResultView::Changes, "one change is left");
    for (id, _) in &asked {
        h.db(DbEvent::Counted { id: *id, result: Ok(1), snapshot: false });
    }
    assert!(edits(&h).checks.matches().is_empty(), "stale answers dropped");
    h.keys("s");
    assert!(pane(&mut h, 120, 40).join("\n").contains("Counting matching rows…"));
    // Counted again only once the changes settle, once.
    let now = h.clock.now();
    h.app.transient = None;
    assert_eq!(h.app.next_tick(now), Some(now + COUNT_SETTLE), "one wakeup, when they settle");
    h.advance(COUNT_SETTLE - Duration::from_millis(1));
    assert!(counts(&mut h).is_empty());
    h.advance(Duration::from_millis(1));
    let again = counts(&mut h);
    assert_eq!(again.len(), 1);
    h.advance(Duration::from_secs(5));
    assert!(counts(&mut h).is_empty(), "once for these changes");
    assert_eq!(h.app.next_tick(h.clock.now()), None, "nothing waits");
    h.db(DbEvent::Counted { id: again[0].0, result: Ok(1), snapshot: false });
    let text = pane(&mut h, 160, 40).join("\n");
    assert!(text.contains("UPDATE \"shop\".\"users\"") && !text.contains("Counting"), "{text}");
}

/// `h` ran the users' query twice in one run: two result tabs, the second shown, both resolved.
fn two_results(mut h: Harness) -> Harness {
    h.db(DbEvent::Keys(Ok(shop_keys())));
    h.app.tab_mut().editor = Editor::new(&format!("{SQL};\n{SQL};"));
    h.sent();
    h.app.focus = Focus::Editor;
    h.keys("ggVG");
    h.ctrl('e');
    let sent = h.sent();
    let Some(DbCommand::Execute { id, .. }) = sent.into_iter().find(|c| matches!(c, DbCommand::Execute { .. })) else {
        panic!("a run")
    };
    h.db(DbEvent::StepRows { id, index: 0, columns: Some(users_columns()), rows: users_rows(), more: false });
    h.db(DbEvent::Page {
        id,
        columns: Some(users_columns()),
        rows: users_rows(),
        more: false,
        elapsed: Duration::ZERO,
    });
    h.app.focus = Focus::Results;
    h
}

#[test]
fn another_result_with_pending_changes_is_named() {
    let mut h = two_results(Harness::connected(Lang::En));
    h.keys("le");
    structure(&mut h, named());
    h.key(KeyCode::Esc);
    type_cell(&mut h, "z@x");
    h.keys("H");
    h.keys("le");
    structure(&mut h, named());
    h.key(KeyCode::Esc);
    h.keys("jD");
    h.command("changes");
    let lines = pane(&mut h, 120, 40);
    assert!(lines[0].contains("Changes — Result 1 · shop.users"), "{lines:#?}");
    assert_eq!(lines[1].trim(), "Result 2 also has 1 pending", "no key that would not go there: {lines:#?}");
}

/// The screens of `setup` (built in each language) at 120x40 and 80x24, as
/// `<name>_<lang>_<size>`: English ones pinned, Korean ones checked to show their catalog's
/// texts.
fn screens(name: &str, setup: impl Fn(Lang) -> Harness) {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = setup(lang);
        let tag = format!("{lang:?}").to_lowercase();
        for (w, hh) in [(120, 40), (80, 24)] {
            assert_screen!(format!("{name}_{tag}_{w}x{hh}"), lang, h.draw(w, hh));
        }
    }
}

#[test]
fn the_changes_tab_on_screen() {
    screens("changes_list", |lang| {
        let mut h = four_changes(ready(lang));
        h.command("changes");
        h.keys("j");
        h
    });
    screens("changes_sql", |lang| {
        let mut h = four_changes(ready(lang));
        h.command("changes");
        h.keys("s");
        h
    });
    screens("changes_problems", |lang| {
        let mut h = resolved(Harness::connected(lang), users_structure());
        type_cell(&mut h, "z@x");
        h.keys("o");
        h.command("changes");
        h.keys("jj");
        h
    });
    screens("changes_by_values", |lang| {
        let mut h = by_values(lang);
        type_cell(&mut h, "z@x");
        h.command("changes");
        h.keys("s");
        h
    });
}

#[test]
fn the_short_label_is_short_in_every_language() {
    for lang in [Lang::En, Lang::Ko] {
        let i18n = datarig_core::i18n::I18n::new(lang);
        for (count, most) in [(3, 6), (99, 6), (999, 7)] {
            let short = i18n.msg(&Msg::ResultsTabChangesShort { count }).to_string();
            assert!(datarig_tui::text::width(&short) <= most, "{short}");
        }
    }
}

/// `Enter` on a line whose row read is on another page: that row's page, the row on screen.
#[test]
fn enter_shows_a_row_on_another_page_on_its_own_page() {
    let mut h = Harness::connected(Lang::En);
    h.app.page_size = 2;
    let mut h = resolved(h, named());
    type_cell(&mut h, "z@x");
    h.keys("n");
    assert_eq!(h.app.tab().grid.page, 1, "the last page shown");
    h.command("changes");
    h.key(KeyCode::Enter);
    h.draw(120, 40);
    let g = &h.app.tab().grid;
    assert_eq!((view(&h), g.page, g.row, g.col, g.top), (ResultView::Rows, 0, 0, 1, 0));
}

/// An edit key in the statements (no line selected) shows the grid and does nothing else: no
/// action on the grid's cursor or a selection left there before.
#[test]
fn an_edit_key_in_the_statements_only_shows_the_grid() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.keys("jVj");
    assert!(h.app.tab().grid.anchor.is_some(), "rows 2 and 3 selected");
    h.command("changes");
    h.keys("s");
    h.keys(" dd");
    assert_eq!(view(&h), ResultView::Rows);
    assert_eq!(edits(&h).batch.counts().total(), 1, "nothing marked for deletion");
    assert_eq!(h.app.key_context(), Ctx::Grid);
}

/// The tab shown with the cell editor open: the editor closes, no editor stays behind it.
#[test]
fn no_cell_editor_stays_open_behind_the_tab() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.keys("e");
    assert!(h.app.cell_edit_open());
    h.app.tab_mut().exec.view = ResultView::Changes;
    h.keys("j");
    assert!(!h.app.cell_edit_open(), "closed");
}

/// While the changes are being applied an edit key in the tab is refused there, the tab kept.
#[test]
fn an_edit_key_while_applying_is_refused_in_the_tab() {
    let mut h = ready(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("apply");
    h.key(KeyCode::Char('y'));
    assert!(edits(&h).applying.is_some(), "applying");
    h.key(KeyCode::Esc);
    h.command("changes");
    assert_eq!(view(&h), ResultView::Changes);
    h.app.transient = None;
    h.keys(" de");
    assert_eq!(view(&h), ResultView::Changes, "the tab kept");
    assert!(matches!(said(&mut h), Some(Msg::EditRefused { .. })));
}

/// Nothing is counted for a Changes tab while another tab is active.
#[test]
fn no_counts_while_another_tab_is_active() {
    let mut h = by_values(Lang::En);
    type_cell(&mut h, "z@x");
    h.command("changes");
    h.command("tabnew");
    assert_eq!(h.app.tabs.len(), 2);
    h.advance(Duration::from_secs(2));
    assert!(counts(&mut h).is_empty(), "the other tab counts nothing");
}
