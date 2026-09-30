//! Saved queries, autosave, tab restore and the second instance through the real `App` event path, on temporary data and state
//! directories. A restart is simulated by dropping the `App` (without quitting, like a crash
//! or a kill) and making a new one on the same directories. The clock is fake, so the autosave
//! debounce is exact.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbEvent, SessionRole};
use datarig_core::i18n::{Label, Lang, Msg};
use datarig_core::paths::Paths;
use datarig_core::profile::{ConnectionConfig, ProfileId};
use datarig_core::scripts::ScriptStore;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::script_tree::TreeRow;
use datarig_tui::app::{AUTOSAVE, App, Focus, NodeState, Startup, TabKind};
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

struct Dirs {
    root: PathBuf,
}

impl Dirs {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("datarig-p6-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Dirs { root }
    }
    fn paths(&self) -> Paths {
        Paths { data: Some(self.root.join("data")), state: Some(self.root.join("state")) }
    }
    fn state(&self) -> PathBuf {
        self.root.join("state")
    }
    fn script(&self, rel: &str) -> PathBuf {
        ScriptStore::open(&self.root.join("data")).file(rel)
    }
    fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.script(rel)).ok()
    }
    fn consoles(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.state().join("consoles"))
            .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        v.sort();
        v
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Two profiles: `local-pg` and `other`.
fn config() -> Config {
    let other = ConnectionConfig { name: "other".into(), database: "postgres".into(), ..ConnectionConfig::test_db() };
    Config { connections: vec![ConnectionConfig::test_db(), other], ..Config::default() }
}

/// The app as the binary starts it, on `dirs`, with the fake driver (sessions are recorded,
/// the test feeds their events).
fn launch(cfg: &Config, dirs: &Dirs) -> Harness {
    let (mut app, clock) = new_app_with_clock(cfg, Lang::En);
    let store = Arc::new(MemoryStore::new());
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.set_paths(dirs.paths());
    let driver = FakeDriver::default();
    let fake = driver.clone();
    app.set_drivers(Arc::new(move |name: &str| {
        matches!(name, "postgres").then(|| Arc::new(fake.clone()) as Arc<dyn datarig_core::driver::Driver>)
    }));
    app.launch(Startup::Normal);
    Harness { app, cancelled: driver.any_cancel.clone(), driver, store, clock }
}

fn id(h: &Harness, name: &str) -> ProfileId {
    h.app.profiles.iter().find(|p| p.name == name).unwrap().id
}

/// Connect `name` from the explorer (its console opens), then put the focus in the editor.
fn connect(h: &mut Harness, name: &str) {
    h.explore(name);
    h.key(KeyCode::Enter);
    h.meta_db(name, DbEvent::Connected);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Editor);
}

/// Replace the active tab's text by typing (Normal mode: `i`, text, `Esc`).
fn type_sql(h: &mut Harness, text: &str) {
    h.keys("i");
    h.type_text(text);
    h.key(KeyCode::Esc);
}

/// Put the explorer's cursor on the row the harness shows as `text`.
fn select_row(h: &mut Harness, text: &str) {
    h.app.focus = Focus::Tree;
    let rows = h.app.explorer_rows();
    let shown = h.rows();
    let i = shown.iter().position(|r| r.trim() == text).unwrap_or_else(|| panic!("no row {text} in {shown:?}"));
    h.app.explorer.select(&rows, i);
}

/// `Ctrl+S` on a console: the tree dialog; `name` typed from the top of the saved queries.
fn save_as(h: &mut Harness, name: &str) {
    h.ctrl('s');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree), "a console asks for a name");
    tree_name(h, name);
    h.key(KeyCode::Enter);
}

/// The tree dialog's selection on the top, and `name` in its name field.
fn tree_name(h: &mut Harness, name: &str) {
    let t = h.app.overlays.script_tree_mut().expect("the tree dialog");
    t.row = TreeRow::Top;
    t.input.set(name);
}

/// Why the name input or the tree dialog did not take the name.
fn name_error(h: &Harness) -> Option<Label> {
    let tree = h.app.overlays.script_tree().and_then(|t| match &t.error {
        Some(Msg::Label(l)) => Some(*l),
        _ => None,
    });
    tree.or_else(|| h.app.overlays.name_input().and_then(|n| n.error))
}

#[test]
fn ctrl_s_turns_a_console_into_a_saved_query() {
    let dirs = Dirs::new("saveas");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    type_sql(&mut h, "select 42");
    let console = h.app.tab().doc.console_id.clone();
    h.advance(AUTOSAVE);
    assert_eq!(dirs.consoles(), [format!("{console}.sql")], "the console autosaved to the state directory");
    save_as(&mut h, "reports/売上 daily");
    assert!(h.overlay_kind().is_none(), "{:?}", name_error(&h));
    let path = "reports/売上 daily.sql";
    let text = h.app.tab().editor.text();
    assert_eq!(dirs.read(path).as_deref(), Some(text.as_str()), "the file holds the SQL only");
    assert_eq!((h.app.tab().kind, h.app.tab().script()), (TabKind::Script, Some(path)));
    let store = ScriptStore::open(&dirs.root.join("data"));
    assert_eq!(store.binding(path), Some(id(&h, "local-pg")), "bound by profile id in the index");
    assert!(dirs.consoles().is_empty(), "the console's own file went");
    let rows = h.rows();
    assert!(
        rows.contains(&"[saved queries] ".trim().to_string()) || rows.iter().any(|r| r.contains("[saved queries]"))
    );
    assert!(rows.iter().any(|r| r.trim() == "reports/") && rows.iter().any(|r| r.trim() == "売上 daily"), "{rows:?}");
    assert!(h.screen(120, 30).contains("Query · reports/売上 daily"), "the editor names it");
    // Ctrl+S on the saved query writes it at once.
    type_sql(&mut h, "-- more ");
    h.ctrl('s');
    assert!(h.overlay_kind().is_none());
    assert_eq!(dirs.read(path), Some(h.app.tab().editor.text()));
    // Another console: names are checked, a taken one (ignoring case) is refused.
    h.ctrl('t');
    type_sql(&mut h, "select 1");
    h.ctrl('s');
    for (typed, err) in [
        ("CON", Label::ValidateScriptReserved),
        ("a//b", Label::ValidateScriptEmptyPart),
        ("../x", Label::ValidateFolderDots),
        ("a:b", Label::ValidateScriptChar),
    ] {
        tree_name(&mut h, typed);
        h.key(KeyCode::Enter);
        assert_eq!(name_error(&h), Some(err), "{typed}");
    }
    let screen = h.screen(80, 24);
    assert!(screen.contains("these characters are not allowed"), "the error is shown: {screen}");
    // A name that exists (ignoring case): open in another tab, it is refused; otherwise it
    // asks before it is replaced, and Enter keeps it.
    tree_name(&mut h, &datarig_core::scripts::stem_path(path).to_uppercase());
    h.key(KeyCode::Enter);
    assert!(matches!(
        h.app.overlays.script_tree().and_then(|t| t.error.clone()),
        Some(Msg::ScriptsOpenElsewhere { .. })
    ));
    std::fs::write(dirs.root.join("data").join("scripts").join("other.sql"), "select 'other'").unwrap();
    tree_name(&mut h, "OTHER");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), "replacing asks");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree), "Enter keeps the file");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.tab().kind, TabKind::Console, "Esc keeps the console");
}

#[test]
fn autosave_writes_within_a_second_and_a_crash_loses_no_more() {
    let dirs = Dirs::new("autosave");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    save_as(&mut h, "q");
    type_sql(&mut h, "select 1;");
    let first = h.app.tab().editor.text();
    h.advance(AUTOSAVE - Duration::from_millis(1));
    assert_ne!(dirs.read("q.sql"), Some(first.clone()), "not yet");
    // Typing on does not push the save back: it happens a second after the first edit.
    type_sql(&mut h, " ");
    let second = h.app.tab().editor.text();
    h.advance(Duration::from_millis(1));
    assert_eq!(dirs.read("q.sql"), Some(second.clone()), "within the second");
    // An edit, then the app dies before its second is up: only that edit is lost.
    type_sql(&mut h, "select 'lost';");
    h.advance(Duration::from_millis(500));
    drop(h);
    let h = launch(&cfg, &dirs);
    assert_eq!(h.app.tab().script(), Some("q.sql"));
    assert_eq!(h.app.tab().editor.text(), second, "everything up to the last second is back");
    let leftovers: Vec<_> = std::fs::read_dir(dirs.root.join("data").join("scripts"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "no torn temporary files");
}

#[test]
fn restart_restores_tabs_without_connecting_until_a_tab_has_the_focus() {
    let dirs = Dirs::new("restore");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    save_as(&mut h, "reports/daily");
    connect(&mut h, "other");
    type_sql(&mut h, "select 'other console'");
    h.keys("0");
    let other_console = h.app.tab().editor.text();
    let cursor = (h.app.tab().editor.row, h.app.tab().editor.col);
    // Open and collapse some folders.
    select_row(&mut h, "reports/");
    h.key(KeyCode::Char('h'));
    assert!(!h.rows().iter().any(|r| r.trim() == "daily"), "folder collapsed");
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    assert!(h.app.quit);
    drop(h);

    let mut h = launch(&cfg, &dirs);
    let tabs: Vec<(Option<String>, Option<ProfileId>)> =
        h.app.tabs.iter().map(|t| (t.script().map(str::to_string), t.profile)).collect();
    assert_eq!(
        tabs,
        [(Some("reports/daily.sql".into()), Some(id(&h, "local-pg"))), (None, Some(id(&h, "other")))],
        "both tabs, each on its profile"
    );
    assert_eq!(h.app.tabs.active_index(), 1, "the active tab");
    assert_eq!(h.app.tab().editor.text(), other_console);
    assert_eq!((h.app.tab().editor.row, h.app.tab().editor.col), cursor);
    assert!(!h.rows().iter().any(|r| r.trim() == "daily"), "the folder is still collapsed");
    // Nothing connected, nothing asked.
    assert_eq!(h.app.focus, Focus::Tree);
    assert!(h.driver.sessions.lock().unwrap().is_empty(), "no session was opened");
    assert!(h.app.conns.attempt().is_none() && h.prompt().is_none());
    // Making a tab active from the explorer connects nothing.
    h.keys(" 1");
    assert_eq!(h.app.tabs.active_index(), 0);
    assert!(h.driver.sessions.lock().unwrap().is_empty(), "the explorer keeps the focus: no connect");
    // The editor gets the focus: that tab's profile connects, and only it.
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Editor);
    assert_eq!(h.app.conns.state(id(&h, "local-pg")), NodeState::Connecting);
    assert_eq!(h.app.conns.state(id(&h, "other")), NodeState::Disconnected);
    assert_eq!(h.roles(), [SessionRole::Meta]);
    // Running in the other tab connects its profile too (and runs once it is up).
    h.meta_db("local-pg", DbEvent::Connected);
    h.keys("gt");
    h.ctrl('e');
    assert_eq!(h.app.conns.state(id(&h, "other")), NodeState::Connecting);
}

