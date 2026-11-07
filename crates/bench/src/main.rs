//! `datarig-bench`: reproducible measurements of datarig's performance, and the budgets CI
//! holds them to. See `docs/perf.md` for how to run them.
//!
//! ```text
//! datarig-bench <scenario>... [--runs N] [--out FILE] [--scratch DIR] [--bin PATH]
//!                               [--delay-ms MS] [--max-pages N] [--idle-secs S]
//! ```
//!
//! Scenarios: `rtt`, `rtt_ssh`, `rtt_mysql`, `paging`, `export_stream`, `export_keyset`, `export_xlsx`, `export_parquet`,
//! `editor`, `editor_block`, `editor_mysql`, `grid`, `plan`, `chart`, `explorer`, `idle`, `startup`, or `all`. The PostgreSQL
//! ones (`rtt`, `rtt_ssh`, `paging`, `export_stream`, `export_keyset`, `export_xlsx`, `export_parquet`, `idle`) need
//! `DATARIG_TEST_PG_URL` and the test database of `dev/init`; `rtt_ssh` also the SSH bastion of the tests (see
//! `ssh.rs`); `rtt_mysql` needs `DATARIG_TEST_MYSQL_URL` (a MySQL server and a database the user may make a temporary
//! table in).
//! `idle` and `startup` run the release binary in `tmux -L perf`.
//!
//! `budget [--budgets FILE]` runs them with the settings of `budgets.toml` and fails when a
//! measurement is over its budget (CI's `perf` job).

mod apps;
mod budget;
mod chart;
mod editor;
mod explorer;
mod export;
mod grid;
mod idle;
mod paging;
mod plan;
mod proxy;
mod rtt;
mod rtt_mysql;
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
            "rtt_mysql",
            "editor",
            "grid",
            "plan",
            "chart",
            "explorer",
            "startup",
            "idle",
            "paging",
            "export_stream",
            "export_keyset",
            "export_xlsx",
            "export_parquet",
            "editor_block",
            "editor_mysql",
        ]
        .map(String::from)
        .to_vec();
    }
    if o.scenarios.is_empty() {
        return Err(
            "name a scenario: rtt, rtt_ssh, rtt_mysql, paging, export_stream, export_keyset, export_xlsx, export_parquet, editor, editor_block, editor_mysql, grid, plan, chart, explorer, idle, startup, all or budget"
                .into(),
        );
    }
    Ok(o)
}

