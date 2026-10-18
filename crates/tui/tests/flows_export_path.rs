//! Where the export dialog writes: the tree dialog over the file system (`Ctrl+O`, the
//! `[Browse…]` button) and `Tab` completion in the path field. Every test runs on a scratch
//! home folder (`HOME` is replaced through the app's environment lookup); the user's home is
//! never looked at.

mod common;

use common::*;
use datarig_core::driver::{ColumnMeta, DbEvent};
use datarig_core::i18n::Lang;
use datarig_tui::app::export_form::ExportField;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::script_tree::{TreeFocus, TreeMode, TreeRow};
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::path::PathBuf;
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