#[test]
fn a_restored_tab_of_a_missing_script_or_profile() {
    let dirs = Dirs::new("missing");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    save_as(&mut h, "gone");
    connect(&mut h, "other");
    type_sql(&mut h, "select 'kept'");
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    drop(h);
    std::fs::remove_file(dirs.script("gone.sql")).unwrap();
    // `other` is deleted from the config.
    let fewer = Config { connections: vec![cfg.connections[0].clone()], ..Config::default() };
    let mut h = launch(&fewer, &dirs);
    assert_eq!(h.app.tabs.len(), 1, "the missing script's tab is dropped");
    assert_eq!(h.app.tab().profile, None, "a deleted profile: no connection");
    assert!(h.app.tab().editor.text().contains("select 'kept'"));
    let screen = h.screen(120, 30);
    assert!(screen.contains("The saved query gone.sql is gone"), "{screen}");
}

#[test]
fn a_script_opens_in_one_tab_only() {
    let dirs = Dirs::new("onetab");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    save_as(&mut h, "daily");
    h.ctrl('t');
    let n = h.app.tabs.len();
    select_row(&mut h, "daily");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tabs.len(), n, "no second tab");
    assert_eq!(h.app.tab().script(), Some("daily.sql"));
    assert_eq!(h.app.focus, Focus::Editor);
    h.ctrl('t');
    h.command("e DAILY");
    assert_eq!((h.app.tabs.len(), h.app.tab().script()), (n + 1, Some("daily.sql")), ":e goes there too");
    // Closed, it opens again bound to its last connection (from the index).
    h.ctrl('w');
    assert!(h.app.tabs.find_script("daily.sql").is_none());
    // The binding is by profile id: renaming the profile keeps it.
    let pid = id(&h, "local-pg");
    h.app.profiles.iter_mut().find(|p| p.id == pid).unwrap().name = "renamed-pg".into();
    select_row(&mut h, "daily");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().profile, Some(pid));
    h.command("e nope");
    assert!(h.screen(80, 24).contains("No saved query named nope"));
}

#[test]
fn rename_move_and_delete_from_the_explorer() {
    let dirs = Dirs::new("ops");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    save_as(&mut h, "a/q1");
    h.ctrl('t');
    save_as(&mut h, "b/q2");
    let pid = id(&h, "local-pg");
    // R: rename (and move with a /); the open tab follows, so does the binding.
    select_row(&mut h, "q1");
    h.keys("R");
    assert_eq!(h.app.overlays.name_input().unwrap().input.text(), "a/q1");
    h.app.overlays.name_input_mut().unwrap().input.set("a/renamed");
    h.key(KeyCode::Enter);
    assert!(h.overlay_kind().is_none());
    assert!(dirs.read("a/q1.sql").is_none() && dirs.read("a/renamed.sql").is_some());
    assert!(h.app.tabs.find_script("a/renamed.sql").is_some(), "the tab points to the new file");
    assert_eq!(ScriptStore::open(&dirs.root.join("data")).binding("a/renamed.sql"), Some(pid));
    // m: move to another folder.
    select_row(&mut h, "renamed");
    h.keys("m");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Chooser));
    let target = h.app.overlays.chooser().unwrap().items.iter().position(|(v, _)| v.as_deref() == Some("b")).unwrap();
    h.app.overlays.chooser_mut().unwrap().selected = target;
    h.key(KeyCode::Enter);
    assert!(dirs.read("b/renamed.sql").is_some() && dirs.read("a/renamed.sql").is_none());
    assert!(h.app.tabs.find_script("b/renamed.sql").is_some());
    // An empty folder can go.
    select_row(&mut h, "a/");
    h.keys("d");
    h.keys("y");
    assert!(!dirs.root.join("data").join("scripts").join("a").exists());
    // d: delete after a confirmation; the open tab closes, Space t u brings its text back.
    let tab = h.app.tabs.find_script("b/renamed.sql").unwrap();
    let deleted_text = h.app.tabs.get(tab).unwrap().editor.text();
    select_row(&mut h, "renamed");
    h.keys("d");
    assert!(h.screen(120, 30).contains("Delete the saved query “b/renamed”?"));
    h.keys("y");
    assert!(dirs.read("b/renamed.sql").is_none());
    assert!(h.app.tabs.find_script("b/renamed.sql").is_none());
    h.keys(" tu");
    assert_eq!(h.app.tab().kind, TabKind::Console, "back as a console");
    assert_eq!(h.app.tab().editor.text(), deleted_text);
}

#[test]
fn reopening_a_closed_script_reads_it_again() {
    let dirs = Dirs::new("reopen");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    save_as(&mut h, "r");
    h.ctrl('t');
    h.command("e r");
    h.ctrl('w');
    std::fs::write(dirs.script("r.sql"), "select 'changed outside'").unwrap();
    h.keys(" tu");
    assert_eq!(h.app.tab().script(), Some("r.sql"));
    assert_eq!(h.app.tab().editor.text(), "select 'changed outside'");
}

#[test]
fn write_commands() {
    let dirs = Dirs::new("cmds");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    type_sql(&mut h, "select 'w'");
    h.command("w named");
    assert_eq!(h.app.tab().script(), Some("named.sql"));
    assert!(dirs.read("named.sql").is_some_and(|t| t.contains("select 'w'")));
    type_sql(&mut h, "x");
    h.command("w");
    assert!(dirs.read("named.sql").is_some_and(|t| t.contains('x')), ":w saves a script at once");
    h.ctrl('t');
    h.command("w");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree), ":w on a console asks for a name");
    h.key(KeyCode::Esc);
    h.ctrl('t');
    let n = h.app.tabs.len();
    assert_eq!(n, 3);
    h.command("wq closed");
    assert!(dirs.read("closed.sql").is_some());
    assert_eq!(h.app.tabs.len(), n - 1, ":wq saves, then closes the tab");
    h.command("q");
    assert_eq!(h.app.tabs.len(), n - 2, ":q closes the tab");
    assert!(!h.app.quit);
    h.command("q");
    assert!(h.app.quit, ":q on the last tab quits");
    // The console `:wq` asked a name for, closed after saving (another tab is open, so `:wq`
    // closes rather than quits).
    let mut h = launch(&cfg, &dirs);
    h.ctrl('t');
    h.ctrl('t');
    h.command("wq");
    h.type_text("asked");
    h.key(KeyCode::Enter);
    assert!(dirs.read("asked.sql").is_some());
    assert!(h.app.tabs.find_script("asked.sql").is_none(), "closed after the name was given");
}

