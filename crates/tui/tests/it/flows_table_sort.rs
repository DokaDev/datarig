//! Sorting a table tab by a column: `s` sorts by the grid's column alone (going up, going
//! down, no order), `S` adds it as one more key (or turns it where it is). The new order is
//! written into the filter line and runs on the server as any run of the tab does: a new run,
//! asked about first over pending edits. A click in a header's sort zone (its last two
//! columns, marked on the cursor's column) sorts as `s` does; a click elsewhere on a header
//! still selects the column. The headers mark the sort keys and the columns the condition
//! compares, as the rows shown were read. Query results have no such keys.

use crate::common;

use common::*;
use datarig_core::config::IconsSetting;
use datarig_core::driver::DbCommand;
use datarig_core::driver::DbEvent;
use datarig_i18n::Lang;
use datarig_tui::app::Results;
use datarig_tui::app::overlay::ConfirmAction;
use datarig_tui::app::table_query::Field;
use datarig_tui::keymap::Ctx;
use datarig_tui::widgets::grid::{HeadMarks, Shape};
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::time::Duration;

const ALL: &str = r#"SELECT * FROM "shop"."users""#;

fn lang_tag(l: Lang) -> &'static str {
    if l == Lang::En { "en" } else { "ko" }
}

/// Open `shop.users` from the explorer (a table tab, which runs its query).
fn open_users(h: &mut Harness) {
    h.key(KeyCode::BackTab);
    h.goto("shop");
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["users".into()], Vec::new()).into()) });
    h.goto("users");
    h.key(KeyCode::Enter);
    assert!(h.app.tab().is_table());
}

/// The statements of the runs sent since the last look.
fn runs(h: &mut Harness) -> Vec<Vec<String>> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { statements, .. } => Some(statements),
            _ => None,
        })
        .collect()
}

/// Answer the active tab's current run with a first page of three rows.
fn answer(h: &mut Harness) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = Some(vec![
        meta("id", "int8", true, false),
        meta("name", "text", false, false),
        meta("email", "text", false, false),
    ]);
    let rows = (0..3).map(|i| vec![Some(i.to_string()), Some(format!("n{i}")), Some(format!("u{i}@x"))]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: false, elapsed: Duration::from_millis(2) });
}

/// `shop.users` open and answered, the grid focused on its first column, nothing sent since.
fn users(lang: Lang) -> Harness {
    let mut h = Harness::connected(lang);
    open_users(&mut h);
    answer(&mut h);
    h.sent();
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h
}

/// The one run sent since the last look, its statement.
fn one_run(h: &mut Harness) -> String {
    let runs = runs(h);
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0].len(), 1, "{runs:?}");
    runs[0][0].clone()
}

fn field_text(h: &Harness, f: Field) -> String {
    h.app.tab().doc.table_query.as_ref().expect("a table query").line.field(f).text().to_string()
}

fn heads(h: &Harness) -> Vec<HeadMarks> {
    h.app.table_head_marks(h.app.tab().id)
}

fn sorted(asc: bool, level: usize) -> HeadMarks {
    HeadMarks { sort: Some((asc, level)), filtered: false }
}

