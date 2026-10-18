//! Folders are picked from a tree: `m` on a profile, the Folder field of the profile form and
//! `m` on a saved query or folder open the tree dialog over the folders only, below
//! "(top level)". The current folder is selected with the folders on the way open; a filter
//! keeps the folders on the way to a match; `n` names a new folder (a profile folder is saved
//! only once it is picked, a folder of the saved queries is made at once).

mod common;

use common::*;
use datarig_core::config::{Config, IconsSetting};
use datarig_core::i18n::{Label, Lang};
use datarig_core::paths::Paths;
use datarig_core::profile::ConnectionConfig;
use datarig_core::profile::folder::FolderPath;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_tui::app::folder_pick::FolderPick;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::profiles::Field;
use datarig_tui::app::script_tree::{TreeFocus, TreeMode, TreeRow};
use datarig_tui::app::{Focus, Startup};
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::path::PathBuf;
use std::sync::Arc;

/// A scratch folder, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("datarig-folder-pick-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Scratch(root)
    }
    fn config(&self) -> PathBuf {
        self.0.join("config.toml")
    }
    /// The config file as written (`None`: never written).
    fn written(&self) -> Option<String> {
        std::fs::read_to_string(self.config()).ok()
    }
    fn scripts(&self) -> PathBuf {
        self.0.join("data").join("scripts")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `local-pg` at the top and `v6` in `a/b` (open in the explorer); the folders `a`, `a/b`,
/// `a/b/c` and `z`. The config is saved to `dir`'s file.
fn profiles(dir: &Scratch) -> Harness {
    let mut cfg = Config {
        connections: vec![
            ConnectionConfig::test_db(),
            ConnectionConfig { name: "v6".into(), folder: Some("a/b".into()), ..ConnectionConfig::test_db() },
        ],
        ..Config::default()
    };
    for f in ["a/b/c", "z"] {
        cfg.folders.insert(&FolderPath::parse(f).unwrap());
    }
    cfg.path = Some(dir.config());
    let mut h = Harness::launched(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal);
    // The explorer shows `v6`.
    let open = ["a", "a/b"].map(|f| FolderPath::parse(f).unwrap());
    h.app.folders.set_expanded(open.iter());
    h
}

/// The saved queries `a/q.sql` and `top.sql` in the folders `a`, `a/b`, `a/b/c` and `z`.
fn scripts(dir: &Scratch) -> Harness {
    let s = dir.scripts();
    std::fs::create_dir_all(s.join("a/b/c")).unwrap();
    std::fs::create_dir_all(s.join("z")).unwrap();
    std::fs::write(s.join("a/q.sql"), "select 1").unwrap();
    std::fs::write(s.join("top.sql"), "select 2").unwrap();
    let cfg = Config { connections: vec![ConnectionConfig::test_db()], ..Config::default() };
    let (mut app, clock) = new_app_with_clock(&cfg, Lang::En);
    let store = Arc::new(MemoryStore::new());
    app.set_secret_store(store.clone() as Arc<dyn SecretStore>);
    app.set_paths(Paths { data: Some(dir.0.join("data")), state: Some(dir.0.join("state")) });
    app.launch(Startup::Normal);
    let driver = FakeDriver::default();
    Harness { app, cancelled: driver.any_cancel.clone(), driver, store, clock }
}

/// Put the explorer's cursor on the row shown as `text`.
fn select_row(h: &mut Harness, text: &str) {
    h.app.focus = Focus::Tree;
    let rows = h.app.explorer_rows();
    let shown = h.rows();
    let i = shown.iter().position(|r| r.trim() == text).unwrap_or_else(|| panic!("no row {text} in {shown:?}"));
    h.app.explorer.select(&rows, i);
}

/// The picker's rows: `top` for the top, a folder as its path.
fn rows(h: &Harness) -> Vec<String> {
    let t = h.app.overlays.script_tree().expect("the folder picker");
    t.rows()
        .into_iter()
        .map(|(r, _)| match r {
            TreeRow::Folder(p) => p,
            TreeRow::Top => "top".into(),
            r => panic!("not a folder row: {r:?}"),
        })
        .collect()
}

fn row(h: &Harness) -> TreeRow {
    h.app.overlays.script_tree().expect("the folder picker").row.clone()
}

fn folder(p: &str) -> TreeRow {
    TreeRow::Folder(p.into())
}

fn profile_folder(h: &Harness, name: &str) -> Option<String> {
    h.app.profiles.iter().find(|p| p.name == name).unwrap().folder.clone()
}

fn mode(h: &Harness) -> TreeMode {
    h.app.overlays.script_tree().expect("the folder picker").mode
}

/// `m` on `v6`: the tree on its folder `a/b`, `a` open; `a/b/c` is inside the closed `a/b`.
#[test]
fn moving_a_profile_opens_the_tree_on_its_folder() {
    let dir = Scratch::new("profile-open");
    let mut h = profiles(&dir);
    h.explore("v6");
    h.keys("m");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree));
    assert!(matches!(mode(&h), TreeMode::Folder(FolderPick::MoveProfile(_))));
    assert_eq!(h.app.overlays.script_tree().unwrap().focus, TreeFocus::Tree);
    assert_eq!(rows(&h), ["top", "a", "a/b", "z"]);
    assert_eq!(row(&h), folder("a/b"));
    insta::assert_snapshot!("folder_pick_move_profile_en_80x24", h.draw(80, 24).backend());
    // With icons, a folder has the folder glyph; the top has none.
    h.app.icons = IconsSetting::On;
    let screen = h.screen(80, 24);
    assert!(screen.contains("\u{f07b} a/") && screen.contains("▸ \u{f07b} b/"), "{screen}");
    assert!(screen.contains("▾ (top level)"), "{screen}");
}

