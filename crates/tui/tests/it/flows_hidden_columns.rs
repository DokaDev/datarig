//! Columns hidden from a table tab's grid: `C` (and the column menu, and `Space` / `C` in the
//! filter dialog's column list) hides one, a view only: nothing runs, the query stays
//! `SELECT *`. The grid steps over hidden columns (`h`, `l`, `0`, `$`), copies and searches
//! the shown ones, says how many are hidden on its border, and with every column hidden says
//! how to show one. They are kept per table in `<state>/table-view.toml`, across a restart; a
//! file that cannot be read is said once and never written over. The dialog's undo brings a
//! hidden column back.

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::keys::OrderKeys;
use datarig_core::driver::{ColumnMeta, DbCommand, DbEvent, ValueKind};
use datarig_core::edit::Value;
use datarig_core::paths::Paths;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_core::table_view::{self as tv, TableKey};
use datarig_i18n::Lang;
use datarig_tui::app::Results;
use datarig_tui::app::Startup;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::KeyCode;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const ALL: &str = r#"SELECT * FROM "shop"."users""#;

/// A state directory of the test's own, removed when it ends.
struct State(PathBuf);

impl State {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("datarig-table-view-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        State(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join(tv::FILE)
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn col(name: &str, ty: &str, kind: ValueKind) -> ColumnMeta {
    ColumnMeta::new(name.into(), ty.into(), kind, None)
}

fn columns() -> Vec<ColumnMeta> {
    vec![
        col("id", "int8", ValueKind::Integer),
        col("name", "text", ValueKind::Text),
        col("score", "float8", ValueKind::Float),
        col("memo", "text", ValueKind::Text),
    ]
}

fn rows() -> Vec<Vec<Option<String>>> {
    let s = |v: &str| Some(v.to_string());
    vec![
        vec![s("1"), s("ann"), s("1.5"), s("x")],
        vec![s("2"), s("bob"), s("2.5"), None],
        vec![s("3"), s("cy"), s("3.5"), s("y")],
    ]
}

fn launch(cfg: &Config, state: Option<&State>) -> Harness {
    let (mut app, clock) = new_app_with_clock(cfg, Lang::En);
    let store = Arc::new(MemoryStore::new());
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.set_paths(Paths { data: None, state: state.map(|s| s.0.clone()) });
    let h = Harness { app, cancelled: Default::default(), driver: FakeDriver::default(), store, clock };
    let mut h = h.with_fake_driver();
    h.app.launch(Startup::Normal);
    h
}

/// Answer the active tab's run with the rows.
fn page(h: &mut Harness) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Page { id, columns: Some(columns()), rows: rows(), more: false, elapsed: Duration::ZERO });
}

fn config() -> Config {
    Config { connections: vec![datarig_core::profile::ConnectionConfig::test_db()], ..Config::default() }
}

/// `shop.users` open in an app and answered, the grid focused, nothing sent since.
fn users(state: Option<&State>) -> Harness {
    users_on(&config(), state)
}

/// [`users`] of profile `cfg`, on `state`.
fn users_on(cfg: &Config, state: Option<&State>) -> Harness {
    let mut h = launch(cfg, state);
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["analytics".into(), "public".into(), "shop".into()])));
    h.explore("local-pg");
    h.keys("jjjj");
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["users".to_string()], Vec::new()).into()) });
    h.keys("jj");
    h.key(KeyCode::Enter);
    assert!(h.app.tab().is_table());
    page(&mut h);
    h.sent();
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h
}

