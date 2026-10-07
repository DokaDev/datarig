//! `datarig-bench`: reproducible measurements of datarig's performance, and the budgets CI
//! holds them to. See `docs/perf.md` for how to run them.
//!
//! ```text
//! datarig-bench <scenario>... [--runs N] [--out FILE] [--scratch DIR] [--bin PATH]
//!                               [--delay-ms MS] [--max-pages N] [--idle-secs S]
//! ```
//!
//! Scenarios: `rtt`, `rtt_ssh`, `paging`, `editor`, `editor_block`, `editor_mysql`, `grid`, `plan`, `chart`, `idle`,
//! `startup`, or `all`. The PostgreSQL ones (`rtt`, `rtt_ssh`, `paging`, `idle`) need `DATARIG_TEST_PG_URL` and the test
//! database of `dev/init`; `rtt_ssh` also the SSH bastion of the tests (see `ssh.rs`).
//! `idle` and `startup` run the release binary in `tmux -L perf`.
//!
//! `budget [--budgets FILE]` runs them with the settings of `budgets.toml` and fails when a
//! measurement is over its budget (CI's `perf` job).

mod apps;
mod budget;
mod chart;
mod editor;
mod grid;
mod idle;
mod paging;
mod plan;
mod proxy;
mod rtt;
mod ssh;
mod stats;

use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

struct Opts {
    scenarios: Vec<String>,
    runs: Option<usize>,
    out: Option<PathBuf>,
    scratch: PathBuf,
    bin: PathBuf,
    delay: Duration,
    max_pages: usize,
    idle_secs: u64,
    budgets: PathBuf,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut o = Opts {
        scenarios: Vec::new(),
        runs: None,
        out: None,
        scratch: std::env::temp_dir().join(format!("datarig-bench-{}", std::process::id())),
        bin: exe.with_file_name(format!("datarig{}", std::env::consts::EXE_SUFFIX)),
        delay: Duration::from_millis(30),
        max_pages: usize::MAX,
        idle_secs: 60,
        budgets: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("budgets.toml"),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--runs" => o.runs = Some(val()?.parse().map_err(|_| "--runs N")?),
            "--out" => o.out = Some(PathBuf::from(val()?)),
            "--scratch" => o.scratch = PathBuf::from(val()?),
            "--bin" => o.bin = PathBuf::from(val()?),
            "--delay-ms" => o.delay = Duration::from_millis(val()?.parse().map_err(|_| "--delay-ms MS")?),
            "--max-pages" => o.max_pages = val()?.parse().map_err(|_| "--max-pages N")?,
            "--idle-secs" => o.idle_secs = val()?.parse().map_err(|_| "--idle-secs S")?,
            "--budgets" => o.budgets = PathBuf::from(val()?),
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s => o.scenarios.push(s.to_string()),
        }
    }
    if o.scenarios.iter().any(|s| s == "all") {
        o.scenarios = [
            "rtt",
            "rtt_ssh",
            "editor",
            "grid",
            "plan",
            "chart",
            "startup",
            "idle",
            "paging",
            "editor_block",
            "editor_mysql",
        ]
        .map(String::from)
        .to_vec();
    }
    if o.scenarios.is_empty() {
        return Err(
            "name a scenario: rtt, rtt_ssh, paging, editor, editor_block, editor_mysql, grid, plan, chart, idle, startup, all or budget"
                .into(),
        );
    }
    Ok(o)
}

