//! Filter a table tab by a cell's value (`F`, `Space r F`, the grid menu's entry): each entry
//! sends exactly its condition added to the filter of the rows shown (an `OR` at its top
//! wrapped first), puts that text in the filter line and runs it as `Enter` there does (pending
//! edits asked about first); a NULL cell offers `IS NULL` only, several cells the `IN` forms
//! with NULL taken out of the list; entries that cannot apply are dimmed with why; a query's
//! rows have no such menu.

use crate::common;

use common::*;
use datarig_core::driver::{ColumnMeta, DbCommand, DbEvent, SessionValues, ValueKind};
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_i18n::Lang;
use datarig_tui::app::Results;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::table_query::Field;
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::time::Duration;

const ALL: &str = r#"SELECT * FROM "shop"."users""#;
const MY_ALL: &str = "SELECT * FROM `shop`.`users`";

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

fn col(name: &str, ty: &str, kind: ValueKind) -> ColumnMeta {
    ColumnMeta::new(name.into(), ty.into(), kind, None)
}

/// The columns of the rows answered: a number, text, a float, bytes, JSON, a time and a type of
/// no known comparison.
fn columns() -> Vec<ColumnMeta> {
    vec![
        col("id", "int8", ValueKind::Integer),
        col("name", "text", ValueKind::Text),
        col("score", "float8", ValueKind::Float),
        col("data", "bytea", ValueKind::Bytes),
        col("doc", "json", ValueKind::Json),
        col("born", "timestamptz", ValueKind::TimestampTz),
        col("spot", "point", ValueKind::Other),
    ]
}

fn rows() -> Vec<Vec<Option<String>>> {
    let s = |v: &str| Some(v.to_string());
    vec![
        vec![s("1"), s("it's"), s("1.5"), s(r"\x01ab"), s(r#"{"a": 1}"#), s("2026-11-10 09:30:00+09"), s("(1,2)")],
        vec![s("2"), None, None, None, None, None, None],
        vec![s("3"), s("50%_off"), s("2.5"), s(r"\x00"), s("[]"), s("2026-11-11 00:00:00+09"), s("(0,0)")],
    ]
}

/// Answer the active tab's current run with `rows` of `columns`.
fn page_with(h: &mut Harness, columns: Vec<ColumnMeta>, rows: Vec<Vec<Option<String>>>) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Page { id, columns: Some(columns), rows, more: false, elapsed: Duration::from_millis(2) });
}

fn page(h: &mut Harness) {
    page_with(h, columns(), rows());
}

/// `shop.users` open and answered, the grid focused, nothing sent since.
fn users() -> Harness {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    page(&mut h);
    h.sent();
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h
}

/// The grid's cursor on (`row`, `col`), no selection.
fn at(h: &mut Harness, row: usize, col: usize) {
    let g = &mut h.app.tab_mut().grid;
    (g.row, g.col, g.anchor) = (row, col, None);
}

/// The open menu's lines: headings as `# text`, items as `label | keys or hint or why`.
fn lines(h: &Harness) -> Vec<String> {
    h.app
        .menu_lines()
        .into_iter()
        .map(|(heading, label, keys)| if heading { format!("# {label}") } else { format!("{label} | {keys}") })
        .collect()
}

fn menu_open(h: &Harness) -> bool {
    h.overlay_kind() == Some(OverlayKind::ContextMenu)
}

