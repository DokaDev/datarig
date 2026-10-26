//! Where the export dialog writes: the tree dialog over the file system (`Ctrl+O`, the
//! `[Browse…]` button) and `Tab` completion in the path field. Every test runs on a scratch
//! home folder (`HOME` is replaced through the app's environment lookup); the user's home is
//! never looked at.

mod common;

use common::*;
use datarig_core::config::IconsSetting;
use datarig_core::driver::{ColumnMeta, DbEvent};
use datarig_core::i18n::Lang;
use datarig_tui::app::export_form::ExportField;
use datarig_tui::app::folder_read::Kind;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::script_tree::{TreeFocus, TreeMode, TreeRow};
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);

/// A scratch home folder, removed when dropped (its locked folders opened first).
struct Home(PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let locked = self.0.join("Downloads").join("locked");
            let _ = std::fs::set_permissions(locked, std::fs::Permissions::from_mode(0o755));
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A home folder with `Downloads`: folders `a` (with `inner.csv`) and `.cache`, files
/// `b.csv`, `report-1.csv`, `report-2.csv` and `.secret`; and a folder `proj` beside it.
fn home(tag: &str) -> Home {
    let h = Home(std::env::temp_dir().join(format!("datarig-export-path-{tag}-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&h.0);
    let d = h.0.join("Downloads");
    std::fs::create_dir_all(d.join("a")).unwrap();
    std::fs::create_dir_all(d.join(".cache")).unwrap();
    std::fs::create_dir_all(h.0.join("proj")).unwrap();
    for f in ["b.csv", "report-1.csv", "report-2.csv", ".secret", "a/inner.csv"] {
        std::fs::write(d.join(f), "old").unwrap();
    }
    h
}

fn meta(name: &str) -> ColumnMeta {
    common::meta(name, "text", false, false)
}

/// The dialog open over a first page of two rows, `HOME` the scratch home.
fn dialog(home: &Home) -> Harness {
    let mut h = Harness::connected(Lang::En);
    let dir = home.0.display().to_string();
    h.app.set_env_lookup(std::sync::Arc::new(move |k: &str| (k == "HOME").then(|| dir.clone())));
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let rows = vec![vec![Some("x".to_string())], vec![Some("y".to_string())]];
    h.tab_db(index, DbEvent::Page { id, columns: Some(vec![meta("v")]), rows, more: false, elapsed: Duration::ZERO });
    h.sent();
    h.key(KeyCode::Tab);
    h.keys(" re");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Export), "the dialog opened");
    h
}

fn path_text(h: &Harness) -> String {
    h.app.overlays.export_form().expect("the dialog").path.text().to_string()
}

fn focus(h: &Harness) -> ExportField {
    h.app.overlays.export_form().expect("the dialog").focus
}

/// The made file name of the dialog (after `~/Downloads/`).
fn made_name(h: &Harness) -> String {
    path_text(h).rsplit('/').next().unwrap().to_string()
}

fn tab_to(h: &mut Harness, f: ExportField) {
    for _ in 0..12 {
        if focus(h) == f {
            return;
        }
        h.key(KeyCode::Down);
    }
    panic!("no field {f:?}");
}

/// Replace the path field's text.
fn set_path(h: &mut Harness, text: &str) {
    tab_to(h, ExportField::Path);
    h.ctrl('u');
    h.type_text(text);
}

fn root(h: &Harness) -> PathBuf {
    let t = h.app.overlays.script_tree().expect("the picker is open");
    assert!(matches!(t.mode, TreeMode::ExportPath(_)));
    t.root.clone()
}

fn row(h: &Harness) -> TreeRow {
    h.app.overlays.script_tree().expect("the picker").row.clone()
}

fn rows(h: &Harness) -> Vec<TreeRow> {
    h.app.overlays.script_tree().expect("the picker").rows().into_iter().map(|(r, _)| r).collect()
}

/// What the picker says (the unreadable and loop tests, Unix only).
#[cfg(unix)]
fn tree_error(h: &Harness) -> Option<String> {
    h.app.overlays.script_tree().and_then(|t| t.error.as_ref()).map(|m| h.app.i18n.msg(m).to_string())
}

/// Select row `r` with the arrows, from the first row down.
fn go_to(h: &mut Harness, r: TreeRow) {
    for _ in 0..rows(h).len() {
        h.key(KeyCode::Up);
    }
    for _ in 0..rows(h).len() {
        if row(h) == r {
            return;
        }
        h.key(KeyCode::Down);
    }
    panic!("no row {r:?} in {:?}", rows(h));
}

/// The foreground color of the first cell of `text` on screen.
fn color_of(h: &mut Harness, text: &str) -> ratatui::style::Color {
    let t = h.draw(120, 40);
    let buf = t.backend().buffer();
    for y in 0..40 {
        let row = row_text(buf, y);
        if let Some(i) = row.find(&format!("{text} ")) {
            let x = datarig_tui::text::width(&row[..i]) as u16;
            return buf[(x, y)].fg;
        }
    }
    panic!("{text} not on screen:\n{}", h.screen(120, 40));
}

fn pump_until_done(h: &mut Harness) {
    let start = std::time::Instant::now();
    while h.app.exporting(h.app.tab().id) {
        assert!(start.elapsed() < WAIT, "the export did not end");
        h.app.pump_exports(Duration::from_millis(50));
    }
}

#[test]
fn ctrl_o_opens_at_the_fields_folder_and_save_here_keeps_the_name() {
    let home = home("open");
    let mut h = dialog(&home);
    let name = made_name(&h);
    h.ctrl('o');
    assert_eq!(root(&h), home.0.join("Downloads"), "the made path's folder");
    assert_eq!(focus(&h), ExportField::Path);
    let screen = h.screen(120, 40);
    assert!(
        screen.contains("Export to") && screen.contains("~/Downloads/") && screen.contains("Save here"),
        "{screen}"
    );
    for listed in ["../", "a/", "b.csv", "report-1.csv"] {
        assert!(screen.contains(listed), "{listed} listed:\n{screen}");
    }
    assert!(!screen.contains(".cache") && !screen.contains(".secret"), "hidden names wait for `.`:\n{screen}");
    insta::assert_snapshot!("export_path_picker_en_120x40", h.draw(120, 40).backend());
    // Files are dimmed: they are what a file of that name would replace.
    assert_eq!(color_of(&mut h, "b.csv"), datarig_tui::theme::DARK.fg_dim);
    assert_ne!(color_of(&mut h, "a/"), datarig_tui::theme::DARK.fg_dim);
    // Enter on the top row ("Save here"): that folder, the field's file name.
    assert_eq!(row(&h), TreeRow::Top);
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Export), "back in the dialog");
    assert_eq!(path_text(&h), format!("~/Downloads/{name}"));
    // Ctrl+S on a folder: that folder.
    h.ctrl('o');
    go_to(&mut h, TreeRow::Folder("a".into()));
    h.ctrl('s');
    assert_eq!(path_text(&h), format!("~/Downloads/a/{name}"));
    assert_eq!(focus(&h), ExportField::Path);
    // Opened again: at `a`, the file name typed kept by Save here.
    set_path(&mut h, "~/Downloads/a/mine.json");
    h.ctrl('o');
    assert_eq!(root(&h), home.0.join("Downloads/a"));
    h.key(KeyCode::Left); // up from the top row
    assert_eq!(root(&h), home.0.join("Downloads"));
    assert_eq!(row(&h), TreeRow::Folder("a".into()), "the folder left is selected");
    h.key(KeyCode::Up);
    assert_eq!(row(&h), TreeRow::Top);
    h.ctrl('s');
    assert_eq!(path_text(&h), "~/Downloads/mine.json");
}