/// Hold the lock like a first instance does, launch a second one on the same directories.
#[test]
fn a_second_instance_is_workspace_read_only_and_scripts_check_for_changes() {
    let dirs = Dirs::new("second");
    let cfg = config();
    let mut first = launch(&cfg, &dirs);
    connect(&mut first, "local-pg");
    save_as(&mut first, "shared");
    first.app.dispatch(datarig_tui::app::action::Action::NewTab);
    let ws = dirs.state().join("workspace.toml");
    let before = std::fs::read_to_string(&ws).unwrap();

    let mut second = launch(&cfg, &dirs);
    assert!(second.app.read_only);
    assert!(second.app.tabs.is_empty(), "nothing restored");
    let screen = second.screen(120, 30);
    assert!(screen.lines().next().unwrap().contains("Another datarig is running"), "{screen}");
    // Its tabs and consoles are never written.
    connect(&mut second, "local-pg");
    type_sql(&mut second, "select 'second'");
    second.advance(AUTOSAVE);
    second.ctrl('t');
    second.app.dispatch(datarig_tui::app::action::Action::Quit);
    assert_eq!(std::fs::read_to_string(&ws).unwrap(), before, "workspace.toml untouched");
    assert!(
        dirs.consoles()
            .iter()
            .all(|c| { !std::fs::read_to_string(dirs.state().join("consoles").join(c)).unwrap().contains("second") })
    );
    // Saved queries still save, with the change check.
    let mut second = launch(&cfg, &dirs);
    select_row(&mut second, "shared");
    second.key(KeyCode::Enter);
    type_sql(&mut second, "select 'from second';");
    second.ctrl('s');
    assert!(dirs.read("shared.sql").unwrap().contains("from second"), "a second instance saves scripts");
    // The first instance still has the old text: its save asks instead of overwriting.
    first.app.dispatch(datarig_tui::app::action::Action::PrevTab);
    type_sql(&mut first, "select 'from first';");
    first.advance(AUTOSAVE);
    assert_eq!(first.overlay_kind(), Some(OverlayKind::Confirm));
    assert!(first.screen(120, 30).contains("changed on disk"));
    assert!(dirs.read("shared.sql").unwrap().contains("from second"), "not overwritten");
    assert!(first.screen(120, 30).contains("shared *"), "the tab shows it is not saved");
    // r: reload what the other instance wrote.
    first.keys("r");
    assert!(first.app.tab().editor.text().contains("from second"));
    // Changed again; o: overwrite.
    std::fs::write(dirs.script("shared.sql"), "select 'third';").unwrap();
    type_sql(&mut first, "-- mine ");
    first.ctrl('s');
    assert_eq!(first.overlay_kind(), Some(OverlayKind::Confirm));
    first.keys("o");
    assert!(dirs.read("shared.sql").unwrap().contains("mine"));
    // Esc: later; quitting then asks before the edits are lost.
    std::fs::write(dirs.script("shared.sql"), "select 'fourth';").unwrap();
    type_sql(&mut first, "-- again ");
    first.ctrl('s');
    first.key(KeyCode::Esc);
    first.ctrl('q');
    assert!(first.screen(120, 30).contains("Quitting loses them"));
    first.keys("n");
    assert!(!first.app.quit);
    drop(second);
}

#[test]
fn without_a_state_directory_nothing_is_written() {
    // The headless harness has no directories: no scripts section, Ctrl+S says why.
    let mut h = Harness::connected(Lang::En);
    assert!(!h.rows().iter().any(|r| r.contains("[saved queries]")));
    h.ctrl('s');
    assert!(h.overlay_kind().is_none());
    assert!(h.screen(120, 30).contains("Saved queries need a data directory"));
    let _ = (Path::new("."), KeyModifiers::NONE, App::new(&Config::default(), None, Lang::En).read_only);
}

// ── consoles are never lost to a workspace file that cannot be read ─────────

/// Two console files from an earlier run, and `workspace` (when given) as `workspace.toml`.
fn leave_consoles(dirs: &Dirs, workspace: Option<&str>) -> Vec<(String, String)> {
    let consoles = vec![
        ("aaaa1111".to_string(), "select 'user text one';".to_string()),
        ("bbbb2222".to_string(), "select 'user text two';".to_string()),
    ];
    let dir = dirs.state().join("consoles");
    std::fs::create_dir_all(&dir).unwrap();
    for (id, text) in &consoles {
        std::fs::write(dir.join(format!("{id}.sql")), text).unwrap();
    }
    if let Some(w) = workspace {
        std::fs::write(dirs.state().join("workspace.toml"), w).unwrap();
    }
    consoles
}

fn recovered_notice(h: &Harness) -> Option<u64> {
    h.app.notices.iter().find_map(|n| match &n.msg {
        Msg::WorkspaceConsolesRecovered { count } => Some(*count),
        _ => None,
    })
}

/// Every console came back as a tab with its text, and every file is still there.
fn assert_all_recovered(mut h: Harness, dirs: &Dirs, consoles: &[(String, String)], what: &str) {
    let texts: Vec<String> = h.app.tabs.iter().map(|t| t.editor.text()).collect();
    for (id, text) in consoles {
        assert!(texts.contains(text), "{what}: {text} is back in a tab: {texts:?}");
        let file = dirs.state().join("consoles").join(format!("{id}.sql"));
        assert_eq!(std::fs::read_to_string(&file).ok().as_deref(), Some(text.as_str()), "{what}: {id} kept");
    }
    assert_eq!(recovered_notice(&h), Some(consoles.len() as u64), "{what}: said how many");
    let screen = h.screen(160, 30);
    assert!(screen.contains("recovered"), "{what}: the tabs say so\n{screen}");
    assert!(h.app.tabs.iter().all(|t| t.profile.is_none()), "{what}: no connection");
    // They are listed now: a normal quit and restart keeps them as ordinary tabs.
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    assert!(h.app.quit, "{what}");
    drop(h);
    let h2 = launch(&config(), dirs);
    let texts: Vec<String> = h2.app.tabs.iter().map(|t| t.editor.text()).collect();
    for (_, text) in consoles {
        assert!(texts.contains(text), "{what}: still there after a restart: {texts:?}");
    }
    assert_eq!(recovered_notice(&h2), None, "{what}: nothing to recover the second time");
}

#[test]
fn a_broken_empty_or_missing_workspace_file_recovers_every_console() {
    for (tag, ws) in [
        ("garbage", Some("this is = = not toml [[[\n")),
        ("empty", Some("")),
        ("missing", None),
        ("truncated", Some("version = 1\nactive = 0\n[[tabs]]\nid = \"aaaa1111\"\nkind = \"con")),
    ] {
        let dirs = Dirs::new(&format!("recover-{tag}"));
        let consoles = leave_consoles(&dirs, ws);
        let h = launch(&config(), &dirs);
        assert_all_recovered(h, &dirs, &consoles, tag);
    }
}

#[test]
fn a_partly_understood_workspace_file_keeps_the_consoles_of_skipped_tabs() {
    let dirs = Dirs::new("recover-partial");
    let ws = "version = 1\nactive = 0\n[[tabs]]\nid = \"aaaa1111\"\nkind = \"console\"\ncursor = [0, 3]\n\
              [[tabs]]\nid = \"bbbb2222\"\nkind = \"notebook\"\n";
    let consoles = leave_consoles(&dirs, Some(ws));
    let h = launch(&config(), &dirs);
    assert_eq!(h.app.tabs.len(), 2, "the listed tab and the recovered one");
    assert_eq!(h.app.tabs.iter().next().unwrap().editor.text(), consoles[0].1, "the listed tab comes back first");
    assert!(!h.app.tabs.iter().next().unwrap().doc.recovered);
    assert_eq!(recovered_notice(&h), Some(1));
    assert!(h.app.tabs.iter().any(|t| t.doc.recovered && t.editor.text() == consoles[1].1));
    for (id, _) in &consoles {
        assert!(dirs.state().join("consoles").join(format!("{id}.sql")).exists());
    }
}

#[cfg(unix)]
#[test]
fn an_unreadable_workspace_file_recovers_every_console() {
    use std::os::unix::fs::PermissionsExt;
    let dirs = Dirs::new("recover-perm");
    let ws = "version = 1\nactive = 0\n[[tabs]]\nid = \"aaaa1111\"\nkind = \"console\"\n";
    let consoles = leave_consoles(&dirs, Some(ws));
    let file = dirs.state().join("workspace.toml");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    let h = launch(&config(), &dirs);
    let bak = dirs.state().join("workspace.toml.bak");
    let _ = std::fs::set_permissions(&bak, std::fs::Permissions::from_mode(0o600));
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), ws, "moved aside as it was");
    assert!(h.app.notices.iter().any(|n| matches!(n.msg, Msg::WorkspaceBroken { .. })));
    assert_all_recovered(h, &dirs, &consoles, "unreadable");
}

#[cfg(unix)]
#[test]
fn an_unreadable_console_file_is_never_written_over() {
    use std::os::unix::fs::PermissionsExt;
    let dirs = Dirs::new("recover-console-perm");
    let ws = "version = 1\nactive = 0\n[[tabs]]\nid = \"aaaa1111\"\nkind = \"console\"\n\
              [[tabs]]\nid = \"bbbb2222\"\nkind = \"console\"\n";
    let consoles = leave_consoles(&dirs, Some(ws));
    let locked = dirs.state().join("consoles").join("aaaa1111.sql");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut h = launch(&config(), &dirs);
    assert!(h.app.notices.iter().any(|n| matches!(n.msg, Msg::WorkspaceConsolesUnreadable { count: 1, .. })));
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(std::fs::read_to_string(&locked).unwrap(), consoles[0].1, "left as it was");
    assert_eq!(h.app.tabs.iter().next().unwrap().editor.text(), consoles[1].1);
}

