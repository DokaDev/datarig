//! The real binary in a terminal: `tmux -L perf` (a tmux server of its own, never the user's)
//! runs `datarig` with a private config, data and state directory, passwords in memory
//! (`DATARIG_SECRET_STORE=memory`) and the frame counters of `DATARIG_BENCH_STATS`.
//!
//! * idle: one connection and three tabs, then nothing for a while: resident memory, CPU
//!   and event loop wakeups and frames per second;
//! * startup: the time from starting the process to its first frame.

use crate::stats::{Summary, cpu_secs, rss_kb};
use datarig_core::profile::ProfileId;
use datarig_core::workspace::{self, ExplorerState, TabState, WorkspaceState};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// The tmux server of the binary scenarios (`tmux -L perf`), never the user's default one.
/// `DATARIG_BENCH_TMUX` names another, so two runs at once do not
/// share sessions.
const SOCKET: &str = "perf";
const SOCKET_ENV: &str = "DATARIG_BENCH_TMUX";
const PW_ENV: &str = "DATARIG_BENCH_PW";
/// How long the idle measurement with a paging countdown lasts (shorter than the policy's 30s).
const PAGING_SECS: u64 = 20;

fn tmux(args: &[&str]) -> Result<String, String> {
    let socket = std::env::var(SOCKET_ENV).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| SOCKET.to_string());
    let out = Command::new("tmux").arg("-L").arg(socket).args(args).output().map_err(|e| format!("tmux: {e}"))?;
    if !out.status.success() {
        return Err(format!("tmux {args:?}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn kill(session: &str) {
    let _ = tmux(&["kill-session", "-t", session]);
}

/// The frame counters the binary wrote.
#[derive(Clone, Copy, Debug, Default)]
struct Counters {
    first_frame_us: u64,
    draws: u64,
    wakeups: u64,
}

fn counters(file: &Path) -> Option<Counters> {
    let text = std::fs::read_to_string(file).ok()?;
    let v: Value = serde_json::from_str(text.trim()).ok()?;
    Some(Counters {
        first_frame_us: v["first_frame_us"].as_u64()?,
        draws: v["draws"].as_u64()?,
        wakeups: v["wakeups"].as_u64()?,
    })
}

/// A private home for one run: the config (one profile on the test database, its password
/// from an environment variable) and a state directory whose workspace has `tabs` consoles on
/// that profile.
struct Home {
    dir: PathBuf,
    config: PathBuf,
    stats: PathBuf,
    password: String,
}

impl Home {
    fn new(scratch: &Path, name: &str, url: Option<&str>, tabs: usize) -> Result<Home, String> {
        let dir = scratch.join(name);
        let _ = std::fs::remove_dir_all(&dir);
        let (data, state) = (dir.join("data"), dir.join("state"));
        for d in [&data, &state] {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let id = ProfileId::new();
        // The portal of a result with more rows is held, so its title counts down to the idle
        // close (`paging = "hold"`).
        let (mut config, mut password) =
            (String::from("version = 2\nicons = \"off\"\n\n[policy.default]\npaging = \"hold\"\n"), String::new());
        if let Some(url) = url {
            let d = datarig_core::profile::dsn::parse(url).map_err(|e| format!("{e:?}"))?;
            password = d.password.clone().unwrap_or_default();
            config.push_str(&format!(
                "\n[[connections]]\nid = \"{}\"\nname = \"bench\"\nhost = \"{}\"\nport = {}\nuser = \"{}\"\ndatabase = \"{}\"\nsslmode = \"disable\"\npassword_source = \"env\"\npassword_env = \"{PW_ENV}\"\n",
                id,
                d.host,
                d.port.unwrap_or(5432),
                d.user,
                d.database
            ));
            let tabs = (0..tabs)
                .map(|i| {
                    let t = workspace::new_id();
                    // The first tab's statement has more rows than a page (see `idle`).
                    let sql = if i == 0 {
                        "SELECT * FROM analytics.events;\n".to_string()
                    } else {
                        format!("-- tab {i}\nSELECT {i};\n")
                    };
                    workspace::write_console(&state, &t, &sql)
                        .map(|()| TabState {
                            id: t,
                            kind: workspace::TabKind::Console,
                            script: None,
                            profile: Some(id),
                            cursor: (if i == 0 { 0 } else { 1 }, 0),
                            top: 0,
                            console: 0,
                            table: None,
                            ddl: None,
                            results: None,
                            database: None,
                            schema: None,
                        })
                        .map_err(|e| e.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            let ws = WorkspaceState { active: 0, explorer: ExplorerState::default(), tabs, unknown_tabs: Vec::new() };
            workspace::save(&state, &ws).map_err(|e| e.to_string())?;
        }
        let config_path = dir.join("config.toml");
        std::fs::write(&config_path, config).map_err(|e| e.to_string())?;
        Ok(Home { config: config_path, stats: dir.join("stats.json"), dir, password })
    }

    /// The shell command tmux runs: the binary itself (`exec`, so the pane's pid is datarig's).
    fn command(&self, bin: &Path, profile: Option<&str>) -> String {
        let q = |p: &Path| format!("'{}'", p.display());
        format!(
            "exec env DATARIG_SECRET_STORE=memory DATARIG_DATA_DIR={} DATARIG_STATE_DIR={} DATARIG_BENCH_STATS={} {PW_ENV}='{}' {} --config {} {}",
            q(&self.dir.join("data")),
            q(&self.dir.join("state")),
            q(&self.stats),
            self.password,
            q(bin),
            q(&self.config),
            profile.unwrap_or("")
        )
    }
}

/// Start `command` in a new detached session of 160x45 and return the pane's pid.
fn spawn(session: &str, command: &str) -> Result<u32, String> {
    kill(session);
    tmux(&["new-session", "-d", "-s", session, "-x", "160", "-y", "45", command])?;
    let pid = tmux(&["display-message", "-p", "-t", session, "#{pane_pid}"])?;
    pid.parse().map_err(|_| format!("pane pid {pid:?}"))
}

fn wait_for(within: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

pub fn idle(scratch: &Path, bin: &Path, url: &str, secs: u64) -> Result<Value, String> {
    let home = Home::new(scratch, "idle", Some(url), 3)?;
    let session = "datarig-idle";
    let pid = spawn(session, &home.command(bin, Some("bench")))?;
    let result = (|| {
        if !wait_for(Duration::from_secs(10), || counters(&home.stats).is_some()) {
            return Err(format!(
                "no first frame; screen:\n{}",
                tmux(&["capture-pane", "-p", "-t", session]).unwrap_or_default()
            ));
        }
        // Let the connection settle (connect, catalog, keys), then keep still.
        std::thread::sleep(Duration::from_secs(5));
        let screen = tmux(&["capture-pane", "-p", "-t", session])?;
        let _ = std::fs::write(home.dir.join("screen.txt"), &screen);
        let c0 = counters(&home.stats).unwrap_or_default();
        let cpu0 = cpu_secs(pid).ok_or("no cpu time")?;
        let t0 = Instant::now();
        let mut rss = Vec::new();
        while t0.elapsed() < Duration::from_secs(secs) {
            if let Some(kb) = rss_kb(pid) {
                rss.push(kb as f64);
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        let elapsed = t0.elapsed().as_secs_f64();
        let cpu1 = cpu_secs(pid).ok_or("no cpu time")?;
        let c1 = counters(&home.stats).unwrap_or_default();
        let cpu_pct = (cpu1 - cpu0) / elapsed * 100.0;
        let wakeups = (c1.wakeups - c0.wakeups) as f64 / elapsed;
        let draws = (c1.draws - c0.draws) as f64 / elapsed;
        let r = Summary::of(&rss);
        // Then a result with more rows than a page: its portal stays open and the results title
        // counts down to the idle close (policy `paging_idle_timeout`, 30s by default). Measured
        // from the key press on, so a burst of timer ticks after the quiet minute counts too.
        tmux(&["send-keys", "-t", session, "C-e"])?;
        let t1 = Instant::now();
        let cpu2 = cpu_secs(pid).ok_or("no cpu time")?;
        std::thread::sleep(Duration::from_secs(PAGING_SECS));
        let paging_elapsed = t1.elapsed().as_secs_f64();
        let cpu3 = cpu_secs(pid).ok_or("no cpu time")?;
        let c2 = counters(&home.stats).unwrap_or_default();
        let paging = json!({
            "secs": PAGING_SECS,
            "cpu_pct": (cpu3 - cpu2) / paging_elapsed * 100.0,
            "wakeups_per_s": (c2.wakeups - c1.wakeups) as f64 / paging_elapsed,
            "frames_per_s": (c2.draws - c1.draws) as f64 / paging_elapsed,
        });
        let _ = std::fs::write(home.dir.join("screen-paging.txt"), tmux(&["capture-pane", "-p", "-t", session])?);
        // The profile's node shows the connected mark, and three consoles are open.
        let connected = screen.lines().any(|l| l.contains('\u{25cf}') && l.contains("bench"));
        println!("idle: {secs}s, 1 connection, 3 tabs ({})", if connected { "connected" } else { "NOT CONNECTED" });
        println!(
            "  RSS {:.0} KiB median, {:.0} max; CPU {cpu_pct:.2}%; {wakeups:.2} wakeups/s, {draws:.2} frames/s",
            r.median, r.max
        );
        println!(
            "  then {PAGING_SECS}s with a paging countdown: CPU {:.2}%; {:.2} wakeups/s, {:.2} frames/s",
            paging["cpu_pct"].as_f64().unwrap_or(0.0),
            paging["wakeups_per_s"].as_f64().unwrap_or(0.0),
            paging["frames_per_s"].as_f64().unwrap_or(0.0)
        );
        Ok(json!({
            "secs": secs,
            "rss_kb": r.json(),
            "cpu_pct": cpu_pct,
            "wakeups_per_s": wakeups,
            "frames_per_s": draws,
            "connected": connected,
            "paging_countdown": paging,
        }))
    })();
    kill(session);
    result
}

pub fn startup(scratch: &Path, bin: &Path, runs: usize) -> Result<Value, String> {
    let home = Home::new(scratch, "startup", None, 0)?;
    let session = "datarig-startup";
    let mut first = Vec::new();
    let mut wall = Vec::new();
    for _ in 0..runs {
        let _ = std::fs::remove_file(&home.stats);
        let t = Instant::now();
        spawn(session, &home.command(bin, None))?;
        let seen = wait_for(Duration::from_secs(10), || counters(&home.stats).is_some());
        let w = t.elapsed();
        kill(session);
        if !seen {
            return Err("no first frame within 10s".into());
        }
        let c = counters(&home.stats).unwrap_or_default();
        first.push(c.first_frame_us as f64 / 1000.0);
        wall.push(w.as_secs_f64() * 1000.0);
    }
    let size = std::fs::metadata(bin).map(|m| m.len()).unwrap_or(0);
    let f = Summary::of(&first);
    let w = Summary::of(&wall);
    println!("startup: {runs} runs");
    println!("  process start to first frame {}", f.line(" ms"));
    println!("  tmux spawn to first frame     {}", w.line(" ms"));
    println!("  binary {} bytes ({:.1} MiB)", size, size as f64 / 1048576.0);
    Ok(json!({ "first_frame_ms": f.json(), "spawn_to_frame_ms": w.json(), "binary_bytes": size }))
}