#[test]
fn a_missing_folder_opens_downloads_and_a_path_beside_it_opens_there() {
    let home = home("where");
    let mut h = dialog(&home);
    set_path(&mut h, "~/nowhere/x.csv");
    h.ctrl('o');
    assert_eq!(root(&h), home.0.join("Downloads"), "a folder that is not there: ~/Downloads");
    h.key(KeyCode::Esc);
    std::fs::write(home.0.join("proj/out.csv"), "old").unwrap();
    set_path(&mut h, "~/proj/out.csv");
    h.ctrl('o');
    assert_eq!(root(&h), home.0.join("proj"));
    assert_eq!(row(&h), TreeRow::File("out.csv".into()), "on the field's file");
    h.key(KeyCode::Esc);
    // An absolute path too.
    let abs = home.0.join("proj").display().to_string();
    set_path(&mut h, &format!("{abs}/new.csv"));
    h.ctrl('o');
    assert_eq!(root(&h), home.0.join("proj"));
}

#[test]
fn the_arrows_open_and_close_a_folder_while_the_empty_filter_has_the_keyboard() {
    let home = home("arrows");
    let mut h = dialog(&home);
    h.ctrl('o');
    h.keys("/");
    let focus = |h: &Harness| h.app.overlays.script_tree().unwrap().focus;
    assert_eq!(focus(&h), TreeFocus::Name);
    // On the folder shown they do not go up (the tree's `←` does).
    go_to(&mut h, TreeRow::Top);
    h.key(KeyCode::Left);
    assert_eq!(root(&h), home.0.join("Downloads"));
    assert_eq!(row(&h), TreeRow::Top);
    go_to(&mut h, TreeRow::Folder("a".into()));
    // `a` was never listed: `→` reads it and opens it.
    h.key(KeyCode::Right);
    assert!(rows(&h).contains(&TreeRow::File("a/inner.csv".into())), "{:?}", rows(&h));
    h.key(KeyCode::Left);
    assert!(!rows(&h).contains(&TreeRow::File("a/inner.csv".into())), "`←` closed it");
    assert_eq!(focus(&h), TreeFocus::Name, "the filter keeps the keyboard");
}

#[test]
fn with_icons_folders_and_files_have_glyphs_and_the_arrow_still_takes_a_click() {
    let home = home("icons");
    let mut h = dialog(&home);
    h.ctrl('o');
    let screen = h.screen(120, 40);
    assert!(screen.contains("▸ a/") && screen.contains("  b.csv"), "text only without icons:\n{screen}");
    assert!(!screen.contains('\u{f07b}') && !screen.contains('\u{f15b}'), "{screen}");
    h.app.icons = IconsSetting::On;
    let screen = h.screen(120, 40);
    assert!(screen.contains("▸ \u{f07b} a/") && screen.contains("\u{f15b} b.csv"), "{screen}");
    assert!(screen.contains("▾ ~/Downloads/"), "the folder shown has none:\n{screen}");
    // The arrow (two columns per level) still opens the folder.
    let list = h.app.overlays.script_tree().unwrap().list;
    let i = rows(&h).iter().position(|r| *r == TreeRow::Folder("a".into())).unwrap() as u16;
    assert_ne!(row(&h), TreeRow::Folder("a".into()), "a first click, not a second one");
    h.mouse(MouseEventKind::Down(MouseButton::Left), list.x + 2, list.y + i);
    assert!(rows(&h).contains(&TreeRow::File("a/inner.csv".into())), "{:?}", rows(&h));
}

#[test]
fn folders_open_one_level_at_a_time_and_esc_leaves_the_dialog_as_it_was() {
    let home = home("nav");
    let mut h = dialog(&home);
    let before = path_text(&h);
    h.ctrl('o');
    // Only the folder shown is listed until a folder opens.
    let t = h.app.overlays.script_tree().unwrap();
    assert!(!t.entries.iter().any(|e| e.path == "a/inner.csv"), "not listed before `a` opens");
    go_to(&mut h, TreeRow::Folder("a".into()));
    h.key(KeyCode::Enter);
    assert!(rows(&h).contains(&TreeRow::File("a/inner.csv".into())), "Enter opened it");
    h.keys("h");
    assert!(!rows(&h).contains(&TreeRow::File("a/inner.csv".into())), "`h` closed it");
    h.keys("l");
    h.keys("j");
    assert_eq!(row(&h), TreeRow::File("a/inner.csv".into()));
    h.keys("h");
    assert_eq!(row(&h), TreeRow::Folder("a".into()), "`h` on a file: its folder");
    // `../` goes up; Enter on it too.
    go_to(&mut h, TreeRow::Top);
    h.key(KeyCode::Up);
    assert_eq!(row(&h), TreeRow::Up);
    h.key(KeyCode::Enter);
    assert_eq!(root(&h), home.0);
    assert_eq!(row(&h), TreeRow::Folder("Downloads".into()));
    h.keys("l");
    assert!(rows(&h).contains(&TreeRow::Folder("Downloads/a".into())));
    // Esc: the dialog as it was.
    h.key(KeyCode::Esc);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Export));
    assert_eq!(path_text(&h), before);
    assert!(h.app.overlays.export_form().unwrap().error.is_none());
    h.ctrl('o');
    h.keys("q");
    assert_eq!(path_text(&h), before, "q cancels too");
}