/// The trashed files: (name, text), oldest first.
fn trash(dirs: &Dirs) -> Vec<(String, String)> {
    let dir = dirs.state().join("consoles").join(".trash");
    let mut v: Vec<(String, String)> = std::fs::read_dir(&dir)
        .map(|d| {
            d.flatten()
                .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read_to_string(e.path()).unwrap()))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[test]
fn a_valid_workspace_file_that_lists_other_consoles_recovers_every_file() {
    let dirs = Dirs::new("recover-nonexistent");
    let ws = "version = 1\nactive = 0\n[[tabs]]\nid = \"ffffffffffffffff\"\nkind = \"console\"\n";
    let consoles = leave_consoles(&dirs, Some(ws));
    let h = launch(&config(), &dirs);
    assert!(trash(&dirs).is_empty(), "nothing is trashed");
    assert!(h.app.notices.iter().any(|n| matches!(n.msg, Msg::WorkspaceConsolesMissing { count: 1 })));
    assert_all_recovered(h, &dirs, &consoles, "nonexistent");
    assert!(trash(&dirs).is_empty(), "nor after a quit and a restart");
}

#[test]
fn a_workspace_file_cut_at_a_tab_boundary_recovers_the_rest() {
    let dirs = Dirs::new("recover-boundary");
    // What a full file looks like, cut right before its second [[tabs]].
    let ws = "version = 1\nactive = 0\n\n[explorer]\nexpanded_folders = []\nscripts_expanded = false\n\
              script_folders = []\n\n[[tabs]]\nid = \"aaaa1111\"\nkind = \"console\"\ncursor = [0, 0]\ntop = 0\n";
    let consoles = leave_consoles(&dirs, Some(ws));
    let mut h = launch(&config(), &dirs);
    assert_eq!(h.app.tabs.len(), 2);
    assert_eq!(recovered_notice(&h), Some(1));
    assert!(h.app.tabs.iter().any(|t| t.doc.recovered && t.editor.text() == consoles[1].1));
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    assert!(trash(&dirs).is_empty(), "nothing is trashed");
    for (id, text) in &consoles {
        let file = dirs.state().join("consoles").join(format!("{id}.sql"));
        assert_eq!(std::fs::read_to_string(file).unwrap(), *text);
    }
}

#[cfg(unix)]
#[test]
fn an_unreadable_console_is_never_trashed_across_launches() {
    let dirs = Dirs::new("recover-perm-twice");
    let ws = "version = 1\nactive = 0\n[[tabs]]\nid = \"aaaa1111\"\nkind = \"console\"\n\
              [[tabs]]\nid = \"bbbb2222\"\nkind = \"console\"\n";
    let consoles = leave_consoles(&dirs, Some(ws));
    let locked = dirs.state().join("consoles").join("aaaa1111.sql");
    set_mode(&locked, 0o000);
    for launch_no in 1..=2 {
        let mut h = launch(&config(), &dirs);
        assert!(
            h.app.notices.iter().any(|n| matches!(n.msg, Msg::WorkspaceConsolesUnreadable { count: 1, .. })),
            "launch {launch_no} says so"
        );
        h.app.dispatch(datarig_tui::app::action::Action::Quit);
        assert!(h.app.quit);
        assert!(locked.exists(), "launch {launch_no}: left where it is");
        assert!(trash(&dirs).is_empty(), "launch {launch_no}: never trashed");
    }
    // Readable again: it comes back as a recovered tab.
    set_mode(&locked, 0o600);
    let h = launch(&config(), &dirs);
    assert_eq!(recovered_notice(&h), Some(1));
    assert!(h.app.tabs.iter().any(|t| t.editor.text() == consoles[0].1));
}

#[test]
fn closed_consoles_go_to_the_trash_and_the_cap_applies_only_at_launch() {
    let dirs = Dirs::new("trash-cap");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    for i in 0..59 {
        h.ctrl('t');
        type_sql(&mut h, &format!("select {i};"));
    }
    h.advance(AUTOSAVE);
    for _ in 0..59 {
        h.ctrl('w');
    }
    let trashed = trash(&dirs);
    assert_eq!(trashed.len(), 59, "every closed console is in the trash, none deleted yet");
    let names: Vec<&String> = trashed.iter().map(|(n, _)| n).collect();
    let mut unique = names.clone();
    unique.dedup();
    assert_eq!(unique.len(), 59, "unique names");
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    drop(h);
    // A relaunch keeps them all (they are young); old ones past the newest 50 go.
    let h = launch(&cfg, &dirs);
    assert_eq!(trash(&dirs).len(), 59);
    drop(h);
    let day_ms: u128 = 24 * 60 * 60 * 1000;
    let old = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() - 40 * day_ms;
    let dir = dirs.state().join("consoles").join(".trash");
    for i in 0..5u128 {
        std::fs::write(dir.join(format!("{:013}-old{i}.sql", old + i)), "old").unwrap();
    }
    std::fs::write(dir.join("notes.txt"), "mine").unwrap();
    let h = launch(&cfg, &dirs);
    let left = trash(&dirs);
    assert_eq!(left.len(), 60, "59 young + the note; the 5 old ones are beyond the newest 50");
    assert!(left.iter().any(|(n, _)| n == "notes.txt"), "a file the trash did not name stays");
    drop(h);
}

#[test]
fn a_closed_console_comes_back_after_a_restart() {
    let dirs = Dirs::new("reopen-restart");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    type_sql(&mut h, "select 'closed yesterday';");
    h.ctrl('t');
    type_sql(&mut h, "select 'closed today';");
    h.ctrl('t');
    h.keys("gT");
    h.ctrl('w');
    h.keys("gT");
    h.ctrl('w');
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    drop(h);
    let mut h = launch(&cfg, &dirs);
    let texts = |h: &Harness| h.app.tabs.iter().map(|t| t.editor.text()).collect::<Vec<_>>();
    assert!(!texts(&h).iter().any(|t| t.contains("closed")), "{:?}", texts(&h));
    h.app.focus = Focus::Editor;
    h.keys(" tu");
    assert!(h.app.tab().editor.text().contains("closed yesterday"), "the newest first: {:?}", texts(&h));
    h.keys(" tu");
    assert!(h.app.tab().editor.text().contains("closed today"));
    h.keys(" tu");
    assert!(h.status(120, 30).contains("No closed tab to reopen"));
    assert!(trash(&dirs).is_empty(), "taken out of the trash");
    // They are ordinary consoles again: kept by a restart.
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    drop(h);
    let h = launch(&cfg, &dirs);
    assert_eq!(texts(&h).iter().filter(|t| t.contains("closed")).count(), 2);
}

#[test]
fn recover_lists_the_trash_and_brings_back_the_pick() {
    let dirs = Dirs::new("recover-cmd");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    for name in ["first", "second"] {
        h.ctrl('t');
        type_sql(&mut h, &format!("select '{name}';"));
        h.ctrl('w');
    }
    h.command("recover");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Chooser));
    let screen = h.screen(120, 30);
    assert!(screen.contains("Closed consoles") && screen.contains("just now · select 'second';"), "{screen}");
    h.keys("j");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().editor.text(), "select 'first';");
    assert_eq!(trash(&dirs).len(), 1);
    // Space t u no longer brings back the one :recover took.
    h.keys(" tu");
    assert_eq!(h.app.tab().editor.text(), "select 'second';");
    h.keys(" tu");
    assert!(h.status(120, 30).contains("No closed tab to reopen"));
}

// ── quitting or closing never drops edits whose save failed ──────────────────

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
fn quit_confirm_shown(h: &mut Harness, how: &str) {
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), "{how} asks");
    let screen = h.screen(140, 30);
    assert!(screen.contains("could not be saved. Quitting loses them"), "{how}\n{screen}");
    h.keys("n");
    assert!(!h.app.quit, "{how}: n stays");
}

#[cfg(unix)]
#[test]
fn a_failed_save_asks_before_quitting_or_closing() {
    let dirs = Dirs::new("unsaved-script");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    save_as(&mut h, "q");
    h.ctrl('t');
    h.keys("gT");
    assert_eq!(h.app.tab().script(), Some("q.sql"));
    let scripts = dirs.root.join("data").join("scripts");
    set_mode(&scripts, 0o555);
    type_sql(&mut h, "select 'not written';");
    h.advance(AUTOSAVE);
    assert!(h.app.tab().doc.save_error.is_some(), "the autosave failed");
    h.command("qa");
    quit_confirm_shown(&mut h, ":qa");
    h.ctrl('q');
    quit_confirm_shown(&mut h, "Ctrl+Q");
    h.app.focus = Focus::Tree;
    h.keys("q");
    quit_confirm_shown(&mut h, "q in the explorer");
    h.key(KeyCode::Tab);
    let n = h.app.tabs.len();
    for close in ["q", "wq"] {
        h.command(close);
        if close == "q" {
            assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), ":{close} asks");
            let screen = h.screen(140, 30);
            assert!(screen.contains("could not be written"), ":{close}\n{screen}");
            assert!(h.screen(140, 30).contains("Not saved: permission denied"), "why, in the details");
            h.keys("n");
        }
        assert_eq!(h.app.tabs.len(), n, ":{close} keeps the tab");
    }
    h.ctrl('w');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), "Ctrl+W asks");
    h.keys("n");
    assert_eq!(h.app.tabs.len(), n);
    // Once the file can be written again, quitting writes it and asks nothing.
    set_mode(&scripts, 0o755);
    h.command("qa");
    assert!(h.app.quit, "quit after the final flush");
    assert!(dirs.read("q.sql").unwrap().contains("not written"));
}

#[cfg(unix)]
#[test]
fn quitting_flushes_a_pending_console_edit_and_asks_when_that_fails() {
    let dirs = Dirs::new("unsaved-console");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    h.advance(AUTOSAVE);
    let consoles = dirs.state().join("consoles");
    set_mode(&consoles, 0o555);
    type_sql(&mut h, "select 'pending';");
    assert!(h.app.tab().doc.save_due.is_some(), "not flushed yet");
    h.command("qa");
    quit_confirm_shown(&mut h, ":qa with a pending console edit");
    set_mode(&consoles, 0o755);
    h.command("qa");
    assert!(h.app.quit);
    let id = h.app.tab().doc.console_id.clone();
    let text = std::fs::read_to_string(consoles.join(format!("{id}.sql"))).unwrap();
    assert!(text.contains("pending"), "{text}");
}