/// The table as the file names it.
fn key(h: &Harness) -> TableKey {
    let p = &h.app.profiles[0];
    TableKey { profile: p.id, database: p.database.clone(), schema: "shop".into(), table: "users".into() }
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

fn at(h: &mut Harness, row: usize, col: usize) {
    let g = &mut h.app.tab_mut().grid;
    (g.row, g.col, g.anchor) = (row, col, None);
}

fn hidden(h: &Harness) -> Vec<bool> {
    h.app.tab().grid.hidden.clone()
}

fn flashed(h: &Harness) -> String {
    h.app.transient.as_ref().map(|(n, _)| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

/// `C` hides the cursor's column (nothing runs); the cursor and `h` `l` `0` `$` step over it;
/// the query bar says how many are shown of how many; the header is gone.
#[test]
fn c_hides_the_cursor_column_and_the_cursor_steps_over_it() {
    let mut h = users(None);
    at(&mut h, 0, 1);
    h.keys("C");
    assert!(runs(&mut h).is_empty(), "a view only: nothing runs");
    assert_eq!(hidden(&h), [false, true, false, false]);
    assert_eq!(h.app.tab().grid.col, 2, "the cursor on the next column shown");
    h.keys("h");
    assert_eq!(h.app.tab().grid.col, 0, "h steps over the hidden column");
    h.keys("l");
    assert_eq!(h.app.tab().grid.col, 2, "so does l");
    // The last column hidden too: `$` goes to the last one shown, `0` to the first.
    h.keys("$C");
    assert_eq!(hidden(&h), [false, true, false, true]);
    h.keys("0");
    assert_eq!(h.app.tab().grid.col, 0);
    h.keys("$");
    assert_eq!(h.app.tab().grid.col, 2);
    h.draw(200, 40);
    let g = h.app.tab().grid.clone();
    assert_eq!(g.hit_cols.iter().map(|c| c.2).collect::<Vec<_>>(), [0, 2], "only the shown columns drawn");
    let screen = h.screen(200, 40);
    assert!(screen.contains("2/4 cols"), "{screen}");
    // `Space r C` is the same key.
    at(&mut h, 0, 0);
    h.keys(" rC");
    assert_eq!(hidden(&h), [true, true, false, true]);
}

/// A copy takes the shown columns only.
#[test]
fn a_copy_takes_the_shown_columns_only() {
    let mut h = users(None);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    at(&mut h, 0, 1);
    h.keys("C");
    at(&mut h, 0, 0);
    h.keys("vjl");
    assert_eq!(h.app.tab().grid.col, 2);
    h.keys("y");
    assert_eq!(clip.last().as_deref(), Some("id\tscore\n1\t1.5\n2\t2.5"));
    at(&mut h, 2, 0);
    h.keys("Y");
    assert_eq!(clip.last().as_deref(), Some("3\t3.5\ty"));
}

/// A search looks through the shown columns: hiding one finds the matches again without it,
/// the cursor staying.
#[test]
fn a_search_is_found_again_over_the_columns_shown() {
    let mut h = users(None);
    h.keys("/");
    h.type_text("bob|y");
    h.key(KeyCode::Enter);
    h.app.pump_grid_search(Duration::from_secs(5));
    let count = |h: &Harness| h.app.grid_search_strip().map(|s| s.count).unwrap_or_default();
    assert!(count(&h).ends_with("/3"), "{}", count(&h));
    let cursor = (h.app.tab().grid.row, h.app.tab().grid.col);
    // Hide `name` (the column of `bob`) from the dialog-free key, on its header.
    at(&mut h, cursor.0, 1);
    h.keys("C");
    h.app.pump_grid_search(Duration::from_secs(5));
    assert!(count(&h).ends_with("/1"), "the name's matches gone: {}", count(&h));
    h.keys("n");
    h.app.pump_grid_search(Duration::from_secs(5));
    assert_eq!(h.app.tab().grid.col, 3, "n goes to a match of a shown column");
}

/// The dialog: `Space` on a column hides it (`Columns 4 · 1 hidden`, the row says `hidden`);
/// `Enter` with nothing else changed runs nothing; `f` `u` `Enter` shows it again.
#[test]
fn the_dialog_hides_and_its_undo_shows_again() {
    let mut h = users(None);
    at(&mut h, 0, 3);
    h.keys("f");
    assert!(h.app.overlays.is_open(OverlayKind::FilterDialog));
    h.keys(" ");
    let screen = h.screen(120, 34);
    assert!(screen.contains("Columns 4 · 1 hidden"), "{screen}");
    assert!(screen.contains("hidden from the grid, still read"), "{screen}");
    assert_screen!("hidden_dialog_en_120x34", Lang::En, h.draw(120, 34));
    assert_screen!("hidden_dialog_en_80x24", Lang::En, h.draw(80, 24));
    h.key(KeyCode::Enter);
    assert!(!h.app.overlays.is_open(OverlayKind::FilterDialog));
    assert!(runs(&mut h).is_empty(), "only hidden columns changed: nothing runs");
    assert_eq!(hidden(&h), [false, false, false, true]);
    h.keys("f");
    h.keys("u");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert_eq!(hidden(&h), [false, false, false, false], "shown again");
    // Hidden with a condition: the run sends it, and the column is hidden once it is sent.
    at(&mut h, 0, 3);
    h.keys("f");
    h.keys(" ");
    h.keys("Q");
    h.type_text("id > 1");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 1)")]]);
    page(&mut h);
    assert_eq!(hidden(&h), [false, false, false, true]);
    // One undo brings back both.
    h.keys("f");
    h.keys("u");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![ALL.to_string()]]);
    page(&mut h);
    assert_eq!(hidden(&h), [false, false, false, false]);
}