/// `→` opens `a/b`, `↓` goes to `a/b/c`, `Enter` moves the profile there and saves.
#[test]
fn a_profile_moves_to_a_nested_folder_and_to_the_top() {
    let dir = Scratch::new("profile-pick");
    let mut h = profiles(&dir);
    h.explore("v6");
    h.keys("m");
    h.keys("l");
    assert_eq!(rows(&h), ["top", "a", "a/b", "a/b/c", "z"]);
    h.keys("j");
    h.key(KeyCode::Enter);
    assert!(h.overlay_kind().is_none());
    assert_eq!(profile_folder(&h, "v6").as_deref(), Some("a/b/c"));
    assert!(dir.written().unwrap().contains("folder = \"a/b/c\""));
    assert!(h.status(160, 45).contains("Moved v6 to a/b/c/"));
    // `←` goes to the parent (a closed folder) or closes (an open one); then the top.
    h.explore("v6");
    h.keys("m");
    assert_eq!(row(&h), folder("a/b/c"));
    h.keys("h");
    assert_eq!(row(&h), folder("a/b"));
    h.keys("h");
    assert_eq!((row(&h), rows(&h)), (folder("a/b"), vec!["top".to_string(), "a".into(), "a/b".into(), "z".into()]));
    h.keys("hhh");
    assert_eq!(row(&h), TreeRow::Top);
    h.key(KeyCode::Enter);
    assert_eq!(profile_folder(&h, "v6"), None);
    assert!(!dir.written().unwrap().contains("folder = \"a/b"));
}