/// The status bar's passing word.
fn flashed(h: &Harness) -> String {
    h.app.transient.as_ref().map(|(n, _)| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

fn where_text(h: &Harness) -> String {
    h.app.tab().doc.table_query.as_ref().expect("a table query").line.field(Field::Where).text().to_string()
}

/// `F` on (`row`, `col`), entry `label` picked: the statements sent.
fn filter(h: &mut Harness, row: usize, col: usize, label: &str) -> Vec<Vec<String>> {
    at(h, row, col);
    h.keys("F");
    assert!(menu_open(h), "{}", flashed(h));
    h.menu_pick(label);
    runs(h)
}

/// The filter cleared and the rows answered again, nothing sent since.
fn clear(h: &mut Harness) {
    clear_with(h, columns(), rows());
}

/// The filter cleared and the run answered with `rows` of `columns`, nothing sent since.
fn clear_with(h: &mut Harness, columns: Vec<ColumnMeta>, rows: Vec<Vec<Option<String>>>) {
    h.command("filter clear");
    page_with(h, columns, rows);
    h.sent();
}

#[test]
fn each_entry_sends_its_condition_and_puts_it_in_the_line() {
    let mut h = users();
    let cases: &[(usize, usize, &str, &str)] = &[
        (0, 0, "= 1", "id = 1"),
        (0, 0, "<> 1", "id <> 1"),
        (0, 0, "<> 1 (NULLs included)", "id IS DISTINCT FROM 1"),
        (0, 0, "< 1", "id < 1"),
        (0, 0, "> 1", "id > 1"),
        (0, 0, "<= 1", "id <= 1"),
        (0, 0, ">= 1", "id >= 1"),
        (0, 1, "= 'it's'", "name = 'it''s'"),
        (0, 1, "<> 'it's' (NULLs included)", "name IS DISTINCT FROM 'it''s'"),
        (0, 1, "starts with 'it's'", "name LIKE 'it''s%' ESCAPE '!'"),
        (0, 1, "contains 'it's' (case-insensitive)", "name ILIKE '%it''s%' ESCAPE '!'"),
        (2, 1, "starts with '50%_off'", "name LIKE '50!%!_off%' ESCAPE '!'"),
        (0, 2, "= 1.5", "score = 1.5"),
        (0, 3, r"= '\x01ab'", r"data = E'\\x01ab'::bytea"),
        (2, 3, r"> '\x00'", r"data > E'\\x00'::bytea"),
        (0, 5, ">= '2026-11-10 09:30:00+09'", "born >= '2026-11-10 09:30:00+09'"),
        (1, 1, "IS NULL", "name IS NULL"),
        (1, 4, "IS NOT NULL", "doc IS NOT NULL"),
    ];
    for &(row, col, label, cond) in cases {
        let sent = filter(&mut h, row, col, label);
        assert_eq!(sent, vec![vec![format!("{ALL} WHERE ({cond})")]], "{label}");
        assert_eq!(where_text(&h), cond, "the line shows what runs: {label}");
        assert_eq!(h.app.tab().doc.table_query.as_ref().unwrap().line.focus, None, "the grid keeps the keys");
        page(&mut h);
        let shown = h.app.tab().doc.table_query.as_ref().and_then(|q| q.shown.clone()).unwrap();
        assert_eq!(shown.filter.map(|f| f.text), Some(cond.to_string()));
        clear(&mut h);
    }
    // `Space r F` opens the same menu.
    at(&mut h, 0, 0);
    h.keys(" rF");
    assert!(menu_open(&h));
    assert_eq!(lines(&h)[0], "# Filter by id");
}

#[test]
fn the_entries_of_a_cell_with_their_hints() {
    let mut h = users();
    at(&mut h, 0, 1);
    h.keys("F");
    assert_eq!(
        lines(&h),
        vec![
            "# Filter by name",
            "= 'it's' | case as the server compares",
            "<> 'it's' | leaves out NULL rows",
            "<> 'it's' (NULLs included) | ",
            "< 'it's' | ",
            "> 'it's' | ",
            "<= 'it's' | ",
            ">= 'it's' | ",
            "starts with 'it's' | case as the server compares",
            "contains 'it's' (case-insensitive) | collation not known: ILIKE",
        ]
    );
    h.key(KeyCode::Esc);
    // A float: equality said to be inexact, still offered.
    at(&mut h, 0, 2);
    h.keys("F");
    let shown = lines(&h);
    assert_eq!(shown[1], "= 1.5 | floating point: = may find nothing");
    assert_eq!(shown[2], "<> 1.5 | floating point: = may find nothing");
    assert!(h.app.menu_dimmed_rows().is_empty());
    h.key(KeyCode::Esc);
    // A NULL cell: tested as NULL only, never with `=`.
    at(&mut h, 1, 2);
    h.keys("F");
    assert_eq!(lines(&h), vec!["# Filter by score", "IS NULL | ", "IS NOT NULL | "]);
}

#[test]
fn several_cells_give_the_in_forms_with_null_apart() {
    let mut h = users();
    // Three numbers: IN, NOT IN (said to leave out NULL rows) and NOT IN keeping them.
    at(&mut h, 0, 0);
    h.keys("vjj");
    h.keys("F");
    assert_eq!(
        lines(&h),
        vec![
            "# Filter by id (3 cells)",
            "IN (1, 2, 3) | ",
            "NOT IN (1, 2, 3) | leaves out NULL rows",
            "NOT IN (1, 2, 3) (NULLs included) | ",
        ]
    );
    h.menu_pick("NOT IN (1, 2, 3) (NULLs included)");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE ((id NOT IN (1, 2, 3) OR id IS NULL))")]]);
    page(&mut h);
    clear(&mut h);
    let picked = |h: &mut Harness, label: &str| {
        at(h, 0, 1);
        h.keys("vjj");
        h.keys("F");
        h.menu_pick(label);
        runs(h)
    };
    // With a NULL among them: NULL tested on its own, and no NOT IN that keeps NULL rows.
    at(&mut h, 0, 1);
    h.keys("vjjF");
    assert_eq!(
        lines(&h),
        vec!["# Filter by name (3 cells)", "IN ('it's', '50%_off', NULL) | ", "NOT IN ('it's', '50%_off', NULL) | "]
    );
    h.key(KeyCode::Esc);
    let sent = picked(&mut h, "IN ('it's', '50%_off', NULL)");
    assert_eq!(sent, vec![vec![format!("{ALL} WHERE ((name IN ('it''s', '50%_off') OR name IS NULL))")]]);
    page(&mut h);
    clear(&mut h);
    let sent = picked(&mut h, "NOT IN ('it's', '50%_off', NULL)");
    assert_eq!(sent, vec![vec![format!("{ALL} WHERE ((name NOT IN ('it''s', '50%_off') AND name IS NOT NULL))")]]);
    page(&mut h);
    // Only NULLs: IS NULL and IS NOT NULL.
    let (cols, mut rs) = (columns(), rows());
    rs[0][1] = None;
    rs.truncate(2);
    clear_with(&mut h, cols, rs);
    at(&mut h, 0, 1);
    h.keys("vjF");
    assert_eq!(lines(&h), vec!["# Filter by name (2 cells)", "IS NULL | ", "IS NOT NULL | "]);
}

#[test]
fn an_or_at_the_top_is_wrapped_and_a_chain_appended() {
    let mut h = users();
    h.command("where id > 0 OR name IS NULL");
    assert_eq!(runs(&mut h).len(), 1);
    page(&mut h);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    let sent = filter(&mut h, 0, 0, "= 1");
    let wrapped = "(id > 0 OR name IS NULL) AND id = 1";
    assert_eq!(sent, vec![vec![format!("{ALL} WHERE ({wrapped})")]]);
    assert_eq!(where_text(&h), wrapped);
    page(&mut h);
    let sent = filter(&mut h, 0, 1, "<> 'it's'");
    let chained = format!("{wrapped} AND name <> 'it''s'");
    assert_eq!(sent, vec![vec![format!("{ALL} WHERE ({chained})")]]);
    assert_eq!(where_text(&h), chained);
}

#[test]
fn a_filter_by_value_on_a_sorted_tab_keeps_the_order_and_marks_its_column() {
    let mut h = users();
    let marks = |h: &Harness| h.app.table_head_marks(h.app.tab().id);
    at(&mut h, 0, 1);
    h.keys("s");
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} ORDER BY "name" ASC"#)]]);
    page(&mut h);
    h.keys("s");
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} ORDER BY "name" DESC"#)]]);
    page(&mut h);
    assert!(marks(&h).iter().all(|m| !m.filtered));
    let sent = filter(&mut h, 0, 2, "> 1.5");
    assert_eq!(sent, vec![vec![format!(r#"{ALL} WHERE (score > 1.5) ORDER BY "name" DESC"#)]]);
    assert_eq!(where_text(&h), "score > 1.5");
    let order = |h: &Harness| h.app.tab().doc.table_query.as_ref().unwrap().line.field(Field::Order).text().to_string();
    assert_eq!(order(&h), "name DESC");
    // The marks are of the rows shown: the filter's run is still on its way.
    assert!(marks(&h).iter().all(|m| !m.filtered));
    page(&mut h);
    let m = marks(&h);
    assert_eq!(m.iter().map(|m| m.filtered).collect::<Vec<_>>(), [false, false, true, false, false, false, false]);
    assert_eq!(m[1].sort, Some((false, 1)));
    assert!(m.iter().enumerate().all(|(i, m)| i == 1 || m.sort.is_none()), "{m:?}");
    // A sort after it keeps the filter.
    at(&mut h, 0, 0);
    h.keys("s");
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} WHERE (score > 1.5) ORDER BY "id" ASC"#)]]);
}

#[test]
fn refused_entries_are_dimmed_with_why_and_send_nothing() {
    let mut h = users();
    // JSON has no equality the app knows of: every entry dimmed, picking one says why.
    at(&mut h, 0, 4);
    h.keys("F");
    assert!(menu_open(&h));
    let dimmed = h.app.menu_dimmed_rows();
    assert_eq!(dimmed.len(), 7, "{dimmed:?}");
    assert!(dimmed.iter().all(|(_, why)| why == "type not compared"), "{dimmed:?}");
    let why = h.app.menu_dimmed_notice(r#"= '{"a": 1}'"#).expect("dimmed").render(&h.app.i18n).to_string();
    assert!(why.contains("values of json (column “doc”)"), "{why}");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    h.key(KeyCode::Esc);
    // A type of no known comparison.
    at(&mut h, 0, 6);
    h.keys("F");
    assert!(h.app.menu_dimmed_rows().iter().any(|(l, why)| *l == "= '(1,2)'" && why == "type not compared"));
    h.key(KeyCode::Esc);
    // Binary data over 64 KiB.
    let (cols, mut rs) = (columns(), rows());
    rs[0][3] = Some(format!(r"\x{}", "ab".repeat(64 * 1024 + 1)));
    clear_with(&mut h, cols, rs);
    at(&mut h, 0, 3);
    h.keys("F");
    let dimmed = h.app.menu_dimmed_rows();
    assert_eq!(dimmed.len(), 7, "{dimmed:?}");
    assert!(dimmed.iter().all(|(_, why)| why == "over 64 KiB"), "{dimmed:?}");
    h.key(KeyCode::Esc);
    // Bytes not shown in their hexadecimal form are not taken.
    let (cols, mut rs) = (columns(), rows());
    rs[0][3] = Some(r"\001\253".into());
    clear_with(&mut h, cols, rs);
    at(&mut h, 0, 3);
    h.keys("F");
    assert!(h.app.menu_dimmed_rows().iter().all(|(_, why)| why == "not shown whole"));
    h.key(KeyCode::Esc);
    // Cells of two columns, a new row, a cell with an edit: no menu, said why.
    at(&mut h, 0, 0);
    h.keys("vlF");
    assert!(!menu_open(&h));
    assert!(flashed(&h).contains("select cells of one column"), "{}", flashed(&h));
    h.key(KeyCode::Esc);
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        let orig = rows()[0].clone();
        assert!(rs.edit.batch.set(&[(0, 1)], &datarig_core::edit::Value::Text("x".into()), |_| orig.clone()));
    }
    at(&mut h, 0, 1);
    h.keys("F");
    assert!(!menu_open(&h));
    assert!(flashed(&h).contains("a selected cell shows an edit"), "{}", flashed(&h));
    assert!(runs(&mut h).is_empty());
}

#[test]
fn changes_not_applied_in_the_line_are_kept_and_nothing_is_added() {
    let mut h = users();
    h.keys("f");
    h.type_text("id > 5");
    // The grid takes the keys back, the text stays in the field, not applied.
    h.app.tab_mut().doc.table_query.as_mut().unwrap().line.focus = None;
    at(&mut h, 0, 0);
    h.keys("F");
    assert!(!menu_open(&h));
    assert!(flashed(&h).contains("changes not applied"), "{}", flashed(&h));
    assert_eq!(where_text(&h), "id > 5");
    assert!(runs(&mut h).is_empty());
}

#[test]
fn pending_edits_are_asked_about_first_as_enter_asks() {
    let mut h = users();
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        assert!(rs.edit.batch.delete(&[2], |_| Vec::new()));
    }
    let sent = filter(&mut h, 0, 0, "= 1");
    assert!(sent.is_empty(), "nothing runs while it asks");
    assert_eq!(
        h.app.overlays.confirm().map(|c| c.action),
        Some(ConfirmAction::LeaveEdits(datarig_tui::app::edit::Leave::Run(h.app.tab().id)))
    );
    // Enter keeps the edits: nothing runs, the line holds the filter, not applied.
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert!(matches!(&h.app.tab().results, Results::Rows(rs) if !rs.edit.batch.is_empty()));
    assert_eq!(where_text(&h), "id = 1");
    // Applied from the line, asked again; `y` drops the edits and runs it.
    h.keys("f");
    h.key(KeyCode::Enter);
    assert!(h.app.overlays.is_open(OverlayKind::Confirm));
    h.keys("y");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id = 1)")]]);
}

#[test]
fn the_grid_menu_has_the_entry_and_a_query_has_none() {
    let mut h = users();
    h.draw(120, 30);
    let g = h.app.tab().grid.clone();
    let x = g.hit_cols.iter().find(|c| c.2 == 1).unwrap().0 + 1;
    h.mouse(MouseEventKind::Down(MouseButton::Right), x, g.data_y);
    assert!(menu_open(&h));
    assert!(lines(&h).contains(&"Table → filter by value | F".to_string()));
    h.menu_pick("Table → filter by value");
    assert_eq!(lines(&h)[0], "# Filter by name", "the same menu as F");
    h.menu_pick("= 'it's'");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (name = 'it''s')")]]);
    // A query's rows: dimmed in the grid's menu, `F` says why.
    let mut h = with_edge_results(Lang::En);
    h.key(KeyCode::Tab);
    h.keys("F");
    assert!(!menu_open(&h));
    assert!(flashed(&h).contains("Only a table tab has a filter line"), "{}", flashed(&h));
    h.keys("  ");
    let dimmed = h.app.menu_dimmed_rows();
    assert!(dimmed.iter().any(|(l, why)| *l == "Table → filter by value" && why == "only on table tabs"), "{dimmed:?}");
}

#[test]
fn a_menu_opened_on_other_cells_runs_nothing() {
    let mut h = users();
    at(&mut h, 0, 0);
    h.keys("F");
    // The cursor moves under the open menu (another key's doing): the entry does not run.
    h.app.tab_mut().grid.row = 2;
    h.type_text("= 1");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert!(!menu_open(&h));
}

/// While the tab's run is on its way the menu does not open (its cells are of the rows shown,
/// which the run is about to replace): said why, and nothing is sent.
#[test]
fn the_menu_does_not_open_while_the_tab_runs() {
    let mut h = users();
    h.command("where id > 0");
    assert_eq!(runs(&mut h).len(), 1);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    at(&mut h, 0, 0);
    h.keys("F");
    assert!(!menu_open(&h), "no menu while the run is on its way");
    assert!(flashed(&h).contains("A query is already running"), "{}", flashed(&h));
    assert!(runs(&mut h).is_empty());
    // Its rows came: the menu is of them.
    page(&mut h);
    h.keys("F");
    assert!(menu_open(&h), "{}", flashed(&h));
}

/// The entry picked runs the condition the menu showed, for the rows it was opened on: rows
/// read again under it (another result in their place), or a run started meanwhile, make it
/// out of date, said, and nothing runs.
#[test]
fn a_menu_over_rows_read_again_runs_nothing() {
    let mut h = users();
    at(&mut h, 0, 0);
    h.keys("F");
    assert!(menu_open(&h));
    // Another result put in the place of the one the menu was opened on, at the same place of
    // the same run.
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        rs.id += 1_000_000;
    }
    h.menu_pick("= 1");
    assert!(runs(&mut h).is_empty(), "nothing runs");
    assert!(!menu_open(&h));
    assert!(flashed(&h).contains("changed"), "{}", flashed(&h));
    assert_eq!(where_text(&h), "", "the line is as it was");
    // A run started under the open menu: out of date too.
    h.keys("F");
    assert!(menu_open(&h));
    h.app.dispatch(datarig_tui::app::action::Action::RunStatement);
    assert_eq!(runs(&mut h).len(), 1);
    h.menu_pick("= 1");
    assert!(runs(&mut h).is_empty(), "nothing more runs");
    assert_eq!(where_text(&h), "");
}

