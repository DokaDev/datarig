//! The layout: a query tab shows its editor, and below it a results pane once
//! it ran (resized, hidden and maximised per tab, by keys and by dragging the divider); a table
//! opened from the explorer gets a tab of its own with the results at full height; tabs are
//! named by their document with the connection attached as a chip; the editor's first line
//! shows the connection; all of it survives a restart.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_core::i18n::Lang;
use datarig_core::paths::Paths;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_tui::app::tabs::PaneLayout;
use datarig_tui::app::{Focus, Startup};
use ratatui::crossterm::event::KeyCode;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Run the statement under the cursor and answer it with `n` one-column rows.
fn run_rows(h: &mut Harness, n: usize) {
    h.ctrl('e');
    answer(h, n);
}

/// Answer the active tab's run with `n` one-column rows.
fn answer(h: &mut Harness, n: usize) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let rows = (0..n).map(|i| vec![Some(i.to_string())]).collect();
    let columns = Some(vec![meta("id", "int8", true, false)]);
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: false, elapsed: Duration::from_millis(3) });
}

/// The statements of the `Execute` commands sent since the last look.
fn executed(h: &mut Harness) -> Vec<Vec<String>> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { statements, .. } => Some(statements),
            _ => None,
        })
        .collect()
}

/// From the editor, open `shop.users` in the explorer (the schema's objects answered).
fn open_users(h: &mut Harness) {
    h.key(KeyCode::BackTab); // explorer (the console has not run: no results pane)
    assert_eq!(h.app.focus, Focus::Tree);
    h.keys("jjjj"); // profile -> its database -> analytics -> public -> shop
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects {
        schema: "shop".into(),
        result: Ok((vec!["orders".to_string(), "users".to_string()], Vec::new()).into()),
    });
    h.keys("jjj"); // Tables, orders, users
    h.key(KeyCode::Enter);
}

#[test]
fn a_query_tab_shows_its_results_pane_once_it_ran() {
    let mut h = Harness::connected(Lang::En);
    h.draw(160, 45);
    assert_eq!(h.app.layout.results.height, 0, "no results pane before a run");
    let editor_full = h.app.layout.editor.height;
    assert_eq!(editor_full, 43, "the editor takes the tab");
    run_rows(&mut h, 3);
    h.draw(160, 45);
    assert!(h.app.layout.results.height > 0);
    assert_eq!(h.app.layout.editor.height + h.app.layout.results.height, editor_full);
    // The default share: 60% of the height for the results (rounded).
    assert_eq!(h.app.layout.results.height, 26);
    assert_eq!(h.app.layout.divider.y, h.app.layout.results.y);
    // Tab now cycles through the three panes.
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Results);
}

#[test]
fn the_results_pane_hides_maximises_and_resizes_by_keys() {
    let mut h = Harness::connected(Lang::En);
    run_rows(&mut h, 3);
    h.key(KeyCode::Tab); // results
    // Taller and shorter by a step, within the limits.
    h.keys("+");
    assert_eq!(h.app.tab().pane.share, 65);
    h.keys("----------");
    assert_eq!(h.app.tab().pane.share, PaneLayout::MIN);
    h.draw(160, 45);
    let small = h.app.layout.results.height;
    h.keys(" r+");
    h.draw(160, 45);
    assert!(h.app.layout.results.height > small);
    // Maximised: the editor is not drawn and cannot have the focus.
    h.keys("z");
    h.draw(160, 45);
    assert!(h.app.tab().pane.maximized);
    assert_eq!(h.app.layout.editor.height, 0);
    assert_eq!(h.app.layout.results.height, 43);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Tree, "Tab skips the editor that is not drawn");
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Results);
    // Esc goes back to the explorer (there is no editor on screen).
    h.key(KeyCode::Esc);
    assert_eq!(h.app.focus, Focus::Tree);
    h.key(KeyCode::Tab);
    h.keys("z");
    assert!(!h.app.tab().pane.maximized);
    // Hidden: the editor takes the height back and the focus goes to it; a run shows it again.
    h.keys(" rh");
    assert!(h.app.tab().pane.hidden);
    assert_eq!(h.app.focus, Focus::Editor);
    h.draw(160, 45);
    assert_eq!(h.app.layout.results.height, 0);
    h.keys(" rh");
    assert!(!h.app.tab().pane.hidden);
    h.keys(" rh");
    run_rows(&mut h, 2);
    assert!(!h.app.tab().pane.hidden, "a run shows the pane again");
    // The pane keys are actions: the command line runs them too.
    h.command("results pane maximise");
    assert!(h.app.tab().pane.maximized);
}