#[test]
fn typing_filters_and_dot_shows_hidden_names() {
    let home = home("filter");
    let mut h = dialog(&home);
    h.ctrl('o');
    h.keys("/");
    assert_eq!(h.app.overlays.script_tree().unwrap().focus, TreeFocus::Name);
    h.type_text("rep");
    assert_eq!(
        rows(&h),
        vec![TreeRow::Up, TreeRow::Top, TreeRow::File("report-1.csv".into()), TreeRow::File("report-2.csv".into())]
    );
    assert_eq!(row(&h), TreeRow::File("report-1.csv".into()), "the first match is selected");
    h.key(KeyCode::Down);
    assert_eq!(row(&h), TreeRow::File("report-2.csv".into()));
    // Esc clears the filter and gives the keyboard back to the tree.
    h.key(KeyCode::Esc);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree));
    assert_eq!(h.app.overlays.script_tree().unwrap().focus, TreeFocus::Tree);
    assert!(rows(&h).contains(&TreeRow::Folder("a".into())));
    // `.` shows the hidden names, and hides them again.
    assert!(!rows(&h).contains(&TreeRow::Folder(".cache".into())));
    h.keys(".");
    assert!(rows(&h).contains(&TreeRow::Folder(".cache".into())));
    assert!(rows(&h).contains(&TreeRow::File(".secret".into())));
    go_to(&mut h, TreeRow::File(".secret".into()));
    h.keys(".");
    assert!(!rows(&h).contains(&TreeRow::File(".secret".into())));
    assert_eq!(row(&h), TreeRow::Top, "a hidden selection goes with it");
    // A typed path: a folder is shown.
    h.key(KeyCode::Tab);
    h.type_text("~/proj");
    h.key(KeyCode::Enter);
    assert_eq!(root(&h), home.0.join("proj"));
}

#[test]
fn enter_on_a_file_takes_it_and_export_still_asks_before_replacing_it() {
    let home = home("file");
    let mut h = dialog(&home);
    h.ctrl('o');
    go_to(&mut h, TreeRow::File("b.csv".into()));
    h.key(KeyCode::Enter);
    assert_eq!(path_text(&h), "~/Downloads/b.csv");
    tab_to(&mut h, ExportField::Export);
    h.key(KeyCode::Enter);
    let start = std::time::Instant::now();
    while h.app.overlays.confirm().is_none() {
        assert!(start.elapsed() < WAIT, "no question");
        h.app.pump_exports(Duration::from_millis(20));
    }
    assert_eq!(h.app.overlays.confirm().map(|c| c.action), Some(ConfirmAction::ExportReplace));
    h.draw(160, 45);
    h.advance(Duration::from_millis(600));
    h.key(KeyCode::Enter);
    pump_until_done(&mut h);
    assert_eq!(std::fs::read_to_string(home.0.join("Downloads/b.csv")).unwrap(), "old", "Enter keeps it");
}

#[test]
fn a_folder_picked_is_where_the_export_is_written() {
    let home = home("write");
    let mut h = dialog(&home);
    let name = made_name(&h);
    h.ctrl('o');
    h.key(KeyCode::Left);
    go_to(&mut h, TreeRow::Folder("proj".into()));
    h.ctrl('s');
    assert_eq!(path_text(&h), format!("~/proj/{name}"));
    tab_to(&mut h, ExportField::Export);
    h.key(KeyCode::Enter);
    pump_until_done(&mut h);
    let text = std::fs::read_to_string(home.0.join("proj").join(&name)).unwrap();
    assert_eq!(text, "v\nx\ny\n");
}

#[test]
fn n_makes_a_folder_and_never_reuses_one() {
    let home = home("new");
    let mut h = dialog(&home);
    h.ctrl('o');
    go_to(&mut h, TreeRow::Folder("a".into()));
    h.keys("n");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::NameInput));
    h.type_text("out");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree));
    assert!(home.0.join("Downloads/a/out").is_dir(), "made on disk");
    assert_eq!(row(&h), TreeRow::Folder("a/out".into()), "and selected");
    // The same name again is refused: the folder exists.
    go_to(&mut h, TreeRow::Folder("a".into()));
    h.keys("n");
    h.type_text("out");
    h.key(KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::NameInput), "still asking");
    let n = h.app.overlays.name_input().unwrap();
    assert_eq!(n.error.map(|l| h.app.i18n.label(l).to_string()).as_deref(), Some("that folder exists already"));
    h.ctrl('u');
    h.type_text("x/y");
    h.key(KeyCode::Enter);
    assert!(h.app.overlays.name_input().unwrap().error.is_some(), "one folder at a time");
    h.key(KeyCode::Esc);
    // At the top: beside the others.
    go_to(&mut h, TreeRow::Top);
    h.keys("n");
    h.type_text("top");
    h.key(KeyCode::Enter);
    assert!(home.0.join("Downloads/top").is_dir());
    assert_eq!(row(&h), TreeRow::Folder("top".into()));
}