/// The column menu's entry hides it; with every column hidden the grid says how to show one.
#[test]
fn every_column_hidden_says_how_to_show_one() {
    let mut h = users(None);
    at(&mut h, 0, 0);
    h.keys("F");
    h.menu_pick("Hide column");
    assert_eq!(hidden(&h), [true, false, false, false]);
    for _ in 0..3 {
        h.keys("C");
    }
    assert_eq!(hidden(&h), [true, true, true, true]);
    let screen = h.screen(200, 40);
    assert!(screen.contains("Every column is hidden from the grid: f, then C on a column shows it"), "{screen}");
    assert_screen!("hidden_all_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("hidden_all_en_120x34", Lang::En, h.draw(120, 34));
}

/// The grid with a column hidden, a filtered and a sorted column (tinted headers), and the
/// border status; with icons too.
#[test]
fn the_hidden_columns_screens() {
    let mut h = users(None);
    h.command("where name <> 'x'");
    runs(&mut h);
    page(&mut h);
    at(&mut h, 0, 2);
    h.keys("s");
    runs(&mut h);
    page(&mut h);
    at(&mut h, 0, 3);
    h.keys("C");
    at(&mut h, 0, 0);
    assert_screen!("hidden_grid_en_120x34", Lang::En, h.draw(120, 34));
    assert_screen!("hidden_grid_en_80x24", Lang::En, h.draw(80, 24));
    h.app.icons = datarig_core::config::IconsSetting::On;
    assert_screen!("hidden_grid_icons_en_120x34", Lang::En, h.draw(120, 34));
}

/// Hidden columns are kept per table across a restart; an export says they are included.
#[test]
fn hidden_columns_are_kept_across_a_restart() {
    let state = State::new("restart");
    let cfg = config();
    {
        let mut h = users_on(&cfg, Some(&state));
        at(&mut h, 0, 1);
        h.keys("C");
        h.app.flush_table_view();
        let v = tv::load(&state.file()).expect("the file reads");
        assert_eq!(v.hidden(&key(&h)), ["name"]);
    }
    let mut h = users_on(&cfg, Some(&state));
    assert_eq!(hidden(&h), [false, true, false, false], "hidden again after a restart");
    h.keys(" re");
    assert!(h.app.overlays.is_open(OverlayKind::Export));
    let screen = h.screen(120, 34);
    assert!(screen.contains("1 column hidden from the grid is included"), "{screen}");
}

/// A file that cannot be read: said once, the columns hidden for the session only, and the
/// file never written over.
#[test]
fn an_unreadable_file_is_never_written_over() {
    let state = State::new("unreadable");
    let junk = "version = 1\n[[tables]\n";
    std::fs::write(state.file(), junk).unwrap();
    let mut h = users(Some(&state));
    let said = h.app.status_line().map(|(m, _, _)| m.to_string()).unwrap_or_default() + &flashed(&h);
    assert!(said.contains("Hidden columns kept for this session only"), "{said}");
    at(&mut h, 0, 1);
    h.keys("C");
    assert_eq!(hidden(&h), [false, true, false, false], "hidden for the session");
    h.app.flush_table_view();
    assert_eq!(std::fs::read_to_string(state.file()).unwrap(), junk, "never written over");
}

/// A column hidden while the search's job is still out: its answer is of other columns and is
/// dropped; the matches found are those of the columns shown.
#[test]
fn a_search_answer_of_other_columns_is_dropped() {
    let mut h = users(None);
    h.keys("/");
    h.type_text("bob|y");
    h.key(KeyCode::Enter);
    at(&mut h, 0, 1);
    h.keys("C");
    h.app.pump_grid_search(Duration::from_secs(5));
    let count = h.app.grid_search_strip().map(|s| s.count).unwrap_or_default();
    assert!(count.ends_with("/1"), "only the memo's match: {count}");
    assert_ne!(h.app.tab().grid.col, 1, "never on the hidden column");
}

/// `shop.users` open as a table tab from a connected app, its rows those of the table (`id`,
/// its primary key, then `email`, and `name` too when `name`, then `nickname`), the key-only read
/// answered; the grid focused, nothing sent since. Its edits are not resolved yet.
fn table_rows(name: bool) -> Harness {
    let mut h = Harness::connected(Lang::En);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    h.key(KeyCode::BackTab);
    h.goto("shop");
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["users".into()], Vec::new()).into()) });
    h.goto("users");
    h.key(KeyCode::Enter);
    assert!(h.app.tab().is_table());
    keys_came_with(&mut h, "local-pg", OrderKeys { primary_key: vec!["id".into()], ..Default::default() });
    let s = |v: &str| Some(v.to_string());
    let mut columns = vec![meta_from("id", "int8", true, USERS, 1), meta_from("email", "text", false, USERS, 2)];
    let mut rows = vec![vec![s("1"), s("a@x")], vec![s("2"), s("b@x")], vec![s("3"), s("c@x")]];
    if name {
        columns.push(meta_from("name", "text", false, USERS, 3));
        for (r, v) in rows.iter_mut().zip(["Ann", "Bob", "Cy"]) {
            r.push(s(v));
        }
    }
    columns.push(meta_from("nickname", "text", false, USERS, 4));
    for (r, v) in rows.iter_mut().zip([s("ann"), None, s("cy")]) {
        r.push(v);
    }
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Page { id, columns: Some(columns), rows, more: false, elapsed: Duration::ZERO });
    h.sent();
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h
}

