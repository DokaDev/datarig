//! The grid keys of a table tab: `=` only the cursor's value (the column's own conditions
//! replaced, the other terms as written), `!` everything but it (rows whose value is NULL kept),
//! `X` no condition and no order (the limit kept), and the column menu (`F`, a right click on a
//! header): each sends exactly its statement through the tab's run (checks, questions, pending
//! edits), the change before it kept for the dialog's undo; what cannot apply is said, or shown
//! dimmed with why in the menu.

use crate::common;

use common::*;
use datarig_core::driver::{ColumnMeta, DbCommand, DbEvent, SessionValues, ValueKind};
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_i18n::Lang;
use datarig_tui::app::Results;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
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

/// The key-only read's answer for `shop.users`: no key, and whether the collation of `name` is
/// deterministic (`None`: not said).
fn collations(name: Option<bool>) -> datarig_core::driver::keys::OrderKeys {
    collations_on(name, None)
}

/// [`collations`] from a server that says whether it takes `LIKE` on a nondeterministic
/// collation (`like`; `None`: not said).
fn collations_on(name: Option<bool>, like: Option<bool>) -> datarig_core::driver::keys::OrderKeys {
    let deterministic = name.map(|d| ("name".to_string(), d)).into_iter().collect();
    datarig_core::driver::keys::OrderKeys { deterministic, like_nondeterministic: like, ..Default::default() }
}

/// `shop.users` open and answered (the collation of `name` read as deterministic), the grid
/// focused, nothing sent since.
fn users() -> Harness {
    users_with(Some(collations(Some(true))))
}

/// [`users`], the key-only read answered with `keys` (`None`: not answered).
fn users_with(keys: Option<datarig_core::driver::keys::OrderKeys>) -> Harness {
    users_from(Harness::connected(Lang::En), keys)
}