/// A folder that cannot be listed says so: it is never shown empty, and nothing is saved there.
#[cfg(unix)]
#[test]
fn a_folder_that_cannot_be_read_says_so() {
    use std::os::unix::fs::PermissionsExt;
    let home = home("locked");
    let locked = home.0.join("Downloads/locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::write(locked.join("inside.csv"), "x").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&locked).is_ok() {
        return; // root reads anything: nothing to prove
    }
    let mut h = dialog(&home);
    h.ctrl('o');
    go_to(&mut h, TreeRow::Folder("locked".into()));
    h.key(KeyCode::Right);
    let screen = h.screen(120, 40);
    assert!(screen.contains("locked/ (can't be read)"), "{screen}");
    assert_eq!(rows(&h).iter().filter(|r| matches!(r, TreeRow::File(p) if p.starts_with("locked/"))).count(), 0);
    assert_eq!(h.app.overlays.script_tree().unwrap().is_open(&TreeRow::Folder("locked".into())), Some(false));
    h.ctrl('s');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree), "not picked");
    let e = tree_error(&h).expect("said why");
    assert!(e.contains("~/Downloads/locked can't be read"), "{e}");
    // Shown as the folder itself: the error, no rows taken for its contents.
    h.key(KeyCode::Esc);
    set_path(&mut h, "~/Downloads/locked/x.csv");
    h.ctrl('o');
    assert_eq!(root(&h), home.0.join("Downloads/locked"));
    assert!(tree_error(&h).is_some(), "the folder shown says it can't be read");
    assert_eq!(rows(&h), vec![TreeRow::Up, TreeRow::Top]);
}

/// A link back to a folder on the way to it is not opened; links elsewhere open one level.
#[cfg(unix)]
#[test]
fn a_link_that_loops_is_not_opened() {
    let home = home("loop");
    let d = home.0.join("Downloads");
    std::os::unix::fs::symlink(&d, d.join("again")).unwrap();
    std::os::unix::fs::symlink(&home.0, d.join("a").join("home")).unwrap();
    std::os::unix::fs::symlink(home.0.join("proj"), d.join("to-proj")).unwrap();
    std::fs::write(home.0.join("proj/p.csv"), "x").unwrap();
    let mut h = dialog(&home);
    h.ctrl('o');
    go_to(&mut h, TreeRow::Folder("again".into()));
    h.key(KeyCode::Right);
    assert_eq!(h.app.overlays.script_tree().unwrap().is_open(&TreeRow::Folder("again".into())), Some(false));
    assert!(tree_error(&h).is_some_and(|e| e.contains("again links back")), "{:?}", tree_error(&h));
    // Deeper: `a/home` is the home folder, which holds `Downloads`.
    go_to(&mut h, TreeRow::Folder("a".into()));
    h.key(KeyCode::Right);
    go_to(&mut h, TreeRow::Folder("a/home".into()));
    h.key(KeyCode::Right);
    assert!(!rows(&h).iter().any(|r| matches!(r, TreeRow::Folder(p) | TreeRow::File(p) if p.starts_with("a/home/"))));
    // A link elsewhere opens like a folder.
    go_to(&mut h, TreeRow::Folder("to-proj".into()));
    h.key(KeyCode::Right);
    assert!(rows(&h).contains(&TreeRow::File("to-proj/p.csv".into())));
}

#[test]
fn tab_completes_the_name_then_lists_then_cycles() {
    let home = home("tab");
    let mut h = dialog(&home);
    set_path(&mut h, "~/Down");
    h.key(KeyCode::Tab);
    assert_eq!(path_text(&h), "~/Downloads/", "a folder ends with /");
    assert_eq!(focus(&h), ExportField::Path, "Tab completed: the focus stays");
    h.type_text("re");
    h.key(KeyCode::Tab);
    assert_eq!(path_text(&h), "~/Downloads/report-", "as far as the names agree");
    h.key(KeyCode::Tab);
    assert_eq!(path_text(&h), "~/Downloads/report-");
    let screen = h.screen(160, 45);
    assert!(screen.contains("Tab again: report-1.csv · report-2.csv"), "listed:\n{screen}");
    h.key(KeyCode::Tab);
    assert_eq!(path_text(&h), "~/Downloads/report-1.csv");
    h.key(KeyCode::Tab);
    assert_eq!(path_text(&h), "~/Downloads/report-2.csv");
    h.key(KeyCode::Tab);
    assert_eq!(path_text(&h), "~/Downloads/report-1.csv", "around");
    // Typing starts over; hidden names only for a leading `.`.
    h.ctrl('u');
    h.type_text("~/Downloads/.c");
    h.key(KeyCode::Tab);
    assert_eq!(path_text(&h), "~/Downloads/.cache/");
    // A bare name is in ~/Downloads.
    h.ctrl('u');
    h.type_text("b");
    h.key(KeyCode::Tab);
    assert_eq!(path_text(&h), "b.csv");
    // Nothing to complete: Tab moves on as before.
    h.type_text("x");
    h.key(KeyCode::Tab);
    assert_eq!(path_text(&h), "b.csvx");
    assert_eq!(focus(&h), ExportField::Export);
}

#[test]
fn the_browse_button_and_the_mouse_pick() {
    let home = home("mouse");
    let mut h = dialog(&home);
    let t = h.draw(160, 45);
    let buf = t.backend().buffer();
    let (x, y) = (0..45u16)
        .find_map(|y| {
            let row = row_text(buf, y);
            row.find("[Browse…]").map(|i| (datarig_tui::text::width(&row[..i]) as u16, y))
        })
        .expect("the button");
    h.mouse(MouseEventKind::Down(MouseButton::Left), x + 2, y);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree));
    assert_eq!(root(&h), home.0.join("Downloads"));
    // A click selects, a second one on a file picks it.
    h.draw(160, 45);
    let list = h.app.overlays.script_tree().unwrap().list;
    let i = rows(&h).iter().position(|r| *r == TreeRow::File("b.csv".into())).unwrap() as u16;
    h.mouse(MouseEventKind::Down(MouseButton::Left), list.x + 4, list.y + i);
    assert_eq!(row(&h), TreeRow::File("b.csv".into()));
    h.mouse(MouseEventKind::Down(MouseButton::Left), list.x + 4, list.y + i);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Export));
    assert_eq!(path_text(&h), "~/Downloads/b.csv");
    // Twice on the top row: save here.
    h.ctrl('o');
    h.draw(160, 45);
    let list = h.app.overlays.script_tree().unwrap().list;
    h.mouse(MouseEventKind::Down(MouseButton::Left), list.x + 4, list.y + 1);
    h.mouse(MouseEventKind::Down(MouseButton::Left), list.x + 4, list.y + 1);
    assert_eq!(path_text(&h), "~/Downloads/b.csv", "the file name kept");
    let spec = datarig_tui::app::action::by_id("export.pick_path").expect("registered");
    assert!((spec.when)(&h.app));
    h.key(KeyCode::Esc);
    assert!(!(spec.when)(&h.app), "only in the export dialog");
}

