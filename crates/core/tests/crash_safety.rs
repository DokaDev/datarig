//! Crash safety of the files datarig keeps: a process killed in the
//! middle of saving never leaves a torn file. This test runs itself again as a child process
//! that saves a console file and a saved query over and over with
//! `fsutil::atomic_write` (what autosave uses), kills it at arbitrary moments, and checks that
//! each file is always one complete version.

use datarig_core::scripts::ScriptStore;
use datarig_core::workspace;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const CHILD: &str = "DATARIG_CRASH_CHILD";

/// Version `n` of a file: a header, a body whose size depends on `n` (so a mix of two
/// versions is visible), and a footer.
fn version(n: u64) -> String {
    let body = "select 1; -- 🐘 あいう\n".repeat((n % 97) as usize * 40 + 1);
    format!("-- begin {n}\n{body}-- end {n}\n")
}

/// Whether `text` is exactly one version.
fn complete(text: &str) -> bool {
    let Some(first) = text.lines().next() else { return false };
    let Some(n) = first.strip_prefix("-- begin ").and_then(|n| n.parse::<u64>().ok()) else { return false };
    text == version(n)
}

/// The child: save both files as fast as it can until it is killed.
fn child(dir: &Path) {
    let store = ScriptStore::open(&dir.join("data"));
    let state = dir.join("state");
    let mut n = 0u64;
    loop {
        n += 1;
        let v = version(n);
        workspace::write_console(&state, "c1", &v).expect("console saved");
        store.save("reports/q.sql", &v, None).expect("script saved");
    }
}

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("datarig-crash-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_killed_save_never_leaves_a_torn_file() {
    if let Ok(dir) = std::env::var(CHILD) {
        return child(Path::new(&dir));
    }
    let dir = temp_dir();
    let exe = std::env::current_exe().unwrap();
    let console = workspace::console_path(&dir.join("state"), "c1");
    let script = ScriptStore::open(&dir.join("data")).file("reports/q.sql");
    for round in 0..12u64 {
        let mut c = Command::new(&exe)
            .args(["--exact", "a_killed_save_never_leaves_a_torn_file", "--nocapture"])
            .env(CHILD, &dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // Let it write for a while, differently every round, then kill it (SIGKILL on Unix).
        for _ in 0..500 {
            if console.exists() && script.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(150 + round * 37 % 120));
        c.kill().unwrap();
        c.wait().unwrap();
        for f in [&console, &script] {
            let text = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("round {round}: {}: {e}", f.display()));
            assert!(complete(&text), "round {round}: {} is torn ({} bytes)", f.display(), text.len());
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