/// `/` filters by name: `c` keeps `a` and `a/b` (on the way, shown open) and hides `z`; the
/// selection goes to the match. `Enter` goes back to the tree, `Enter` again picks.
#[test]
fn a_filter_keeps_the_way_to_a_match() {
    let dir = Scratch::new("profile-filter");
    let mut h = profiles(&dir);
    h.explore("local-pg");
    h.keys("m");
    assert_eq!(row(&h), TreeRow::Top, "a profile at the top level");
    h.keys("/");
    assert_eq!(h.app.overlays.script_tree().unwrap().focus, TreeFocus::Name);
    h.type_text("C");
    assert_eq!(rows(&h), ["top", "a", "a/b", "a/b/c"]);
    assert_eq!(row(&h), folder("a/b/c"));
    let t = h.app.overlays.script_tree().unwrap();
    assert_eq!((t.is_open(&folder("a")), t.is_open(&folder("a/b"))), (Some(true), Some(true)));
    // `↑`/`↓` move the selection; the keyboard stays on the filter.
    h.key(KeyCode::Up);
    assert_eq!(row(&h), folder("a/b"));
    h.key(KeyCode::Down);
    assert_eq!(row(&h), folder("a/b/c"));
    assert_eq!(h.app.overlays.script_tree().unwrap().focus, TreeFocus::Name);
    // `Esc` clears the filter (the tree is back); `/` and typing again.
    h.key(KeyCode::Esc);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree));
    assert_eq!(rows(&h), ["top", "a", "a/b", "a/b/c", "z"], "the selection stays in sight");
    assert_eq!(row(&h), folder("a/b/c"));
    h.keys("/");
    h.type_text("z");
    assert_eq!(rows(&h), ["top", "z"]);
    h.key(KeyCode::Enter);
    assert_eq!(h.app.overlays.script_tree().unwrap().focus, TreeFocus::Tree);
    h.key(KeyCode::Enter);
    assert_eq!(profile_folder(&h, "local-pg").as_deref(), Some("z"));
}

/// `n` makes a row only: nothing is written until it is picked, and `Esc` drops it. Picked,
/// the folder and the move are saved together.
#[test]
fn a_new_profile_folder_is_saved_only_when_picked() {
    let dir = Scratch::new("profile-new");
    let mut h = profiles(&dir);
    let before = dir.written();
    h.explore("v6");
    h.keys("m");
    h.keys("n");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::NameInput));
    h.type_text("new");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree));
    assert_eq!(rows(&h), ["top", "a", "a/b", "a/b/c", "a/b/new", "z"], "a/b opens");
    assert_eq!(row(&h), folder("a/b/new"));
    // A name that is there, or an invalid one, is refused.
    h.keys("h");
    h.keys("n");
    h.type_text("new");
    h.key(KeyCode::Enter);
    assert!(h.screen(80, 24).contains("that folder exists already"));
    // Beside `c`, `C` is the same name (names ignore case); a name with spaces or dots is no
    // name.
    for (typed, error) in
        [("C", Label::ValidateFolderExists), ("x /y", Label::ValidateFolderSpaces), ("..", Label::ValidateFolderDots)]
    {
        h.ctrl('u');
        h.type_text(typed);
        h.key(KeyCode::Enter);
        assert_eq!(h.app.overlays.name_input().map(|n| n.error), Some(Some(error)), "{typed:?}");
    }
    h.key(KeyCode::Esc);
    h.key(KeyCode::Esc);
    assert!(h.overlay_kind().is_none());
    assert_eq!(dir.written(), before, "browsing wrote the config");
    assert!(!h.app.folders.iter().any(|f| f.as_str() == "a/b/new"));
    assert_eq!(profile_folder(&h, "v6").as_deref(), Some("a/b"));
    // Picked: the folder and the move.
    h.explore("v6");
    h.keys("m");
    h.keys("n");
    h.type_text("new");
    h.key(KeyCode::Enter);
    h.key(KeyCode::Enter);
    assert_eq!(profile_folder(&h, "v6").as_deref(), Some("a/b/new"));
    assert!(h.app.folders.iter().any(|f| f.as_str() == "a/b/new"));
    let written = dir.written().unwrap();
    assert!(written.contains("\"a/b/new\"") && written.contains("folder = \"a/b/new\""), "{written}");
}

