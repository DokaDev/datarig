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
use datarig_core::driver::{ColumnMeta, DbCommand, DbEvent, ValueKind};
use datarig_core::paths::Paths;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_core::table_view::{self as tv, TableKey};
use datarig_i18n::Lang;
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
/// the border says how many are hidden; the header is gone.
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
    assert!(screen.contains("2 hidden"), "{screen}");
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