fn mysql_url() -> Result<String, String> {
    std::env::var("DATARIG_TEST_MYSQL_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .ok_or_else(|| "set DATARIG_TEST_MYSQL_URL=mysql://datarig:datarig@127.0.0.1:53306/datarig".into())
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
        "rtt_mysql" => rtt_mysql::run(&mysql_url()?, o.delay, o.runs.unwrap_or(20)).await,
        "paging" => paging::run(&pg_url()?, &o.scratch, "SELECT * FROM analytics.events", o.max_pages, 250).await,
        "export_stream" => export::run(&pg_url()?, &o.scratch, false, "csv").await,
        "export_keyset" => export::run(&pg_url()?, &o.scratch, true, "csv").await,
        "export_xlsx" => export::run(&pg_url()?, &o.scratch, false, "xlsx").await,
        "export_parquet" => export::run(&pg_url()?, &o.scratch, false, "parquet").await,
        "editor" => editor::run(&o.scratch, 5 * 1024 * 1024, o.runs.unwrap_or(300)),
        "editor_block" => editor::block_in_own_process(&o.scratch, 5 * 1024 * 1024),
        "editor_mysql" => editor::run_mysql_in_own_process(&o.scratch, 5 * 1024 * 1024, o.runs.unwrap_or(300)),
        "grid" => grid::run(2_000, 24, o.runs.unwrap_or(400)),
        "plan" => plan::run(PLAN_JOINS, PLAN_PARTITIONS, o.runs.unwrap_or(300)),
        "chart" => chart::run_in_own_process(&o.scratch, CHART_ROWS, o.runs.unwrap_or(300)),
        "explorer" => explorer::run(EXPLORER_SCHEMAS, EXPLORER_PER_SCHEMA, o.runs.unwrap_or(300)),
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

/// The tree of the `explorer` scenario: this many schemas, each with this many relations.
const EXPLORER_SCHEMAS: usize = 20;
const EXPLORER_PER_SCHEMA: usize = 500;

/// The plan of the `plan` scenario: this many joins deep, over this many partitions.
const PLAN_JOINS: usize = 40;
const PLAN_PARTITIONS: usize = 500;

/// The scenarios `budget` runs, in this order.
const BUDGET_SCENARIOS: [&str; 17] = [
    "rtt",
    "rtt_ssh",
    "rtt_mysql",
    "editor",
    "editor_block",
    "editor_mysql",
    "grid",
    "plan",
    "chart",
    "explorer",
    "paging",
    "export_stream",
    "export_keyset",
    "export_xlsx",
    "export_parquet",
    "idle",
    "startup",
];

/// Scenario `name` with the settings of the budgets `b`.
async fn budget_scenario(name: &str, o: &Opts, b: &toml::Table) -> Result<Value, String> {
    let int = |section: &str, key: &str| budget::num(b, section, key).map(|n| n as usize);
    let ms = |section: &str| int(section, "one_way_ms").map(|n| Duration::from_millis(n as u64));
    match name {
        "rtt" => rtt::run(&pg_url()?, ms("rtt")?, int("rtt", "runs")?).await,
        "rtt_ssh" => ssh::run(&pg_url()?, &o.scratch, ms("rtt_ssh")?, int("rtt_ssh", "runs")?).await,
        "rtt_mysql" => rtt_mysql::run(&mysql_url()?, ms("rtt_mysql")?, int("rtt_mysql", "runs")?).await,
        "editor" => editor::run(&o.scratch, 5 * 1024 * 1024, 100),
        "editor_block" => editor::block_in_own_process(&o.scratch, 5 * 1024 * 1024),
        "editor_mysql" => editor::run_mysql_in_own_process(&o.scratch, 5 * 1024 * 1024, 100),
        "grid" => grid::run(2_000, 24, 200),
        "plan" => plan::run(PLAN_JOINS, PLAN_PARTITIONS, 200),
        "chart" => chart::run_in_own_process(&o.scratch, CHART_ROWS, 200),
        "explorer" => explorer::run(EXPLORER_SCHEMAS, EXPLORER_PER_SCHEMA, 200),
        "paging" => {
            paging::run(&pg_url()?, &o.scratch, "SELECT * FROM analytics.events", int("paging", "max_pages")?, 250)
                .await
        }
        "export_stream" => export::run(&pg_url()?, &o.scratch, false, "csv").await,
        "export_keyset" => export::run(&pg_url()?, &o.scratch, true, "csv").await,
        "export_xlsx" => export::run(&pg_url()?, &o.scratch, false, "xlsx").await,
        "export_parquet" => export::run(&pg_url()?, &o.scratch, false, "parquet").await,
        "idle" => idle::idle(&o.scratch, &o.bin, &pg_url()?, int("idle", "secs")? as u64),
        "startup" => idle::startup(&o.scratch, &o.bin, int("startup", "runs")?),
        _ => Err(format!("unknown scenario {name}")),
    }
}

/// Hold the result of scenario `name` to the budgets `b`.
fn budget_check(name: &str, c: &mut budget::Checks, b: &toml::Table, r: &Value) -> Result<(), String> {
    c.scenario(name);
    match name {
        "rtt" => budget::rtt(c, b, r),
        "rtt_ssh" | "rtt_mysql" => budget::rtt_section(c, b, name, r),
        "editor" => budget::editor(c, b, r),
        "editor_block" => budget::editor_block(c, b, r),
        "editor_mysql" => budget::editor_mysql(c, b, r),
        "grid" => budget::grid(c, b, r),
        "plan" => budget::plan(c, b, r),
        "chart" => budget::chart(c, b, r),
        "explorer" => budget::explorer(c, b, r),
        "paging" => budget::paging(c, b, r),
        "export_stream" | "export_keyset" | "export_xlsx" | "export_parquet" => budget::export(c, b, name, r),
        "idle" => budget::idle(c, b, r),
        "startup" => budget::startup(c, b, r),
        _ => Err(format!("unknown scenario {name}")),
    }
}

/// The runner probe (`stats::probe`), printed and recorded before measuring `attempt`.
fn budget_probe(o: &Opts, attempt: u32) {
    let p = stats::probe(&o.scratch);
    println!("[probe] runner: {p} (load average {})", stats::load_avg());
    record(
        o,
        &json!({ "scenario": "probe", "budget": true, "attempt": attempt, "load": stats::load_avg(), "result": p }),
    );
}

/// Every scenario, held to the budgets; a scenario with a time over budget runs once more (see
/// `budget.rs`). What failed.
async fn budget(o: &Opts) -> Result<Vec<String>, String> {
    let b = budget::load(&o.budgets)?;
    budget_probe(o, 1);
    let mut first = budget::Checks::default();
    for name in BUDGET_SCENARIOS {
        let r = budget_scenario(name, o, &b).await?;
        budget_check(name, &mut first, &b, &budget_record(o, name, 1, r))?;
    }
    let mut again = budget::Checks::default();
    let rerun = first.timing_failed();
    if !rerun.is_empty() {
        println!("a time over budget in {}: measuring once more", rerun.join(", "));
        budget_probe(o, 2);
        for name in &rerun {
            let r = budget_scenario(name, o, &b).await?;
            budget_check(name, &mut again, &b, &budget_record(o, name, 2, r))?;
        }
    }
    Ok(first.failed(&again))
}

/// Print the load average and record the result of scenario `name`, measured for `attempt`.
fn budget_record(o: &Opts, name: &str, attempt: u32, result: Value) -> Value {
    println!("[{name}] load average {}", stats::load_avg());
    record(
        o,
        &json!({ "scenario": name, "budget": true, "attempt": attempt, "load": stats::load_avg(), "result": result }),
    );
    result
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