/// [`users_with`] in harness `h`.
fn users_from(mut h: Harness, keys: Option<datarig_core::driver::keys::OrderKeys>) -> Harness {
    open_users(&mut h);
    if let Some(keys) = keys {
        keys_came_with(&mut h, "local-pg", keys);
    }
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

/// The condition the table tab's next run sends.
fn where_text(h: &Harness) -> String {
    let tq = h.app.tab().doc.table_query.as_ref().expect("a table query");
    tq.draft.filter.as_ref().map(|f| f.text.clone()).unwrap_or_default()
}

/// `keys` on (`row`, `col`): the statements sent.
fn key_on(h: &mut Harness, row: usize, col: usize, keys: &str) -> Vec<Vec<String>> {
    at(h, row, col);
    h.keys(keys);
    runs(h)
}

/// `F` on (`row`, `col`), entry `label` picked: the statements sent.
fn menu(h: &mut Harness, row: usize, col: usize, label: &str) -> Vec<Vec<String>> {
    at(h, row, col);
    h.keys("F");
    assert!(menu_open(h), "{}", flashed(h));
    h.menu_pick(label);
    runs(h)
}

/// `cond` sent as the condition of `shop.users`.
fn sent(cond: &str) -> Vec<Vec<String>> {
    vec![vec![format!("{ALL} WHERE ({cond})")]]
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

/// A filter by value whose whole text would hold a line break (one of the other terms has one)
/// is dimmed in the column menu for that reason, not for the column's name.
#[test]
fn a_filter_text_with_a_line_break_is_dimmed_as_one() {
    let mut h = users();
    assert_eq!(key_on(&mut h, 0, 0, "="), sent("id = 1"));
    page(&mut h);
    // The rows shown are those of a condition written over two lines.
    let tq = h.app.tab_mut().doc.table_query.as_mut().expect("a table query");
    for q in [&mut tq.draft].into_iter().chain(tq.shown.as_mut()).chain(tq.requested.as_mut().map(|(_, q)| q)) {
        q.filter.as_mut().expect("a filter").text = "id = 1\nOR id = 3".into();
    }
    at(&mut h, 0, 1);
    h.keys("F");
    assert!(menu_open(&h), "{}", flashed(&h));
    let dimmed = h.app.menu_dimmed_rows();
    assert!(dimmed.iter().any(|(l, w)| *l == "Only this value" && w == "line break"), "{dimmed:?}");
    assert!(!dimmed.iter().any(|(_, w)| w == "name not one line"), "{dimmed:?}");
}

#[test]
fn equals_sends_only_the_cells_value() {
    let mut h = users();
    let cases: &[(usize, usize, &str)] = &[
        (0, 0, "id = 1"),
        (0, 1, "name = 'it''s'"),
        (2, 1, "name = '50%_off'"),
        (0, 2, "score = 1.5"),
        (0, 3, r"data = E'\\x01ab'::pg_catalog.bytea"),
        (0, 5, "born = '2026-11-10 09:30:00+09'"),
        (1, 1, "name IS NULL"),
        (1, 4, "doc IS NULL"),
    ];
    for &(row, col, cond) in cases {
        assert_eq!(key_on(&mut h, row, col, "="), sent(cond), "{cond}");
        assert_eq!(where_text(&h), cond, "the draft is what runs");
        assert_eq!(h.app.key_context(), Ctx::Grid, "the grid keeps the keys");
        page(&mut h);
        let shown = h.app.tab().doc.table_query.as_ref().and_then(|q| q.shown.clone()).unwrap();
        assert_eq!(shown.filter.map(|f| f.text), Some(cond.to_string()));
        clear(&mut h);
    }
    // `Space r =` is the same key.
    assert_eq!(key_on(&mut h, 0, 0, " r="), sent("id = 1"));
}

#[test]
fn exclude_keeps_the_rows_whose_value_is_null() {
    let mut h = users();
    let cases: &[(usize, usize, &str)] = &[
        (0, 0, "id IS DISTINCT FROM 1"),
        (0, 1, "name IS DISTINCT FROM 'it''s'"),
        (0, 3, r"data IS DISTINCT FROM E'\\x01ab'::pg_catalog.bytea"),
        (1, 1, "name IS NOT NULL"),
        (1, 6, "spot IS NOT NULL"),
    ];
    for &(row, col, cond) in cases {
        assert_eq!(key_on(&mut h, row, col, "!"), sent(cond), "{cond}");
        assert_eq!(where_text(&h), cond);
        page(&mut h);
        clear(&mut h);
    }
    assert_eq!(key_on(&mut h, 0, 0, " r!"), sent("id IS DISTINCT FROM 1"));
}

/// `=` takes the place of the column's own conditions (in the place of the first of them); an
/// expression naming the column and the other columns' terms stay as written; `!` is added
/// after the others.
#[test]
fn equals_replaces_the_columns_own_conditions_and_exclude_adds() {
    let mut h = users();
    h.command("where name  =  'a' AND id > 0 AND lower(name) <> 'x' AND name <> 'b'");
    runs(&mut h);
    page(&mut h);
    let replaced = "name = 'it''s' AND id > 0 AND lower(name) <> 'x'";
    assert_eq!(key_on(&mut h, 0, 1, "="), sent(replaced));
    page(&mut h);
    let added = format!("{replaced} AND id IS DISTINCT FROM 3");
    assert_eq!(key_on(&mut h, 2, 0, "!"), sent(&added));
    page(&mut h);
    // `=` on the id: its own condition replaced in its place.
    let again = "name = 'it''s' AND id = 2 AND lower(name) <> 'x'";
    assert_eq!(key_on(&mut h, 1, 0, "="), sent(again));
}

#[test]
fn an_or_at_the_top_is_wrapped_and_a_chain_appended() {
    let mut h = users();
    h.command("where id > 0 OR name IS NULL");
    assert_eq!(runs(&mut h).len(), 1);
    page(&mut h);
    let wrapped = "(id > 0 OR name IS NULL) AND id = 1";
    assert_eq!(key_on(&mut h, 0, 0, "="), sent(wrapped));
    assert_eq!(where_text(&h), wrapped);
    page(&mut h);
    let chained = format!("{wrapped} AND name IS DISTINCT FROM 'it''s'");
    assert_eq!(key_on(&mut h, 0, 1, "!"), sent(&chained));
}

/// The rows of `shop.users` with one more, whose `name` is `b`.
fn four_rows() -> Vec<Vec<Option<String>>> {
    let mut rs = rows();
    let s = |v: &str| Some(v.to_string());
    rs.push(vec![s("4"), s("b"), s("0.5"), s(r"\x02"), s("{}"), s("2026-11-12 00:00:00+09"), s("(2,2)")]);
    rs
}

/// A selection in one column: `=` a list (with `OR … IS NULL` when NULL is among the cells),
/// `!` a list that keeps the rows whose value is NULL unless NULL is among the cells; the forms
/// read back as the column's own condition, which `=` and the menu's clear replace or take out.
#[test]
fn selections_give_lists_with_null_apart() {
    let mut h = users();
    clear_with(&mut h, columns(), four_rows());
    let select = |h: &mut Harness, from: usize, to: usize| {
        let g = &mut h.app.tab_mut().grid;
        (g.row, g.col, g.anchor) = (to, 1, Some((from, 1)));
    };
    let cases: &[(usize, usize, &str, &str)] = &[
        (0, 2, "=", "(name IN ('it''s', '50%_off') OR name IS NULL)"),
        (0, 2, "!", "(name NOT IN ('it''s', '50%_off') AND name IS NOT NULL)"),
        (2, 3, "=", "name IN ('50%_off', 'b')"),
        (2, 3, "!", "(name NOT IN ('50%_off', 'b') OR name IS NULL)"),
    ];
    for &(from, to, key, cond) in cases {
        select(&mut h, from, to);
        h.keys(key);
        assert_eq!(runs(&mut h), sent(cond), "{key} {from}..{to}");
        page_with(&mut h, columns(), four_rows());
        // Read back as the column's own condition: `=` replaces it.
        assert_eq!(key_on(&mut h, 3, 1, "="), sent("name = 'b'"), "{cond}");
        page_with(&mut h, columns(), four_rows());
        clear_with(&mut h, columns(), four_rows());
    }
    // Through the history and the dialog: the same text, still the column's.
    select(&mut h, 2, 3);
    h.keys("!");
    runs(&mut h);
    page_with(&mut h, columns(), four_rows());
    h.keys("f");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), sent("(name NOT IN ('50%_off', 'b') OR name IS NULL)"));
    page_with(&mut h, columns(), four_rows());
    assert_eq!(menu(&mut h, 0, 1, "Clear filter on this column"), vec![vec![ALL.to_string()]]);
}