/// Entry `by` of the open menu picked (by what it filters, not its label).
fn pick(h: &mut Harness, by: datarig_tui::app::filter_value::ByValue) {
    use datarig_tui::app::menu::{MenuItem, MenuLine};
    let m = h.app.overlays.menu_mut().expect("the filter menu");
    let i = m.lines.iter().position(|l| *l == MenuLine::Item(MenuItem::Filter(by))).expect("the entry");
    m.selected = i;
    h.key(KeyCode::Enter);
}

/// The rows of `shop.users` whose `name` holds line breaks (LF, CR, CRLF), in rows 0 to 2.
fn broken_lines() -> Vec<Vec<Option<String>>> {
    let mut rs = rows();
    for (r, v) in ["a\nb", "a\rb", "a\r\nb"].into_iter().enumerate() {
        rs[r][1] = Some(v.to_string());
    }
    rs
}

/// A value with a line break goes into the filter line on one line, as the text that runs: `F`
/// sends the condition the line shows, and `Enter` in the line runs the same statement again.
#[test]
fn a_value_with_line_breaks_runs_as_the_line_shows_it() {
    use datarig_tui::app::filter_value::ByValue as B;
    let mut h = users();
    clear_with(&mut h, columns(), broken_lines());
    let cases: &[(usize, B, &str)] = &[
        (0, B::Eq, r"name = E'a\nb'"),
        (1, B::Ne, r"name <> E'a\rb'"),
        (2, B::Eq, r"name = E'a\r\nb'"),
        (2, B::StartsWith, r"name LIKE E'a\r\nb%' ESCAPE '!'"),
    ];
    for &(row, by, cond) in cases {
        at(&mut h, row, 1);
        h.keys("F");
        assert!(menu_open(&h), "{}", flashed(&h));
        pick(&mut h, by);
        let want = vec![vec![format!("{ALL} WHERE ({cond})")]];
        assert_eq!(runs(&mut h), want, "{by:?}");
        assert_eq!(where_text(&h), cond, "the line shows what runs");
        page_with(&mut h, columns(), broken_lines());
        // Applied again from the line: the same statement.
        h.keys("f");
        h.key(KeyCode::Enter);
        assert_eq!(runs(&mut h), want, "Enter in the line: {by:?}");
        page_with(&mut h, columns(), broken_lines());
        // The grid takes the keys back.
        h.app.tab_mut().doc.table_query.as_mut().unwrap().line.focus = None;
        assert_eq!(h.app.key_context(), Ctx::Grid);
        clear_with(&mut h, columns(), broken_lines());
    }
    // Several cells: the list on one line too.
    at(&mut h, 0, 1);
    h.keys("vjjF");
    pick(&mut h, B::In);
    let cond = r"name IN (E'a\nb', E'a\rb', E'a\r\nb')";
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE ({cond})")]]);
    assert_eq!(where_text(&h), cond);
}

