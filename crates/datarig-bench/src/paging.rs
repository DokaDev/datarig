//! Paging a large result to its end with `n` (the next page), as a user
//! holding the key would: the time of each page (key press to the page in the grid) and the
//! process's resident memory as rows pile up.

use crate::apps;
use crate::stats::{Summary, ms, rss_kb};
use datarig_tui::app::{Focus, Results};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Rows fetched so far, and whether the server has more.
fn fetched(app: &datarig_tui::app::App) -> (usize, bool) {
    match &app.tab().results {
        Results::Rows(rs) => (rs.rows.len(), rs.more),
        _ => (0, false),
    }
}

pub async fn run(
    url: &str,
    scratch: &std::path::Path,
    sql: &str,
    max_pages: usize,
    sample_every: usize,
) -> Result<Value, String> {
    let state = scratch.join("paging-state");
    let _ = std::fs::remove_dir_all(&state);
    let (mut app, mut rx) = apps::connected(url, &state).await?;
    let mut term = apps::terminal();
    let pid = std::process::id();
    let rss0 = rss_kb(pid).unwrap_or(0);
    println!("paging: {sql} (up to {max_pages} pages, {} KiB before)", rss0);
    let t0 = Instant::now();
    app.run(vec![sql.to_string()]);
    apps::pump(&mut app, &mut rx, Duration::from_secs(60), |a| matches!(a.tab().results, Results::Rows(_))).await?;
    let first = ms(t0.elapsed());
    app.focus = Focus::Results;
    apps::draw(&mut term, &mut app);
    let mut per_page = Vec::new();
    let mut samples = vec![json!({ "pages": 1, "rows": fetched(&app).0, "rss_kb": rss_kb(pid) })];
    let mut pages = 1;
    let started = Instant::now();
    while pages < max_pages && fetched(&app).1 {
        let t = Instant::now();
        apps::char(&mut app, 'n');
        apps::pump(&mut app, &mut rx, Duration::from_secs(60), |a| a.tab().exec.running.is_none()).await?;
        apps::draw(&mut term, &mut app);
        per_page.push(ms(t.elapsed()));
        pages += 1;
        if pages % sample_every == 0 || !fetched(&app).1 {
            let (rows, _) = fetched(&app);
            let rss = rss_kb(pid);
            samples
                .push(json!({ "pages": pages, "rows": rows, "rss_kb": rss, "secs": started.elapsed().as_secs_f64() }));
            println!("  {pages:>6} pages {rows:>9} rows  RSS {} KiB", rss.unwrap_or(0));
        }
    }
    let (rows, more) = fetched(&app);
    let spilled = match &app.tab().results {
        Results::Rows(rs) => rs.rows.spilled(),
        _ => 0,
    };
    let s = Summary::of(&per_page);
    println!("  first page {first:.1} ms; per page {}; {rows} rows, more: {more}", s.line(" ms"));
    let peak = samples.iter().filter_map(|v| v["rss_kb"].as_u64()).max().unwrap_or(0);
    println!(
        "  RSS peak {peak} KiB ({:.0} MiB); spill file {:.0} MiB",
        peak as f64 / 1024.0,
        spilled as f64 / 1048576.0
    );
    // Closing the app's sessions leaves nothing open on the server.
    drop(app);
    Ok(json!({
        "sql": sql,
        "pages": pages,
        "rows": rows,
        "complete": !more,
        "first_page_ms": first,
        "page_ms": s.json(),
        "rss_kb_before": rss0,
        "rss_kb_peak": peak,
        "spilled_bytes": spilled,
        "samples": samples,
    }))
}
