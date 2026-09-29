//! The real binary in a pseudo terminal (`script`), ended from outside with SIGTERM or SIGHUP:
//! it restores the terminal as on a normal quit (alternate screen left, mouse and bracketed
//! paste off, the cursor shown with the user's shape) and writes the console it was editing.
//! Unix only; skipped with a visible note when `script` is not there.
//!
//! Every path out of a test (an assertion that fails, a timeout) kills and reaps what it
//! started ([`Run`]): `script` puts the binary in raw mode on a pseudo terminal of its own, so
//! neither ends when the test process does, and a failed run once left both running for hours.
#![cfg(unix)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// One run of the binary inside `script`, with its scratch directory. Dropped on every path
/// (also a panic): the binary and `script` are each killed if they still run and waited for, then
/// the directory is removed (after them: the binary writes its state there as it ends).
struct Run {
    child: Child,
    dir: PathBuf,
}

impl Drop for Run {
    fn drop(&mut self) {
        // Each of the two apart: `script` may have ended while the binary still runs.
        if let Some(pid) = app_pid(&self.dir.join("config.toml")) {
            let _ = Command::new("kill").arg("-KILL").arg(pid.to_string()).status();
        }
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        // The binary is `script`'s child, not ours: wait until it is gone too.
        let deadline = Instant::now() + Duration::from_secs(10);
        while app_pid(&self.dir.join("config.toml")).is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Run {
    /// Start the binary in a new scratch directory `dir`; `None` (with a note) when `script`
    /// cannot start.
    fn start(dir: PathBuf) -> Option<Run> {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "").unwrap();
        let child = start(&dir, &dir.join("typescript"))?;
        Some(Run { child, dir })
    }

    /// What the binary wrote to its terminal so far.
    fn output(&self) -> String {
        std::fs::read(self.dir.join("typescript")).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
    }

    /// Wait until the first frame is out: the alternate screen is on and a frame went out (it
    /// hides the cursor; the pseudo terminal of `script` may have no size, so the frame can be
    /// empty).
    fn drawn(&self) {
        wait_for("the first frame", 20, || {
            let t = self.output();
            t.find("\x1b[?1049h").is_some_and(|at| t[at..].contains("\x1b[?25l"))
        });
    }

    /// The binary's process id.
    fn app_pid(&self) -> u32 {
        app_pid(&self.dir.join("config.toml")).expect("the binary's process")
    }
}

/// Whether process `pid` exists.
fn alive(pid: u32) -> bool {
    Command::new("kill").arg("-0").arg(pid.to_string()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// Start the binary inside `script`, recording what it writes to the terminal in `out`.
fn start(dir: &Path, out: &Path) -> Option<Child> {
    let bin = env!("CARGO_BIN_EXE_datarig");
    let config = dir.join("config.toml");
    let mut cmd = Command::new("script");
    if cfg!(target_os = "linux") {
        let line = format!("{bin} --config {}", config.display());
        cmd.args(["-q", "-f", "-e", "-c", &line]).arg(out);
    } else {
        cmd.args(["-q", "-F"]).arg(out).arg(bin).arg("--config").arg(&config);
    }
    cmd.env("DATARIG_SECRET_STORE", "memory")
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("DATARIG_DATA_DIR", dir.join("data"))
        .env("DATARIG_STATE_DIR", dir.join("state"))
        .env("TERM", "xterm-256color")
        .env_remove("TMUX")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match cmd.spawn() {
        Ok(child) => Some(child),
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "SKIPPED signals: `script` could not start: {e}");
            None
        }
    }
}

/// The process id of the binary: a process whose command line names `config` (so no other
/// datarig) and whose program is datarig (not `script`, nor a shell it started).
fn app_pid(config: &Path) -> Option<u32> {
    let out = Command::new("pgrep").arg("-f").arg(config.display().to_string()).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse::<u32>().ok()).find(|pid| {
        Command::new("ps")
            .args(["-o", "comm=", "-p", &pid.to_string()])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim().ends_with("datarig"))
    })
}

fn wait_for(what: &str, secs: u64, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn sigterm_and_sighup_restore_the_terminal() {
    for signal in ["TERM", "HUP"] {
        let dir = std::env::temp_dir().join(format!("datarig-signals-{}-{signal}", std::process::id()));
        let Some(mut run) = Run::start(dir.clone()) else { return };
        run.drawn();
        let pid = run.app_pid();
        let killed = Command::new("kill").arg(format!("-{signal}")).arg(pid.to_string()).status().unwrap();
        assert!(killed.success());
        wait_for("the binary to end", 20, || run.child.try_wait().ok().flatten().is_some());
        let text = run.output();
        let after = &text[text.rfind("\x1b[?1049h").unwrap()..];
        for (seq, what) in [
            ("\x1b[?2004l", "bracketed paste off"),
            ("\x1b[?1000l", "mouse capture off"),
            ("\x1b[?25h", "the cursor shown"),
            ("\x1b[0 q", "the user's cursor shape"),
            ("\x1b[?1049l", "the main screen"),
        ] {
            assert!(after.contains(seq), "SIG{signal}: {what} ({seq:?}) missing from {after:?}");
        }
        // The alternate screen is left last: nothing is drawn after it.
        assert!(after.rfind("\x1b[?1049l") > after.rfind("\x1b[0 q"), "SIG{signal}: {after:?}");
        // The workspace state was written.
        assert!(dir.join("state/workspace.toml").exists(), "SIG{signal}: nothing written");
    }
}

/// A run that fails before it ends the binary (here, a panic right after the first frame)
/// leaves no process behind: neither the binary nor `script`, and its directory is removed.
#[test]
fn a_failed_run_leaves_no_process_behind() {
    let dir = std::env::temp_dir().join(format!("datarig-signals-{}-failed", std::process::id()));
    let Some(run) = Run::start(dir.clone()) else { return };
    run.drawn();
    let (app, script) = (run.app_pid(), run.child.id());
    assert!(alive(app) && alive(script));
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _run = run;
        panic!("a failed assertion before the binary was ended");
    }));
    assert!(failed.is_err());
    assert!(!alive(app), "the binary still runs");
    assert!(!alive(script), "`script` still runs");
    assert!(!dir.exists(), "{}", dir.display());
}

/// A run whose `script` ended while the binary still runs (the binary is stopped here, so the
/// hangup cannot end it, then `script` is killed) still ends the binary: each of the two is
/// killed if it runs, whatever the other does.
#[test]
fn a_run_whose_script_ended_first_leaves_no_process_behind() {
    let dir = std::env::temp_dir().join(format!("datarig-signals-{}-orphan", std::process::id()));
    let Some(mut run) = Run::start(dir.clone()) else { return };
    run.drawn();
    let (app, script) = (run.app_pid(), run.child.id());
    let signal = |sig: &str, pid: u32| Command::new("kill").arg(format!("-{sig}")).arg(pid.to_string()).status();
    assert!(signal("STOP", app).unwrap().success());
    assert!(signal("KILL", script).unwrap().success());
    wait_for("`script` to end", 20, || run.child.try_wait().ok().flatten().is_some());
    assert!(alive(app), "the binary ended with `script`: nothing to check");
    drop(run);
    let left = alive(app);
    if left {
        // Not the test's to leave behind, whatever it finds.
        let _ = signal("KILL", app);
    }
    assert!(!left, "the binary still runs");
    assert!(!dir.exists(), "{}", dir.display());
}