/// The folder shown cannot be listed: it says so on its row, and keeps saying so after keys,
/// a filter and moves (it never reads as empty).
#[cfg(unix)]
#[test]
fn an_unreadable_folder_shown_stays_marked_after_keys() {
    use std::os::unix::fs::PermissionsExt;
    let home = home("locked-root");
    let locked = home.0.join("Downloads/locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&locked).is_ok() {
        return;
    }
    let mut h = dialog(&home);
    set_path(&mut h, "~/Downloads/locked/x.csv");
    h.ctrl('o');
    assert_eq!(root(&h), locked);
    let marked = |h: &mut Harness| {
        let screen = h.screen(120, 40);
        screen.contains("~/Downloads/locked/  Save here (can't be read)")
            || screen.contains("~/Downloads/locked/ (can't be read)")
            || screen.lines().any(|l| l.contains("~/Downloads/locked/") && l.contains("(can't be read)"))
    };
    assert!(marked(&mut h), "{}", h.screen(120, 40));
    h.key(KeyCode::Down);
    h.key(KeyCode::Up);
    assert!(marked(&mut h), "after moves:\n{}", h.screen(120, 40));
    h.keys("/");
    h.type_text("q");
    assert!(marked(&mut h), "after a filter:\n{}", h.screen(120, 40));
    let t = h.app.overlays.script_tree().unwrap();
    assert!(t.unreadable(&TreeRow::Top));
}

/// Coming up out of a hidden folder: its row is selected and shown, never selected unseen.
#[test]
fn going_up_out_of_a_hidden_folder_shows_it() {
    let home = home("hidden-up");
    let mut h = dialog(&home);
    set_path(&mut h, "~/Downloads/.cache/x.csv");
    h.ctrl('o');
    assert_eq!(root(&h), home.0.join("Downloads/.cache"));
    h.key(KeyCode::Left);
    assert_eq!(row(&h), TreeRow::Folder(".cache".into()));
    assert!(rows(&h).contains(&row(&h)), "the selection is a row on screen: {:?}", rows(&h));
}

/// A cycle made of two links is caught too: `x/l1` is `y`, whose `l2` is `x` again.
#[cfg(unix)]
#[test]
fn a_cycle_of_two_links_is_not_opened() {
    let home = home("loop2");
    let d = home.0.join("Downloads");
    std::fs::create_dir_all(d.join("x")).unwrap();
    std::fs::create_dir_all(d.join("y")).unwrap();
    std::os::unix::fs::symlink(d.join("y"), d.join("x/l1")).unwrap();
    std::os::unix::fs::symlink(d.join("x"), d.join("y/l2")).unwrap();
    let mut h = dialog(&home);
    h.ctrl('o');
    go_to(&mut h, TreeRow::Folder("x".into()));
    h.key(KeyCode::Right);
    go_to(&mut h, TreeRow::Folder("x/l1".into()));
    h.key(KeyCode::Right);
    go_to(&mut h, TreeRow::Folder("x/l1/l2".into()));
    h.key(KeyCode::Right);
    assert_eq!(h.app.overlays.script_tree().unwrap().is_open(&TreeRow::Folder("x/l1/l2".into())), Some(false));
    assert!(tree_error(&h).is_some_and(|e| e.contains("links back")), "{:?}", tree_error(&h));
}

/// The dialog goes stale under the picker and its new-folder prompt: all three close.
#[test]
fn a_stale_dialog_closes_the_picker_and_its_prompt() {
    let home = home("stale");
    let mut h = dialog(&home);
    h.key(KeyCode::Esc);
    h.ctrl('e');
    let qid = h.app.tab().exec.query_id;
    h.sent();
    h.keys(" re");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Export));
    h.ctrl('o');
    h.keys("n");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::NameInput));
    let index = h.app.tabs.active_index();
    let rows = vec![vec![Some("z".to_string())]];
    h.tab_db(
        index,
        DbEvent::Page { id: qid, columns: Some(vec![meta("v")]), rows, more: false, elapsed: Duration::ZERO },
    );
    assert!(h.app.overlays.script_tree().is_none(), "the picker closed");
    assert!(h.app.overlays.name_input().is_none(), "its prompt closed");
    assert_eq!(h.overlay_kind(), None);
}

/// A new folder inside `a` lists `a` again: a folder open below it is closed, never left
/// open and empty.
#[test]
fn a_new_folder_leaves_no_open_folder_empty() {
    let home = home("reopen");
    std::fs::create_dir_all(home.0.join("Downloads/a/sub")).unwrap();
    std::fs::write(home.0.join("Downloads/a/sub/deep.csv"), "x").unwrap();
    let mut h = dialog(&home);
    h.ctrl('o');
    go_to(&mut h, TreeRow::Folder("a".into()));
    h.key(KeyCode::Right);
    go_to(&mut h, TreeRow::Folder("a/sub".into()));
    h.key(KeyCode::Right);
    assert!(rows(&h).contains(&TreeRow::File("a/sub/deep.csv".into())));
    go_to(&mut h, TreeRow::Folder("a".into()));
    h.keys("n");
    h.type_text("made");
    h.key(KeyCode::Enter);
    let t = h.app.overlays.script_tree().unwrap();
    let open = t.is_open(&TreeRow::Folder("a/sub".into())) == Some(true);
    assert!(!open || rows(&h).contains(&TreeRow::File("a/sub/deep.csv".into())), "open and empty: {:?}", rows(&h));
}

/// A field that names a folder (no `/` at its end): Save here keeps the made file name.
#[test]
fn save_here_on_a_folder_typed_without_a_slash() {
    let home = home("noslash");
    let mut h = dialog(&home);
    let name = made_name(&h);
    set_path(&mut h, "~/proj");
    h.ctrl('o');
    assert_eq!(root(&h), home.0.join("proj"));
    h.key(KeyCode::Enter);
    assert_eq!(path_text(&h), format!("~/proj/{name}"));
}