#[test]
fn a_sorted_tab_keeps_its_order() {
    let mut h = users();
    at(&mut h, 0, 1);
    h.keys("s");
    page(&mut h);
    h.keys("s");
    assert_eq!(runs(&mut h).len(), 2);
    page(&mut h);
    assert_eq!(key_on(&mut h, 0, 2, "="), vec![vec![format!(r#"{ALL} WHERE (score = 1.5) ORDER BY "name" DESC"#)]]);
}

#[test]
fn what_cannot_be_filtered_is_said_and_nothing_is_sent() {
    let mut h = users();
    let refused = |h: &mut Harness, row: usize, col: usize, key: &str, why: &str| {
        assert!(key_on(h, row, col, key).is_empty(), "{key} on {row},{col}: nothing sent");
        assert!(flashed(h).contains(why), "{key} on {row},{col}: {}", flashed(h));
        assert_eq!(where_text(h), "");
    };
    // JSON and a point have no equality the app knows of.
    refused(&mut h, 0, 4, "=", "how json compares is not known (“doc”");
    refused(&mut h, 0, 6, "!", "how point compares is not known");
    // Binary data over 64 KiB; bytes not shown in their hexadecimal form.
    let mut rs = rows();
    rs[0][3] = Some(format!(r"\x{}", "ab".repeat(64 * 1024 + 1)));
    clear_with(&mut h, columns(), rs);
    refused(&mut h, 0, 3, "=", "a value over 64 KiB");
    let mut rs = rows();
    rs[0][3] = Some(r"\001\253".into());
    clear_with(&mut h, columns(), rs);
    refused(&mut h, 0, 3, "!", "column “data”");
    clear(&mut h);
    // Cells of two columns; a cell with an edit.
    at(&mut h, 0, 0);
    h.keys("vl=");
    assert!(runs(&mut h).is_empty());
    assert!(flashed(&h).contains("select cells of one column"), "{}", flashed(&h));
    h.key(KeyCode::Esc);
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        let orig = rows()[0].clone();
        assert!(rs.edit.batch.set(&[(0, 1)], &datarig_core::edit::Value::Text("x".into()), |_| orig.clone()));
    }
    refused(&mut h, 0, 1, "=", "a selected cell shows an edit");
    // A query's rows: only a table tab.
    let mut h = with_edge_results(Lang::En);
    h.key(KeyCode::Tab);
    for key in ["=", "!", "X", "F"] {
        h.keys(key);
        assert!(!menu_open(&h));
        assert!(flashed(&h).contains("Only a table tab has a filter dialog"), "{key}: {}", flashed(&h));
    }
}

/// While the tab's run is on its way nothing is taken from the rows shown, which it replaces.
#[test]
fn nothing_is_taken_while_the_tab_runs() {
    let mut h = users();
    h.command("where id > 0");
    assert_eq!(runs(&mut h).len(), 1);
    for key in ["=", "!", "X"] {
        assert!(key_on(&mut h, 0, 0, key).is_empty(), "{key}");
        assert!(flashed(&h).contains("A query is already running"), "{key}: {}", flashed(&h));
    }
    page(&mut h);
    assert_eq!(key_on(&mut h, 0, 0, "="), sent("id = 1"));
}

#[test]
fn pending_edits_are_asked_about_first() {
    let mut h = users();
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        assert!(rs.edit.batch.delete(&[2], |_| Vec::new()));
    }
    assert!(key_on(&mut h, 0, 0, "=").is_empty(), "nothing runs while it asks");
    assert_eq!(
        h.app.overlays.confirm().map(|c| c.action),
        Some(ConfirmAction::LeaveEdits(datarig_tui::app::edit::Leave::Run(h.app.tab().id)))
    );
    // Enter keeps the edits: nothing runs, the filter is the draft, not applied.
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert!(matches!(&h.app.tab().results, Results::Rows(rs) if !rs.edit.batch.is_empty()));
    assert_eq!(where_text(&h), "id = 1");
    // `X` asks too; `y` drops the edits and runs it.
    h.command("where id > 0");
    h.keys("y");
    runs(&mut h);
    page(&mut h);
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        assert!(rs.edit.batch.delete(&[2], |_| Vec::new()));
    }
    assert!(key_on(&mut h, 0, 0, "X").is_empty());
    assert!(h.app.overlays.is_open(OverlayKind::Confirm));
    h.keys("y");
    assert_eq!(runs(&mut h), vec![vec![ALL.to_string()]]);
}

/// A tab whose filter calls a function the user let run, made read-only after: `=` checks the
/// whole filter again as any run does, and the read-only profile refuses the call; nothing is
/// sent.
#[test]
fn a_read_only_profile_refuses_what_the_filter_calls() {
    use datarig_core::config::Config;
    use datarig_core::policy::Policy;
    use datarig_core::profile::ConnectionConfig;
    let mut cfg = Config {
        connections: vec![ConnectionConfig { policy: Some("prod".into()), ..ConnectionConfig::test_db() }],
        ..Config::default()
    };
    cfg.policies.insert("prod", Policy::default());
    let mut h = Harness::with_driver(&cfg, Lang::En, FakeDriver::default());
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["shop".into()])));
    open_users(&mut h);
    keys_came_with(&mut h, "local-pg", collations(Some(true)));
    page(&mut h);
    h.sent();
    h.key(KeyCode::Tab);
    h.command("where lower2(name) = 'a'");
    assert!(h.app.overlays.is_open(OverlayKind::Confirm), "the call is asked about");
    h.keys("y");
    assert_eq!(runs(&mut h), sent("lower2(name) = 'a'"));
    page(&mut h);
    h.app.policies.insert("prod", Policy { read_only: true, ..Policy::default() });
    assert!(key_on(&mut h, 0, 0, "=").is_empty(), "nothing sent");
    assert!(!h.app.overlays.is_open(OverlayKind::Confirm), "not asked");
    let screen = h.screen(120, 34);
    assert!(screen.contains("Filter refused: lower2"), "{screen}");
}