/// The structure of `shop.users` answered (`name`, NOT NULL, has a default: new rows can be
/// added).
fn structure_came(h: &mut Harness) {
    let name = h.app.profiles[0].name.clone();
    let mut st = users_structure();
    st.columns[2].default = Some("''::text".into());
    h.meta_db(&name, DbEvent::Structure { schema: "shop".into(), table: "users".into(), result: Ok(Box::new(st)) });
}

/// [`table_rows`] without `name`, the result resolved for edits (new rows can be added); the
/// grid focused, nothing sent or said since.
fn editable() -> Harness {
    let mut h = table_rows(false);
    // `e` on the first row's email resolves the result; the editor closed again.
    at(&mut h, 0, 1);
    h.keys("e");
    structure_came(&mut h);
    assert_eq!(h.app.key_context(), Ctx::CellEdit);
    h.key(KeyCode::Esc);
    h.app.transient = None;
    h.sent();
    h
}

/// The value pending in row `r`, column `c` of the rows read.
fn pending(h: &Harness, r: usize, c: usize) -> Option<Value> {
    match &h.app.tab().results {
        Results::Rows(rs) => rs.edit.batch.value(r, c).cloned(),
        _ => None,
    }
}

/// The cells of the new rows, in order.
fn drafts(h: &Harness) -> Vec<Vec<Value>> {
    match &h.app.tab().results {
        Results::Rows(rs) => rs.edit.batch.drafts().iter().map(|d| d.cells.clone()).collect(),
        _ => Vec::new(),
    }
}

fn text(v: &str) -> Value {
    Value::Text(v.to_string())
}

