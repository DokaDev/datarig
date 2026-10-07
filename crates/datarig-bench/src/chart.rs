//! A chart of a large fetched result: 100,000 rows (a timestamp and two numbers, spilled past
//! the rows kept in memory) drawn as lines and as bars at 160x45 while the cursor moves.
//! Measured: the frame that reads the rows (once per result and choice), the key + frame time
//! after it, and the work per frame (`widgets::chart`'s count: rows read, points and dot columns
//! walked, cells painted). It runs in a process of its own (the bench binary again), so the
//! memory its rows took never counts in another scenario's.

use crate::apps;
use crate::stats::{Summary, ms};
use datarig_core::driver::{ColumnMeta, DbEvent};
use datarig_tui::app::Focus;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

fn column(name: &str, type_name: &str, numeric: bool) -> ColumnMeta {
    ColumnMeta { name: name.into(), type_name: type_name.into(), numeric, json: false, origin: None }
}

/// `n` rows: a minute each from 2026-01-01, orders and revenue.
pub fn rows(n: usize) -> (Vec<ColumnMeta>, Vec<Vec<Option<String>>>) {
    let columns =
        vec![column("at", "timestamptz", false), column("orders", "int8", true), column("revenue", "numeric", true)];
    let rows = (0..n)
        .map(|i| {
            let (day, minute) = (i / 1440, i % 1440);
            let (month, d) = (1 + day / 28, 1 + day % 28);
            vec![
                Some(format!("2026-{month:02}-{d:02} {:02}:{:02}:00+09", minute / 60, minute % 60)),
                Some(((i * 7919) % 1000).to_string()),
                Some(format!("{}.{:02}", (i * 104_729) % 100_000, i % 100)),
            ]
        })
        .collect();
    (columns, rows)
}

/// Set in the process [`run_in_own_process`] starts: it runs [`run`] itself.
const IN_PROCESS: &str = "DATARIG_BENCH_CHART_IN_PROCESS";

/// [`run`] in a process of its own.
pub fn run_in_own_process(scratch: &std::path::Path, n_rows: usize, n: usize) -> Result<Value, String> {
    if std::env::var(IN_PROCESS).is_ok_and(|v| v == "1") {
        return run(scratch, n_rows, n);
    }
    let out = scratch.join("chart.jsonl");
    let _ = std::fs::remove_file(&out);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let status = std::process::Command::new(exe)
        .arg("chart")
        .arg("--scratch")
        .arg(scratch)
        .arg("--out")
        .arg(&out)
        .arg("--runs")
        .arg(n.to_string())
        .env(IN_PROCESS, "1")
        .status()
        .map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let record: Value =
        serde_json::from_str(text.lines().last().unwrap_or("")).map_err(|e| format!("{}: {e}", out.display()))?;
    match record.get("result") {
        Some(r) if status.success() => Ok(r.clone()),
        _ => Err(format!("chart: {}", record.get("error").unwrap_or(&Value::Null))),
    }
}

pub fn run(scratch: &std::path::Path, n_rows: usize, n: usize) -> Result<Value, String> {
    let state = scratch.join("chart-state");
    std::fs::create_dir_all(&state).map_err(|e| format!("{}: {e}", state.display()))?;
    let mut app = apps::offline("SELECT at, orders, revenue FROM t;", Some(state));
    let (columns, data) = rows(n_rows);
    app.on_db_event(DbEvent::Page { id: 0, columns: Some(columns), rows: data, more: true, elapsed: Duration::ZERO });
    app.focus = Focus::Results;
    let mut term = apps::terminal();
    apps::draw(&mut term, &mut app);
    let mut kinds = serde_json::Map::new();
    let (mut all, mut worst, mut build_worst) = (Vec::new(), 0u64, 0u64);
    for (keys, kind) in [(&['c'][..], "line"), (&['1'][..], "bars"), (&['2'][..], "hbars")] {
        datarig_tui::widgets::chart::take_work();
        let t = Instant::now();
        for &k in keys {
            apps::char(&mut app, k);
        }
        apps::draw(&mut term, &mut app);
        let build_ms = ms(t.elapsed());
        let build = datarig_tui::widgets::chart::take_work();
        build_worst = build_worst.max(build);
        if app.tab().exec.chart.as_ref().and_then(|c| c.model()).is_none() {
            return Err(format!("{kind}: no chart"));
        }
        let mut frame = Vec::new();
        let mut work = Vec::new();
        for i in 0..n {
            let code = match i % 10 {
                0..=5 => KeyCode::Char('l'),
                6 => KeyCode::Char('G'),
                7 => KeyCode::Char('h'),
                8 => KeyCode::Char('j'),
                _ => KeyCode::Char('k'),
            };
            let t = Instant::now();
            apps::key(&mut app, code, KeyModifiers::NONE);
            apps::draw(&mut term, &mut app);
            frame.push(ms(t.elapsed()));
            let w = datarig_tui::widgets::chart::take_work();
            worst = worst.max(w);
            work.push(w as f64);
        }
        let f = Summary::of(&frame);
        let w = Summary::of(&work);
        println!("chart {kind}: {n_rows} rows at {}x{}", apps::WIDTH, apps::HEIGHT);
        println!("  read the rows and draw  {build_ms:.1} ms, work {build}");
        println!("  key+frame  {}", f.line(" ms"));
        println!("  work per frame  {}", w.line(""));
        kinds.insert(
            kind.into(),
            json!({ "build_ms": build_ms, "build_work": build, "frame_ms": f.json(), "work": w.json() }),
        );
        all.extend(frame);
    }
    let f = Summary::of(&all);
    Ok(json!({
        "rows": n_rows,
        "frame_ms": f.json(),
        "work_max": worst,
        "build_work_per_row_max": build_worst as f64 / n_rows as f64,
        "kinds": kinds,
    }))
}
