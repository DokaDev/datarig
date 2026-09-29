//! Results larger than the window of rows kept in memory: the rows spill to
//! a file in the state directory, the grid, the inspector and copies read them back, the
//! spill limit stops fetching and says so, and the files go away with their result, at quit
//! and, for a crashed run, at the next launch.

mod common;

use common::*;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_core::i18n::Lang;
use datarig_core::paths::Paths;
use datarig_core::policy::SpillLimit;
use datarig_tui::app::{Focus, Paging, Results, Startup};
use ratatui::crossterm::event::KeyCode;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A test's state directory, removed when dropped (after the harness declared below it).
struct TempState(PathBuf);

impl std::ops::Deref for TempState {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_state(tag: &str) -> TempState {
    let dir = std::env::temp_dir().join(format!("datarig-flows-spill-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    TempState(dir)
}

/// Row `i`: its number, a wide text (Hangul from code points) and a NULL or an empty string.
fn row(i: usize) -> Vec<Option<String>> {
    let wide: String = (0..i % 7).map(|k| char::from_u32(0xAC00 + ((i + k) % 11_000) as u32).unwrap()).collect();
    vec![
        Some(format!("r{i:05}")),
        Some(format!("{wide}#{i}")),
        if i.is_multiple_of(2) { None } else { Some(String::new()) },
    ]
}

fn page(from: usize, n: usize) -> Vec<Vec<Option<String>>> {
    (from..from + n).map(row).collect()
}

/// Connected, with its state directory in `state`, a window of 1,000 rows and `limit`.
fn harness(state: &Path, limit: SpillLimit) -> Harness {
    let mut cfg = test_db_config();
    cfg.result_window_rows = 1_000;
    cfg.spill_limit = limit;
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.app.set_paths(Paths { data: None, state: Some(state.to_path_buf()) });
    h.db(DbEvent::Connected);
    h
}

/// Run the statement under the cursor and answer with `pages` pages of 500 rows, the portal
/// open after them (the last page says whether more follow). Returns the statement id.
fn fetched(h: &mut Harness, pages: usize, more_after: bool) -> u64 {
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let columns = Some(vec![
        meta("id", "text", false, false),
        meta("wide", "text", false, false),
        meta("x", "text", false, false),
    ]);
    let elapsed = Duration::from_millis(1);
    h.tab_db(0, DbEvent::Page { id, columns, rows: page(0, 500), more: true, elapsed });
    for p in 1..pages {
        let more = p + 1 < pages || more_after;
        h.tab_db(0, DbEvent::Page { id, columns: None, rows: page(p * 500, 500), more, elapsed });
    }
    id
}

fn rows_of(h: &Harness) -> &datarig_tui::widgets::grid::ResultSet {
    match &h.app.tab().results {
        Results::Rows(rs) => rs,
        _ => panic!("no rows"),
    }
}

fn spill_files(state: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(state.join("spill"))
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn data_files(state: &Path) -> Vec<PathBuf> {
    spill_files(state).into_iter().filter(|p| p.extension().is_some_and(|e| e == "rows")).collect()
}

#[test]
fn rows_past_the_window_spill_and_read_back_everywhere() {
    let state = temp_state("read");
    let mut h = harness(&state, SpillLimit::default());
    fetched(&mut h, 10, false);
    let rs = rows_of(&h);
    assert_eq!(rs.rows.len(), 5_000);
    assert!(rs.rows.resident().len() <= 1_000, "memory holds the window: {:?}", rs.rows.resident());
    let files = data_files(&state);
    assert_eq!(files.len(), 1, "one spill file for the result");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&files[0]).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // The first rows are on disk now; the grid and the inspector read them back.
    h.key(KeyCode::Tab); // results
    assert_eq!(h.app.focus, Focus::Results);
    h.keys("gg");
    let screen = h.screen(160, 45);
    assert!(screen.contains("r00000") && screen.contains("r00021"), "{screen}");
    assert!(!screen.contains("(not read)"), "{screen}");
    h.keys("jjj");
    let screen = h.screen(160, 45);
    assert!(screen.contains("r00003"), "the inspector shows the selected cell: {screen}");
    // Copy every row (they stream from the file) and a range across the window's edge.
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.command("copy tsv_header");
    let want: Vec<String> = std::iter::once("id\twide\tx".to_string())
        .chain((0..5_000).map(|i| {
            let r = row(i);
            format!("{}\t{}\t", r[0].as_deref().unwrap(), r[1].as_deref().unwrap())
        }))
        .collect();
    assert_eq!(clip.last().unwrap(), want.join("\n"));
    h.keys("gg");
    h.keys("v");
    h.app.tab_mut().grid.row = 2_999;
    h.keys("y");
    let got = clip.last().unwrap();
    assert_eq!(got.lines().count(), 3_001, "header and rows 0..=2999");
    assert_eq!(got.lines().nth(3_000), Some("r02999"));
    // Paging to the end and back (from the store: nothing is fetched) never shows a row as
    // not read.
    h.sent();
    for _ in 0..9 {
        h.keys("n");
        assert!(!h.screen(160, 45).contains("(not read)"));
    }
    h.keys("G");
    assert!(h.screen(160, 45).contains("r04999"));
    for _ in 0..9 {
        h.keys("p");
        h.key(KeyCode::PageUp);
        assert!(!h.screen(160, 45).contains("(not read)"));
    }
    assert!(h.screen(160, 45).contains("r00000"));
    assert!(h.sent().is_empty(), "every page came from the rows fetched");
    drop(h);
    assert!(spill_files(&state).is_empty(), "nothing is left at quit");
}

#[test]
fn the_spill_limit_stops_fetching_and_says_so() {
    let state = temp_state("cap");
    let mut h = harness(&state, SpillLimit(Some(64 * 1024)));
    let id = fetched(&mut h, 6, true);
    let rs = rows_of(&h);
    let kept = rs.rows.len();
    assert!((1_000..3_000).contains(&kept), "{kept}");
    assert!(!rs.more);
    assert_eq!(h.app.tab().exec.paging, Paging::Stopped);
    assert!(rs.rows.spilled() <= 64 * 1024);
    let sent = h.sent();
    assert!(sent.iter().any(|c| matches!(c, DbCommand::ClosePortal { id: i } if *i == id)), "{sent:?}");
    let status = h.status(160, 45);
    assert!(status.contains("temporary file reached its limit of 64 KB"), "{status}");
    let screen = h.screen(160, 45);
    assert!(
        screen.contains(" · fetching stopped) ›") && screen.contains("fetching stopped — narrow the query"),
        "{screen}"
    );
    // Paging keys at the end fetch nothing and say why.
    h.key(KeyCode::Tab);
    h.keys("G");
    h.keys("j");
    assert!(!h.sent().iter().any(|c| matches!(c, DbCommand::FetchMore { .. })));
}

#[test]
fn spill_files_go_with_their_result() {
    let state = temp_state("cleanup");
    let mut h = harness(&state, SpillLimit::default());
    fetched(&mut h, 4, true);
    let first = data_files(&state);
    assert_eq!(first.len(), 1);
    // Running again replaces the result: its file goes.
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    fetched(&mut h, 4, false);
    let second = data_files(&state);
    assert_eq!(second.len(), 1);
    assert_ne!(first, second);
    // Closing the tab.
    h.command("tabclose");
    assert!(h.app.tabs.is_empty() || !matches!(h.app.tab().results, Results::Rows(_)));
    assert!(data_files(&state).is_empty(), "{:?}", spill_files(&state));
}

/// A process id that is not running: a child that has exited.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
        .args(if cfg!(windows) { &["/C", "exit"][..] } else { &[][..] })
        .spawn()
        .unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

#[test]
fn a_launch_sweeps_what_a_crashed_run_left() {
    let state = temp_state("sweep");
    let dir = state.join("spill");
    std::fs::create_dir_all(&dir).unwrap();
    let dead = dead_pid();
    let left = [format!("datarig-spill-{dead}.lock"), format!("datarig-spill-{dead}-3.rows")];
    for f in &left {
        std::fs::write(dir.join(f), b"rows").unwrap();
    }
    let mine = dir.join(format!("datarig-spill-{}-9.rows", std::process::id()));
    std::fs::write(&mine, b"rows").unwrap();
    std::fs::write(dir.join("keep.txt"), b"not ours").unwrap();
    let cfg = test_db_config();
    let mut app = new_app(&cfg, Lang::En);
    app.set_paths(Paths { data: None, state: Some(state.to_path_buf()) });
    app.launch(Startup::Normal);
    for f in &left {
        assert!(!dir.join(f).exists(), "{f} was left by a crashed run");
    }
    assert!(mine.exists(), "a live process's file stays");
    assert!(dir.join("keep.txt").exists());
    drop(app);
}

/// A spill directory that is not private and cannot be made private (here: macOS's immutable
/// flag makes restricting it fail) is refused: nothing is written into it, fetching stops, the
/// rows so far stay, and the status says why in words.
#[cfg(target_os = "macos")]
#[test]
fn a_spill_dir_that_cannot_be_made_private_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let state = temp_state("notprivate");
    let spill = state.join("spill");
    std::fs::create_dir_all(&spill).unwrap();
    std::fs::set_permissions(&spill, std::fs::Permissions::from_mode(0o755)).unwrap();
    let flag = |f: &str| std::process::Command::new("chflags").arg(f).arg(&spill).status().unwrap().success();
    assert!(flag("uchg"));
    let mut h = harness(&state, SpillLimit(None));
    fetched(&mut h, 4, true);
    let status = h.status(200, 45);
    let files = std::fs::read_dir(&spill).map(|d| d.count()).unwrap_or(0);
    let mode = std::fs::metadata(&spill).unwrap().permissions().mode() & 0o777;
    assert!(flag("nouchg"));
    assert!(status.contains("Stopped fetching") && status.contains("not private to you"), "{status}");
    assert_eq!(files, 0, "nothing was written into it");
    assert_eq!(mode, 0o755);
    assert_eq!(h.app.tab().exec.paging, Paging::Stopped);
    assert_eq!(rows_of(&h).rows.len(), 1_000, "the rows shown so far stay");
}

/// Dragging past the grid's bottom scrolls over rows that are on disk, and
/// the rectangle it selects copies them back.
#[test]
fn a_drag_past_the_bottom_scrolls_over_spilled_rows() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let state = temp_state("drag");
    let mut h = harness(&state, SpillLimit::default());
    fetched(&mut h, 6, false);
    h.key(KeyCode::Tab);
    h.keys("gg");
    h.app.detail.visible = false;
    h.draw(160, 45);
    let g = &h.app.tab().grid;
    let (x0, y0) = (g.hit_cols[0].0 + 2, g.data_y);
    let below = h.app.layout.results.y + h.app.layout.results.height + 1;
    h.mouse(MouseEventKind::Down(MouseButton::Left), x0, y0);
    for _ in 0..60 {
        h.mouse(MouseEventKind::Drag(MouseButton::Left), x0, below);
        let screen = h.screen(160, 45);
        assert!(!screen.contains("(not read)"), "{screen}");
    }
    h.mouse(MouseEventKind::Up(MouseButton::Left), x0, below);
    let g = &h.app.tab().grid;
    assert_eq!(g.anchor, Some((0, 0)));
    assert!(g.row > 60 && g.top > 0, "scrolled down: row {}, top {}", g.row, g.top);
    let last = g.row;
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("y");
    let got = clip.last().unwrap();
    assert_eq!(got.lines().nth(1), Some("r00000"), "the first row, from the spill file");
    assert_eq!(got.lines().last().map(str::to_string), Some(format!("r{last:05}")));
}