/// `Tab` in an empty path field completes nothing: the focus moves on.
#[test]
fn tab_on_an_empty_path_moves_on() {
    let home = home("empty-tab");
    let mut h = dialog(&home);
    set_path(&mut h, "");
    h.key(KeyCode::Tab);
    assert_eq!(focus(&h), ExportField::Export);
}

// ── Folder reads on a worker ─────────────────────────────────────────────────────────────

/// The dialog over `home` with `fake` as the file system.
fn faked(home: &Home) -> (Harness, Arc<FakeFs>) {
    let mut h = dialog(home);
    let fake = Arc::new(FakeFs::default());
    h.app.set_file_system(fake.clone());
    (h, fake)
}

/// A key straight to the app: what a read started by it says is not waited for.
fn press(h: &mut Harness, code: KeyCode) {
    let k = KeyEvent::new(code, KeyModifiers::NONE);
    let start = std::time::Instant::now();
    h.app.handle_event(Event::Key(k));
    assert!(start.elapsed() < Duration::from_secs(2), "the key waited for the disk: {:?}", start.elapsed());
}

fn said(h: &Harness) -> Option<String> {
    h.app.overlays.script_tree().and_then(|t| t.error.as_ref()).map(|m| h.app.i18n.msg(m).to_string())
}

/// A folder that does not answer is read on a worker: its row says it is being read, the keys
/// keep answering, and its entries come in once it answers.
#[test]
fn keys_answer_while_a_folder_read_hangs() {
    let home = home("hang");
    let (mut h, fake) = faked(&home);
    fake.hang.lock().unwrap().push(home.0.join("Downloads/a"));
    h.ctrl('o');
    go_to(&mut h, TreeRow::Folder("a".into()));
    press(&mut h, KeyCode::Right);
    let screen = h.screen(120, 40);
    assert!(screen.contains("a/ (reading…)"), "{screen}");
    insta::assert_snapshot!("export_path_reading_en_120x40", h.draw(120, 40).backend());
    let t = h.app.overlays.script_tree().unwrap();
    assert_eq!(t.is_open(&TreeRow::Folder("a".into())), Some(false), "not open and empty");
    // The keys answer meanwhile: moves, the hidden names, the filter.
    press(&mut h, KeyCode::Down);
    assert_ne!(row(&h), TreeRow::Folder("a".into()));
    press(&mut h, KeyCode::Char('.'));
    assert!(rows(&h).contains(&TreeRow::Folder(".cache".into())));
    press(&mut h, KeyCode::Char('.'));
    press(&mut h, KeyCode::Up);
    assert_eq!(row(&h), TreeRow::Folder("a".into()));
    // It answers: listed and open.
    fake.release();
    h.app.pump_folder_reads(WAIT);
    assert!(rows(&h).contains(&TreeRow::File("a/inner.csv".into())), "{:?}", rows(&h));
    assert_eq!(h.app.overlays.script_tree().unwrap().is_open(&TreeRow::Folder("a".into())), Some(true));
    assert!(!h.screen(120, 40).contains("(reading…)"));
}

/// An answer nobody waits for any more is dropped: the picker went up to another folder, or
/// closed; the field's text changed under a `Tab`.
#[test]
fn a_stale_answer_is_dropped() {
    let home = home("stale-read");
    let (mut h, fake) = faked(&home);
    fake.hang.lock().unwrap().push(home.0.join("Downloads/a"));
    h.ctrl('o');
    go_to(&mut h, TreeRow::Folder("a".into()));
    press(&mut h, KeyCode::Right);
    // Up to the home folder while `a` is still being read.
    press(&mut h, KeyCode::Up);
    assert_eq!(row(&h), TreeRow::Top);
    press(&mut h, KeyCode::Left);
    h.app.pump_folder_reads(Duration::from_millis(500));
    assert_eq!(root(&h), home.0);
    assert!(rows(&h).contains(&TreeRow::Folder("Downloads".into())), "{:?}", rows(&h));
    fake.release();
    h.app.pump_folder_reads(WAIT);
    let t = h.app.overlays.script_tree().unwrap();
    assert!(t.entries.iter().all(|e| !e.path.starts_with("a/")), "{:?}", t.entries);
    assert!(!t.disk.reading("a"));
    h.key(KeyCode::Esc);
    // Tab, then more typed before the folder answers: the answer is not put in.
    let (mut h, fake) = faked(&home);
    fake.hang.lock().unwrap().push(home.0.join("Downloads"));
    set_path(&mut h, "~/Downloads/rep");
    press(&mut h, KeyCode::Tab);
    assert!(h.screen(160, 45).contains("Reading ~/Downloads…"), "{}", h.screen(160, 45));
    assert_eq!(focus(&h), ExportField::Path);
    press(&mut h, KeyCode::Char('x'));
    fake.release();
    h.app.pump_folder_reads(WAIT);
    assert_eq!(path_text(&h), "~/Downloads/repx", "the old answer is not put in");
    assert_eq!(focus(&h), ExportField::Path);
}

/// A read that takes longer than the limit says so, never as an empty folder; asked again, it
/// is read again.
#[test]
fn a_read_that_takes_too_long_says_so() {
    let home = home("slow");
    let (mut h, fake) = faked(&home);
    h.app.set_folder_timeout(Duration::from_millis(200));
    fake.hang.lock().unwrap().push(home.0.join("Downloads/a"));
    h.ctrl('o');
    go_to(&mut h, TreeRow::Folder("a".into()));
    press(&mut h, KeyCode::Right);
    assert_eq!(h.app.pump_folder_reads(WAIT), 1, "answered within the limit");
    let screen = h.screen(120, 40);
    assert!(screen.contains("a/ (took too long to read)"), "{screen}");
    assert!(said(&h).is_some_and(|e| e.contains("~/Downloads/a took too long to read")), "{:?}", said(&h));
    let t = h.app.overlays.script_tree().unwrap();
    assert_eq!(t.is_open(&TreeRow::Folder("a".into())), Some(false));
    assert!(t.unreadable(&TreeRow::Folder("a".into())));
    assert!(!rows(&h).iter().any(|r| matches!(r, TreeRow::File(p) if p.starts_with("a/"))));
    // Save here into it is refused (it is not known what it holds).
    h.ctrl('s');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree), "not picked");
    // The drive answers again: `→` reads it again.
    fake.release();
    h.key(KeyCode::Right);
    assert!(rows(&h).contains(&TreeRow::File("a/inner.csv".into())), "{:?}", rows(&h));
    // The folder shown itself: marked, no rows taken for its contents.
    h.key(KeyCode::Esc);
    let (mut h, fake) = faked(&home);
    h.app.set_folder_timeout(Duration::from_millis(200));
    fake.hang.lock().unwrap().push(home.0.join("Downloads"));
    h.ctrl('o');
    assert_eq!(rows(&h), vec![TreeRow::Up, TreeRow::Top]);
    let screen = h.screen(120, 40);
    assert!(screen.lines().any(|l| l.contains("~/Downloads/") && l.contains("(took too long to read)")), "{screen}");
}

