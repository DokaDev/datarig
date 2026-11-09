//! Quitting never waits for the update check: the check runs on a thread of its own that is
//! never joined, so a request stuck anywhere (a name lookup that never returns, a server that
//! never answers) cannot hold the process. Proved with a process of its own: this test runs
//! itself again as a child whose check is stuck for two minutes, quits it, and times the exit.
mod common;
use common::*;
use datarig_core::config::Config;
use datarig_core::paths::Paths;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_core::update::Answer;
use datarig_i18n::Lang;
use datarig_tui::app::Startup;
use datarig_tui::app::action::Action;
use datarig_update::Fetch;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Set in the child.
const CHILD: &str = "DATARIG_TEST_UPDATE_QUIT_CHILD";

/// A request stuck for two minutes, as a name lookup that never returns.
struct Stuck(AtomicBool);

impl Fetch for Stuck {
    fn fetch(&self, _etag: Option<&str>) -> Answer {
        self.0.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_secs(120));
        Answer::NotModified
    }
}

#[test]
fn quitting_does_not_wait_for_a_stuck_check() {
    if std::env::var_os(CHILD).is_some() {
        return child();
    }
    let mut c = Command::new(std::env::current_exe().unwrap())
        .args(["quitting_does_not_wait_for_a_stuck_check", "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(c.stdout.take().unwrap()).lines();
    let started = Instant::now();
    let mut quit_at = None;
    for line in lines.by_ref() {
        let line = line.unwrap();
        if line.contains("CHILD stuck") {
            assert!(started.elapsed() < Duration::from_secs(30), "the check did not start");
        }
        if line.contains("CHILD quit") {
            quit_at = Some(Instant::now());
            break;
        }
    }
    let quit_at = quit_at.expect("the child quit");
    // The rest of its output, then its end.
    for _ in lines {}
    let status = c.wait().unwrap();
    let after = quit_at.elapsed();
    assert!(status.success(), "{status:?}");
    assert!(after < Duration::from_secs(5), "the process ended {after:?} after quitting, its check stuck");
}

/// The app as the binary runs it (a Tokio runtime, an event channel), its check stuck; then
/// `Ctrl+Q`'s action, and the end of `main`.
fn child() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let state = std::env::temp_dir().join(format!("datarig-update-quit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    let cfg = Config { connections: sample_profiles(), ..Default::default() };
    let (mut app, _clock) = new_app_with_clock(&cfg, Lang::En);
    app.set_secret_store(Arc::new(MemoryStore::new()) as Arc<dyn SecretStore>);
    app.set_paths(Paths { data: None, state: Some(state.clone()) });
    // Not the process's environment: CI services set `CI`.
    app.set_env_lookup(Arc::new(|_: &str| None));
    let stuck = Arc::new(Stuck(AtomicBool::new(false)));
    app.updates.fetch = Some(stuck.clone() as Arc<dyn Fetch>);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    {
        let _g = rt.enter();
        app.start(tx, Startup::Normal);
        app.first_frame_drawn();
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    while !stuck.0.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "the check did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    println!("CHILD stuck");
    app.dispatch(Action::Quit);
    assert!(app.quit, "Ctrl+Q's action quits at once");
    println!("CHILD quit");
    drop(app);
    drop(rt);
    let _ = std::fs::remove_dir_all(&state);
}