/// With `copy_format = "insert"` and the key column hidden, `y` and `Y` keep the cells the grid
/// shows for `g p`: one cell alone (its value copied), a block and a row without the hidden
/// key, though their SQL INSERT names it. `g p` then lays them over the shown columns from the
/// cursor, the hidden key never shifted into a shown column.
#[test]
fn a_copy_keeps_the_shown_cells_for_g_p_while_its_sql_names_the_hidden_key() {
    let mut h = editable();
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.command("set copy_format=insert");
    h.app.transient = None;
    at(&mut h, 0, 0);
    h.keys("C");
    assert_eq!(hidden(&h), [true, false, false]);
    // One cell: its value, kept alone.
    at(&mut h, 0, 1);
    h.keys("y");
    assert_eq!(clip.last().as_deref(), Some("a@x"));
    at(&mut h, 1, 1);
    h.keys("gp");
    assert_eq!(pending(&h, 1, 1), Some(text("a@x")));
    assert_eq!(pending(&h, 1, 2), None, "one cell pasted, nothing in the next column");
    h.keys("u");
    // A block: the SQL names the key, the shown cells are kept.
    at(&mut h, 0, 1);
    h.keys("vjly");
    let sql = clip.last().unwrap_or_default();
    assert!(sql.contains(r#"("id", "email", "nickname")"#), "{sql}");
    at(&mut h, 2, 1);
    h.keys("gp");
    assert_eq!(pending(&h, 2, 1), Some(text("a@x")));
    assert_eq!(pending(&h, 2, 2), Some(text("ann")));
    assert_eq!(drafts(&h), vec![vec![Value::Default, text("b@x"), Value::Null]]);
    h.keys("u");
    // A row: the same.
    at(&mut h, 0, 2);
    h.keys("Y");
    let sql = clip.last().unwrap_or_default();
    assert!(sql.contains(r#"("id", "email", "nickname")"#), "{sql}");
    at(&mut h, 2, 1);
    h.keys("gp");
    assert_eq!(pending(&h, 2, 1), Some(text("a@x")));
    assert_eq!(pending(&h, 2, 2), Some(text("ann")));
    assert!(drafts(&h).is_empty(), "one row over one row");
}

/// The pending changes' revision of the active tab's rows.
fn revision(h: &Harness) -> u64 {
    match &h.app.tab().results {
        Results::Rows(rs) => rs.edit.batch.revision(),
        _ => 0,
    }
}

/// Every column hidden, the cursor last on a nullable column that is no key: no key reads or
/// writes a cell the grid does not show (the cell editor, NULL, DEFAULT, rows added, cloned or
/// deleted, a paste, a copy, `=`, `!`, the column menu, a sort, a selection); each says how to
/// show a column, and nothing changes, runs or is copied. The grid's menu offers nothing for a
/// cell. The dialog still opens and shows a column again, and then the keys act on it.
#[test]
fn with_every_column_hidden_no_key_reads_or_writes_a_cell() {
    let mut h = editable();
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    // A cell kept for `g p` and `p`.
    at(&mut h, 0, 2);
    h.keys("y");
    let copied = clip.texts.lock().unwrap().len();
    for c in [0, 1, 2] {
        at(&mut h, 0, c);
        h.keys("C");
    }
    assert_eq!(hidden(&h), [true, true, true]);
    h.sent();
    let before = revision(&h);
    for keys in
        ["e", " de", " dn", " dD", "o", "O", "D", " dr", "gp", "p", "y", "Y", " ryt", "=", "!", "F", "s", "C", "v", "V"]
    {
        h.app.transient = None;
        h.keys(keys);
        assert!(flashed(&h).starts_with("Every column is hidden from the grid"), "{keys}: {}", flashed(&h));
        assert_eq!(h.overlay_kind(), None, "{keys}: nothing opens");
        assert_eq!(h.app.key_context(), Ctx::Grid, "{keys}");
        assert_eq!(revision(&h), before, "{keys}: nothing changes");
        assert!(runs(&mut h).is_empty(), "{keys}: nothing runs");
        assert_eq!(clip.texts.lock().unwrap().len(), copied, "{keys}: nothing copied");
        assert_eq!(h.app.tab().grid.anchor, None, "{keys}: nothing selected");
    }
    // A terminal paste neither.
    h.app.transient = None;
    h.event(ratatui::crossterm::event::Event::Paste("z".into()));
    assert!(flashed(&h).starts_with("Every column is hidden from the grid"), "{}", flashed(&h));
    assert_eq!(revision(&h), before, "nothing pasted");
    // The grid's menu: no entry for a cell.
    h.app.dispatch(datarig_tui::app::action::Action::OpenMenu);
    let lines: Vec<String> = h.app.menu_lines().into_iter().map(|(_, l, _)| l.to_string()).collect();
    assert!(lines.iter().any(|l| l.contains("export")), "the pane's entries: {lines:?}");
    for word in ["NULL", "DEFAULT", "Edit", "Only", "Exclude", "Copy", "Select"] {
        assert!(!lines.iter().any(|l| l.contains(word)), "{word}: {lines:?}");
    }
    h.key(KeyCode::Esc);
    // The dialog shows the nickname again; then NULL is set on it.
    h.keys("f");
    assert!(h.app.overlays.is_open(OverlayKind::FilterDialog));
    h.keys(" ");
    h.key(KeyCode::Enter);
    assert_eq!(hidden(&h), [true, true, false]);
    assert_eq!(h.app.tab().grid.col, 2);
    h.keys(" dn");
    assert_eq!(pending(&h, 0, 2), Some(Value::Null));
}

/// A paste waiting for the table's structure (a terminal paste, `g p`, `space d v` once the
/// clipboard was read) while the columns shown change: when the structure comes, nothing is
/// pasted and that is said; the cells never move onto other columns.
#[test]
fn a_paste_waiting_for_the_table_is_dropped_when_the_columns_shown_change() {
    for how in ["paste", "gp", "space d v"] {
        let mut h = table_rows(true);
        let clip = FakeClipboard::attach(&mut h, false, &[]);
        let before = revision(&h);
        if how == "gp" {
            at(&mut h, 0, 1);
            h.keys("vly");
        }
        at(&mut h, 1, 1);
        match how {
            "paste" => h.event(ratatui::crossterm::event::Event::Paste("x@x\tXx".into())),
            "gp" => h.keys("gp"),
            _ => {
                clip.hold("x@x\tXx");
                h.app.prefs.clipboard = datarig_core::config::ClipboardSetting::System;
                h.keys(" dv");
                h.app.pump_clipboard_reads(Duration::from_secs(2));
            }
        }
        assert!(flashed(&h).contains("Loading"), "{how}: waits for the table: {}", flashed(&h));
        // `name` hidden meanwhile (the cursor stays on `email`).
        let n = 4;
        h.app.tab_mut().grid.set_hidden(vec![false, false, true, false], n);
        assert_eq!(h.app.tab().grid.col, 1);
        structure_came(&mut h);
        assert_eq!(revision(&h), before, "{how}: nothing pasted");
        assert_eq!((pending(&h, 1, 1), pending(&h, 1, 3)), (None, None), "{how}");
        assert_eq!(flashed(&h), "Paste cancelled: the columns shown changed", "{how}");
    }
}

/// `space d V` on a paste whose column was hidden since: refused before anything changes, the
/// paste staying as it was (pending, undone by `u`); with the column shown again it reads it
/// again.
#[test]
fn a_paste_read_again_after_its_column_was_hidden_stays_as_it_was() {
    let mut h = editable();
    at(&mut h, 0, 2);
    h.event(ratatui::crossterm::event::Event::Paste("zz".into()));
    assert_eq!(pending(&h, 0, 2), Some(text("zz")));
    h.keys("C");
    assert_eq!(hidden(&h), [false, false, true]);
    let before = revision(&h);
    h.app.transient = None;
    h.keys(" dV");
    assert_eq!(flashed(&h), "Paste cancelled: the columns shown changed");
    assert_eq!(pending(&h, 0, 2), Some(text("zz")), "the paste stays");
    assert_eq!(revision(&h), before, "nothing changed");
    h.keys("u");
    assert_eq!(pending(&h, 0, 2), None, "its step is still the one u undoes");
    h.ctrl('r');
    assert_eq!(pending(&h, 0, 2), Some(text("zz")), "and redoes");
}

/// Every column hidden, the note drawn: a click or a drag where the row numbers were selects
/// nothing.
#[test]
fn with_every_column_hidden_the_mouse_selects_nothing() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let mut h = users(None);
    h.draw(120, 34);
    let g = h.app.tab().grid.clone();
    let (x, y) = (g.gutter_x.0, g.data_y + 1);
    assert!(g.gutter_x.1 > g.gutter_x.0, "the row numbers were drawn");
    for _ in 0..4 {
        h.keys("C");
    }
    assert_eq!(hidden(&h), [true, true, true, true]);
    h.draw(120, 34);
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Drag(MouseButton::Left), x, y + 1);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y + 1);
    assert_eq!(h.app.tab().grid.anchor, None, "nothing selected");
    // A header's place neither.
    h.mouse(MouseEventKind::Down(MouseButton::Left), g.hit_cols[0].0 + 1, g.header_y);
    assert_eq!(h.app.tab().grid.anchor, None);
}

/// The inspector lists every column, the hidden ones too: `j` and `k` step onto a hidden one
/// (the grid's cell staying on a shown column), `Enter` and a second click on its entry show
/// its long value whole. The grid's own keys still act only on the cell it shows.
#[test]
fn the_inspector_reaches_a_hidden_column() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let mut h = users(None);
    let long = "a long memo ".repeat(30);
    h.app.dispatch(datarig_tui::app::action::Action::RunStatement);
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let mut rows = rows();
    rows[0][3] = Some(long.clone());
    h.tab_db(index, DbEvent::Page { id, columns: Some(columns()), rows, more: false, elapsed: Duration::ZERO });
    at(&mut h, 0, 3);
    h.keys("C");
    assert_eq!(hidden(&h), [false, false, false, true]);
    assert_eq!(h.app.tab().grid.col, 2);
    h.app.detail.visible = true;
    h.app.focus = datarig_tui::app::Focus::Inspector;
    h.keys("j");
    h.draw(160, 45);
    assert_eq!(h.app.tab().grid.inspected(), 3, "on the hidden memo");
    assert_eq!(h.app.tab().grid.col, 2, "the grid's cell stays on a shown column");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::CellViewer));
    assert_eq!(h.viewer().unwrap().text, long);
    h.key(KeyCode::Esc);
    h.keys("k");
    assert_eq!((h.app.tab().grid.inspected(), h.app.tab().grid.col), (2, 2));
    // A click on the memo's entry picks it; a second shows it whole.
    h.draw(160, 45);
    h.advance(datarig_tui::app::overlay::ARM_DELAY);
    let l = h.app.layout.detail_list;
    let y = l.y + (3 - h.app.detail.first) as u16;
    for _ in 0..2 {
        h.mouse(MouseEventKind::Down(MouseButton::Left), l.x + 3, y);
        h.mouse(MouseEventKind::Up(MouseButton::Left), l.x + 3, y);
        h.draw(160, 45);
    }
    assert_eq!(h.app.tab().grid.inspected(), 3);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::CellViewer));
    assert_eq!(h.viewer().unwrap().text, long);
    h.key(KeyCode::Esc);
    // The grid's keys: its cell, a shown one.
    h.key(KeyCode::Tab);
    h.app.focus = datarig_tui::app::Focus::Results;
    h.keys("C");
    assert_eq!(hidden(&h), [false, false, true, true], "C hides the grid's cell's column");
}

