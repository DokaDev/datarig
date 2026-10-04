//! The real binary in a pseudo terminal (`script`), ended from outside with SIGTERM or SIGHUP:
//! it restores the terminal as on a normal quit (alternate screen left, mouse and bracketed
//! paste off, the cursor shown with the user's shape) and writes the console it was editing.
//! It also hands the terminal to an external editor (`Ctrl+G`, a script in place of the
//! editor) and suspends (`Ctrl+Z`), taking the terminal back after. Unix only; skipped with a
//! visible note when `script` is not there.
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
        Run::start_in(dir, false)
    }

    /// [`Run::start`], with the binary out of the pseudo terminal's session on Linux (see
    /// [`start`]).
    fn start_in(dir: PathBuf, own_session: bool) -> Option<Run> {
        Run::start_with(dir, own_session, "", &[], &[])
    }

    /// [`Run::start_in`] with `config` as the config file, `files` (path in the directory,
    /// text) written first, and `env` set for the binary.
    fn start_with(
        dir: PathBuf,
        own_session: bool,
        config: &str,
        files: &[(&str, &str)],
        env: &[(&str, String)],
    ) -> Option<Run> {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), config).unwrap();
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let child = start(&dir, &dir.join("typescript"), own_session, env)?;
        Some(Run { child, dir })
    }

    /// Type `bytes` on the pseudo terminal.
    fn type_bytes(&mut self, bytes: &[u8]) {
        let stdin = self.child.stdin.as_mut().expect("stdin");
        stdin.write_all(bytes).unwrap();
        stdin.flush().unwrap();
    }

    /// Type `bytes` until `done` (the workspace is restored after the first frame, so a key
    /// typed at once may find no tab yet), at most 20 seconds.
    fn type_until(&mut self, bytes: &[u8], what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.type_bytes(bytes);
            let wait = Instant::now() + Duration::from_secs(1);
            while !done() && Instant::now() < wait {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
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
///
/// `own_session` (Linux only): the binary runs in a session of its own (`setsid -w`), so the
/// hangup of the pseudo terminal when `script` ends does not reach it. Linux sends the session
/// leader SIGCONT with SIGHUP, which would resume a stopped binary to handle the hangup; macOS
/// sends SIGHUP alone, which a stopped binary keeps pending.
fn start(dir: &Path, out: &Path, own_session: bool, env: &[(&str, String)]) -> Option<Child> {
    let bin = env!("CARGO_BIN_EXE_datarig");
    let config = dir.join("config.toml");
    let mut cmd = Command::new("script");
    if cfg!(target_os = "linux") {
        let setsid = if own_session { "setsid -w " } else { "" };
        let line = format!("{setsid}{bin} --config {}", config.display());
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
        .env_remove("VISUAL")
        .env_remove("EDITOR")
        .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
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

/// A run whose `script` ended while the binary still runs (the binary is stopped here, and on
/// Linux in a session of its own, so the hangup cannot end it, then `script` is killed) still
/// ends the binary: each of the two is killed if it runs, whatever the other does.
#[test]
fn a_run_whose_script_ended_first_leaves_no_process_behind() {
    let dir = std::env::temp_dir().join(format!("datarig-signals-{}-orphan", std::process::id()));
    let Some(mut run) = Run::start_in(dir.clone(), true) else { return };
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

/// A profile (never connected here) so the workspace shows, and a console that comes back as
/// a recovered tab holding `SELECT 1`. No icons question.
const WITH_TAB: &str = "version = 2\nicons = \"off\"\n\n[[connections]]\nname = \"t\"\ndriver = \"postgres\"\n\
                        host = \"127.0.0.1\"\nport = 1\nuser = \"u\"\ndatabase = \"d\"\npassword_source = \"prompt\"\n";

/// The text of the console files in `dir`'s state directory.
fn consoles(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir.join("state/consoles")) else { return Vec::new() };
    rd.filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "sql"))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect()
}

/// How often the alternate screen was entered and left in `text`.
fn screens(text: &str) -> (usize, usize) {
    (text.matches("\x1b[?1049h").count(), text.matches("\x1b[?1049l").count())
}

/// `Ctrl+G`: the editor (a script here) runs with the file as its last argument, without a shell
/// (an argument with a space stays one), on the terminal (its stdin is a terminal), on a file
/// only its owner reads; the alternate screen is left for it and entered again after. SIGINT
/// and SIGQUIT sent while it runs (the editor's Ctrl+C) do not end datarig, and what the editor
/// saved is the console's text.
#[test]
fn ctrl_g_hands_the_terminal_to_the_editor_and_takes_its_text() {
    let dir = std::env::temp_dir().join(format!("datarig-signals-{}-editor", std::process::id()));
    // The file is the last argument.
    let editor = format!(
        "printf '%s|' \"$@\" > '{d}/args'\n\
         for f; do :; done\n\
         if [ -t 0 ]; then echo tty > '{d}/stdin'; fi\n\
         ls -l \"$f\" | cut -c1-10 > '{d}/mode'\n\
         cat \"$f\" > '{d}/given'\n\
         kill -INT $PPID\n\
         kill -QUIT $PPID\n\
         sleep 0.3\n\
         printf 'SELECT 2\\n' > \"$f\"\n",
        d = dir.display()
    );
    let visual = format!("sh '{}' 'two words'", dir.join("my editor.sh").display());
    let files = [("state/consoles/zz.sql", "SELECT 1"), ("my editor.sh", editor.as_str())];
    let Some(mut run) = Run::start_with(dir.clone(), false, WITH_TAB, &files, &[("VISUAL", visual)]) else { return };
    run.drawn();
    let pid = run.app_pid();
    run.type_until(b"\x07", "the editor to run", || dir.join("args").exists());
    wait_for("the screen to come back", 20, || dir.join("given").exists() && screens(&run.output()).0 >= 2);
    let args = std::fs::read_to_string(dir.join("args")).unwrap();
    let args: Vec<&str> = args.trim_end_matches('|').split('|').collect();
    assert_eq!(args[0], "two words", "{args:?}");
    assert!(
        args[1].starts_with(&dir.join("state/edit/").display().to_string()) && args[1].ends_with(".sql"),
        "{args:?}"
    );
    assert_eq!(std::fs::read_to_string(dir.join("stdin")).unwrap().trim(), "tty");
    assert_eq!(std::fs::read_to_string(dir.join("mode")).unwrap().trim(), "-rw-------");
    assert_eq!(std::fs::read_to_string(dir.join("given")).unwrap(), "SELECT 1\n");
    assert!(!Path::new(args[1]).exists(), "the file is removed");
    let text = run.output();
    let first = text.find("\x1b[?1049h").unwrap();
    let left = first + text[first..].find("\x1b[?1049l").expect("left for the editor");
    assert!(text[left..].contains("\x1b[?1049h"), "entered again");
    assert!(text[first..left].contains("\x1b[?2004l") && text[first..left].contains("\x1b[0 q"), "restored first");
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        alive(pid) && run.child.try_wait().ok().flatten().is_none(),
        "SIGINT/SIGQUIT during the editor ended datarig"
    );
    run.type_bytes(b"\x11"); // Ctrl+Q
    wait_for("the binary to end", 20, || run.child.try_wait().ok().flatten().is_some());
    assert!(consoles(&dir).iter().any(|t| t == "SELECT 2"), "{:?}", consoles(&dir));
}

/// SIGTERM while the editor runs ends datarig once the editor has ended, with the edited text
/// written and the terminal restored.
#[test]
fn sigterm_during_the_editor_ends_datarig_after_it() {
    let dir = std::env::temp_dir().join(format!("datarig-signals-{}-editor-term", std::process::id()));
    let editor = format!(
        "touch '{d}/started'\nkill -TERM $PPID\nsleep 0.3\nprintf 'SELECT 3\\n' > \"$1\"\ntouch '{d}/done'\n",
        d = dir.display()
    );
    let visual = format!("sh '{}'", dir.join("ed.sh").display());
    let files = [("state/consoles/zz.sql", "SELECT 1"), ("ed.sh", editor.as_str())];
    let Some(mut run) = Run::start_with(dir.clone(), false, WITH_TAB, &files, &[("VISUAL", visual)]) else { return };
    run.drawn();
    run.type_until(b"\x07", "the editor to run", || dir.join("started").exists());
    wait_for("the binary to end", 20, || run.child.try_wait().ok().flatten().is_some());
    assert!(dir.join("done").exists(), "the editor ran to its end");
    assert!(consoles(&dir).iter().any(|t| t == "SELECT 3"), "{:?}", consoles(&dir));
    let text = run.output();
    let (entered, left) = screens(&text);
    assert_eq!((entered, left), (2, 2), "{text:?}");
    assert!(text.trim_end().ends_with("\x1b[?1049l"), "ends on the main screen: {text:?}");
}

/// `Ctrl+Z`: the terminal is restored and the process group gets SIGTSTP; once it runs again
/// (`fg`: SIGCONT here) the alternate screen is entered again and the app goes on. (In the
/// pseudo terminal of `script` the binary leads a process group the shell does not control, so
/// the system may drop the stop; the test sends SIGCONT when it did stop.)
#[test]
fn ctrl_z_suspends_and_comes_back() {
    let dir = std::env::temp_dir().join(format!("datarig-signals-{}-suspend", std::process::id()));
    let Some(mut run) = Run::start_with(dir.clone(), false, WITH_TAB, &[], &[]) else { return };
    run.drawn();
    let pid = run.app_pid();
    let typescript = dir.join("typescript");
    let given_back = || screens(&String::from_utf8_lossy(&std::fs::read(&typescript).unwrap_or_default())).1 >= 1;
    run.type_until(b"\x1a", "the terminal to be given back", given_back);
    let stopped = || {
        Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains('T'))
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    while screens(&run.output()).0 < 2 {
        assert!(Instant::now() < deadline, "timed out waiting for the screen to come back");
        if stopped() {
            let _ = Command::new("kill").arg("-CONT").arg(pid.to_string()).status();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let text = run.output();
    let first = text.find("\x1b[?1049h").unwrap();
    let left = first + text[first..].find("\x1b[?1049l").unwrap();
    assert!(text[first..left].contains("\x1b[0 q"), "the user's cursor shape first");
    assert!(alive(pid));
    run.type_bytes(b"\x11");
    wait_for("the binary to end", 20, || run.child.try_wait().ok().flatten().is_some());
    let (entered, left) = screens(&run.output());
    assert_eq!((entered, left), (2, 2));
}
