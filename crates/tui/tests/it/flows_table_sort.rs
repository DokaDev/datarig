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

/// The three fields of the line, as they read.
fn fields(h: &Harness) -> [String; 3] {
    [field_text(h, Field::Where), field_text(h, Field::Order), field_text(h, Field::Limit)]
}

fn draft(h: &Harness) -> datarig_core::query::Query {
    h.app.tab().doc.table_query.as_ref().expect("a table query").draft.clone()
}

/// Field `f` of the line holds `text`, typed and not applied; the grid has the keys.
fn typed(h: &mut Harness, f: Field, text: &str) {
    let line = &mut h.app.tab_mut().doc.table_query.as_mut().expect("a table query").line;
    match f {
        Field::Where => line.where_.set(text),
        Field::Order => line.order.set(text),
        Field::Limit => line.limit.set(text),
    }
    line.focus = None;
}

/// The sort zone of column `col`'s header, as last drawn at 120x30.
fn zone(h: &mut Harness, col: usize) -> (u16, u16) {
    h.draw(120, 30);
    let g = h.app.tab().grid.clone();
    let (x, ..) = g.sort_zones.iter().copied().find(|z| z.2 == col).expect("a sort zone");
    (x, g.header_y)
}

/// A sort runs the condition and the limit the rows shown were read with. While the line holds
/// a change not applied, in any field, a sort (`s`, `S`, a click in a sort zone) is refused as
/// a filter by value is, said, and nothing changes: what was typed stays, the draft is as it
/// was, nothing is sent.
#[test]
fn a_sort_over_changes_not_applied_is_refused_and_keeps_them() {
    let mut h = users(Lang::En);
    h.keys("f");
    h.type_text("id > 1");
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.type_text("50");
    h.key(KeyCode::Enter);
    assert_eq!(one_run(&mut h), format!("{ALL} WHERE (id > 1)"));
    answer(&mut h);
    h.app.tab_mut().doc.table_query.as_mut().unwrap().line.focus = None;
    // Nothing typed: the condition and the limit applied run with the order.
    h.keys("ls");
    assert_eq!(one_run(&mut h), format!(r#"{ALL} WHERE (id > 1) ORDER BY "name" ASC"#));
    assert_eq!(h.app.tab().doc.table_query.as_ref().unwrap().applied().limit, Some(50));
    answer(&mut h);
    assert_eq!(fields(&h), ["id > 1", "name ASC", "50"]);
    for (f, text) in [(Field::Where, "id > 2"), (Field::Order, "email desc"), (Field::Limit, "7")] {
        typed(&mut h, f, text);
        let (before, d) = (fields(&h), draft(&h));
        for how in ["s", "S", "click"] {
            match how {
                "click" => {
                    let (x, y) = zone(&mut h, 2);
                    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
                    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
                }
                keys => h.keys(keys),
            }
            assert!(runs(&mut h).is_empty(), "{f:?} {how}: nothing sent");
            let status = h.status(120, 30);
            assert!(status.contains("Not sorted: filter line edited"), "{f:?} {how}: {status}");
            assert_eq!(fields(&h), before, "{f:?} {how}: the typed text stays");
            assert_eq!(draft(&h), d, "{f:?} {how}: the draft is as it was");
            assert_eq!(heads(&h)[1], sorted(true, 1), "{f:?} {how}");
        }
        // Put back: sorting works again.
        let back = match f {
            Field::Where => "id > 1",
            Field::Order => "name ASC",
            Field::Limit => "50",
        };
        typed(&mut h, f, back);
    }
    // The clicks put the cursor on `email`.
    assert_eq!(h.app.tab().grid.col, 2);
    h.keys("s");
    assert_eq!(one_run(&mut h), format!(r#"{ALL} WHERE (id > 1) ORDER BY "email" ASC"#));
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

/// Over pending edits a sort asks first, as any run does. Enter keeps the edits: nothing runs,
/// and the line and the draft are as they were (the sort's order is written only once its run
/// is sent); `y` drops them and runs it.
#[test]
fn a_sort_over_pending_edits_asks_first_and_enter_keeps_them() {
    let mut h = users(Lang::En);
    h.command("where id > 0");
    answer(&mut h);
    h.sent();
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        assert!(rs.edit.batch.delete(&[0], |_| Vec::new()));
    }
    let (before, d) = (fields(&h), draft(&h));
    for how in ["s", "S", "click"] {
        match how {
            "click" => {
                let (x, y) = zone(&mut h, 1);
                h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
                h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
            }
            keys => h.keys(&format!("{}{keys}", if h.app.tab().grid.col == 1 { "" } else { "l" })),
        }
        assert_eq!(
            h.app.overlays.confirm().map(|c| c.action),
            Some(ConfirmAction::LeaveEdits(datarig_tui::app::edit::Leave::Run(h.app.tab().id))),
            "{how}"
        );
        h.key(KeyCode::Enter);
        assert!(runs(&mut h).is_empty(), "{how}: Enter keeps the edits, nothing runs");
        assert!(matches!(&h.app.tab().results, Results::Rows(rs) if !rs.edit.batch.is_empty()));
        // Nothing of the sort stays: the line and the draft are as they were.
        assert_eq!(fields(&h), before, "{how}");
        assert_eq!(draft(&h), d, "{how}");
        assert!(heads(&h).iter().all(|m| m.sort.is_none()));
    }
    // `y` drops them: a new run, never a page read again.
    h.keys("s");
    h.keys("y");
    let sent = h.sent();
    let want = vec![format!(r#"{ALL} WHERE (id > 0) ORDER BY "name" ASC"#)];
    assert!(sent.iter().any(|c| matches!(c, DbCommand::Execute { statements, .. } if *statements == want)), "{sent:?}");
    assert!(!sent.iter().any(|c| matches!(c, DbCommand::FetchMore { .. } | DbCommand::Resume { .. })));
    answer(&mut h);
    h.draw(120, 30);
    assert_eq!(fields(&h), ["id > 0", "name ASC", ""], "the line shows the order sent");
}

/// A sort asked about for the reads that lock rows (MySQL at SERIALIZABLE without autocommit)
/// and declined leaves the line and the draft as they were; `y` runs it.
#[test]
fn a_sort_declined_for_locking_reads_leaves_the_line() {
    use datarig_core::driver::Isolation;
    use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
    let driver = FakeDriver::default();
    driver.mysql.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut p = datarig_core::profile::ConnectionConfig::test_db();
    (p.name, p.driver) = ("local-my".into(), "mysql".into());
    let cfg = datarig_core::config::Config { connections: vec![p], ..Default::default() };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["shop".into()])));
    open_users(&mut h);
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Language(Language::Sql(Dialect::MySql(MySqlMode::default()))));
    h.tab_db(index, DbEvent::Connected);
    answer(&mut h);
    h.sent();
    h.key(KeyCode::Tab);
    h.tab_db(index, DbEvent::Isolation(Some(Isolation::Serializable)));
    h.tab_db(index, DbEvent::Autocommit(false));
    let (before, d) = (fields(&h), draft(&h));
    h.keys("ls");
    let id = h.app.tab().id;
    assert_eq!(h.app.overlays.confirm().map(|c| c.action), Some(ConfirmAction::TableReadLocks(id)));
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty(), "declined: nothing sent");
    assert_eq!(fields(&h), before);
    assert_eq!(draft(&h), d);
    h.keys("s");
    h.keys("y");
    assert_eq!(one_run(&mut h), "SELECT * FROM `shop`.`users` ORDER BY `name` ASC");
    answer(&mut h);
    assert_eq!(field_text(&h, Field::Order), "name ASC");
}

/// A sort the read-only environment refuses (the filter shown calls a function whose effects
/// are not known, let through before the environment turned read-only) sends nothing and leaves
/// the line and the draft as they were.
#[test]
fn a_sort_refused_as_read_only_leaves_the_line() {
    use datarig_core::policy::Policy;
    let mut h = users(Lang::En);
    h.command("where my_fn(id) > 0");
    h.keys("y");
    assert_eq!(one_run(&mut h), format!("{ALL} WHERE (my_fn(id) > 0)"));
    answer(&mut h);
    h.app.focus = datarig_tui::app::Focus::Results;
    h.app.policies.insert("default", Policy { read_only: true, ..Policy::default() });
    let (before, d) = (fields(&h), draft(&h));
    h.keys("ls");
    assert!(runs(&mut h).is_empty(), "refused: nothing sent");
    assert!(h.status(120, 30).contains("read-only"), "{}", h.status(120, 30));
    assert_eq!(fields(&h), before);
    assert_eq!(draft(&h), d);
    assert!(heads(&h).iter().all(|m| m.sort.is_none()));
}

#[test]
fn sorting_is_only_for_table_tabs() {
    let mut h = with_edge_results(Lang::En);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    for keys in ["s", "S", " rs", " rS"] {
        h.keys(keys);
        let status = h.status(120, 30);
        assert!(status.contains("not a table (sort a query in its text)"), "{keys:?}: {status}");
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

#[test]
fn the_empty_filter_line_names_the_filter_and_sort_keys_of_the_key_map() {
    let screen = users(Lang::En).screen(120, 30);
    assert!(screen.contains("WHERE  f filter · s sort "), "{screen}");
    let mut cfg = test_db_config();
    cfg.keymap = datarig_core::config::parse(
        "[keymap.grid]\n\"f\" = \"none\"\n\"s\" = \"none\"\n\"f8\" = \"table.filter.edit\"\n\"f9\" = \"table.sort.cycle\"\n",
    )
    .unwrap()
    .keymap;
    let mut h = Harness::connected_with(&cfg, Lang::En);
    open_users(&mut h);
    answer(&mut h);
    let screen = h.screen(120, 30);
    assert!(screen.contains("WHERE  F8 filter · F9 sort "), "{screen}");
}

/// After a run of the line's query that brought no rows (refused by the server, cancelled),
/// the rows shown are an earlier query's while the line shows the one that did not land. A
/// grid action (`s`, `S`, a click in a sort zone, `F` and an entry of its menu) is refused as
/// over changes not applied, said, and nothing changes: nothing is sent, the line and the draft
/// keep the query that did not land.
#[test]
fn grid_actions_over_a_query_that_did_not_land_are_refused() {
    use datarig_core::driver::{DbError, ServerMessage};
    use datarig_tui::app::overlay::OverlayKind;
    let server =
        DbError::Server(ServerMessage { text: "column \"idd\" does not exist".into(), position: None, hint: None });
    for (case, error, cancelled) in [("failed", server, false), ("cancelled", DbError::Cancelled, true)] {
        let mut h = users(Lang::En);
        h.keys("f");
        h.type_text("idd > 1");
        h.key(KeyCode::Enter);
        assert_eq!(one_run(&mut h), format!("{ALL} WHERE (idd > 1)"), "{case}");
        let (id, index) = (h.app.tab().exec.query_id, h.app.tabs.active_index());
        h.tab_db(index, DbEvent::Failed { id, error, cancelled });
        let tq = h.app.tab().doc.table_query.as_ref().unwrap();
        assert!(tq.failure().is_some(), "{case}");
        assert_eq!(tq.shown.as_ref().and_then(|q| q.filter.as_ref()), None, "{case}: the earlier rows");
        h.app.focus = datarig_tui::app::Focus::Results;
        h.app.tab_mut().doc.table_query.as_mut().unwrap().line.focus = None;
        assert_eq!(h.app.key_context(), Ctx::Grid, "{case}");
        let (before, d) = (fields(&h), draft(&h));
        assert_eq!(before[0], "idd > 1", "{case}");
        for how in ["s", "S", "click", "F"] {
            match how {
                "click" => {
                    let (x, y) = zone(&mut h, 1);
                    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
                    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
                }
                keys => h.keys(keys),
            }
            assert!(runs(&mut h).is_empty(), "{case} {how}: nothing sent");
            assert_ne!(h.overlay_kind(), Some(OverlayKind::ContextMenu), "{case} {how}: no menu");
            let status = h.status(120, 30);
            let says =
                if how == "F" { "Not filtered: showing previous rows" } else { "Not sorted: showing previous rows" };
            assert!(status.contains(says), "{case} {how}: {status}");
            assert_eq!(fields(&h), before, "{case} {how}: the line keeps the query");
            assert_eq!(draft(&h), d, "{case} {how}: the draft is as it was");
            assert!(heads(&h).iter().all(|m| m.sort.is_none()), "{case} {how}");
        }
    }
    // An entry picked from a menu opened over rows of the line's query, once the line's query
    // is one whose rows are not shown (as a run that did not land leaves it): refused too.
    let mut h = users(Lang::En);
    h.keys("F");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
    let qid = h.app.tab().exec.query_id;
    let tq = h.app.tab_mut().doc.table_query.as_mut().unwrap();
    let other = datarig_core::query::Query {
        filter: Some(datarig_core::query::Filter { text: "idd > 1".into(), recognised: None }),
        ..tq.shown.clone().unwrap()
    };
    tq.requested = Some((qid, other));
    h.menu_pick("= 0");
    assert!(runs(&mut h).is_empty(), "the entry sends nothing");
    let status = h.status(120, 30);
    assert!(status.contains("Not filtered: showing previous rows"), "{status}");
}

/// The notice the status line shows now, in full.
fn flashed(h: &Harness) -> String {
    h.app.transient.as_ref().map(|(n, _)| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

/// At 80 columns a refused sort or filter by value says why in full: the status line cuts a
/// long notice in its middle, which would lose the reason and keep only the keys after it.
#[test]
fn a_refused_sort_or_filter_says_why_at_80_columns() {
    // Changes in the line not applied.
    let mut h = users(Lang::En);
    typed(&mut h, Field::Where, "id > 2");
    for (keys, name) in [("s", "sort"), ("F", "filter")] {
        h.keys(keys);
        assert!(runs(&mut h).is_empty(), "{keys}: nothing sent");
        let said = flashed(&h);
        assert!(said.contains("filter line edited"), "{keys}: {said}");
        let status = h.status(80, 24);
        assert!(status.contains(&said), "{keys}: {status}");
        assert_screen!(format!("table_sort_refused_unapplied_{name}_en_80x24"), Lang::En, h.draw(80, 24));
    }
    // The rows shown are an earlier run's: the line's run was cancelled.
    let mut h = users(Lang::En);
    h.keys("f");
    h.type_text("idd > 1");
    h.key(KeyCode::Enter);
    assert_eq!(one_run(&mut h), format!("{ALL} WHERE (idd > 1)"));
    let (id, index) = (h.app.tab().exec.query_id, h.app.tabs.active_index());
    h.tab_db(index, DbEvent::Failed { id, error: datarig_core::driver::DbError::Cancelled, cancelled: true });
    h.app.focus = datarig_tui::app::Focus::Results;
    h.app.tab_mut().doc.table_query.as_mut().unwrap().line.focus = None;
    for (keys, name) in [("s", "sort"), ("F", "filter")] {
        h.keys(keys);
        assert!(runs(&mut h).is_empty(), "{keys}: nothing sent");
        let said = flashed(&h);
        assert!(said.contains("showing previous rows"), "{keys}: {said}");
        let status = h.status(80, 24);
        assert!(status.contains(&said), "{keys}: {status}");
        assert_screen!(format!("table_sort_refused_not_shown_{name}_en_80x24"), Lang::En, h.draw(80, 24));
    }
    // A query's results.
    let mut h = with_edge_results(Lang::En);
    h.key(KeyCode::Tab);
    h.keys("s");
    let said = flashed(&h);
    assert!(said.contains("not a table (sort a query in its text)"), "{said}");
    assert!(h.status(80, 24).contains(&said), "{}", h.status(80, 24));
}

/// Every refusal of a sort or a filter by value, in either language, fits the status line at
/// 80 columns whole (a column, a type and a collation of usual length).
#[test]
fn every_refusal_of_a_sort_or_filter_by_value_fits_80_columns() {
    use datarig_i18n::{Label, Msg};
    use datarig_tui::app::{Level, Notice};
    for lang in [Lang::En, Lang::Ko] {
        let mut h = users(lang);
        let column = || "email".to_string();
        let ty = || "jsonb".to_string();
        let reason = h.app.i18n.label(Label::CopySqlWhyNotShownWhole).to_string();
        let why = h.app.i18n.label(Label::TableFilterWhyUnclosed).to_string();
        let all: Vec<Msg> = vec![
            Label::TableSortOnlyTableTabs.into(),
            Label::TableSortNoColumns.into(),
            Label::TableSortUnapplied.into(),
            Label::TableSortNotShown.into(),
            Msg::TableSortName { column: column() },
            Label::TableFilterByValueNoRows.into(),
            Label::TableFilterByValueUnapplied.into(),
            Label::TableFilterByValueNotShown.into(),
            Msg::TableFilterByValueName { column: column() },
            Label::TableFilterByValueNotOneLine.into(),
            Label::TableFilterByValueNewRows.into(),
            Label::TableFilterByValueOneColumn.into(),
            Label::TableFilterByValueEdited.into(),
            Label::TableFilterByValueUnread.into(),
            Label::TableFilterByValueTooLong.into(),
            Msg::TableFilterByValueTooMany { max: "1000".into() },
            Msg::TableFilterByValueTooBig { column: column() },
            Msg::TableFilterByValueNotWhole { column: column(), reason },
            Msg::TableFilterByValueType { column: column(), ty: ty() },
            Msg::TableFilterByValueOrder { column: column(), ty: ty() },
            Msg::TableFilterByValueNondeterministic { collation: "case_insensitive".into() },
            Msg::TableFilterByValueNondeterministicColumn { column: column() },
            Msg::TableFilterByValueCollationUnknown { column: column() },
            Msg::TableFilterByValueLikeRefused { column: column() },
            Msg::TableFilterByValueLikeUnknown { column: column() },
            Msg::TableFilterByValueBase { why },
        ];
        for m in all {
            h.app.transient = Some((Notice::new(m.clone(), Level::Warning), std::time::Instant::now()));
            let said = h.app.i18n.msg(&m).to_string();
            let status = h.status(80, 24);
            assert!(status.contains(&said), "{} {m:?}: {status}", lang_tag(lang));
        }
    }
}