/// A folder whose read fails says why, and is marked; one that holds an entry of unknown kind
/// shows it so (never as a file or dropped).
#[test]
fn a_read_that_fails_says_why_and_an_unknown_kind_is_shown() {
    let home = home("fails");
    let (mut h, fake) = faked(&home);
    let d = home.0.join("Downloads");
    fake.fake.lock().unwrap().insert(d.join("a"), Err(std::io::ErrorKind::PermissionDenied));
    fake.fake.lock().unwrap().insert(
        d.clone(),
        Ok(vec![("a".into(), Kind::Folder), ("odd".into(), Kind::Unknown), ("b.csv".into(), Kind::File)]),
    );
    h.ctrl('o');
    assert!(rows(&h).contains(&TreeRow::File("odd".into())), "{:?}", rows(&h));
    let screen = h.screen(120, 40);
    assert!(screen.contains("odd (kind unknown)"), "{screen}");
    assert!(!screen.contains("b.csv (kind unknown)"), "{screen}");
    go_to(&mut h, TreeRow::Folder("a".into()));
    h.key(KeyCode::Right);
    let screen = h.screen(120, 40);
    assert!(screen.contains("a/ (can't be read)"), "{screen}");
    assert!(said(&h).is_some_and(|e| e.starts_with("~/Downloads/a could not be read: ")), "{:?}", said(&h));
    assert_eq!(h.app.overlays.script_tree().unwrap().is_open(&TreeRow::Folder("a".into())), Some(false));
}

/// `Tab` in the path field in a folder that cannot be read says so below the field; the focus
/// stays, and `Tab` again moves it on. No such folder moves it on at once.
#[test]
fn tab_in_an_unreadable_folder_says_so() {
    let home = home("tab-locked");
    let (mut h, fake) = faked(&home);
    fake.fake.lock().unwrap().insert(home.0.join("Downloads"), Err(std::io::ErrorKind::PermissionDenied));
    set_path(&mut h, "~/Downloads/rep");
    h.key(KeyCode::Tab);
    assert_eq!(focus(&h), ExportField::Path, "the focus stays");
    assert_eq!(path_text(&h), "~/Downloads/rep");
    let screen = h.screen(160, 45);
    assert!(screen.contains("~/Downloads could not be read, so nothing is completed"), "{screen}");
    h.key(KeyCode::Tab);
    assert_eq!(focus(&h), ExportField::Export, "Tab again moves on");
    set_path(&mut h, "~/nowhere/x");
    h.key(KeyCode::Tab);
    assert_eq!(focus(&h), ExportField::Export, "no such folder: nothing to complete");
}

/// The real file system: a link that leads nowhere is of unknown kind (it was shown as a file).
#[cfg(unix)]
#[test]
fn a_broken_link_is_of_unknown_kind() {
    let home = home("broken");
    std::os::unix::fs::symlink(home.0.join("gone"), home.0.join("Downloads/dangling")).unwrap();
    let mut h = dialog(&home);
    h.ctrl('o');
    let screen = h.screen(120, 40);
    assert!(screen.contains("dangling (kind unknown)"), "{screen}");
}

/// `n`: the new folder is made on a worker; the prompt stays until it is made, then the folder
/// is listed and selected.
#[test]
fn a_new_folder_is_made_on_a_worker() {
    let home = home("make-hang");
    let (mut h, fake) = faked(&home);
    let made = home.0.join("Downloads/made");
    fake.hang.lock().unwrap().push(made.clone());
    h.ctrl('o');
    h.keys("n");
    h.type_text("made");
    press(&mut h, KeyCode::Enter);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::NameInput), "the prompt waits");
    assert!(!made.exists(), "not made on the UI thread");
    fake.release();
    h.app.pump_folder_reads(WAIT);
    assert!(made.is_dir());
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree));
    assert_eq!(row(&h), TreeRow::Folder("made".into()));
}

/// A folder made after its prompt was closed: the prompt open by then (another name) is left
/// as it is, and the folder is made.
#[test]
fn a_late_new_folder_answer_leaves_another_prompt_alone() {
    let home = home("make-late");
    let (mut h, fake) = faked(&home);
    let made = home.0.join("Downloads/x");
    fake.hang.lock().unwrap().push(made.clone());
    h.ctrl('o');
    h.keys("n");
    h.type_text("x");
    press(&mut h, KeyCode::Enter);
    press(&mut h, KeyCode::Esc);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree));
    press(&mut h, KeyCode::Char('n'));
    h.app.pump_folder_reads(Duration::from_millis(500));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::NameInput));
    press(&mut h, KeyCode::Char('y'));
    fake.release();
    h.app.pump_folder_reads(WAIT);
    assert!(made.is_dir());
    assert_eq!(h.overlay_kind(), Some(OverlayKind::NameInput), "the prompt for y stays");
    let n = h.app.overlays.name_input().unwrap();
    assert_eq!(n.input.text(), "y");
    assert!(n.error.is_none(), "{:?}", n.error);
}

