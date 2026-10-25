//! Exporting every row of `analytics.events` (4,000,000 rows) to a CSV file as a user would
//! (`:export all <file>`), with the headless app: once as a console's statement, read with one
//! streaming statement, and once as a table tab, read in chunks by its key; and to an Excel
//! workbook (`<file>.xlsx`, four sheets), streamed. What it measures: the rows written per second
//! and the process's resident memory as the rows go by, and each second nothing is said (a
//! workbook is zipped at the end), which must stay flat (the rows go from the session through a
//! bounded channel to the file, never into the app's result).

use crate::apps;
use crate::stats::rss_kb;
use datarig_tui::app::export::{ExportEvent, ExportNews};
use datarig_tui::app::tabs::TableRef;
use datarig_tui::app::{AppEvent, Results};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Rows between two samples of the resident memory.
const SAMPLE_ROWS: u64 = 250_000;

/// Every row as CSV (`keyset`: the table tab, read by its key), or (`xlsx`) a workbook streamed.
pub async fn run(url: &str, scratch: &std::path::Path, keyset: bool, xlsx: bool) -> Result<Value, String> {
    let name = match (keyset, xlsx) {
        (_, true) => "export_xlsx",
        (true, false) => "export_keyset",
        (false, false) => "export_stream",
    };
    let state = scratch.join(format!("{name}-state"));
    let _ = std::fs::remove_dir_all(&state);
    let (mut app, mut rx) = apps::connected(url, &state).await?;
    let table = TableRef { schema: "analytics".into(), name: "events".into() };
    let sql = if keyset {
        app.tab_mut().doc.table = Some(table.clone());
        table.query(datarig_core::sql::dialect::Dialect::Postgres)
    } else {
        "SELECT * FROM analytics.events".to_string()
    };
    app.run(vec![sql.clone()]);
    apps::pump(&mut app, &mut rx, Duration::from_secs(60), |a| {
        matches!(a.tab().results, Results::Rows(_)) && a.tab().exec.running.is_none()
    })
    .await?;
    let path = scratch.join(format!("{name}.{}", if xlsx { "xlsx" } else { "csv" }));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(datarig_tui::app::export::partial_path(&path));
    let req = app.export_request(&format!("all {}", path.display())).map_err(|n| format!("{:?}", n.msg))?;
    let pid = std::process::id();
    let rss0 = rss_kb(pid).unwrap_or(0);
    println!("{name}: {sql} to {} ({rss0} KiB before)", path.display());
    let t0 = Instant::now();
    let tab = app.tab().id;
    app.start_export(req);
    let mut samples = vec![json!({ "rows": 0, "rss_kb": rss0 })];
    let mut next = SAMPLE_ROWS;
    let mut rows = 0;
    let deadline = Instant::now() + Duration::from_secs(1_800);
    while app.exporting(tab) {
        let left = deadline.saturating_duration_since(Instant::now());
        let ev = match tokio::time::timeout(left.min(Duration::from_secs(1)), rx.recv()).await {
            Ok(Some(ev)) => ev,
            Ok(None) => return Err("the event channel closed".into()),
            Err(_) if left > Duration::ZERO => {
                // Nothing said for a second (a workbook being zipped): memory all the same.
                samples.push(json!({ "rows": rows, "rss_kb": rss_kb(pid), "secs": t0.elapsed().as_secs_f64() }));
                continue;
            }
            Err(_) => return Err(format!("{name}: not done within 30 minutes")),
        };
        if let AppEvent::Export(ExportEvent { news: ExportNews::Progress { rows: r, .. }, .. }) = &ev {
            rows = *r;
            if rows >= next {
                next = rows + SAMPLE_ROWS;
                let rss = rss_kb(pid);
                samples.push(json!({ "rows": rows, "rss_kb": rss, "secs": t0.elapsed().as_secs_f64() }));
                println!("  {rows:>9} rows  RSS {} KiB", rss.unwrap_or(0));
            }
        }
        app.on_app_event(ev);
    }
    let secs = t0.elapsed().as_secs_f64();
    let status = app.tab().status.as_ref().map(|n| n.render(&app.i18n).to_string()).unwrap_or_default();
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    // The keyset scenario must have read by the key (not one statement).
    let chunked =
        app.tab().exec.run.notes.iter().any(|n| matches!(n.msg, datarig_core::i18n::Msg::ExportRunsInChunks { .. }));
    let complete = status.starts_with("Exported 4,000,000 rows") && chunked == keyset;
    let peak = samples.iter().filter_map(|v| v["rss_kb"].as_u64()).max().unwrap_or(0);
    let rows_per_s = rows as f64 / secs.max(0.001);
    println!(
        "  {rows} rows, {:.0} MiB in {secs:.1} s ({rows_per_s:.0} rows/s); RSS peak {peak} KiB; {status}",
        bytes as f64 / 1048576.0
    );
    let _ = std::fs::remove_file(&path);
    drop(app);
    Ok(json!({
        "sql": sql,
        "rows": rows,
        "bytes": bytes,
        "secs": secs,
        "rows_per_s": rows_per_s,
        "complete": complete,
        "chunked": chunked,
        "status": status,
        "rss_kb_before": rss0,
        "rss_kb_peak": peak,
        "samples": samples,
    }))
}