#[test]
fn a_broken_index_keeps_the_connections_it_can_and_says_so() {
    let dirs = Dirs::new("index");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    save_as(&mut h, "one");
    connect(&mut h, "other");
    save_as(&mut h, "two");
    let (local, other) = (id(&h, "local-pg"), id(&h, "other"));
    // Only `one` stays open: the launch writes its binding, `two` must survive that too.
    h.ctrl('w');
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    drop(h);
    let index = dirs.root.join("data").join("scripts.toml");
    let broken = format!(
        "version = 1\n[bindings.\"one.sql\"]\nprofile = \"{local}\"\n<<<<<<< merge\n[bindings.\"two.sql\"]\nprofile = \"{other}\"\n"
    );
    std::fs::write(&index, &broken).unwrap();
    let mut h = launch(&cfg, &dirs);
    let notice = h.app.notices.iter().find_map(|n| match &n.msg {
        Msg::ScriptsIndexRebuilt { count, path, .. } => Some((*count, path.clone())),
        _ => None,
    });
    let bak = dirs.root.join("data").join("scripts.toml.bak");
    assert_eq!(notice, Some((2, bak.display().to_string())));
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), broken, "backed up first");
    let store = ScriptStore::open(&dirs.root.join("data"));
    assert_eq!((store.binding("one.sql"), store.binding("two.sql")), (Some(local), Some(other)));
    select_row(&mut h, "two");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().profile, Some(other), "opens on its connection");
}

#[test]
fn wq_on_the_last_tab_quits_like_q_and_asks_the_same() {
    let dirs = Dirs::new("wq-last");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    type_sql(&mut h, "select 'last';");
    assert_eq!(h.app.tabs.len(), 1);
    // A transaction is open: the quit question, as for `:q`.
    h.app.tabs.active_mut().exec.tx_open = true;
    h.command("wq last");
    assert!(dirs.read("last.sql").is_some_and(|t| t.contains("'last'")), "saved first");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    assert!(h.screen(140, 30).contains("Quit datarig?"));
    h.keys("n");
    assert!(!h.app.quit && h.app.tabs.len() == 1, "n: nothing closed");
    h.app.tabs.active_mut().exec.tx_open = false;
    h.command("wq");
    assert!(h.app.quit, ":wq on the last tab quits");
    drop(h);
    // A console asks for a name first, then quits.
    let mut h = launch(&cfg, &dirs);
    h.ctrl('w');
    assert!(h.app.tabs.is_empty(), "the last tab closed: none left");
    h.explore("local-pg");
    h.keys("o");
    assert_eq!(h.app.tabs.len(), 1);
    h.command("wq");
    h.type_text("asked-last");
    h.key(KeyCode::Enter);
    assert!(dirs.read("asked-last.sql").is_some());
    assert!(h.app.quit, "quit after the name was given");
}

// ── failures read as friendly, localized reasons ─────────────────────────────

#[test]
fn a_name_too_long_for_the_file_system_says_so_in_words() {
    let dirs = Dirs::new("too-long");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    // Every part is a valid name; the whole path is longer than any OS allows (Windows, with
    // long paths as on its CI runners, takes 32,767 characters). Pasted: typed key by key, a
    // command line this long takes minutes.
    let long = vec!["a".repeat(200); 170].join("/");
    let w = |h: &mut Harness| {
        h.ctrl('k');
        h.app.handle_event(ratatui::crossterm::event::Event::Paste(format!("w {long}")));
        h.key(KeyCode::Enter);
    };
    w(&mut h);
    let screen = h.screen(200, 30);
    assert!(screen.contains("Not saved: the name is too long for the file system"), "{screen}");
    assert!(!screen.contains("Io(") && !screen.contains("os error"), "no debug text: {screen}");
    h.command("set language=ko");
    w(&mut h);
    let screen = h.screen(200, 30);
    let error = ko(datarig_core::i18n::Label::IoNameTooLong).to_string();
    assert!(screen.contains(&ko_msg(&datarig_core::i18n::Msg::ScriptsSaveFailed { error })), "{screen}");
}

#[test]
fn reloading_a_script_that_is_gone_says_why() {
    let dirs = Dirs::new("reload-gone");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    save_as(&mut h, "gone");
    std::fs::remove_file(dirs.script("gone.sql")).unwrap();
    type_sql(&mut h, "x");
    h.ctrl('s');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), "the conflict question");
    h.keys("r");
    let screen = h.screen(160, 30);
    assert!(screen.contains("Could not open gone.sql: the file or folder does not exist"), "{screen}");
}

#[test]
fn an_unknown_driver_is_named_in_words() {
    let dirs = Dirs::new("driver");
    let odd = ConnectionConfig { name: "odd".into(), driver: "mysql".into(), ..ConnectionConfig::test_db() };
    let cfg = Config { connections: vec![odd], ..Config::default() };
    let mut h = launch(&cfg, &dirs);
    h.explore("odd");
    h.key(KeyCode::Enter);
    assert_eq!(h.node_error("odd").as_deref(), Some("Unknown driver “mysql” in this profile"));
}

// ── a folder that cannot be read is never taken for an empty one ─────────────

#[cfg(unix)]
#[test]
fn saving_or_renaming_into_an_unreadable_folder_is_refused() {
    let dirs = Dirs::new("unreadable-folder");
    let cfg = config();
    let store = ScriptStore::open(&dirs.root.join("data"));
    store.create("d/x.sql", "ORIGINAL precious").unwrap();
    store.create("mine.sql", "select 'mine';").unwrap();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    let folder = dirs.root.join("data").join("scripts").join("d");
    set_mode(&folder, 0o300);
    if std::fs::read_dir(&folder).is_ok() {
        set_mode(&folder, 0o755);
        return; // permissions do not apply (root)
    }
    type_sql(&mut h, "select 'clobber';");
    h.command("w d/x");
    let screen = h.screen(160, 30);
    assert!(screen.contains("can't be read"), "the reason is said:\n{screen}");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.tab().script(), None, "still a console");
    // Rename a saved query onto it from the explorer: refused the same way.
    select_row(&mut h, "mine");
    h.keys("R");
    h.app.overlays.name_input_mut().unwrap().input.set("d/x");
    h.key(KeyCode::Enter);
    assert_eq!(name_error(&h), Some(Label::ValidateScriptUnreadable));
    h.key(KeyCode::Esc);
    set_mode(&folder, 0o755);
    assert_eq!(dirs.read("d/x.sql").as_deref(), Some("ORIGINAL precious"), "never written over");
    assert_eq!(dirs.read("mine.sql").as_deref(), Some("select 'mine';"));
}

#[cfg(unix)]
#[test]
fn closing_a_console_whose_save_failed_asks_first() {
    let dirs = Dirs::new("unsaved-console-close");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    h.ctrl('t');
    type_sql(&mut h, "select 'saved';");
    h.advance(AUTOSAVE);
    let consoles = dirs.state().join("consoles");
    set_mode(&consoles, 0o555);
    type_sql(&mut h, "select 'not saved';");
    let n = h.app.tabs.len();
    for how in ["Ctrl+W", ":q", ":tabclose"] {
        match how {
            "Ctrl+W" => h.ctrl('w'),
            ":q" => h.command("q"),
            _ => h.command("tabclose"),
        }
        assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm), "{how} asks");
        let screen = h.screen(140, 30);
        assert!(screen.contains("This console's edits could not be saved"), "{how}\n{screen}");
        assert!(screen.contains("Not saved: permission denied"), "{how}: why, like the quit confirm\n{screen}");
        h.keys("n");
        assert_eq!(h.app.tabs.len(), n, "{how}: n keeps the tab");
    }
    // Writable again: it saves and closes without asking; its text is in the trash.
    set_mode(&consoles, 0o755);
    h.ctrl('w');
    assert!(h.overlay_kind().is_none());
    assert_eq!(h.app.tabs.len(), n - 1);
    assert!(trash(&dirs).iter().any(|(_, t)| t.contains("not saved")));
}

#[cfg(unix)]
#[test]
fn wq_whose_save_fails_says_so_every_time_and_keeps_the_tab() {
    let dirs = Dirs::new("wq-failed");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    save_as(&mut h, "q");
    h.ctrl('t');
    h.keys("gT");
    let scripts = dirs.root.join("data").join("scripts");
    set_mode(&scripts, 0o555);
    type_sql(&mut h, "select 'edit';");
    h.advance(AUTOSAVE);
    assert!(h.app.tab().doc.save_error.is_some(), "the autosave failed already");
    let n = h.app.tabs.len();
    for attempt in 1..=2 {
        h.app.status = None;
        h.command("wq");
        assert_eq!(h.app.tabs.len(), n, "attempt {attempt}: the tab stays");
        let status = h.status(140, 30);
        assert!(status.contains("Not saved: permission denied; the tab stays open"), "attempt {attempt}: {status}");
    }
    // :wq <name> that cannot be written keeps the tab too.
    h.ctrl('t');
    let n = h.app.tabs.len();
    h.command("wq other");
    assert_eq!(h.app.tabs.len(), n, ":wq <name> whose write failed keeps the tab");
    assert!(h.status(140, 30).contains("Not saved"));
    set_mode(&scripts, 0o755);
}

/// Every notice's text in `lang`.
fn notice_texts(h: &Harness, lang: Lang) -> Vec<String> {
    let i18n = datarig_core::i18n::I18n::new(lang);
    h.app.notices.iter().map(|n| i18n.msg(&n.msg).to_string()).collect()
}