/// `Esc` (and `q`) leave everything as it was.
#[test]
fn cancel_changes_nothing() {
    let dir = Scratch::new("profile-cancel");
    let mut h = profiles(&dir);
    let before = dir.written();
    h.explore("v6");
    for k in [KeyCode::Esc, KeyCode::Char('q')] {
        h.keys("m");
        h.keys("kk");
        h.key(k);
        assert!(h.overlay_kind().is_none());
        assert_eq!(profile_folder(&h, "v6").as_deref(), Some("a/b"));
    }
    assert_eq!(dir.written(), before);
}

/// The mouse: a click selects, one on a folder's arrow opens or closes it, one again on the
/// selected row picks it; the wheel scrolls; a click on the filter puts the keyboard there.
#[test]
fn the_mouse_selects_toggles_picks_and_scrolls() {
    let dir = Scratch::new("profile-mouse");
    let mut h = profiles(&dir);
    for i in 0..20 {
        h.app.folders.insert(&FolderPath::parse(&format!("f{i:02}")).unwrap());
    }
    h.explore("v6");
    h.keys("m");
    h.draw(80, 24);
    let list = h.app.overlays.script_tree().unwrap().list;
    let click = |h: &mut Harness, x: u16, y: u16| {
        h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
        h.draw(80, 24);
    };
    assert_eq!(rows(&h)[..4], ["top", "a", "a/b", "f00"]);
    click(&mut h, list.x + 6, list.y);
    assert_eq!(row(&h), TreeRow::Top, "the top row");
    click(&mut h, list.x + 6, list.y + 1);
    assert_eq!((row(&h), &rows(&h)[..4]), (folder("a"), &["top", "a", "a/b", "f00"].map(String::from)[..]));
    // The arrow of the selected `a` (depth 1: columns 2 and 3) closes and opens it, no pick.
    click(&mut h, list.x + 3, list.y + 1);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree), "the arrow does not pick");
    assert_eq!(rows(&h)[..3], ["top", "a", "f00"]);
    click(&mut h, list.x + 2, list.y + 1);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree));
    assert_eq!(rows(&h)[..4], ["top", "a", "a/b", "f00"]);
    // `a/b` (depth 2: its arrow at columns 4 and 5).
    click(&mut h, list.x + 4, list.y + 2);
    assert_eq!(rows(&h)[..5], ["top", "a", "a/b", "a/b/c", "f00"], "the arrow opens it");
    click(&mut h, list.x + 5, list.y + 2);
    assert_eq!(rows(&h)[..4], ["top", "a", "a/b", "f00"], "and closes it");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree));
    click(&mut h, list.x + 6, list.y + 3);
    assert_eq!(row(&h), folder("f00"));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree), "one click selects only");
    click(&mut h, list.x + 6, list.y + 3);
    assert!(h.overlay_kind().is_none(), "again on the selected row: picked");
    assert_eq!(profile_folder(&h, "v6").as_deref(), Some("f00"));
    // The wheel scrolls the rows, the selection stays.
    h.keys("m");
    h.draw(80, 24);
    let list = h.app.overlays.script_tree().unwrap().list;
    assert!(rows(&h).len() > list.height as usize, "more rows than the list shows");
    h.mouse(MouseEventKind::ScrollDown, list.x + 6, list.y + 1);
    h.draw(80, 24);
    let t = h.app.overlays.script_tree().unwrap();
    assert!(t.scroll > 0);
    assert_eq!(t.row, folder("f00"));
    // A click on the filter field.
    click(&mut h, list.x + 12, list.y + list.height + 1);
    assert_eq!(h.app.overlays.script_tree().unwrap().focus, TreeFocus::Name);
    h.type_text("f19");
    assert_eq!(row(&h), folder("f19"));
}