/// A second new folder (another name) while the first is still being made: made too, and
/// its prompt closes once it is.
#[test]
fn a_second_new_folder_is_made_while_the_first_hangs() {
    let home = home("make-second");
    let (mut h, fake) = faked(&home);
    let (x, y) = (home.0.join("Downloads/x"), home.0.join("Downloads/y"));
    fake.hang.lock().unwrap().push(x.clone());
    h.ctrl('o');
    h.keys("n");
    h.type_text("x");
    press(&mut h, KeyCode::Enter);
    press(&mut h, KeyCode::Esc);
    press(&mut h, KeyCode::Char('n'));
    h.app.pump_folder_reads(Duration::from_millis(500));
    press(&mut h, KeyCode::Char('y'));
    press(&mut h, KeyCode::Enter);
    h.app.pump_folder_reads(Duration::from_millis(500));
    assert!(y.is_dir() && !x.exists());
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree));
    assert_eq!(row(&h), TreeRow::Folder("y".into()));
    fake.release();
    h.app.pump_folder_reads(WAIT);
    assert!(x.is_dir());
}

/// Listing a folder again (a folder made in it) drops the reads of what was inside it: their
/// answers come to no row.
#[test]
fn a_reload_drops_the_reads_inside_it() {
    let home = home("reload-hang");
    std::fs::create_dir_all(home.0.join("Downloads/a/sub")).unwrap();
    std::fs::write(home.0.join("Downloads/a/sub/deep.csv"), "x").unwrap();
    let (mut h, fake) = faked(&home);
    fake.hang.lock().unwrap().push(home.0.join("Downloads/a/sub"));
    h.ctrl('o');
    go_to(&mut h, TreeRow::Folder("a".into()));
    h.key(KeyCode::Right);
    go_to(&mut h, TreeRow::Folder("a/sub".into()));
    press(&mut h, KeyCode::Right);
    press(&mut h, KeyCode::Up);
    assert_eq!(row(&h), TreeRow::Folder("a".into()));
    press(&mut h, KeyCode::Char('n'));
    h.app.pump_folder_reads(Duration::from_millis(500));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::NameInput));
    for c in "new".chars() {
        press(&mut h, KeyCode::Char(c));
    }
    press(&mut h, KeyCode::Enter);
    h.app.pump_folder_reads(Duration::from_millis(500));
    assert_eq!(row(&h), TreeRow::Folder("a/new".into()), "{:?}", rows(&h));
    fake.release();
    h.app.pump_folder_reads(WAIT);
    let t = h.app.overlays.script_tree().unwrap();
    assert!(!t.entries.iter().any(|e| e.path == "a/sub/deep.csv"), "the old answer was put in: {:?}", t.entries);
    assert!(!t.loaded.contains("a/sub"));
}

/// A typed path, or Save here, answered after the text or the selection changed: dropped.
#[test]
fn an_answer_after_the_user_moved_on_is_dropped() {
    let home = home("moved");
    let (mut h, fake) = faked(&home);
    let before = path_text(&h);
    fake.hang.lock().unwrap().push(home.0.join("Downloads/x.csv"));
    fake.hang.lock().unwrap().push(home.0.join("Downloads/a"));
    h.ctrl('o');
    h.keys("/");
    h.type_text("~/Downloads/x.csv");
    press(&mut h, KeyCode::Enter);
    assert!(h.screen(120, 40).contains("(reading…)"), "{}", h.screen(120, 40));
    press(&mut h, KeyCode::Esc);
    // Save here into `a`, then the selection moves on.
    go_to_raw(&mut h, TreeRow::Folder("a".into()));
    h.app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)));
    press(&mut h, KeyCode::Down);
    fake.release();
    h.app.pump_folder_reads(WAIT);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree), "neither answer picked anything");
    assert_eq!(path_text(&h), before);
}

/// Select row `r` with keys straight to the app (reads still out are not waited for).
fn go_to_raw(h: &mut Harness, r: TreeRow) {
    for _ in 0..rows(h).len() {
        press(h, KeyCode::Up);
    }
    for _ in 0..rows(h).len() {
        if row(h) == r {
            return;
        }
        press(h, KeyCode::Down);
    }
    panic!("no row {r:?} in {:?}", rows(h));
}

/// A folder that does not answer is read by one thread: asked again (after it took too long),
/// the new read waits for the one still out instead of starting another.
#[test]
fn one_stuck_read_per_folder() {
    let home = home("one-stuck");
    let (mut h, fake) = faked(&home);
    h.app.set_folder_timeout(Duration::from_millis(200));
    let a = home.0.join("Downloads/a");
    fake.hang.lock().unwrap().push(a.clone());
    h.ctrl('o');
    go_to(&mut h, TreeRow::Folder("a".into()));
    for _ in 0..3 {
        press(&mut h, KeyCode::Right);
        assert_eq!(h.app.pump_folder_reads(WAIT), 1);
        assert!(h.screen(120, 40).contains("a/ (took too long to read)"));
    }
    assert_eq!(fake.lists_of(&a), 1, "one read of a, not one per try");
    fake.release();
    // Once it returned, the next try reads again (within a limit it cannot miss).
    h.app.set_folder_timeout(WAIT);
    h.key(KeyCode::Right);
    assert!(rows(&h).contains(&TreeRow::File("a/inner.csv".into())), "{:?}", rows(&h));
    assert_eq!(fake.lists_of(&a), 2);
}

/// A late `Tab` answer, after the focus left the field: nothing is put in.
#[test]
fn a_late_tab_answer_after_the_focus_left_is_dropped() {
    let home = home("tab-left");
    let (mut h, fake) = faked(&home);
    fake.hang.lock().unwrap().push(home.0.join("Downloads"));
    set_path(&mut h, "~/Downloads/rep");
    press(&mut h, KeyCode::Tab);
    press(&mut h, KeyCode::Down);
    assert_ne!(focus(&h), ExportField::Path);
    fake.release();
    h.app.pump_folder_reads(WAIT);
    assert_eq!(path_text(&h), "~/Downloads/rep");
}

/// A read that fails inside says it could not read the folder, not that it took too long.
#[test]
fn a_read_that_breaks_is_not_called_slow() {
    let home = home("broke");
    let (mut h, fake) = faked(&home);
    fake.broken.lock().unwrap().push(home.0.join("Downloads/a"));
    h.ctrl('o');
    go_to(&mut h, TreeRow::Folder("a".into()));
    h.key(KeyCode::Right);
    let screen = h.screen(120, 40);
    assert!(screen.contains("a/ (can't be read)"), "{screen}");
    assert!(said(&h).is_some_and(|e| e.contains("could not be read")), "{:?}", said(&h));
}