#[test]
fn broken_state_files_are_reported_in_words_and_the_detail_goes_to_the_log() {
    let dirs = Dirs::new("typed-broken");
    std::fs::create_dir_all(dirs.state()).unwrap();
    std::fs::write(dirs.state().join("workspace.toml"), "version = 1\nactive = [\n").unwrap();
    std::fs::create_dir_all(dirs.root.join("data")).unwrap();
    std::fs::write(dirs.root.join("data").join("scripts.toml"), "version = 1\n[bindings\n").unwrap();
    let h = launch(&config(), &dirs);
    let en = notice_texts(&h, Lang::En);
    let ws = en.iter().find(|t| t.starts_with("The saved tabs could not be read")).expect("workspace notice");
    assert!(ws.contains("(a TOML error on line 2)") || ws.contains("(a TOML error on line 3)"), "{ws}");
    let idx = en.iter().find(|t| t.starts_with("The index of saved queries")).expect("index notice");
    assert!(idx.contains("a TOML error on line 2"), "{idx}");
    for t in &en {
        assert!(!t.contains('\n') && !t.contains("expected") && !t.contains("TOML parse error"), "raw text: {t}");
    }
    let log = std::fs::read_to_string(dirs.state().join("errors.log")).expect("the detail is logged");
    assert!(log.contains("workspace.broken: Toml") && log.contains("scripts.index: Toml"), "{log}");
    assert_eq!(log.lines().count(), 2, "one line each: {log}");
}

// ── a config file that cannot be read never costs a binding ──────────────────

/// [`launch`] when the config file cannot be read (`App::new` gets the error and no profiles,
/// as the binary does).
fn launch_unreadable_config(dirs: &Dirs) -> Harness {
    let error = datarig_core::config::ConfigError::Syntax(datarig_core::fault::Fault::new(
        datarig_core::fault::FaultKind::Toml { line: Some(14) },
        "expected `=`",
    ));
    let mut app = App::new(&Config::default(), Some(error), Lang::En);
    let clock = FakeClock::new();
    let c = clock.clone();
    app.set_clock(Arc::new(move || c.now()));
    let store = Arc::new(MemoryStore::new());
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.set_paths(dirs.paths());
    let driver = FakeDriver::default();
    app.launch(Startup::Normal);
    Harness { app, cancelled: driver.any_cancel.clone(), driver, store, clock }
}

/// Every saved tab as (saved query, profile), from `workspace.toml`.
fn saved_tabs(dirs: &Dirs) -> Vec<(Option<String>, Option<ProfileId>)> {
    let loaded = datarig_core::workspace::load(&dirs.state());
    assert!(loaded.broken.is_none());
    loaded.state.tabs.iter().map(|t| (t.script.clone(), t.profile)).collect()
}

fn saved_binding(dirs: &Dirs, rel: &str) -> Option<ProfileId> {
    ScriptStore::open(&dirs.root.join("data")).binding(rel)
}

#[test]
fn an_unreadable_config_keeps_every_binding_until_it_is_fixed() {
    let dirs = Dirs::new("config-unreadable");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    let (pg, other) = (id(&h, "local-pg"), id(&h, "other"));
    connect(&mut h, "local-pg");
    save_as(&mut h, "daily");
    connect(&mut h, "other");
    type_sql(&mut h, "select 'on other';");
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    drop(h);
    let before = saved_tabs(&dirs);
    assert_eq!(before, [(Some("daily.sql".into()), Some(pg)), (None, Some(other))]);
    assert_eq!(saved_binding(&dirs, "daily.sql"), Some(pg));

    // The config cannot be read: no profiles, a notice that says why, and every binding is
    // written back as it was, through the ways a tab comes and goes.
    let mut h = launch_unreadable_config(&dirs);
    let en = notice_texts(&h, Lang::En);
    assert!(
        en.iter().any(|t| t.starts_with("The config file can't be read, so connections are unavailable")),
        "{en:?}"
    );
    assert!(h.app.notices.iter().any(|n| matches!(n.msg, Msg::Label(Label::ConfigConnectionsUnavailable))));
    let ko = notice_texts(&h, Lang::Ko);
    assert!(ko.iter().all(|t| !t.starts_with("The config file")), "translated: {ko:?}");
    assert_eq!(h.app.tabs.len(), 2);
    assert!(h.app.tabs.iter().all(|t| t.profile.is_none()), "no connection while the profiles are unknown");
    // Without profiles only the explorer and the saved queries are there; `:e` goes to the
    // saved query's tab.
    h.command("e daily");
    assert_eq!(h.app.tab().script(), Some("daily.sql"));
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    assert!(h.app.quit);
    drop(h);
    let mut after = saved_tabs(&dirs);
    after.sort();
    let mut expected = before.clone();
    expected.sort();
    assert_eq!(after, expected, "workspace.toml keeps the bindings");
    assert_eq!(saved_binding(&dirs, "daily.sql"), Some(pg), "scripts.toml keeps the binding");

    // Fixed: the tabs are on their profiles again.
    let h = launch(&cfg, &dirs);
    let mut tabs: Vec<(Option<String>, Option<ProfileId>)> =
        h.app.tabs.iter().map(|t| (t.script().map(str::to_string), t.profile)).collect();
    tabs.sort();
    assert_eq!(tabs, expected, "the bindings work again");
}

#[test]
fn only_deleting_a_profile_in_the_app_drops_its_bindings() {
    let dirs = Dirs::new("config-delete");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    let other = id(&h, "other");
    connect(&mut h, "other");
    save_as(&mut h, "on-other");
    h.ctrl('w');
    assert_eq!(saved_binding(&dirs, "on-other.sql"), Some(other));
    // That was the last tab; other is still connected: `o` opens a console on it.
    h.explore("other");
    h.keys("o");
    type_sql(&mut h, "select 'console on other';");
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    drop(h);
    // A profile the config does not list (edited by hand, or a file that is not the usual
    // one) is unknown, not deleted: the tabs have no connection but keep the binding, through
    // `:e`, closing and `Space t u`.
    let fewer = Config { connections: vec![cfg.connections[0].clone()], ..Config::default() };
    let mut h = launch(&fewer, &dirs);
    h.app.focus = Focus::Editor;
    assert!(h.app.tabs.iter().all(|t| t.profile.is_none()));
    h.command("e on-other");
    assert_eq!((h.app.tab().script(), h.app.tab().profile), (Some("on-other.sql"), None));
    h.ctrl('w');
    h.command("e on-other");
    assert_eq!(h.app.tab().script(), Some("on-other.sql"));
    let console = h.app.tabs.iter().position(|t| t.script().is_none()).unwrap();
    h.app.tabs.activate(console);
    h.ctrl('w');
    h.keys(" tu");
    assert!(h.app.tab().editor.text().contains("console on other"));
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    assert!(h.app.quit);
    drop(h);
    assert_eq!(saved_binding(&dirs, "on-other.sql"), Some(other), "unknown is not deleted");
    let mut tabs = saved_tabs(&dirs);
    tabs.sort();
    assert_eq!(tabs, [(None, Some(other)), (Some("on-other.sql".into()), Some(other))]);
    // Deleted in the app: the binding goes.
    let mut h = launch(&cfg, &dirs);
    h.explore("other");
    h.keys("dy");
    assert!(h.app.profiles.iter().all(|p| p.id != other));
    assert_eq!(saved_binding(&dirs, "on-other.sql"), None, "deleted in the app");
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    drop(h);
    assert!(saved_tabs(&dirs).iter().all(|(_, p)| p.is_none()), "{:?}", saved_tabs(&dirs));
}

// ── one console file, one tab ────────────────────────────────────────────────

#[test]
fn a_console_listed_twice_opens_once_and_closing_never_takes_it_from_another_tab() {
    let dirs = Dirs::new("dup-console");
    let one = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1";
    let tab = |id: &str| format!("[[tabs]]\nid = \"{id}\"\nkind = \"console\"\ncursor = [0, 0]\ntop = 0\n");
    let dir = dirs.state().join("consoles");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{one}.sql")), "select 'original';").unwrap();
    std::fs::write(dir.join("ccccccccccccccccccccccccccccccc3.sql"), "select 'other';").unwrap();
    let ws = ["version = 1\nactive = 0\n".to_string(), tab(one), tab(one), tab("ccccccccccccccccccccccccccccccc3")];
    std::fs::write(dirs.state().join("workspace.toml"), ws.concat()).unwrap();

    let mut h = launch(&config(), &dirs);
    let ids: Vec<String> = h.app.tabs.iter().map(|t| t.doc.console_id.clone()).collect();
    assert_eq!(ids, [one, "ccccccccccccccccccccccccccccccc3"], "the second tab of that console is left out");
    let en = notice_texts(&h, Lang::En);
    assert!(en.contains(&"1 saved tab was left out: it listed the console of an earlier tab".to_string()), "{en:?}");
    assert!(h.app.notices.iter().any(|n| matches!(n.msg, Msg::WorkspaceDuplicateTabs { count: 1 })));
    let ko = notice_texts(&h, Lang::Ko);
    assert!(ko.iter().all(|t| !t.contains("saved tab was left out")), "translated: {ko:?}");
    // Edit the first tab (autosaved), close every other tab, quit: the edit is on disk.
    h.app.focus = Focus::Editor;
    h.app.tabs.activate(0);
    h.keys("A");
    h.type_text(" -- edited");
    h.key(KeyCode::Esc);
    h.advance(AUTOSAVE);
    h.keys("gt");
    h.ctrl('w');
    assert_eq!(h.app.tabs.len(), 1);
    // Should a second tab ever show the same file, closing it leaves the file to the first.
    connect(&mut h, "local-pg");
    assert_eq!(h.app.tabs.len(), 2);
    let second = h.app.tab().id;
    h.app.tabs.get_mut(second).unwrap().doc.console_id = one.to_string();
    h.ctrl('w');
    assert_eq!(h.app.tabs.len(), 1);
    assert!(dir.join(format!("{one}.sql")).exists(), "not trashed from under the first tab");
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    assert!(h.app.quit);
    drop(h);
    let file = std::fs::read_to_string(dir.join(format!("{one}.sql"))).unwrap();
    assert_eq!(file, "select 'original'; -- edited");
    let h = launch(&config(), &dirs);
    assert_eq!(h.app.tab().editor.text(), "select 'original'; -- edited");
}