/// A change in a hidden column: `Enter` on its line in the Changes tab shows the column again
/// (said) with the cursor on it; an edit key or a revert there acts on that exact cell.
#[test]
fn a_change_in_a_hidden_column_is_reached_from_the_changes_tab() {
    let mut h = editable();
    at(&mut h, 0, 2);
    h.keys("e");
    h.type_text("zz");
    h.key(KeyCode::Enter);
    assert_eq!(pending(&h, 0, 2), Some(text("annzz")));
    let hide = |h: &mut Harness| {
        at(h, 0, 2);
        h.keys("C");
        assert_eq!(hidden(h), [false, false, true]);
    };
    hide(&mut h);
    h.command("changes");
    h.app.transient = None;
    h.key(KeyCode::Enter);
    assert_eq!(hidden(&h), [false, false, false], "shown again");
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (0, 2));
    assert_eq!(flashed(&h), "Column “nickname” is shown again to reach its change");
    // NULL from the Changes tab.
    hide(&mut h);
    h.command("changes");
    h.keys(" dn");
    assert_eq!(pending(&h, 0, 2), Some(Value::Null));
    assert_eq!(hidden(&h), [false, false, false]);
    // A revert from the Changes tab.
    hide(&mut h);
    h.command("changes");
    h.keys(" dr");
    assert_eq!(pending(&h, 0, 2), None, "reverted");
}