/// `X`: no condition, no order, the limit kept; said when there is nothing to clear. `f` `u`
/// `Enter` brings back what was applied before it.
#[test]
fn clear_all_keeps_the_limit_and_the_dialog_undoes_it() {
    let mut h = users();
    assert!(key_on(&mut h, 0, 0, "X").is_empty());
    assert!(flashed(&h).contains("Nothing cleared"), "{}", flashed(&h));
    h.command("where id > 0 AND name IS NOT NULL");
    runs(&mut h);
    page(&mut h);
    at(&mut h, 0, 1);
    h.keys("s");
    runs(&mut h);
    page(&mut h);
    // The limit, through the dialog's field (a row budget: not in the grid's statement).
    at(&mut h, 0, 2);
    h.keys("f");
    while h.app.key_context() != Ctx::FilterDialogText {
        h.key(KeyCode::Tab);
    }
    h.ctrl('u');
    h.type_text("50");
    h.key(KeyCode::Enter);
    let before = format!(r#"{ALL} WHERE (id > 0 AND name IS NOT NULL) ORDER BY "name" ASC"#);
    assert_eq!(runs(&mut h), vec![vec![before.clone()]]);
    page(&mut h);
    let limit = |h: &Harness| h.app.tab().doc.table_query.as_ref().unwrap().applied().limit;
    assert_eq!(limit(&h), Some(50));
    assert_eq!(key_on(&mut h, 0, 0, "X"), vec![vec![ALL.to_string()]]);
    assert_eq!(limit(&h), Some(50), "the limit kept");
    page(&mut h);
    h.keys("f");
    h.keys("u");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![before]]);
    assert_eq!(limit(&h), Some(50));
    page(&mut h);
    // `Space r X` is the same key.
    assert_eq!(key_on(&mut h, 0, 0, " rX"), vec![vec![ALL.to_string()]]);
}

/// `=` and `!` are undone by the dialog as any applied change.
#[test]
fn the_dialog_undoes_a_grid_key() {
    let mut h = users();
    h.command("where id > 0");
    runs(&mut h);
    page(&mut h);
    assert_eq!(key_on(&mut h, 0, 1, "!"), sent("id > 0 AND name IS DISTINCT FROM 'it''s'"));
    page(&mut h);
    h.keys("fu");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), sent("id > 0"));
}