// ── a trashed file that cannot be read ───────────────────────────────────────

#[cfg(unix)]
#[test]
fn an_unreadable_trashed_console_is_skipped_by_space_t_u_and_marked_in_recover() {
    let dirs = Dirs::new("trash-unreadable");
    let trash_dir = dirs.state().join("consoles").join(".trash");
    std::fs::create_dir_all(&trash_dir).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
    let older = format!("{}-aaaa1111.sql", now - 2000);
    let newest = format!("{}-bbbb2222.sql", now - 1000);
    std::fs::write(trash_dir.join(&older), "select 'older';").unwrap();
    std::fs::write(trash_dir.join(&newest), "select 'newest';").unwrap();
    set_mode(&trash_dir.join(&newest), 0o000);
    if std::fs::read(trash_dir.join(&newest)).is_ok() {
        return set_mode(&trash_dir.join(&newest), 0o600); // root reads anything
    }
    let mut h = launch(&config(), &dirs);
    h.app.focus = Focus::Editor;
    h.keys(" tu");
    assert_eq!(h.app.tab().editor.text(), "select 'older';", "the next one comes back");
    let status = h.status(200, 30);
    assert!(
        status.contains("1 closed console in the trash could not be read (permission denied); it was skipped"),
        "{status}"
    );
    // :recover lists it as unreadable; picking it says so, and it stays.
    h.command("recover");
    let screen = h.screen(160, 30);
    assert!(screen.contains("cannot be read (permission denied)"), "{screen}");
    h.key(KeyCode::Enter);
    let status = h.status(200, 30);
    assert!(status.contains("This closed console could not be read (permission denied)"), "{status}");
    set_mode(&trash_dir.join(&newest), 0o600);
    assert_eq!(std::fs::read_to_string(trash_dir.join(&newest)).unwrap(), "select 'newest';");
    let msg = Msg::TabTrashFilesSkipped { count: 1, error: "e".into() };
    let (en, ko) = (datarig_core::i18n::I18n::new(Lang::En), datarig_core::i18n::I18n::new(Lang::Ko));
    assert_ne!(ko.msg(&msg).to_string(), en.msg(&msg).to_string(), "translated");
}

// ── no unbound console at launch ─────────────────────────────────────────────

/// A restored workspace never brings back a console without a connection that nobody wrote
/// in (blank): the workspace starts empty and those files go. A
/// console without a connection that has text still comes back, with its banner.
#[test]
fn restore_never_brings_back_an_unused_console_without_a_connection() {
    let dirs = Dirs::new("unused-unbound");
    let starter = "\n\t \n";
    let (blank, old, kept, orphan) = (
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa2",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa3",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa4",
    );
    let tab = |id: &str| format!("[[tabs]]\nid = \"{id}\"\nkind = \"console\"\ncursor = [0, 0]\ntop = 0\n");
    let dir = dirs.state().join("consoles");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{blank}.sql")), "  \n").unwrap();
    std::fs::write(dir.join(format!("{old}.sql")), starter).unwrap();
    std::fs::write(dir.join(format!("{kept}.sql")), "select 'mine';").unwrap();
    // Not listed and empty: nothing to recover either.
    std::fs::write(dir.join(format!("{orphan}.sql")), "").unwrap();
    let ws = ["version = 1\nactive = 0\n".to_string(), tab(blank), tab(old), tab(kept)];
    std::fs::write(dirs.state().join("workspace.toml"), ws.concat()).unwrap();
    let mut h = launch(&config(), &dirs);
    assert_eq!(h.app.tabs.len(), 1, "only the console with text");
    assert_eq!((h.app.tab().profile, h.app.tab().editor.text()), (None, "select 'mine';".to_string()));
    assert!(h.screen(120, 30).contains("No connection ·"), "its banner");
    let files: Vec<String> = dirs.consoles().into_iter().filter(|f| f.ends_with(".sql")).collect();
    assert_eq!(files, [format!("{kept}.sql")], "the unused ones' files are gone");
    // Its only tab closed: the next launch starts with none.
    h.ctrl('w');
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    drop(h);
    let h = launch(&config(), &dirs);
    assert!(h.app.tabs.is_empty());
}

// ── workspace.toml of another version (unknown is not absent) ─────────

/// A `workspace.toml` of a later version: its tabs open as far as this version knows them,
/// the banner says the workspace is not saved, and nothing of it (the file, its consoles) is
/// written, whatever the user does. Not even tried at launch: no "could not save" and nothing
/// in `errors.log` (the banner says it all).
#[test]
fn a_later_versions_workspace_opens_but_is_never_written() {
    let dirs = Dirs::new("ws-newer");
    let ws = "version = 4\nactive = 0\nfuture_top = 1\n\n[layout]\nsplit = \"v\"\n\n\
              [[tabs]]\nid = \"aaaa1111\"\nkind = \"console\"\npinned = true\n\n\
              [[tabs]]\nid = \"bbbb2222\"\nkind = \"console\"\n\n[[tabs]]\nid = \"cccc\"\nkind = \"chart\"\n";
    let consoles = leave_consoles(&dirs, Some(ws));
    let mut h = launch(&config(), &dirs);
    assert_eq!(h.app.workspace_newer, Some(4));
    assert!(h.app.read_only);
    let status = h.status(160, 30);
    assert!(!status.contains("Could not save"), "{status}");
    assert!(!dirs.state().join("errors.log").exists(), "nothing failed");
    let texts: Vec<String> = h.app.tabs.iter().map(|t| t.editor.text()).collect();
    assert!(consoles.iter().all(|(_, text)| texts.contains(text)), "{texts:?}");
    let screen = h.screen(160, 30);
    assert!(screen.lines().next().unwrap().contains("workspace.toml is from a later datarig (version 4)"), "{screen}");
    type_sql(&mut h, "select 'changed';");
    h.advance(AUTOSAVE);
    h.ctrl('t');
    h.app.dispatch(datarig_tui::app::action::Action::Quit);
    assert_eq!(std::fs::read_to_string(dirs.state().join("workspace.toml")).unwrap(), ws, "untouched");
    for (id, text) in &consoles {
        let file = dirs.state().join("consoles").join(format!("{id}.sql"));
        assert_eq!(std::fs::read_to_string(file).unwrap(), *text, "{id} untouched");
    }
}

/// A version 1 file is kept as `workspace.toml.v1.bak` before it is written as version 2, and
/// the file's `active` (which counts every tab, those of unknown kinds too) opens the tab it
/// names; the keys this version does not know are written back.
#[test]
fn a_version_1_workspace_is_kept_and_opens_its_active_tab() {
    let dirs = Dirs::new("ws-v1");
    let ws = "version = 1\nactive = 2\nfuture_top = 1\n\n[[tabs]]\nid = \"aaaa1111\"\nkind = \"console\"\n\n\
              [[tabs]]\nid = \"cccc\"\nkind = \"chart\"\n\n[[tabs]]\nid = \"bbbb2222\"\nkind = \"console\"\n\
              pinned = true\n";
    leave_consoles(&dirs, Some(ws));
    let h = launch(&config(), &dirs);
    assert_eq!(h.app.tab().editor.text(), "select 'user text two';", "the file's third tab");
    assert_eq!(std::fs::read_to_string(dirs.state().join("workspace.toml.v1.bak")).unwrap(), ws);
    let written = std::fs::read_to_string(dirs.state().join("workspace.toml")).unwrap();
    assert!(written.contains("version = 3") && written.contains("active = 1"), "{written}");
    assert!(
        written.contains("future_top = 1") && written.contains("pinned = true") && written.contains("kind = \"chart\""),
        "{written}"
    );
}

// ── the folder tree of "save as" and "open" ────────────────────────────────

/// The rows of the tree dialog as text (indent, arrow, name), one per row.
fn tree_rows(h: &Harness) -> Vec<String> {
    let t = h.app.overlays.script_tree().expect("the tree dialog");
    t.rows()
        .iter()
        .map(|(r, d)| {
            let name = match r {
                TreeRow::Up => "../".to_string(),
                TreeRow::Top => "/".to_string(),
                TreeRow::Folder(p) => format!("{p}/"),
                TreeRow::File(p) => p.clone(),
            };
            format!("{}{name}", "  ".repeat(*d))
        })
        .collect()
}