#[test]
fn dragging_the_divider_resizes_the_panes() {
    let mut h = Harness::connected(Lang::En);
    run_rows(&mut h, 3);
    h.draw(160, 45);
    let divider = h.app.layout.divider;
    let body = h.app.layout.body;
    // Up by five lines: the results grow.
    h.drag((divider.x + 20, divider.y), &[(divider.x + 20, divider.y - 2), (divider.x + 20, divider.y - 5)]);
    let rows = body.y + body.height - (divider.y - 5);
    assert_eq!(h.app.tab().pane.share, (u32::from(rows) * 100 / u32::from(body.height)) as u16);
    h.draw(160, 45);
    assert_eq!(h.app.layout.divider.y, divider.y - 5);
    // Far up: the largest share, never more.
    let d = h.app.layout.divider;
    h.drag((d.x + 20, d.y), &[(d.x + 20, 2)]);
    assert_eq!(h.app.tab().pane.share, PaneLayout::MAX);
    // Far down: the smallest share, never less.
    let d = h.app.layout.divider;
    h.drag((d.x + 20, d.y), &[(d.x + 20, 44)]);
    assert_eq!(h.app.tab().pane.share, PaneLayout::MIN);
    // A drag that starts in the editor selects text instead.
    let before = h.app.tab().pane.share;
    let e = h.app.layout.editor_text;
    h.drag((e.x + 2, e.y), &[(e.x + 2, e.y + 3)]);
    assert_eq!(h.app.tab().pane.share, before);
}

#[test]
fn a_small_terminal_keeps_both_panes_usable() {
    let mut h = Harness::connected(Lang::Ko);
    run_rows(&mut h, 30);
    h.key(KeyCode::Tab);
    h.keys("++++++++++");
    h.draw(80, 24);
    assert!(h.app.layout.editor.height >= datarig_tui::app::pane::MIN_EDITOR_ROWS);
    h.keys("----------");
    h.draw(80, 24);
    assert!(h.app.layout.results.height >= datarig_tui::app::pane::MIN_RESULTS_ROWS);
    // What the smallest pane shows, in the catalog's Korean: every line whole, the results
    // title, the column and the first rows.
    let i18n = datarig_core::i18n::I18n::new(Lang::Ko);
    let screen = h.screen(80, 24);
    let lines: Vec<&str> = screen.lines().collect();
    assert!(lines.iter().all(|l| datarig_tui::text::width(l) == 80), "{screen}");
    let title = i18n.msg(&datarig_core::i18n::Msg::PaneResultsRows { count: 30 }).to_string();
    let top = lines[h.app.layout.results.y as usize];
    assert!(top.contains(&title), "{top}");
    let grid =
        &lines[h.app.layout.results.y as usize + 1..(h.app.layout.results.y + h.app.layout.results.height) as usize];
    assert!(grid.iter().any(|l| l.contains("│ id ")), "{screen}");
    for row in ["1 │    0", "2 │    1", "3 │    2"] {
        assert!(grid.iter().any(|l| l.contains(row)), "{row}: {screen}");
    }
    let console = i18n.msg(&datarig_core::i18n::Msg::TabConsole { n: "1".into() }).to_string();
    assert!(lines[0].contains(&format!("1 {console} ×")), "{}", lines[0]);
}