#[test]
fn s_sorts_by_the_column_up_then_down_then_not_at_all() {
    let mut h = users(Lang::En);
    h.keys("l");
    h.keys("s");
    assert_eq!(one_run(&mut h), format!(r#"{ALL} ORDER BY "name" ASC"#));
    answer(&mut h);
    assert_eq!(field_text(&h, Field::Order), "name ASC");
    assert_eq!(heads(&h), vec![HeadMarks::default(), sorted(true, 1), HeadMarks::default()]);
    h.keys("s");
    assert_eq!(one_run(&mut h), format!(r#"{ALL} ORDER BY "name" DESC"#));
    answer(&mut h);
    assert_eq!(field_text(&h, Field::Order), "name DESC");
    assert_eq!(heads(&h)[1], sorted(false, 1));
    h.keys("s");
    assert_eq!(one_run(&mut h), ALL);
    answer(&mut h);
    assert_eq!(field_text(&h, Field::Order), "");
    assert_eq!(heads(&h), vec![HeadMarks::default(); 3]);
    // Another column replaces the keys there are.
    h.keys("s");
    answer(&mut h);
    h.sent();
    h.keys("l");
    h.keys(" rs");
    assert_eq!(one_run(&mut h), format!(r#"{ALL} ORDER BY "email" ASC"#));
    // Ctrl+E runs the sorted query again (it is the draft now).
    answer(&mut h);
    h.ctrl('e');
    assert_eq!(one_run(&mut h), format!(r#"{ALL} ORDER BY "email" ASC"#));
}

#[test]
fn shift_s_adds_the_column_as_the_next_key() {
    let mut h = users(Lang::En);
    h.keys("s");
    assert_eq!(one_run(&mut h), format!(r#"{ALL} ORDER BY "id" ASC"#));
    answer(&mut h);
    h.keys("lS");
    assert_eq!(one_run(&mut h), format!(r#"{ALL} ORDER BY "id" ASC, "name" ASC"#));
    answer(&mut h);
    assert_eq!(heads(&h), vec![sorted(true, 1), sorted(true, 2), HeadMarks::default()]);
    h.keys("lS");
    assert_eq!(one_run(&mut h), format!(r#"{ALL} ORDER BY "id" ASC, "name" ASC, "email" ASC"#));
    answer(&mut h);
    // A key in the middle turns where it is, then goes; the others keep their places.
    h.keys("hS");
    assert_eq!(one_run(&mut h), format!(r#"{ALL} ORDER BY "id" ASC, "name" DESC, "email" ASC"#));
    answer(&mut h);
    assert_eq!(heads(&h), vec![sorted(true, 1), sorted(false, 2), sorted(true, 3)]);
    h.keys(" rS");
    assert_eq!(one_run(&mut h), format!(r#"{ALL} ORDER BY "id" ASC, "email" ASC"#));
    answer(&mut h);
    assert_eq!(field_text(&h, Field::Order), "id ASC, email ASC");
    assert_eq!(heads(&h), vec![sorted(true, 1), HeadMarks::default(), sorted(true, 2)]);
    // `s` on a column among several: it alone, from its own direction.
    h.keys("ls");
    assert_eq!(one_run(&mut h), format!(r#"{ALL} ORDER BY "email" DESC"#));
}

/// With the table's key read, the order is made total: the key is appended (shown in the line,
/// not marked in the headers), and a sort on the key column makes it the user's own key.
#[test]
fn the_tie_breaker_keeps_working_and_is_not_marked() {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    keys_came(&mut h, "local-pg", &["id"]);
    answer(&mut h);
    h.sent();
    h.key(KeyCode::Tab);
    h.keys("ls");
    assert_eq!(one_run(&mut h), format!(r#"{ALL} ORDER BY "name" ASC, "id" ASC"#));
    answer(&mut h);
    assert_eq!(heads(&h), vec![HeadMarks::default(), sorted(true, 1), HeadMarks::default()]);
    assert!(h.screen(200, 45).contains("ORDER BY name ASC + id"), "{}", h.screen(200, 45));
    h.keys("hS");
    assert_eq!(one_run(&mut h), format!(r#"{ALL} ORDER BY "name" ASC, "id" ASC"#));
    answer(&mut h);
    assert_eq!(heads(&h), vec![sorted(true, 2), sorted(true, 1), HeadMarks::default()]);
    h.keys("S");
    assert_eq!(one_run(&mut h), format!(r#"{ALL} ORDER BY "name" ASC, "id" DESC"#));
}

/// A sort keeps the condition and the limit of the rows shown. What is typed in the line and
/// not applied: a condition stays typed (not run), an order gives way to the sort's.
#[test]
fn a_sort_keeps_the_shown_condition_and_limit_and_not_the_typed_ones() {
    let mut h = users(Lang::En);
    h.keys("f");
    h.type_text("id > 1");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.type_text("50");
    h.key(KeyCode::Enter);
    assert_eq!(one_run(&mut h), format!("{ALL} WHERE (id > 1)"));
    answer(&mut h);
    // Typed, not applied, and the grid clicked: the fields keep what was typed.
    h.keys("f");
    h.ctrl('l');
    h.type_text("id > 2");
    h.key(KeyCode::Tab);
    h.type_text("email desc");
    h.draw(120, 30);
    let grid = h.app.tab().grid.clone();
    let (x, y) = (grid.hit_cols[0].0 + 1, grid.data_y);
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h.keys("ls");
    assert_eq!(one_run(&mut h), format!(r#"{ALL} WHERE (id > 1) ORDER BY "name" ASC"#));
    let q = h.app.tab().doc.table_query.as_ref().unwrap();
    assert_eq!(q.applied().limit, Some(50));
    answer(&mut h);
    assert_eq!(field_text(&h, Field::Where), "id > 2", "the typed condition stays typed");
    assert_eq!(field_text(&h, Field::Order), "name ASC", "the sort's order replaced the typed one");
    assert_eq!(field_text(&h, Field::Limit), "50");
}

#[test]
fn a_click_in_the_sort_zone_sorts_and_elsewhere_selects_the_column() {
    let mut h = users(Lang::En);
    h.draw(120, 30);
    let g = h.app.tab().grid.clone();
    assert_eq!(g.sort_zones.len(), 3, "{:?}", g.sort_zones);
    // A plain header click selects the column: nothing runs.
    let (a, _, c) = g.hit_cols[1];
    h.mouse(MouseEventKind::Down(MouseButton::Left), a + 2, g.header_y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), a + 2, g.header_y);
    assert!(runs(&mut h).is_empty());
    let g2 = h.app.tab().grid.clone();
    assert_eq!((g2.col, g2.shape, g2.anchor.is_some()), (c, Shape::Cols, true));
    // The type row under the zone selects the column too.
    let (za, _, zc) = g.sort_zones.iter().copied().find(|z| z.2 == 2).unwrap();
    h.mouse(MouseEventKind::Down(MouseButton::Left), za, g.header_y + 1);
    h.mouse(MouseEventKind::Up(MouseButton::Left), za, g.header_y + 1);
    assert!(runs(&mut h).is_empty());
    // The zone of another column (not marked there) sorts by that column, which becomes the
    // cursor's; the range selected goes.
    h.draw(120, 30);
    h.mouse(MouseEventKind::Down(MouseButton::Left), za + 1, g.header_y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), za + 1, g.header_y);
    assert_eq!(one_run(&mut h), format!(r#"{ALL} ORDER BY "email" ASC"#));
    assert_eq!((h.app.tab().grid.col, h.app.tab().grid.anchor), (zc, None));
    answer(&mut h);
    // Its marks widen the column: the zone moves with its end.
    h.draw(120, 30);
    let (za, ..) = h.app.tab().grid.sort_zones.iter().copied().find(|z| z.2 == 2).unwrap();
    h.mouse(MouseEventKind::Down(MouseButton::Left), za, g.header_y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), za, g.header_y);
    assert_eq!(one_run(&mut h), format!(r#"{ALL} ORDER BY "email" DESC"#));
}

#[test]
fn a_sort_over_pending_edits_asks_first_and_enter_keeps_them() {
    let mut h = users(Lang::En);
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        assert!(rs.edit.batch.delete(&[0], |_| Vec::new()));
    }
    let draft = h.app.tab().doc.table_query.as_ref().unwrap().draft.clone();
    h.keys("ls");
    assert_eq!(
        h.app.overlays.confirm().map(|c| c.action),
        Some(ConfirmAction::LeaveEdits(datarig_tui::app::edit::Leave::Run(h.app.tab().id)))
    );
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty(), "Enter keeps the edits: nothing runs");
    assert!(matches!(&h.app.tab().results, Results::Rows(rs) if !rs.edit.batch.is_empty()));
    // Nothing of the sort stays: the line and the draft are as they were.
    assert_eq!(field_text(&h, Field::Order), "");
    assert_eq!(h.app.tab().doc.table_query.as_ref().unwrap().draft, draft);
    assert!(heads(&h).iter().all(|m| m.sort.is_none()));
    // `y` drops them: a new run, never a page read again.
    h.keys("s");
    h.keys("y");
    let sent = h.sent();
    let want = vec![format!(r#"{ALL} ORDER BY "name" ASC"#)];
    assert!(sent.iter().any(|c| matches!(c, DbCommand::Execute { statements, .. } if *statements == want)), "{sent:?}");
    assert!(!sent.iter().any(|c| matches!(c, DbCommand::FetchMore { .. } | DbCommand::Resume { .. })));
}

#[test]
fn sorting_is_only_for_table_tabs() {
    let mut h = with_edge_results(Lang::En);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    for keys in ["s", "S", " rs", " rS"] {
        h.keys(keys);
        let status = h.status(120, 30);
        assert!(status.contains("Only a table tab sorts by a column"), "{keys:?}: {status}");
    }
    assert!(runs(&mut h).is_empty());
    // No sort zone on a query's headers.
    h.draw(120, 30);
    assert!(h.app.tab().grid.sort_zones.is_empty());
    assert!(heads(&h).is_empty());
}

/// The filter mark is on the columns the condition compares as the app read it; a condition
/// it did not read marks none (not a claim that it compares none).
#[test]
fn the_filter_mark_follows_the_condition_read() {
    let mut h = users(Lang::En);
    h.command("where name = 'n1' and id > 0");
    answer(&mut h);
    let marks: Vec<bool> = heads(&h).iter().map(|m| m.filtered).collect();
    assert_eq!(marks, vec![true, true, false]);
    h.command("where lower(email) like '%x%'");
    answer(&mut h);
    assert!(heads(&h).iter().all(|m| !m.filtered), "{:?}", heads(&h));
    // A run on its way does not change them: they are of the rows shown.
    h.command("where email = 'a'");
    assert!(heads(&h).iter().all(|m| !m.filtered));
}

#[test]
fn the_sort_screens() {
    for lang in [Lang::En, Lang::Ko] {
        let tag = lang_tag(lang);
        let mut h = users(lang);
        h.keys("i");
        h.command("where name <> 'zz'");
        answer(&mut h);
        h.keys("S");
        answer(&mut h);
        h.keys("lS");
        answer(&mut h);
        h.keys("S");
        answer(&mut h);
        assert_eq!(heads(&h)[0].sort, Some((true, 1)));
        assert_eq!(heads(&h)[1].sort, Some((false, 2)));
        // Icons off: `^`, `v2`, `=`, and `^v` in the cursor's column's sort zone.
        assert_screen!(format!("table_sort_text_{tag}_80x24"), lang, h.draw(80, 24));
        assert_screen!(format!("table_sort_text_{tag}_120x30"), lang, h.draw(120, 30));
        h.app.icons = IconsSetting::On;
        assert_screen!(format!("table_sort_icons_{tag}_80x24"), lang, h.draw(80, 24));
        assert_screen!(format!("table_sort_icons_{tag}_120x30"), lang, h.draw(120, 30));
        h.app.icons = IconsSetting::Off;
    }
}
