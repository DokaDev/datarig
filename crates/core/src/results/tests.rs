use super::spill::{self, SpillDir};
use super::*;
use std::path::PathBuf;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("datarig-results-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Row `i`: its number, a NULL or an empty string by turns, and wide text of varying length.
fn row(i: usize) -> Vec<Cell> {
    let wide: String = (0..i % 17).map(|k| char::from_u32(0xAC00 + ((i * 7 + k) % 11_000) as u32).unwrap()).collect();
    vec![Some(i.to_string()), if i.is_multiple_of(3) { None } else { Some(String::new()) }, Some(format!("{wide}|{i}"))]
}

fn rows(r: Range<usize>) -> Vec<Vec<Cell>> {
    r.map(row).collect()
}

/// A store of `n` rows fetched in pages of `page`, spilling past `window` rows.
fn paged(state: &std::path::Path, n: usize, page: usize, window: usize) -> (RowStore, Arc<SpillDir>) {
    let dir = Arc::new(SpillDir::new(state));
    let mut s = RowStore::new(rows(0..page.min(n)), Some(dir.clone()), Limits { window, cap: None });
    let mut at = page.min(n);
    while at < n {
        let e = (at + page).min(n);
        assert_eq!(s.append(rows(at..e)).unwrap(), Appended { kept: e - at, capped: false });
        at = e;
    }
    (s, dir)
}

#[test]
fn a_result_within_the_window_stays_in_memory() {
    let state = temp_dir("small");
    let (s, _dir) = paged(&state, 1_000, 100, MIN_WINDOW);
    assert_eq!(s.len(), 1_000);
    assert_eq!(s.spilled(), 0);
    assert!(!state.join("spill").exists(), "nothing written");
    assert_eq!(s.row(999), Row::Here(&row(999)[..]));
    assert_eq!(s.row(1_000), Row::Missing);
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn rows_past_the_window_spill_and_read_back_exactly() {
    let state = temp_dir("spill");
    let (mut s, _dir) = paged(&state, 10_000, 500, MIN_WINDOW);
    assert_eq!(s.len(), 10_000);
    assert!(s.spilled() > 0);
    let res = s.resident();
    assert_eq!(res.len(), MIN_WINDOW, "memory holds the window only");
    assert_eq!(res.end, 10_000, "the window follows the pages");
    assert_eq!(s.row(0), Row::NotRead, "a row not in memory is unknown, never empty");
    // Every row, through windows at the start, the end, around block and window edges.
    for (a, b) in [(0, 40), (9_960, 10_000), (255, 257), (4_990, 5_030), (1_000, 1_001), (7_777, 7_800)] {
        s.load(a..b).unwrap();
        for i in a..b {
            assert_eq!(s.row(i), Row::Here(&row(i)[..]), "row {i}");
        }
        assert!(s.resident().len() <= MIN_WINDOW);
    }
    // Scrolling down a screen at a time never leaves a shown row unread.
    s.load(0..40).unwrap();
    let mut reloads = 0;
    let mut last = s.resident();
    for top in (0..9_960).step_by(40) {
        s.load(top..top + 40).unwrap();
        for i in top..top + 40 {
            assert!(matches!(s.row(i), Row::Here(_)), "row {i}");
        }
        if s.resident() != last {
            reloads += 1;
            last = s.resident();
        }
    }
    assert!(reloads <= 10_000 / (MIN_WINDOW / 4), "read-ahead moves the window in large steps: {reloads}");
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn streaming_reads_every_row_without_moving_the_window() {
    let state = temp_dir("stream");
    let (mut s, _dir) = paged(&state, 5_000, 500, MIN_WINDOW);
    s.load(2_000..2_040).unwrap();
    let window = s.resident();
    for (range, chunk) in [(0..5_000, 700), (1_500..3_600, 64), (4_999..5_000, 1), (3_000..3_000, 10)] {
        let mut got = Vec::new();
        s.for_each_chunk(range.clone(), chunk, |c| {
            assert!(c.len() <= chunk);
            got.extend(c.iter().cloned());
        })
        .unwrap();
        assert_eq!(got, rows(range));
    }
    assert_eq!(s.resident(), window);
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn the_cap_stops_the_spill_file_at_a_row_boundary() {
    let state = temp_dir("cap");
    let dir = Arc::new(SpillDir::new(&state));
    let mut s = RowStore::new(rows(0..500), Some(dir), Limits { window: MIN_WINDOW, cap: Some(64 * 1024) });
    let mut at = 500;
    let capped = loop {
        let got = s.append(rows(at..at + 500)).unwrap();
        at += got.kept;
        if got.capped {
            break got;
        }
    };
    assert!(capped.kept < 500);
    assert_eq!(s.len(), at);
    assert!(s.spilled() <= 64 * 1024 && s.spilled() > 60 * 1024, "{}", s.spilled());
    // Nothing more fits.
    assert_eq!(s.append(rows(at..at + 10)).unwrap(), Appended { kept: 0, capped: true });
    s.load(0..s.len()).unwrap_or(());
    let mut got = Vec::new();
    s.for_each_chunk(0..s.len(), 1000, |c| got.extend(c.iter().cloned())).unwrap();
    assert_eq!(got, rows(0..at), "every kept row, none of the dropped ones");
    let _ = std::fs::remove_dir_all(&state);
}

#[cfg(unix)]
#[test]
fn spill_files_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let state = temp_dir("mode");
    let (s, _dir) = paged(&state, 3_000, 500, MIN_WINDOW);
    let path = s.spill_path().unwrap().to_path_buf();
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(state.join("spill")).unwrap().permissions().mode() & 0o777, 0o700);
    let _ = std::fs::remove_dir_all(&state);
}

/// A spill directory that was there already with broader permissions (an older version, a lax
/// umask) is restricted to `0700` before anything spills into it.
#[cfg(unix)]
#[test]
fn a_spill_dir_that_was_open_to_others_is_made_private() {
    use std::os::unix::fs::PermissionsExt;
    let state = temp_dir("widemode");
    let spill = state.join("spill");
    std::fs::create_dir_all(&spill).unwrap();
    std::fs::set_permissions(&spill, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (s, _dir) = paged(&state, 3_000, 500, MIN_WINDOW);
    assert!(s.spill_path().is_some(), "it spilled");
    assert_eq!(std::fs::metadata(&spill).unwrap().permissions().mode() & 0o777, 0o700);
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn files_go_away_with_their_result_and_the_lock_with_the_process() {
    let state = temp_dir("cleanup");
    let (s, dir) = paged(&state, 3_000, 500, MIN_WINDOW);
    let (t, _) = paged(&state, 3_000, 500, MIN_WINDOW);
    let (file, other) = (s.spill_path().unwrap().to_path_buf(), t.spill_path().unwrap().to_path_buf());
    let lock = state.join("spill").join(format!("datarig-spill-{}.lock", std::process::id()));
    assert!(file.exists() && other.exists() && lock.exists());
    drop(s);
    assert!(!file.exists(), "a closed or re-run result deletes its file");
    assert!(other.exists());
    drop(t);
    drop(dir);
    assert!(!other.exists() && !lock.exists(), "nothing is left at quit");
    let _ = std::fs::remove_dir_all(&state);
}

#[test]
fn a_damaged_spill_file_is_an_error_never_empty_rows() {
    let state = temp_dir("damaged");
    let (mut s, _dir) = paged(&state, 5_000, 500, MIN_WINDOW);
    let path = s.spill_path().unwrap().to_path_buf();
    std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(100).unwrap();
    assert!(s.load(0..40).is_err());
    assert_eq!(s.row(0), Row::NotRead);
    assert!(s.for_each_chunk(0..10, 10, |_| {}).is_err());
    let _ = std::fs::remove_dir_all(&state);
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
fn a_sweep_deletes_only_what_dead_runs_left() {
    let state = temp_dir("sweep");
    let dir = state.join("spill");
    std::fs::create_dir_all(&dir).unwrap();
    let dead = dead_pid();
    let held = dead_pid();
    let me = std::process::id();
    let write = |name: &str| {
        let p = dir.join(name);
        std::fs::write(&p, b"x").unwrap();
        p
    };
    let gone = [
        write(&format!("datarig-spill-{dead}.lock")),
        write(&format!("datarig-spill-{dead}-1.rows")),
        write(&format!("datarig-spill-{dead}-22.rows")),
    ];
    // A dead id whose lock is held all the same (an owner that took over the id's files):
    // left alone.
    let held_lock = write(&format!("datarig-spill-{held}.lock"));
    let held_rows = write(&format!("datarig-spill-{held}-1.rows"));
    let holder = std::fs::File::open(&held_lock).unwrap();
    let locked = holder.try_lock().is_ok();
    let kept = [
        write(&format!("datarig-spill-{me}-1.rows")),
        write("datarig-spill-12x-1.rows"),
        write(&format!("datarig-spill-{dead}-1.rows.bak")),
        write(&format!("datarig-spill-{dead}.rows")),
        write("notes.txt"),
    ];
    let r = spill::sweep(&state);
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    for p in &gone {
        assert!(!p.exists(), "{p:?} was left by a dead run");
    }
    for p in &kept {
        assert!(p.exists(), "{p:?} is not a dead run's spill file");
    }
    if locked {
        assert!(held_lock.exists() && held_rows.exists(), "a held lock means a live owner");
    }
    let mut removed = r.removed.clone();
    removed.sort();
    let mut want = gone.to_vec();
    want.sort();
    if !locked {
        want.extend([held_lock.clone(), held_rows.clone()]);
        want.sort();
    }
    assert_eq!(removed, want);
    drop(holder);
    // A missing directory is nothing to do; an unreadable one is reported, not guessed at.
    assert!(spill::sweep(&state.join("nowhere")).failed.is_empty());
    let _ = std::fs::remove_dir_all(&state);
}