/// A hidden column picked in the inspector holds until the grid's cell is moved to a column:
/// after `0` and `$`, a click away and back, or a jump from the Changes tab, the inspector
/// shows the grid's cell again (Enter shows that value), and the grid's keys act on it.
#[test]
fn the_grid_moving_drops_the_inspectors_hidden_column() {
    use datarig_tui::app::Focus;
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let mut h = users(None);
    at(&mut h, 0, 3);
    h.keys("C");
    h.app.detail.visible = true;
    let pick_memo = |h: &mut Harness| {
        h.app.focus = Focus::Inspector;
        h.keys("j");
        assert_eq!((h.app.tab().grid.inspected(), h.app.tab().grid.col), (3, 2), "the hidden memo");
        h.app.focus = Focus::Results;
    };
    // `0` then `$`: back on `score`, which the inspector shows.
    pick_memo(&mut h);
    h.keys("0");
    assert_eq!((h.app.tab().grid.inspected(), h.app.tab().grid.col), (0, 0));
    h.keys("$");
    assert_eq!((h.app.tab().grid.inspected(), h.app.tab().grid.col), (2, 2));
    h.app.focus = Focus::Inspector;
    h.key(KeyCode::Enter);
    assert_eq!(h.viewer().unwrap().text, "1.5", "the value of score");
    h.key(KeyCode::Esc);
    // A click on `name`, then on `score`.
    pick_memo(&mut h);
    h.draw(160, 45);
    let g = h.app.tab().grid.clone();
    let x = |c: usize| g.hit_cols.iter().find(|h| h.2 == c).unwrap().0 + 1;
    for c in [1, 2] {
        h.mouse(MouseEventKind::Down(MouseButton::Left), x(c), g.data_y);
        h.mouse(MouseEventKind::Up(MouseButton::Left), x(c), g.data_y);
        h.advance(Duration::from_secs(1));
    }
    assert_eq!((h.app.tab().grid.inspected(), h.app.tab().grid.col), (2, 2));
    // The grid's key acts on its cell, `score`.
    h.keys("C");
    assert_eq!(hidden(&h), [false, false, true, true]);
}

/// A jump from the Changes tab back to the grid's column drops a hidden column the inspector
/// picked: the inspector shows the change's cell, and an edit key acts on it.
#[test]
fn a_changes_jump_drops_the_inspectors_hidden_column() {
    use datarig_tui::app::Focus;
    let mut h = editable();
    at(&mut h, 0, 1);
    h.keys("e");
    h.type_text("z");
    h.key(KeyCode::Enter);
    at(&mut h, 0, 2);
    h.keys("C");
    assert_eq!(h.app.tab().grid.col, 1);
    h.app.detail.visible = true;
    h.app.focus = Focus::Inspector;
    h.keys("j");
    assert_eq!((h.app.tab().grid.inspected(), h.app.tab().grid.col), (2, 1), "the hidden nickname");
    h.app.focus = Focus::Results;
    h.command("changes");
    h.key(KeyCode::Enter);
    assert_eq!((h.app.tab().grid.inspected(), h.app.tab().grid.col), (1, 1), "the email's change");
    // An edit key: the email's cell (`space d r` reverts its change).
    h.keys(" dr");
    assert_eq!(pending(&h, 0, 1), None, "the email's change reverted");
    assert_eq!(hidden(&h), [false, false, true], "the nickname still hidden");
}