/// A MySQL profile with `shop.users` open on it, the session's values `charset`, the grid
/// focused.
fn mysql(charset: &str, columns: Vec<ColumnMeta>, rows: Vec<Vec<Option<String>>>) -> Harness {
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
    h.tab_db(index, DbEvent::SessionValues(SessionValues { time_zone: "+00:00".into(), charset: charset.into() }));
    page_with(&mut h, columns, rows);
    h.sent();
    h.key(KeyCode::Tab);
    h
}

#[test]
fn mysql_literals_follow_the_session_and_case_the_collation() {
    let cols = || {
        vec![
            col("id", "int", ValueKind::Integer),
            col("name", "varchar", ValueKind::Text),
            col("data", "varbinary", ValueKind::Bytes),
            col("w", "float", ValueKind::Float),
            col("at", "timestamp", ValueKind::TimestampTz),
        ]
    };
    let s = |v: &str| Some(v.to_string());
    let rows = || vec![vec![s("1"), s(r"it's\"), s("0x01AB"), s("1.5"), s("2026-11-10 09:30:00")]];
    let mut h = mysql("utf8mb4", cols(), rows());
    let my = |cond: &str| vec![vec![format!("{MY_ALL} WHERE ({cond})")]];
    // A backslash in text: its bytes, read the same in every SQL mode.
    let sent = filter(&mut h, 0, 1, r"= 'it's\'");
    assert_eq!(sent, my("name = _utf8mb4 X'697427735C'"));
    page_with(&mut h, cols(), rows());
    clear_with(&mut h, cols(), rows());
    // No collation known: lowered on both sides, the pattern escaped with `!`.
    let sent = filter(&mut h, 0, 1, r"contains 'it's\' (case-insensitive)");
    assert_eq!(sent, my("LOWER(name) LIKE LOWER(_utf8mb4 X'25697427735C25') ESCAPE '!'"));
    page_with(&mut h, cols(), rows());
    clear_with(&mut h, cols(), rows());
    // Bytes as X'…'; a TIMESTAMP as shown, in the zone it was read in.
    assert_eq!(filter(&mut h, 0, 2, "= '0x01AB'"), my("data = X'01AB'"));
    page_with(&mut h, cols(), rows());
    clear_with(&mut h, cols(), rows());
    assert_eq!(filter(&mut h, 0, 4, "< '2026-11-10 09:30:00'"), my("at < '2026-11-10 09:30:00'"));
    // A MySQL FLOAT is sent rounded: not taken.
    at(&mut h, 0, 3);
    h.keys("F");
    assert!(h.app.menu_dimmed_rows().iter().all(|(_, why)| why == "not shown whole"));
    // Text read in another character set than utf8mb4: not taken.
    let mut h = mysql("latin1", cols(), rows());
    at(&mut h, 0, 1);
    h.keys("F");
    let dimmed = h.app.menu_dimmed_rows();
    assert!(!dimmed.is_empty() && dimmed.iter().all(|(_, why)| why == "not shown whole"), "{dimmed:?}");
}

#[test]
fn the_filter_menu_screens() {
    let shot = |h: &mut Harness, name: &str| {
        assert_screen!(format!("table_filter_value_{name}_en_80x24"), Lang::En, h.draw(80, 24));
        assert_screen!(format!("table_filter_value_{name}_en_120x30"), Lang::En, h.draw(120, 30));
        h.key(KeyCode::Esc);
    };
    let mut h = users();
    at(&mut h, 0, 1);
    h.keys("F");
    shot(&mut h, "cell");
    at(&mut h, 1, 1);
    h.keys("F");
    shot(&mut h, "null");
    at(&mut h, 0, 1);
    h.keys("vjjF");
    shot(&mut h, "cells");
    // The selection ends too.
    h.key(KeyCode::Esc);
    at(&mut h, 0, 4);
    h.keys("F");
    shot(&mut h, "refused");
}

/// On MySQL a value with a line break is its UTF-8 bytes: one line in the filter line, and the
/// same statement when the line is applied again.
#[test]
fn a_mysql_value_with_line_breaks_runs_as_the_line_shows_it() {
    use datarig_tui::app::filter_value::ByValue as B;
    let cols = || vec![col("id", "int", ValueKind::Integer), col("name", "varchar", ValueKind::Text)];
    let s = |v: &str| Some(v.to_string());
    let rows = || vec![vec![s("1"), s("a\nb")], vec![s("2"), s("a\rb")], vec![s("3"), s("a\r\nb")]];
    let mut h = mysql("utf8mb4", cols(), rows());
    for (row, cond) in
        [(0, "name = _utf8mb4 X'610A62'"), (1, "name = _utf8mb4 X'610D62'"), (2, "name = _utf8mb4 X'610D0A62'")]
    {
        at(&mut h, row, 1);
        h.keys("F");
        pick(&mut h, B::Eq);
        let want = vec![vec![format!("{MY_ALL} WHERE ({cond})")]];
        assert_eq!(runs(&mut h), want);
        assert_eq!(where_text(&h), cond);
        page_with(&mut h, cols(), rows());
        h.keys("f");
        h.key(KeyCode::Enter);
        assert_eq!(runs(&mut h), want, "Enter in the line");
        page_with(&mut h, cols(), rows());
        // The grid takes the keys back.
        h.app.tab_mut().doc.table_query.as_mut().unwrap().line.focus = None;
        assert_eq!(h.app.key_context(), Ctx::Grid);
        clear_with(&mut h, cols(), rows());
    }
}