fn pg_url() -> Result<String, String> {
    std::env::var("DATARIG_TEST_PG_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .ok_or_else(|| "set DATARIG_TEST_PG_URL=postgres://datarig:datarig@127.0.0.1:55432/datarig".into())
}

async fn scenario(name: &str, o: &Opts) -> Result<Value, String> {
    match name {
        "rtt" => rtt::run(&pg_url()?, o.delay, o.runs.unwrap_or(20)).await,
        "rtt_ssh" => ssh::run(&pg_url()?, &o.scratch, o.delay, o.runs.unwrap_or(20)).await,
        "paging" => paging::run(&pg_url()?, &o.scratch, "SELECT * FROM analytics.events", o.max_pages, 250).await,
        "editor" => editor::run(&o.scratch, 5 * 1024 * 1024, o.runs.unwrap_or(300)),
        "editor_block" => editor::block_in_own_process(&o.scratch, 5 * 1024 * 1024),
        "editor_mysql" => editor::run_mysql(5 * 1024 * 1024, o.runs.unwrap_or(300)),
        "grid" => grid::run(2_000, 24, o.runs.unwrap_or(400)),
        "plan" => plan::run(PLAN_JOINS, PLAN_PARTITIONS, o.runs.unwrap_or(300)),
        "chart" => chart::run_in_own_process(&o.scratch, CHART_ROWS, o.runs.unwrap_or(300)),
        "idle" => idle::idle(&o.scratch, &o.bin, &pg_url()?, o.idle_secs),
        "startup" => idle::startup(&o.scratch, &o.bin, o.runs.unwrap_or(20)),
        _ => Err(format!("unknown scenario {name}")),
    }
}

/// Append `record` to the `--out` file, if there is one.
fn record(o: &Opts, record: &Value) {
    if let Some(out) = &o.out {
        use std::io::Write;
        let line = format!("{record}\n");
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(out)
            .and_then(|mut f| f.write_all(line.as_bytes()));
        if let Err(e) = written {
            eprintln!("{}: {e}", out.display());
        }
    }
}

/// The rows of the `chart` scenario.
const CHART_ROWS: usize = 100_000;

/// The plan of the `plan` scenario: this many joins deep, over this many partitions.
const PLAN_JOINS: usize = 40;
const PLAN_PARTITIONS: usize = 500;

/// Every scenario, held to the budgets.
async fn budget(o: &Opts) -> Result<Vec<String>, String> {
    let b = budget::load(&o.budgets)?;
    let url = pg_url()?;
    let mut c = budget::Checks::default();
    let int = |section: &str, key: &str| budget::num(&b, section, key).map(|n| n as usize);
    let run = |name: &str, result: Value| {
        println!("[{name}] load average {}", stats::load_avg());
        record(o, &json!({ "scenario": name, "budget": true, "load": stats::load_avg(), "result": result }));
        result
    };
    let r =
        rtt::run(&url, std::time::Duration::from_millis(int("rtt", "one_way_ms")? as u64), int("rtt", "runs")?).await?;
    budget::rtt(&mut c, &b, &run("rtt", r))?;
    let r = ssh::run(
        &url,
        &o.scratch,
        std::time::Duration::from_millis(int("rtt_ssh", "one_way_ms")? as u64),
        int("rtt_ssh", "runs")?,
    )
    .await?;
    budget::rtt_section(&mut c, &b, "rtt_ssh", &run("rtt_ssh", r))?;
    let r = editor::run(&o.scratch, 5 * 1024 * 1024, 100)?;
    budget::editor(&mut c, &b, &run("editor", r))?;
    let r = editor::block_in_own_process(&o.scratch, 5 * 1024 * 1024)?;
    budget::editor_block(&mut c, &b, &run("editor_block", r))?;
    let r = editor::run_mysql(5 * 1024 * 1024, 100)?;
    budget::editor_mysql(&mut c, &b, &run("editor_mysql", r))?;
    let r = plan::run(PLAN_JOINS, PLAN_PARTITIONS, 200)?;
    budget::plan(&mut c, &b, &run("plan", r))?;
    let r = chart::run_in_own_process(&o.scratch, CHART_ROWS, 200)?;
    budget::chart(&mut c, &b, &run("chart", r))?;
    let r = paging::run(&url, &o.scratch, "SELECT * FROM analytics.events", int("paging", "max_pages")?, 250).await?;
    budget::paging(&mut c, &b, &run("paging", r))?;
    let r = idle::idle(&o.scratch, &o.bin, &url, int("idle", "secs")? as u64)?;
    budget::idle(&mut c, &b, &run("idle", r))?;
    let r = idle::startup(&o.scratch, &o.bin, int("startup", "runs")?)?;
    budget::startup(&mut c, &b, &run("startup", r))?;
    Ok(c.failed().into_iter().map(String::from).collect())
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let o = match parse(&args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = std::fs::create_dir_all(&o.scratch) {
        eprintln!("{}: {e}", o.scratch.display());
        return ExitCode::FAILURE;
    }
    if o.scenarios == ["budget"] {
        return match budget(&o).await {
            Ok(failed) if failed.is_empty() => {
                println!("every budget holds");
                ExitCode::SUCCESS
            }
            Ok(failed) => {
                eprintln!("over budget: {}", failed.join(", "));
                ExitCode::FAILURE
            }
            Err(e) => {
                eprintln!("budget: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let mut failed = false;
    for name in &o.scenarios {
        let load_before = stats::load_avg();
        println!("[{name}] load average {load_before}");
        let result = scenario(name, &o).await;
        let load_after = stats::load_avg();
        let record_value = match result {
            Ok(v) => json!({ "scenario": name, "load_before": load_before, "load_after": load_after, "result": v }),
            Err(e) => {
                failed = true;
                eprintln!("[{name}] failed: {e}");
                json!({ "scenario": name, "error": e })
            }
        };
        record(&o, &record_value);
    }
    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}