/// The Folder field of the profile form: the tree on its folder; the pick sets the field and
/// the config is written only when the form is saved (a new folder with it).
#[test]
fn the_profile_forms_folder_is_picked_from_the_tree() {
    let dir = Scratch::new("form");
    let mut h = profiles(&dir);
    let before = dir.written();
    h.explore("v6");
    h.keys("e");
    h.app.overlays.form_mut().unwrap().focus_field(Field::Folder);
    h.key(KeyCode::Enter);
    assert_eq!(mode(&h), TreeMode::Folder(FolderPick::ProfileForm));
    assert_eq!((rows(&h), row(&h)), (vec!["top".to_string(), "a".into(), "a/b".into(), "z".into()], folder("a/b")));
    insta::assert_snapshot!("folder_pick_profile_form_en_80x24", h.draw(80, 24).backend());
    // A nested folder.
    h.keys("lj");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ProfileForm));
    assert_eq!(h.form().folder.as_deref(), Some("a/b/c"));
    // The top level, through the filter: `(top level)` stays.
    h.key(KeyCode::Enter);
    h.keys("/");
    h.type_text("b");
    assert_eq!(rows(&h), ["top", "a", "a/b"]);
    assert_eq!(row(&h), folder("a/b"), "a/b/c does not match: the first match");
    h.key(KeyCode::Tab);
    h.keys("kk");
    h.key(KeyCode::Enter);
    assert_eq!(h.form().folder, None);
    // Cancel leaves the field.
    h.key(KeyCode::Enter);
    h.keys("j");
    h.key(KeyCode::Esc);
    assert_eq!(h.form().folder, None);
    // A new folder: the field's value and a choice; nothing written before the form is saved.
    h.key(KeyCode::Enter);
    h.keys("jn");
    h.type_text("fresh");
    h.key(KeyCode::Enter);
    assert_eq!(row(&h), folder("a/fresh"));
    h.key(KeyCode::Enter);
    assert_eq!(h.form().folder.as_deref(), Some("a/fresh"));
    assert!(h.form().folders.iter().any(|f| f == "a/fresh"));
    assert_eq!(dir.written(), before, "the pick wrote the config");
    // The tree shows the field's folder again (not yet in the config).
    h.key(KeyCode::Enter);
    assert_eq!(row(&h), folder("a/fresh"));
    h.key(KeyCode::Esc);
    h.ctrl('s');
    assert!(!h.form_open());
    assert_eq!(profile_folder(&h, "v6").as_deref(), Some("a/fresh"));
    assert!(h.app.folders.iter().any(|f| f.as_str() == "a/fresh"));
    assert!(dir.written().unwrap().contains("folder = \"a/fresh\""));
}

/// `m` on a saved query: every folder, its own selected; on a folder: it and what it holds are
/// no rows.
#[test]
fn moving_a_saved_query_or_folder_lists_the_other_folders() {
    let dir = Scratch::new("scripts-open");
    let mut h = scripts(&dir);
    select_row(&mut h, "a/");
    h.keys("l");
    select_row(&mut h, "q");
    h.keys("m");
    assert_eq!(mode(&h), TreeMode::Folder(FolderPick::MoveScript));
    assert_eq!((rows(&h), row(&h)), (vec!["top".to_string(), "a".into(), "z".into()], folder("a")));
    h.keys("l");
    assert_eq!(rows(&h), ["top", "a", "a/b", "z"], "a saved query: no folder is left out");
    h.key(KeyCode::Esc);
    select_row(&mut h, "b/");
    h.keys("m");
    assert_eq!((rows(&h), row(&h)), (vec!["top".to_string(), "a".into(), "z".into()], folder("a")));
    insta::assert_snapshot!("folder_pick_move_script_en_80x24", h.draw(80, 24).backend());
    // Its own parent: nothing moves, nothing is said.
    h.app.status = None;
    h.key(KeyCode::Enter);
    assert!(h.overlay_kind().is_none());
    assert!(dir.scripts().join("a/b/c").is_dir());
    assert!(h.app.status.is_none(), "a notice: {:?}", h.app.status);
    // Into `z`, then back to the top through the filter.
    select_row(&mut h, "b/");
    h.keys("m");
    h.keys("/");
    h.type_text("z");
    h.key(KeyCode::Enter);
    h.key(KeyCode::Enter);
    assert!(dir.scripts().join("z/b/c").is_dir() && !dir.scripts().join("a/b").exists());
    select_row(&mut h, "q");
    h.keys("m");
    h.keys("k");
    assert_eq!(row(&h), TreeRow::Top);
    h.key(KeyCode::Enter);
    assert!(dir.scripts().join("q.sql").is_file() && !dir.scripts().join("a/q.sql").exists());
}