/// The column menu: the column's sort, the dialog, only and exclude the cursor's value, the
/// column's own conditions out; each runs what it showed.
#[test]
fn the_column_menu_entries_run_what_they_show() {
    let mut h = users();
    at(&mut h, 0, 1);
    h.keys("F");
    assert_eq!(
        lines(&h),
        [
            "# name",
            "↑ Sort ascending, NULLs last | s",
            "↓ Sort descending, NULLs first | ",
            "Clear sort | not sorted",
            "# ",
            "Filter and sort… | f",
            "Only 'it's' | =",
            "Exclude 'it's' | !",
            "Clear filter on this column | no condition",
            "# ",
            "Hide column | C",
        ]
    );
    h.key(KeyCode::Esc);
    assert_eq!(
        menu(&mut h, 0, 1, "↓ Sort descending, NULLs first"),
        vec![vec![format!(r#"{ALL} ORDER BY "name" DESC"#)]]
    );
    page(&mut h);
    at(&mut h, 0, 0);
    h.keys("S");
    page(&mut h);
    assert_eq!(runs(&mut h).len(), 1);
    // Out of the order: the other key stays.
    assert_eq!(menu(&mut h, 0, 1, "Clear sort"), vec![vec![format!(r#"{ALL} ORDER BY "id" ASC"#)]]);
    page(&mut h);
    // Sorted so already: dimmed, with why.
    at(&mut h, 0, 0);
    h.keys("F");
    assert!(h.app.menu_dimmed_rows().iter().any(|(l, w)| *l == "↑ Sort ascending, NULLs last" && w == "sorted so"));
    let why = h.app.menu_dimmed_notice("↑ Sort ascending, NULLs last").expect("dimmed").render(&h.app.i18n).to_string();
    assert!(why.contains("sorted by this column alone already"), "{why}");
    h.key(KeyCode::Esc);
    assert_eq!(
        menu(&mut h, 0, 1, "Only 'it's'"),
        vec![vec![format!(r#"{ALL} WHERE (name = 'it''s') ORDER BY "id" ASC"#)]]
    );
    page(&mut h);
    assert_eq!(
        menu(&mut h, 0, 0, "Exclude 1"),
        vec![vec![format!(r#"{ALL} WHERE (name = 'it''s' AND id IS DISTINCT FROM 1) ORDER BY "id" ASC"#)]]
    );
    page(&mut h);
    assert_eq!(
        menu(&mut h, 0, 1, "Clear filter on this column"),
        vec![vec![format!(r#"{ALL} WHERE (id IS DISTINCT FROM 1) ORDER BY "id" ASC"#)]]
    );
    page(&mut h);
    // The dialog on the column.
    at(&mut h, 0, 2);
    h.keys("F");
    h.menu_pick("Filter and sort…");
    assert!(h.app.overlays.is_open(OverlayKind::FilterDialog));
}

/// Entries that cannot apply are dimmed with why; picking one says it and sends nothing.
#[test]
fn the_column_menu_dims_what_cannot_apply() {
    let mut h = users();
    at(&mut h, 0, 4);
    h.keys("F");
    let dimmed = h.app.menu_dimmed_rows();
    let why: Vec<(String, String)> = dimmed.iter().map(|(l, w)| (l.to_string(), w.clone())).collect();
    assert!(why.contains(&("Only this value".to_string(), "type not compared".to_string())), "{why:?}");
    assert!(why.contains(&("Exclude this value".to_string(), "type not compared".to_string())), "{why:?}");
    let notice = h.app.menu_dimmed_notice("Only this value").expect("dimmed").render(&h.app.i18n).to_string();
    assert!(notice.contains("how json compares is not known"), "{notice}");
    h.key(KeyCode::Esc);
    // Two columns selected, a new row, an edited cell: said on the value entries.
    at(&mut h, 0, 0);
    h.app.tab_mut().grid.anchor = Some((0, 1));
    h.keys("F");
    assert!(h.app.menu_dimmed_rows().iter().any(|(l, w)| *l == "Only this value" && w == "several columns"));
    h.key(KeyCode::Esc);
    h.key(KeyCode::Esc);
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        let orig = rows()[0].clone();
        assert!(rs.edit.batch.set(&[(0, 1)], &datarig_core::edit::Value::Text("x".into()), |_| orig.clone()));
    }
    at(&mut h, 0, 1);
    h.keys("F");
    assert!(h.app.menu_dimmed_rows().iter().any(|(l, w)| *l == "Exclude this value" && w == "edited"));
    let why = h.app.menu_dimmed_notice("Only this value").expect("dimmed").render(&h.app.i18n).to_string();
    assert!(why.contains("a selected cell shows an edit"), "{why}");
    h.type_text("Only");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty(), "a dimmed entry runs nothing");
}

/// A right click on a table tab's header opens that column's menu (the cursor goes there); on
/// a cell, the grid's menu, whose entry opens the column menu.
#[test]
fn a_right_click_on_a_header_opens_its_column_menu() {
    let mut h = users();
    h.draw(120, 30);
    let g = h.app.tab().grid.clone();
    let x = g.hit_cols.iter().find(|c| c.2 == 1).unwrap().0 + 1;
    h.mouse(MouseEventKind::Down(MouseButton::Right), x, g.header_y);
    assert!(menu_open(&h));
    assert_eq!(lines(&h)[0], "# name");
    assert_eq!(h.app.tab().grid.col, 1);
    h.menu_pick("Only 'it's'");
    assert_eq!(runs(&mut h), sent("name = 'it''s'"));
    page(&mut h);
    h.draw(120, 30);
    let g = h.app.tab().grid.clone();
    let x = g.hit_cols.iter().find(|c| c.2 == 0).unwrap().0 + 1;
    h.mouse(MouseEventKind::Down(MouseButton::Right), x, g.data_y);
    assert!(lines(&h).contains(&"Table → this column's menu (sort, filter, hide) | F".to_string()), "{:?}", lines(&h));
    h.menu_pick("Table → this column's menu (sort, filter, hide)");
    assert_eq!(lines(&h)[0], "# id");
}

/// The value entry picked runs the condition the menu showed, for the cells it was opened on.
#[test]
fn a_menu_over_rows_read_again_runs_nothing() {
    let mut h = users();
    at(&mut h, 0, 0);
    h.keys("F");
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        rs.id += 1_000_000;
    }
    h.menu_pick("Only 1");
    assert!(runs(&mut h).is_empty(), "nothing runs");
    assert!(flashed(&h).contains("changed"), "{}", flashed(&h));
    assert_eq!(where_text(&h), "");
}

#[test]
fn the_column_menu_screens() {
    let shot = |h: &mut Harness, name: &str| {
        assert_screen!(format!("column_menu_{name}_en_80x24"), Lang::En, h.draw(80, 24));
        assert_screen!(format!("column_menu_{name}_en_120x34"), Lang::En, h.draw(120, 34));
        h.key(KeyCode::Esc);
    };
    let mut h = users();
    at(&mut h, 0, 1);
    h.keys("F");
    shot(&mut h, "cell");
    at(&mut h, 0, 4);
    h.keys("F");
    shot(&mut h, "refused");
}

/// A value with a line break goes into the filter as a literal on one line: `=` sends it, and
/// `Enter` in the filter dialog runs the same statement again.
#[test]
fn a_value_with_line_breaks_runs_as_one_line() {
    let mut h = users();
    let mut rs = rows();
    for (r, v) in ["a\nb", "a\rb", "a\r\nb"].into_iter().enumerate() {
        rs[r][1] = Some(v.to_string());
    }
    clear_with(&mut h, columns(), rs.clone());
    for (row, key, cond) in
        [(0, "=", r"name = E'a\nb'"), (1, "!", r"name IS DISTINCT FROM E'a\rb'"), (2, "=", r"name = E'a\r\nb'")]
    {
        assert_eq!(key_on(&mut h, row, 1, key), sent(cond));
        page_with(&mut h, columns(), rs.clone());
        h.keys("f");
        h.key(KeyCode::Enter);
        assert_eq!(runs(&mut h), sent(cond), "Enter in the dialog");
        page_with(&mut h, columns(), rs.clone());
        clear_with(&mut h, columns(), rs.clone());
    }
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
    h.tab_db(index, DbEvent::Connected);
    h.tab_db(index, DbEvent::SessionValues(SessionValues { time_zone: "+00:00".into(), charset: charset.into() }));
    page_with(&mut h, columns, rows);
    h.sent();
    h.key(KeyCode::Tab);
    h
}

#[test]
fn mysql_literals_follow_the_session() {
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
    assert_eq!(key_on(&mut h, 0, 1, "="), my("name = _utf8mb4 X'697427735C'"));
    page_with(&mut h, cols(), rows());
    clear_with(&mut h, cols(), rows());
    assert_eq!(key_on(&mut h, 0, 1, "!"), my("NOT (name <=> _utf8mb4 X'697427735C')"));
    page_with(&mut h, cols(), rows());
    clear_with(&mut h, cols(), rows());
    // Bytes as X'…'; a TIMESTAMP as shown with the offset of the zone it was read in, so
    // that it is the same instant in a session of another zone (run again from the history).
    assert_eq!(key_on(&mut h, 0, 2, "="), my("data = X'01AB'"));
    page_with(&mut h, cols(), rows());
    clear_with(&mut h, cols(), rows());
    assert_eq!(key_on(&mut h, 0, 4, "="), my("at = '2026-11-10 09:30:00+00:00'"));
    page_with(&mut h, cols(), rows());
    clear_with(&mut h, cols(), rows());
    // A MySQL FLOAT is sent rounded: not taken.
    assert!(key_on(&mut h, 0, 3, "=").is_empty());
    // The column menu's sorts say where this server puts NULLs: first going up.
    at(&mut h, 0, 1);
    h.keys("F");
    let shown = lines(&h);
    assert!(shown.contains(&"↑ Sort ascending, NULLs first | s".to_string()), "{shown:?}");
    assert!(shown.contains(&"↓ Sort descending, NULLs last | ".to_string()), "{shown:?}");
    h.key(KeyCode::Esc);
    assert!(flashed(&h).contains("column “w”"), "{}", flashed(&h));
    // Text read in another character set than utf8mb4: not taken.
    let mut h = mysql("latin1", cols(), rows());
    assert!(key_on(&mut h, 0, 1, "=").is_empty());
    assert!(flashed(&h).contains("column “name”"), "{}", flashed(&h));
}

/// A column whose name holds a line break, which a filter shown on one line cannot hold as it
/// is: `=` and `!` on it are refused, said; nothing is sent.
#[test]
fn a_column_named_with_a_line_break_is_not_filtered() {
    let mut h = users();
    let cols = vec![
        col("id", "int8", ValueKind::Integer),
        col("a\nb", "text", ValueKind::Text),
        col("a b", "text", ValueKind::Text),
    ];
    let s = |v: &str| Some(v.to_string());
    clear_with(&mut h, cols, vec![vec![s("1"), s("x"), s("y")], vec![s("2"), None, None]]);
    for (row, key) in [(0, "="), (1, "!")] {
        assert!(key_on(&mut h, row, 1, key).is_empty(), "{key}: nothing sent");
        assert!(flashed(&h).contains("a line break in the column name"), "{key}: {}", flashed(&h));
        assert_eq!(where_text(&h), "");
    }
    assert_eq!(key_on(&mut h, 0, 2, "="), vec![vec![format!(r#"{ALL} WHERE ("a b" = 'y')"#)]]);
}

/// An array compares for equality only where its element type has equality; NULL is tested
/// as NULL whatever the element type.
#[test]
fn arrays_compare_only_where_their_elements_have_equality() {
    use datarig_core::driver::ArrayElement as E;
    let mut h = users();
    let cols = vec![
        col("id", "int8", ValueKind::Integer),
        col("tags", "text[]", ValueKind::Array(E::Text)),
        col("docs", "xml[]", ValueKind::Array(E::Text)),
        col("odd", "tsquery[]", ValueKind::Array(E::Text)),
    ];
    let s = |v: &str| Some(v.to_string());
    let rows = vec![vec![s("1"), s("{a,b}"), s("{<a/>}"), s("{'a'}")], vec![s("2"), None, None, None]];
    clear_with(&mut h, cols.clone(), rows.clone());
    for c in [2, 3] {
        assert!(key_on(&mut h, 0, c, "=").is_empty(), "{c}: nothing sent");
        assert!(flashed(&h).contains("compares is not known"), "{c}: {}", flashed(&h));
    }
    assert_eq!(key_on(&mut h, 1, 2, "="), sent("docs IS NULL"));
    page_with(&mut h, cols.clone(), rows.clone());
    clear_with(&mut h, cols, rows);
    assert_eq!(key_on(&mut h, 0, 1, "="), sent("tags = '{a,b}'"));
}

/// The header of a column with a condition of its own or a sort key is tinted (the accent
/// blended into the surface; the accent as the text's color in the terminal's colors); one
/// only an expression names is not, nor are the others, and the cursor's header keeps its own.
#[test]
fn the_headers_of_filtered_and_sorted_columns_are_tinted() {
    use datarig_tui::theme;
    for (name, th) in [("dark", &theme::DARK), ("light", &theme::LIGHT), ("terminal", &theme::TERMINAL)] {
        let mut h = users();
        h.app.theme = std::sync::Arc::new(th.clone());
        h.command("where name IS NOT NULL AND lower(doc::text) <> 'x'");
        runs(&mut h);
        page(&mut h);
        at(&mut h, 0, 2);
        h.keys("s");
        runs(&mut h);
        page(&mut h);
        at(&mut h, 0, 0);
        let t = h.draw(160, 30);
        let g = h.app.tab().grid.clone();
        let buf = t.backend().buffer();
        let head = |c: usize| {
            let x = g.hit_cols.iter().find(|h| h.2 == c).expect("drawn").0 + 1;
            (buf[(x, g.header_y)].clone(), buf[(x, g.header_y + 1)].clone())
        };
        let tint = th.filtered_header();
        for c in [1, 2] {
            for cell in [head(c).0, head(c).1] {
                match tint.bg {
                    Some(bg) => {
                        assert_eq!(cell.bg, bg, "{name}: column {c}");
                        assert_ne!(bg, th.surface, "{name}: a tint apart from the surface");
                    }
                    None => assert_eq!(cell.fg, th.accent, "{name}: column {c}"),
                }
            }
        }
        // The expression's column, another column, and the cursor's.
        for c in [3, 4] {
            assert_eq!(head(c).0.bg, th.surface, "{name}: column {c} untinted");
        }
        if let Some(bg) = th.grid_cursor_header.bg {
            assert_eq!(head(0).0.bg, bg, "{name}: the cursor's header");
        }
        if let Some(bg) = tint.bg {
            assert_ne!(Some(bg), th.grid_cursor_header.bg, "{name}: apart from the cursor's header");
        }
    }
}

/// Columns hidden: a selection of whole rows (`V`) with one column shown is a selection of
/// that column; `=` and `!` take its values, NULL among them, not the hidden ones.
#[test]
fn a_row_selection_over_one_shown_column_filters_by_it() {
    let mut h = users();
    for c in [0, 2, 3, 4, 5, 6] {
        at(&mut h, 0, c);
        h.keys("C");
    }
    assert!(runs(&mut h).is_empty(), "hiding runs nothing");
    for (key, cond) in
        [("=", "(name IN ('it''s') OR name IS NULL)"), ("!", "(name NOT IN ('it''s') AND name IS NOT NULL)")]
    {
        at(&mut h, 0, 1);
        h.keys("Vj");
        h.keys(key);
        assert_eq!(runs(&mut h), sent(cond), "{key}: {}", flashed(&h));
        page(&mut h);
        clear(&mut h);
    }
}

/// `F` on a cell, and the grid menu's entry after a right click on a cell (the cursor on
/// another column before), open the column menu of that cell's column.
#[test]
fn the_column_menu_from_a_cell_is_of_that_cells_column() {
    let mut h = users();
    at(&mut h, 1, 2);
    h.keys("F");
    assert!(menu_open(&h));
    assert_eq!(lines(&h)[0], "# score");
    h.key(KeyCode::Esc);
    at(&mut h, 0, 0);
    h.draw(120, 30);
    let g = h.app.tab().grid.clone();
    let x = g.hit_cols.iter().find(|c| c.2 == 3).unwrap().0 + 1;
    h.mouse(MouseEventKind::Down(MouseButton::Right), x, g.data_y + 2);
    h.menu_pick("Table → this column's menu (sort, filter, hide)");
    assert_eq!(lines(&h)[0], "# data");
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (2, 3));
}

/// The column menu has no filter line (its few entries need none): `j`/`k` and the arrows
/// move, `Enter` runs the entry, and the keys shown next to the entries run them.
#[test]
fn the_column_menu_moves_by_keys_without_a_filter() {
    let mut h = users();
    at(&mut h, 0, 1);
    h.keys("F");
    let screen = h.screen(120, 34);
    assert!(!screen.contains("type to filter"), "{screen}");
    // `↓ Sort descending, NULLs first`, then back up and down again: `Enter` runs it.
    h.keys("jkj");
    h.key(KeyCode::Up);
    h.key(KeyCode::Down);
    h.key(KeyCode::Enter);
    assert!(!menu_open(&h));
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} ORDER BY \"name\" DESC")]]);
    page(&mut h);
    // `=` runs `Only`, as its key shows.
    at(&mut h, 0, 1);
    h.keys("F");
    h.keys("=");
    assert!(!menu_open(&h));
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (name = 'it''s') ORDER BY \"name\" DESC")]]);
}

/// The keys the column menu shows are the keys it runs: one key each, never one that moves
/// through the menu, none for an entry no single key runs (sorting descending). On a column
/// sorted ascending its entry is dimmed and its key says why, the menu staying open.
#[test]
fn the_column_menu_shows_only_keys_it_runs() {
    let keys_of = |h: &Harness| -> Vec<String> { lines(h).into_iter().filter(|l| !l.starts_with('#')).collect() };
    let mut h = users();
    at(&mut h, 0, 1);
    h.keys("s");
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} ORDER BY \"name\" ASC")]]);
    page(&mut h);
    at(&mut h, 0, 1);
    h.keys("F");
    let shown = keys_of(&h);
    assert!(shown.contains(&"↑ Sort ascending, NULLs last | sorted so".to_string()), "{shown:?}");
    assert!(shown.contains(&"↓ Sort descending, NULLs first | ".to_string()), "no key runs it: {shown:?}");
    h.keys("s");
    assert!(menu_open(&h), "the dimmed entry says why, the menu stays");
    assert!(runs(&mut h).is_empty());
    h.key(KeyCode::Esc);
    // Keys moved: `C` off, hiding on `j` (which moves in the menu), `=` on two keys.
    let mut cfg = test_db_config();
    cfg.keymap = datarig_core::config::parse(
        "[keymap.grid]\n\"C\" = \"none\"\n\"j\" = \"table.column.hide\"\n\"=\" = \"none\"\n\"g o\" = \"table.filter.only\"\n",
    )
    .unwrap()
    .keymap;
    let mut h = users_from(Harness::connected_with(&cfg, Lang::En), Some(collations(Some(true))));
    at(&mut h, 0, 1);
    h.keys("F");
    let shown = keys_of(&h);
    assert!(shown.contains(&"Hide column | ".to_string()), "{shown:?}");
    assert!(shown.contains(&"Only 'it's' | ".to_string()), "{shown:?}");
    let selected = h.app.overlays.menu().unwrap().selected;
    h.keys("j");
    assert!(menu_open(&h), "j moves");
    assert_ne!(h.app.overlays.menu().unwrap().selected, selected);
    assert_eq!(hidden_cols(&h), 0, "nothing hidden");
    h.keys("g");
    h.keys("o");
    assert!(menu_open(&h));
    assert!(runs(&mut h).is_empty(), "no two-key entries in the menu");
}

fn hidden_cols(h: &Harness) -> usize {
    h.app.tab().grid.hidden.iter().filter(|x| **x).count()
}

/// With a Korean input source the column menu's keys are the QWERTY keys at the same places
/// (the jamo at `j` moves as `j`, the one at `s` sorts as `s`); a menu that filters as it is typed in keeps the
/// Hangul typed.
#[test]
fn the_column_menu_takes_keys_typed_with_a_korean_input_source() {
    let mut h = users();
    at(&mut h, 0, 1);
    h.keys("F");
    let selected = h.app.overlays.menu().unwrap().selected;
    h.key(KeyCode::Char('\u{3153}'));
    assert!(menu_open(&h));
    assert_ne!(h.app.overlays.menu().unwrap().selected, selected, "moved down");
    h.key(KeyCode::Char('\u{314F}'));
    assert_eq!(h.app.overlays.menu().unwrap().selected, selected, "and up");
    h.key(KeyCode::Char('\u{3134}'));
    assert!(!menu_open(&h));
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} ORDER BY \"name\" ASC")]]);
    page(&mut h);
    // The grid's menu filters: the jamo is typed into its filter.
    h.app.dispatch(datarig_tui::app::action::Action::OpenMenu);
    assert!(menu_open(&h));
    h.key(KeyCode::Char('\u{3153}'));
    assert_eq!(h.app.overlays.menu().unwrap().filter.text(), "\u{3153}");
}

/// An entry's grid key that the menu's own keys take first (a key of the menu, or the start of
/// one) is neither shown nor run as the entry's: the menu's key keeps its meaning.
#[test]
fn the_column_menu_shows_no_key_its_own_keys_take() {
    for menu_keys in ["\"f2\" = \"help.context\"", "\"f2 f3\" = \"help.context\""] {
        let mut cfg = test_db_config();
        let text = format!(
            "[keymap.grid]\n\"f2\" = \"table.column.hide\"\n\n[keymap.\"overlay.context_menu\"]\n{menu_keys}\n"
        );
        cfg.keymap = datarig_core::config::parse(&text).unwrap().keymap;
        let mut h = users_from(Harness::connected_with(&cfg, Lang::En), Some(collations(Some(true))));
        at(&mut h, 0, 1);
        h.keys("F");
        let shown: Vec<String> = lines(&h).into_iter().filter(|l| l.starts_with("Hide")).collect();
        assert_eq!(shown, ["Hide column | C"], "{menu_keys}: the grid's other key, which the menu runs");
        h.key(KeyCode::F(2));
        assert_eq!(hidden_cols(&h), 0, "{menu_keys}: F2 is the menu's");
    }
}