#[test]
fn save_as_picks_a_folder_in_a_tree_and_makes_one() {
    let dirs = Dirs::new("tree-save");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    std::fs::create_dir_all(dirs.root.join("data/scripts/reports/2026")).unwrap();
    std::fs::write(dirs.root.join("data/scripts/reports/daily.sql"), "select 1").unwrap();
    type_sql(&mut h, "select 'weekly'");
    h.ctrl('s');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree));
    // Folders first, closed; the top selected (nothing saved yet this run); the name field has
    // the keyboard.
    assert_eq!(tree_rows(&h), ["/", "  reports/"]);
    assert_eq!(h.app.key_context(), datarig_tui::keymap::Ctx::ScriptTreeName);
    let screen = h.screen(100, 30);
    assert!(screen.contains("Save query as") && screen.contains("Name: "), "{screen}");
    // Tab to the tree, down to `reports`, open it with →.
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), datarig_tui::keymap::Ctx::ScriptTree);
    h.key(KeyCode::Down);
    h.key(KeyCode::Right);
    assert_eq!(tree_rows(&h), ["/", "  reports/", "    reports/2026/", "    reports/daily.sql"]);
    // `n`: a new folder under the selected one; it is made and selected.
    h.keys("n");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::NameInput));
    assert!(h.screen(100, 30).contains("New folder in reports"));
    h.type_text("weekly");
    h.key(KeyCode::Enter);
    assert!(dirs.root.join("data/scripts/reports/weekly").is_dir());
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree));
    assert_eq!(h.app.overlays.script_tree().unwrap().row, TreeRow::Folder("reports/weekly".into()));
    // A folder that exists is refused (names ignore case), never taken over.
    h.key(KeyCode::Tab);
    h.key(KeyCode::Up);
    h.key(KeyCode::Up);
    assert_eq!(h.app.overlays.script_tree().unwrap().row, TreeRow::Folder("reports".into()));
    h.keys("n");
    h.type_text("WEEKLY");
    h.key(KeyCode::Enter);
    assert_eq!(name_error(&h), Some(Label::ValidateFolderExists));
    h.key(KeyCode::Esc);
    h.key(KeyCode::Down);
    h.key(KeyCode::Down);
    assert_eq!(h.app.overlays.script_tree().unwrap().row, TreeRow::Folder("reports/weekly".into()));
    // The name goes into the selected folder; `/` in it still makes folders there.
    h.key(KeyCode::Tab);
    h.app.overlays.script_tree_mut().unwrap().input.set("q/sales");
    let screen = h.screen(100, 30);
    assert!(screen.contains("Name: reports/weekly/q/sales"), "{screen}");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), None);
    assert_eq!(h.app.tab().script(), Some("reports/weekly/q/sales.sql"));
    assert_eq!(dirs.read("reports/weekly/q/sales.sql").as_deref(), Some("select 'weekly'"));
    // The next console's "save as" starts in the folder saved into last.
    h.ctrl('t');
    type_sql(&mut h, "select 2");
    h.ctrl('s');
    assert_eq!(h.app.overlays.script_tree().unwrap().row, TreeRow::Folder("reports/weekly/q".into()));
    // Selecting a file gives its name; replacing it asks, Cancel first; `y` replaces.
    h.key(KeyCode::Tab);
    h.key(KeyCode::Down);
    assert_eq!(h.app.overlays.script_tree().unwrap().row, TreeRow::File("reports/weekly/q/sales.sql".into()));
    assert_eq!(h.app.overlays.script_tree().unwrap().input.text(), "sales");
    h.key(KeyCode::Enter);
    assert_eq!(name_error(&h), None);
    assert!(matches!(
        h.app.overlays.script_tree().and_then(|t| t.error.clone()),
        Some(Msg::ScriptsOpenElsewhere { .. })
    ));
    // Close that tab, then replacing the file asks.
    h.key(KeyCode::Esc);
    h.keys(" 1");
    h.ctrl('w');
    h.ctrl('s');
    h.key(KeyCode::Tab);
    h.key(KeyCode::Down);
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    let screen = h.screen(100, 30);
    assert!(screen.contains("Replace this saved query?") && screen.contains("n / Enter keep"), "{screen}");
    h.keys("y");
    assert_eq!(h.overlay_kind(), None);
    assert_eq!(dirs.read("reports/weekly/q/sales.sql").as_deref(), Some("select 2"));
    assert_eq!(h.app.tab().script(), Some("reports/weekly/q/sales.sql"));
}

#[cfg(unix)]
#[test]
fn an_unreadable_folder_is_said_and_never_saved_into() {
    use std::os::unix::fs::PermissionsExt;
    let dirs = Dirs::new("tree-unreadable");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    let locked = dirs.root.join("data/scripts/locked");
    std::fs::create_dir_all(locked.join("inner")).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    type_sql(&mut h, "select 1");
    h.ctrl('s');
    let screen = h.screen(100, 30);
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
    assert!(screen.contains("locked/ (can't be read)"), "{screen}");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    h.key(KeyCode::Tab);
    h.key(KeyCode::Down);
    h.key(KeyCode::Right);
    assert_eq!(tree_rows(&h), ["/", "  locked/"], "it does not open as an empty folder");
    h.key(KeyCode::Tab);
    h.app.overlays.script_tree_mut().unwrap().input.set("x");
    h.key(KeyCode::Enter);
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
    assert_eq!(name_error(&h), Some(Label::ValidateScriptUnreadable));
    assert!(!locked.join("x.sql").exists());
}

#[test]
fn open_filters_the_tree_and_the_mouse_picks() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let dirs = Dirs::new("tree-open");
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    std::fs::create_dir_all(dirs.root.join("data/scripts/reports")).unwrap();
    std::fs::write(dirs.root.join("data/scripts/reports/daily.sql"), "select 'daily'").unwrap();
    std::fs::write(dirs.root.join("data/scripts/top.sql"), "select 'top'").unwrap();
    h.keys(" so");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree));
    assert_eq!(tree_rows(&h), ["/", "  reports/", "  top.sql"]);
    // Typing filters by path; the folders on the way open.
    h.type_text("dai");
    assert_eq!(tree_rows(&h), ["/", "  reports/", "    reports/daily.sql"]);
    assert_eq!(h.app.overlays.script_tree().unwrap().row, TreeRow::File("reports/daily.sql".into()));
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().script(), Some("reports/daily.sql"));
    // `:e` alone opens the same tree, at the tab's saved query; a click on a folder's arrow
    // closes and opens it, a second click on a file opens that file.
    h.command("e");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree));
    assert_eq!(h.app.overlays.script_tree().unwrap().row, TreeRow::File("reports/daily.sql".into()));
    h.draw(100, 30);
    let list = h.app.overlays.script_tree().unwrap().list;
    h.mouse(MouseEventKind::Down(MouseButton::Left), list.x + 2, list.y + 1);
    assert_eq!(tree_rows(&h), ["/", "  reports/", "  top.sql"]);
    h.mouse(MouseEventKind::Down(MouseButton::Left), list.x + 2, list.y + 1);
    assert_eq!(tree_rows(&h), ["/", "  reports/", "    reports/daily.sql", "  top.sql"]);
    h.draw(100, 30);
    h.mouse(MouseEventKind::Down(MouseButton::Left), list.x + 6, list.y + 3);
    assert_eq!(h.app.overlays.script_tree().unwrap().row, TreeRow::File("top.sql".into()));
    h.mouse(MouseEventKind::Down(MouseButton::Left), list.x + 6, list.y + 3);
    assert_eq!(h.overlay_kind(), None);
    assert_eq!(h.app.tab().script(), Some("top.sql"));
}

#[test]
fn the_save_key_named_in_messages_follows_the_key_map() {
    let dirs = Dirs::new("save-key");
    let mut cfg = config();
    cfg.keymap = datarig_core::config::parse("[keymap.workspace]\n\"ctrl+s\" = \"none\"\n\"f2\" = \"script.save\"\n")
        .unwrap()
        .keymap;
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    h.keys(" so");
    let status = h.status(160, 45);
    assert!(status.contains("No saved queries yet — F2 in a console saves one"), "{status}");
    let screen = h.screen(160, 45);
    let above = screen.lines().filter(|l| !l.contains("No saved queries")).collect::<Vec<_>>().join("\n");
    assert!(above.contains("F2 in a console saves one"), "the explorer's empty Saved queries: {screen}");
}

/// The explorer's Saved queries show a folder that cannot be read as such,
/// `(can't be read)`, with no arrow, never as a folder that opens to nothing.
#[cfg(unix)]
#[test]
fn the_explorer_marks_an_unreadable_saved_folder() {
    use datarig_tui::app::explorer::RowKind;
    use std::os::unix::fs::PermissionsExt;
    let dirs = Dirs::new("explorer-unreadable");
    let locked = dirs.root.join("data/scripts/locked");
    std::fs::create_dir_all(locked.join("inner")).unwrap();
    std::fs::write(dirs.root.join("data/scripts/open.sql"), "select 1").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
    h.key(KeyCode::F(6));
    let rows = h.app.explorer_rows();
    let header = rows.iter().position(|r| r.kind == RowKind::ScriptsHeader).expect("the section");
    h.app.explorer.select(&rows, header);
    h.key(KeyCode::Char('l'));
    let rows = h.app.explorer_rows();
    let at = rows.iter().position(|r| r.kind == RowKind::ScriptFolder("locked".into())).expect("the folder");
    assert_eq!(h.app.explorer_arrow(&rows[at]), None, "no arrow");
    let screen = h.screen(120, 40);
    assert!(screen.contains("locked (can't be read)"), "{screen}");
    h.app.explorer.select(&rows, at);
    h.key(KeyCode::Enter);
    h.key(KeyCode::Char('l'));
    assert_eq!(h.app.explorer_rows().len(), rows.len(), "it opens to nothing");
}

/// Save As with the name a folder has says a folder has it (it said a saved
/// query did).
#[test]
fn a_name_a_folder_has_says_so() {
    let dirs = Dirs::new("folder-name");
    std::fs::create_dir_all(dirs.root.join("data/scripts/dirsql.sql")).unwrap();
    let cfg = config();
    let mut h = launch(&cfg, &dirs);
    connect(&mut h, "local-pg");
    type_sql(&mut h, "select 1");
    h.command("w dirsql");
    let screen = h.screen(120, 40);
    assert!(screen.contains("a folder has this name"), "{screen}");
    assert!(!screen.contains("a saved query with this name exists"), "{screen}");
}