/// `n` in the picker of the saved queries makes the folder at once and selects it; the list
/// keeps out the moved folder.
#[test]
fn a_new_folder_of_the_saved_queries_is_made_at_once() {
    let dir = Scratch::new("scripts-new");
    let mut h = scripts(&dir);
    select_row(&mut h, "z/");
    h.keys("m");
    assert_eq!((rows(&h), row(&h)), (vec!["top".to_string(), "a".into()], TreeRow::Top));
    h.keys("j");
    assert_eq!(row(&h), folder("a"));
    h.keys("n");
    h.type_text("made");
    h.key(KeyCode::Enter);
    assert!(dir.scripts().join("a/made").is_dir());
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree));
    assert_eq!(rows(&h), ["top", "a", "a/b", "a/made"]);
    assert_eq!(row(&h), folder("a/made"));
    assert_eq!(h.app.overlays.script_tree().unwrap().focus, TreeFocus::Tree);
    // Cancel: `z` stays where it was.
    h.key(KeyCode::Esc);
    assert!(dir.scripts().join("z").is_dir() && !dir.scripts().join("a/made/z").exists());
    // Picked.
    select_row(&mut h, "z/");
    h.keys("m");
    h.keys("/");
    h.type_text("made");
    h.key(KeyCode::Enter);
    h.key(KeyCode::Enter);
    assert!(dir.scripts().join("a/made/z").is_dir());
}

/// Folders `A`, `a` and `A/x`: `A/x` is under `A`, siblings by name ignoring case.
#[test]
fn folders_differing_in_case_keep_their_own_children() {
    let dir = Scratch::new("profile-case");
    let mut h = profiles(&dir);
    h.app.folders = Default::default();
    for f in ["a", "A/x"] {
        h.app.folders.insert(&FolderPath::parse(f).unwrap());
    }
    h.explore("local-pg");
    h.keys("m");
    assert_eq!(rows(&h), ["top", "A", "a"]);
    h.keys("jl");
    assert_eq!(rows(&h), ["top", "A", "A/x", "a"]);
    h.keys("j");
    h.key(KeyCode::Enter);
    assert_eq!(profile_folder(&h, "local-pg").as_deref(), Some("A/x"));
}

/// A folder of the saved queries that cannot be read is not picked: the dialog says so and
/// stays (`Enter` and a click again on it).
#[cfg(unix)]
#[test]
fn an_unreadable_folder_is_not_picked() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Scratch::new("scripts-unreadable");
    let z = dir.scripts().join("z");
    std::fs::create_dir_all(&z).unwrap();
    std::fs::set_permissions(&z, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut h = scripts(&dir);
    select_row(&mut h, "top");
    h.keys("m");
    h.keys("jj");
    assert_eq!(row(&h), folder("z"));
    assert!(h.app.overlays.script_tree().unwrap().unreadable(&folder("z")));
    h.key(KeyCode::Enter);
    let error = |h: &Harness| h.app.overlays.script_tree().and_then(|t| t.error.clone());
    assert_eq!(error(&h), Some(datarig_core::i18n::Msg::Label(Label::ValidateScriptUnreadable)));
    h.draw(80, 24);
    let list = h.app.overlays.script_tree().unwrap().list;
    h.app.overlays.script_tree_mut().unwrap().error = None;
    h.mouse(MouseEventKind::Down(MouseButton::Left), list.x + 6, list.y + 2);
    assert_eq!(error(&h), Some(datarig_core::i18n::Msg::Label(Label::ValidateScriptUnreadable)));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree));
    std::fs::set_permissions(&z, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(dir.scripts().join("top.sql").is_file(), "nothing moved");
}