#[test]
fn a_table_opens_in_a_tab_of_its_own() {
    let mut h = Harness::connected(Lang::En);
    let console = h.app.tab().id;
    open_users(&mut h);
    assert_eq!(executed(&mut h), [[r#"SELECT * FROM "shop"."users""#]]);
    assert_eq!(h.app.tabs.len(), 2);
    assert!(h.app.tab().is_table());
    assert_eq!(h.app.focus, Focus::Tree, "the explorer keeps the focus");
    // The results take the whole tab; no editor to go to.
    let (cols, rows) = edge_rows();
    let id = h.app.tab().exec.query_id;
    h.tab_db(1, DbEvent::Page { id, columns: Some(cols), rows, more: false, elapsed: Duration::from_millis(3) });
    h.draw(160, 45);
    assert_eq!(h.app.layout.editor.height, 0);
    assert_eq!(h.app.layout.results.height, 43);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Results);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Tree);
    let bar = h.screen(160, 45).lines().next().unwrap().to_string();
    assert!(bar.contains("1 console 1 ×") && bar.contains("2 shop.users ×"), "{bar}");
    insta::assert_snapshot!("table_tab_en_160x45", h.draw(160, 45).backend());
    insta::assert_snapshot!("table_tab_en_80x24", h.draw(80, 24).backend());
    // Opening it again goes to its tab: nothing runs again.
    h.keys(" 1");
    assert_eq!(h.app.tab().id, console);
    h.key(KeyCode::Enter);
    assert!(executed(&mut h).is_empty(), "no run");
    assert!(h.app.tab().is_table());
    // `Ctrl+E` reloads it.
    h.key(KeyCode::Tab);
    h.ctrl('e');
    assert_eq!(executed(&mut h), [[r#"SELECT * FROM "shop"."users""#]]);
    // A table belongs to its database: no other connection, and nothing to save.
    for id in ["tab.set_connection", "script.save", "script.save_as"] {
        let spec = datarig_tui::app::action::by_id(id).unwrap();
        assert!(!(spec.when)(&h.app), "{id} on a table tab");
    }
}

#[test]
fn tabs_are_named_by_their_document_with_the_connection_attached() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('t');
    h.ctrl('t');
    let bar = h.screen(160, 45).lines().next().unwrap().to_string();
    assert!(bar.contains("1 console 1 ×") && bar.contains("2 console 2 ×") && bar.contains("3 console 3 ×"), "{bar}");
    // A closed console's number is free again.
    h.keys(" 2");
    h.ctrl('w');
    h.ctrl('t');
    let bar = h.screen(160, 45).lines().next().unwrap().to_string();
    assert!(bar.contains("console 2 ×"), "{bar}");
    // The editor's first line: the connection, where it points, the policy, the switch key.
    let screen = h.screen(160, 45);
    let bar = screen.lines().nth(2).unwrap();
    assert!(bar.contains("local-pg / datarig  127.0.0.1:55432 · policy: default"), "{bar}");
    assert!(bar.contains("Space c s switch connection"), "{bar}");
    let mut ko = Harness::connected(Lang::Ko);
    let i18n = datarig_core::i18n::I18n::new(Lang::Ko);
    let console = i18n.msg(&datarig_core::i18n::Msg::TabConsole { n: "1".into() }).to_string();
    let switch = i18n.msg(&datarig_core::i18n::Msg::ConnbarSwitch { key: "Space c s".into() }).to_string();
    let wide = ko.screen(160, 45);
    assert!(wide.lines().next().unwrap().contains(&format!("1 {console} ×")), "{wide}");
    assert!(wide.lines().nth(2).unwrap().contains(&switch), "{wide}");
    // A narrow pane keeps the connection whole and leaves the key to which-key and the help.
    let narrow = ko.screen(80, 24);
    let bar = narrow.lines().nth(2).unwrap();
    assert!(bar.contains("local-pg / datarig  127.0.0.1:55432") && !bar.contains("Space c s"), "{bar}");
}

struct State(PathBuf);

impl Drop for State {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn launch(cfg: &Config, state: &State) -> Harness {
    let (mut app, clock) = new_app_with_clock(cfg, Lang::En);
    let store = Arc::new(MemoryStore::new());
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.set_paths(Paths { data: None, state: Some(state.0.clone()) });
    let h = Harness { app, cancelled: Default::default(), driver: FakeDriver::default(), store, clock };
    let mut h = h.with_fake_driver();
    h.app.launch(Startup::Normal);
    h
}

/// Tabs, consoles' numbers, table tabs and each pane's layout come back after a restart; a
/// table tab does not run by itself.
#[test]
fn the_layout_survives_a_restart() {
    let state = State(std::env::temp_dir().join(format!("datarig-layout-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&state.0);
    let cfg = test_db_config();
    let mut h = launch(&cfg, &state);
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["analytics".into(), "public".into(), "shop".into()])));
    h.key(KeyCode::Tab);
    h.keys("iselect 1");
    h.key(KeyCode::Esc);
    run_rows(&mut h, 3);
    h.key(KeyCode::Tab); // results
    h.keys("---");
    h.keys("z");
    open_users(&mut h);
    assert!(h.app.tab().is_table());
    answer(&mut h, 2);
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    assert!(h.app.quit);
    let text = std::fs::read_to_string(state.0.join("workspace.toml")).unwrap();
    assert!(text.contains("version = 3") && text.contains("kind = \"table\""), "{text}");
    drop(h);

    let mut h = launch(&cfg, &state);
    assert_eq!(h.app.tabs.len(), 2);
    let first = h.app.tabs.iter().next().unwrap();
    assert_eq!((first.doc.console_no, first.pane.share, first.pane.maximized), (1, 45, true));
    assert!(h.app.tab().is_table(), "the table tab is the active one");
    assert_eq!(h.app.tab().doc.table.as_ref().unwrap().label(), "shop.users");
    // Focusing it neither connects nor runs: it says how to load it.
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Results);
    assert!(h.driver.sessions.lock().unwrap().is_empty(), "nothing opened");
    let screen = h.screen(160, 45);
    assert!(screen.contains("Not loaded yet — Ctrl+E loads it"), "{screen}");
    h.ctrl('e');
    assert_eq!(h.app.conns.state(h.app.profiles[0].id), datarig_tui::app::NodeState::Connecting);
}

#[test]
fn a_closed_table_tab_leaves_no_console_file_and_comes_back() {
    let state = State(std::env::temp_dir().join(format!("datarig-layout-close-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&state.0);
    let mut h = launch(&test_db_config(), &state);
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["analytics".into(), "public".into(), "shop".into()])));
    h.key(KeyCode::Tab);
    open_users(&mut h);
    answer(&mut h, 2);
    h.key(KeyCode::Tab);
    h.ctrl('w');
    assert_eq!(h.app.tabs.len(), 1);
    let trash = state.0.join("consoles").join(".trash");
    let trashed = std::fs::read_dir(&trash).map(|d| d.count()).unwrap_or(0);
    assert_eq!(trashed, 0, "a table tab has no console to trash");
    h.keys(" tu");
    assert!(h.app.tab().is_table());
    assert_eq!(h.app.tab().doc.table.as_ref().unwrap().label(), "shop.users");
}

/// On a table tab `Ctrl+S` and `Space c s` say why they do nothing (a table tab has no text to
/// save and stays on its connection), instead of doing nothing silently.
#[test]
fn a_table_tab_says_why_it_cannot_be_saved_or_switched() {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    executed(&mut h);
    h.key(KeyCode::Tab); // results
    h.ctrl('s');
    let status = h.status(220, 45);
    assert!(status.contains("A table tab has no text to save"), "{status}");
    h.app.transient = None;
    h.keys(" cs");
    assert!(h.overlay_kind().is_none(), "no connection list");
    let status = h.status(220, 45);
    assert!(status.contains("A table tab stays on the connection it was opened from"), "{status}");
    assert!(h.sent().is_empty());
}

/// A tab's title is its number
/// (in the profile's color once connected, muted before, the spinner while it runs), its
/// document, `RO` for a read-only policy, a warning mark if any and `×`; no icon or letters of
/// the profile (its name is on the editor's first line). The marks are plain Unicode, the same
/// with icons on and off; a table tab keeps its table glyph with icons on. At 120 columns
/// everything is whole; at 80 the names give way first, and the active tab keeps its number
/// (here its spinner) and `×`. In English and Korean (read from the catalog).
#[test]
fn tab_titles_are_number_document_state_and_close() {
    use datarig_core::config::IconsSetting;
    for lang in [Lang::En, Lang::Ko] {
        let i18n = datarig_core::i18n::I18n::new(lang);
        let console = |n: &str| i18n.msg(&datarig_core::i18n::Msg::TabConsole { n: n.into() }).to_string();
        let mut h = Harness::connected(lang);
        h.ctrl('t');
        open_users(&mut h);
        let th = datarig_tui::theme::DARK;
        for icons in [IconsSetting::Off, IconsSetting::On] {
            h.app.icons = icons;
            let (bar, x0) = tab_bar_row(&mut h, 120, 30);
            assert!(bar.contains(&format!("1 {} ×", console("1"))), "{bar}");
            assert!(bar.contains(&format!("2 {} ×", console("2"))), "{bar}");
            let table = if icons == IconsSetting::On {
                format!("{} shop.users", datarig_tui::icons::TABLE)
            } else {
                "shop.users".to_string()
            };
            assert!(bar.contains(&format!(" ⠋ {table} ×")), "the table tab runs: {bar}");
            assert!(
                !bar.contains("local-pg") && !bar.contains("loc ") && !bar.contains('·'),
                "no name, no chip: {bar}"
            );
            let glyph = datarig_tui::icons::glyph(&h.app.profiles[0]);
            assert!(!bar.contains(glyph), "no profile icon: {bar}");
            // Not connected yet: the consoles' numbers muted; the table tab's spinner in the accent.
            let t = h.draw(120, 30);
            let buf = t.backend().buffer();
            for (n, color) in [("1", th.fg_muted), ("2", th.fg_muted), ("⠋", th.accent)] {
                let at = (x0..120).find(|x| buf[(*x, 0)].symbol() == n && buf[(*x + 1, 0)].symbol() == " ").unwrap();
                assert_eq!(buf[(at, 0)].fg, color, "{n}: {bar}");
            }
            // 80 columns: shorter names; the active (table) tab keeps its spinner and ×.
            let (bar, _) = tab_bar_row(&mut h, 80, 30);
            assert!(bar.contains(" ⠋ ") && !bar.contains(" 3 "), "{bar}");
            assert!(bar.matches('×').count() >= 2, "{bar}");
            assert!(datarig_tui::text::width(&bar) <= 80, "{bar}");
        }
    }
}

/// The tab bar row (right of the explorer) and the column it starts at.
fn tab_bar_row(h: &mut Harness, w: u16, rows: u16) -> (String, u16) {
    let t = h.draw(w, rows);
    let x0 = h.app.layout.tree.x + h.app.layout.tree.width;
    let buf = t.backend().buffer();
    let (mut s, mut x) = (String::new(), x0);
    while x < w {
        let sym = buf[(x, 0)].symbol();
        s.push_str(sym);
        x += datarig_tui::text::width(sym).max(1) as u16;
    }
    (s, x0)
}

/// A tab's database and schema come back after a restart (workspace version 3),
/// and its first run opens its session there.
#[test]
fn a_tabs_database_and_schema_survive_a_restart() {
    let state = State(std::env::temp_dir().join(format!("datarig-context-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&state.0);
    let cfg = test_db_config();
    let mut h = launch(&cfg, &state);
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    h.db(DbEvent::Connected);
    h.command("use sales.shop");
    let want = datarig_core::driver::SessionContext { database: Some("sales".into()), schema: Some("shop".into()) };
    assert_eq!(h.app.tab().context, want);
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    let text = std::fs::read_to_string(state.0.join("workspace.toml")).unwrap();
    assert!(text.contains("context = { database = \"sales\", schema = \"shop\" }"), "{text}");
    drop(h);
    let mut h = launch(&cfg, &state);
    assert_eq!(h.app.tab().context, want);
    h.key(KeyCode::Tab);
    h.keys("iselect 1");
    h.key(KeyCode::Esc);
    h.ctrl('e');
    h.db(DbEvent::Connected);
    let s = h.driver.sessions.lock().unwrap();
    let q = s.iter().find(|s| s.role == datarig_core::driver::SessionRole::Query).expect("a query session");
    assert_eq!(q.opts.context, want);
}

/// A table of another database opens in a table tab bound
/// to that database; the same table of the profile's own database is another tab. After a
/// restart it comes back in that database, named `database.schema.table`, not run, and its
/// first run opens its session there.
#[test]
fn a_table_tab_of_another_database_survives_a_restart() {
    use datarig_core::driver::{SessionContext, SessionRole};
    let state = State(std::env::temp_dir().join(format!("datarig-aux-table-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&state.0);
    let cfg = test_db_config();
    let mut h = launch(&cfg, &state);
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["analytics".into(), "public".into(), "shop".into()])));
    h.meta_db("local-pg", DbEvent::Databases(Ok(vec!["datarig".into(), "sales".into()])));
    let on = |h: &mut Harness, text: &str| {
        let rows = h.app.explorer_rows();
        let i = h.rows().iter().position(|r| r == text).unwrap_or_else(|| panic!("no {text:?} in {:?}", h.rows()));
        h.app.explorer.select(&rows, i);
    };
    let aux = |h: &mut Harness, ev: DbEvent| {
        let p = h.app.profiles[0].id;
        let a = h.app.conns.aux(p, "sales").expect("an aux session").id;
        h.app.on_app_event(datarig_tui::app::AppEvent::Db {
            target: datarig_tui::app::EventTarget::Aux(a),
            generation: a,
            ev,
        });
    };
    on(&mut h, "  db:sales");
    h.key(KeyCode::Char('l'));
    aux(&mut h, DbEvent::Connected);
    aux(&mut h, DbEvent::Schemas(Ok(vec!["shop".into()])));
    // Its `shop` (the own database's is above it).
    let rows = h.rows();
    let at = rows.iter().position(|r| r == "  db:sales").unwrap();
    let all = h.app.explorer_rows();
    h.app.explorer.select(&all, at + 1);
    assert_eq!(h.rows()[at + 1], "    shop");
    h.key(KeyCode::Char('l'));
    aux(&mut h, DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["users".into()], vec![]).into()) });
    let rows = h.rows();
    let at = rows.iter().position(|r| r == "  db:sales").unwrap();
    let users = rows[at..].iter().position(|r| r.trim() == "users").map(|i| at + i).expect("sales' users");
    let all = h.app.explorer_rows();
    h.app.explorer.select(&all, users);
    h.key(KeyCode::Enter);
    let sales = SessionContext { database: Some("sales".into()), schema: None };
    assert!(h.app.tab().is_table());
    assert_eq!(h.app.tab().context, sales);
    let other = h.app.tab().id;
    // The own database's `shop.users`: a tab of its own.
    assert_eq!(h.app.focus, Focus::Tree, "the focus stays in the explorer");
    on(&mut h, "  db:datarig*");
    open_users_here(&mut h);
    assert!(h.app.tab().is_table() && h.app.tab().id != other, "another tab");
    assert_eq!(h.app.tab().context, SessionContext::default());
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    drop(h);
    let mut h = launch(&cfg, &state);
    let back: Vec<_> = h.app.tabs.iter().filter(|t| t.is_table()).map(|t| t.context.clone()).collect();
    assert_eq!(back, [sales.clone(), SessionContext::default()], "both back, each in its database");
    let i = h.app.tabs.iter().position(|t| t.is_table() && t.context == sales).unwrap();
    h.app.tabs.activate(i);
    let screen = h.screen(160, 40);
    assert!(screen.contains("sales.shop.users"), "{screen}");
    assert!(executed(&mut h).is_empty(), "not run by itself");
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    h.db(DbEvent::Connected);
    h.app.tabs.activate(i);
    h.ctrl('e');
    let s = h.driver.sessions.lock().unwrap();
    let q = s.iter().rfind(|s| s.role == SessionRole::Query).expect("a query session");
    assert_eq!(q.opts.context, sales, "its session opens in that database");
}

/// From the explorer's cursor on the profile's own database: open `shop.users` (the schema's
/// objects answered).
fn open_users_here(h: &mut Harness) {
    h.keys("jjj"); // analytics -> public -> shop
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects {
        schema: "shop".into(),
        result: Ok((vec!["orders".to_string(), "users".to_string()], Vec::new()).into()),
    });
    h.keys("jjj"); // Tables, orders, users
    h.key(KeyCode::Enter);
}

/// A restored table tab is not loaded by the restore, a tab switch or a focus, but
/// opening its table from the explorer is the intent that loads a new table tab, so it loads
/// that one too. Once it ran, opening it again only goes to it (`Ctrl+E` reloads).
#[test]
fn opening_a_restored_table_tab_from_the_explorer_loads_it() {
    let state = State(std::env::temp_dir().join(format!("datarig-layout-reopen-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&state.0);
    let cfg = test_db_config();
    let connect = |h: &mut Harness| {
        h.explore("local-pg");
        h.key(KeyCode::Enter);
        h.db(DbEvent::Connected);
        h.db(DbEvent::Schemas(Ok(vec!["analytics".into(), "public".into(), "shop".into()])));
    };
    let mut h = launch(&cfg, &state);
    connect(&mut h);
    h.key(KeyCode::Tab);
    open_users(&mut h);
    answer(&mut h, 2);
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    drop(h);

    let mut h = launch(&cfg, &state);
    assert_eq!(h.app.tabs.len(), 2);
    let table = h.app.tab().id;
    assert!(h.app.tab().is_table());
    // Switching tabs and connecting load nothing.
    h.explore("local-pg");
    h.keys(" 1");
    h.keys(" 2");
    assert_eq!(h.app.tab().id, table);
    connect(&mut h);
    assert!(executed(&mut h).is_empty(), "nothing runs by itself");
    // `Enter` on the table in the explorer: its tab, loaded.
    h.keys(" 1");
    h.keys("jjjj"); // profile -> its database -> analytics -> public -> shop
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects {
        schema: "shop".into(),
        result: Ok((vec!["orders".to_string(), "users".to_string()], Vec::new()).into()),
    });
    h.keys("jjj"); // Tables, orders, users
    h.key(KeyCode::Enter);
    assert_eq!((h.app.tabs.len(), h.app.tab().id), (2, table), "the same tab");
    assert_eq!(h.app.focus, Focus::Tree, "the focus stays in the explorer");
    assert_eq!(executed(&mut h), [[r#"SELECT * FROM "shop"."users""#]]);
    // More rows than a page: they page in the driver's own transaction, not the user's.
    answer_paging(&mut h);
    assert_not_the_users_tx(&mut h);
    // Loaded: opening it again only goes to it.
    h.keys(" 1");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().id, table);
    assert!(executed(&mut h).is_empty(), "not run again");
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    assert!(h.app.quit, "quitting asks nothing");
}

/// Answer the active tab's run with a first page of 500 rows whose portal stays open, as the
/// driver reports it outside the user's block: its own transaction, then the page.
fn answer_paging(h: &mut Harness) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::TxOpen(true));
    let rows = (0..500).map(|i| vec![Some(i.to_string())]).collect();
    let columns = Some(vec![meta("id", "int8", true, false)]);
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: true, elapsed: Duration::from_millis(3) });
}

/// The active tab pages in a transaction that is not the user's: connected, no `◆`, no "TX open",
/// no label of the user's transaction, and nothing to ask about a rollback.
fn assert_not_the_users_tx(h: &mut Harness) {
    use datarig_tui::widgets::tabbar::{self, State};
    assert!(h.app.tab().exec.tx_open, "the server has the portal's transaction open");
    assert!(!h.app.tab().exec.in_block && !h.app.tab().exec.tx_at_risk());
    assert_eq!(tabbar::state(&h.app, h.app.tab()), State::Connected);
    let screen = h.screen(160, 45);
    let bar = screen.lines().next().unwrap();
    assert!(bar.contains("shop.users ×") && !bar.contains('◆'), "{bar}");
    let status = screen.lines().last().unwrap();
    assert!(!status.contains("TX open"), "{status}");
    assert!(!screen.contains("uncommitted (in tx)"), "{screen}");
    assert!(!h.app.any_tx_at_risk());
}

/// `Ctrl+E` on a restored table tab of a table with more rows
/// than a page does not look like an open transaction.
#[test]
fn a_restored_table_tab_loaded_with_ctrl_e_is_not_in_a_transaction() {
    let state = State(std::env::temp_dir().join(format!("datarig-layout-ctrl-e-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&state.0);
    let cfg = test_db_config();
    let mut h = launch(&cfg, &state);
    h.explore("local-pg");
    h.key(KeyCode::Enter);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["analytics".into(), "public".into(), "shop".into()])));
    h.key(KeyCode::Tab);
    open_users(&mut h);
    answer(&mut h, 2);
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    drop(h);

    let mut h = launch(&cfg, &state);
    assert!(h.app.tab().is_table());
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Results);
    h.ctrl('e');
    h.db(DbEvent::Connected);
    assert_eq!(executed(&mut h), [[r#"SELECT * FROM "shop"."users""#]]);
    answer_paging(&mut h);
    assert_not_the_users_tx(&mut h);
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    assert!(h.app.quit, "quitting asks nothing");
}

/// The same for a restored table tab of another database, which loads on that
/// database's session.
#[test]
fn opening_a_restored_table_tab_of_another_database_loads_it() {
    use datarig_core::driver::{SessionContext, SessionRole};
    let state = State(std::env::temp_dir().join(format!("datarig-aux-reopen-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&state.0);
    let cfg = test_db_config();
    let on = |h: &mut Harness, text: &str| {
        let rows = h.app.explorer_rows();
        let i = h.rows().iter().position(|r| r == text).unwrap_or_else(|| panic!("no {text:?} in {:?}", h.rows()));
        h.app.explorer.select(&rows, i);
    };
    let aux = |h: &mut Harness, ev: DbEvent| {
        let p = h.app.profiles[0].id;
        let a = h.app.conns.aux(p, "sales").expect("an aux session").id;
        h.app.on_app_event(datarig_tui::app::AppEvent::Db {
            target: datarig_tui::app::EventTarget::Aux(a),
            generation: a,
            ev,
        });
    };
    // Connect, open `sales` and its `shop`, and open (`Enter`) its `users`.
    let open_sales_users = |h: &mut Harness| {
        h.explore("local-pg");
        h.key(KeyCode::Enter);
        h.db(DbEvent::Connected);
        h.db(DbEvent::Schemas(Ok(vec!["analytics".into(), "public".into(), "shop".into()])));
        h.meta_db("local-pg", DbEvent::Databases(Ok(vec!["datarig".into(), "sales".into()])));
        on(h, "  db:sales");
        h.key(KeyCode::Char('l'));
        aux(h, DbEvent::Connected);
        aux(h, DbEvent::Schemas(Ok(vec!["shop".into()])));
        let at = h.rows().iter().position(|r| r == "  db:sales").unwrap();
        let all = h.app.explorer_rows();
        h.app.explorer.select(&all, at + 1);
        h.key(KeyCode::Char('l'));
        aux(h, DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["users".into()], vec![]).into()) });
        let rows = h.rows();
        let at = rows.iter().position(|r| r == "  db:sales").unwrap();
        let users = rows[at..].iter().position(|r| r.trim() == "users").map(|i| at + i).expect("sales' users");
        let all = h.app.explorer_rows();
        h.app.explorer.select(&all, users);
        h.key(KeyCode::Enter);
    };
    let sales = SessionContext { database: Some("sales".into()), schema: None };
    let mut h = launch(&cfg, &state);
    open_sales_users(&mut h);
    assert!(h.app.tab().is_table() && h.app.tab().context == sales);
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    drop(h);

    let mut h = launch(&cfg, &state);
    let table = h.app.tabs.iter().find(|t| t.is_table() && t.context == sales).expect("restored").id;
    open_sales_users(&mut h);
    assert_eq!(h.app.tab().id, table, "the restored tab");
    assert_eq!(h.app.tabs.iter().filter(|t| t.is_table()).count(), 1, "no second tab");
    assert_eq!(executed(&mut h), [[r#"SELECT * FROM "shop"."users""#]]);
    let s = h.driver.sessions.lock().unwrap();
    let q = s.iter().rfind(|s| s.role == SessionRole::Query).expect("a query session");
    assert_eq!(q.opts.context, sales, "loaded in that database");
}
