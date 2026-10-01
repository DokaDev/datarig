//! `datarig-bench budget`: run the scenarios and hold them to `budgets.toml`. Every check
//! prints its measurement and its budget; any check over budget fails the run.

use serde_json::Value;
use std::path::Path;
use toml::Table;

/// The checks made so far: what was checked, and whether it passed.
#[derive(Default)]
pub struct Checks {
    results: Vec<(String, bool)>,
}

impl Checks {
    fn check(&mut self, what: &str, measured: f64, max: f64, unit: &str) {
        let ok = measured <= max;
        println!("  {} {what}: {measured:.3}{unit} (budget {max}{unit})", if ok { "PASS" } else { "FAIL" });
        self.results.push((what.to_string(), ok));
    }

    fn missing(&mut self, what: &str) {
        println!("  FAIL {what}: not measured");
        self.results.push((what.to_string(), false));
    }

    pub fn failed(&self) -> Vec<&str> {
        self.results.iter().filter(|(_, ok)| !ok).map(|(w, _)| w.as_str()).collect()
    }
}

pub fn load(path: &Path) -> Result<Table, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    text.parse::<Table>().map_err(|e| format!("{}: {e}", path.display()))
}

/// A number of `section.key` in the budgets.
pub fn num(b: &Table, section: &str, key: &str) -> Result<f64, String> {
    let v = b.get(section).and_then(|s| s.get(key)).ok_or(format!("budgets: {section}.{key} is missing"))?;
    v.as_float().or_else(|| v.as_integer().map(|i| i as f64)).ok_or(format!("budgets: {section}.{key} is not a number"))
}

fn f(v: &Value, path: &[&str]) -> Option<f64> {
    let mut v = v;
    for p in path {
        v = v.get(p)?;
    }
    v.as_f64()
}

const MIB: f64 = 1024.0;

pub fn rtt(c: &mut Checks, b: &Table, result: &Value) -> Result<(), String> {
    rtt_section(c, b, "rtt", result)
}

/// The round trips of every scenario `[<name>]` of the budgets names (`rtt`, `rtt_ssh`).
pub fn rtt_section(c: &mut Checks, b: &Table, name_of: &str, result: &Value) -> Result<(), String> {
    let section = b.get(name_of).and_then(|s| s.as_table()).ok_or(format!("budgets: [{name_of}] is missing"))?;
    for (name, limit) in section.iter().filter(|(_, v)| v.is_table()) {
        let scenario = result["scenarios"].as_array().and_then(|a| a.iter().find(|s| s["name"] == name.as_str()));
        for (key, field) in [("to_result", "rtt_to_result"), ("total", "rtt_total")] {
            let max =
                limit.get(key).and_then(toml::Value::as_integer).ok_or(format!("budgets: {name_of}.{name}.{key}"))?;
            match scenario.and_then(|s| f(s, &[field, "max"])) {
                Some(m) => c.check(&format!("{name_of} {name} {key}"), m, max as f64, " round trips"),
                None => c.missing(&format!("{name_of} {name} {key}")),
            }
        }
    }
    Ok(())
}

pub fn paging(c: &mut Checks, b: &Table, result: &Value) -> Result<(), String> {
    let peak = f(result, &["rss_kb_peak"]).map(|k| k / MIB);
    match peak {
        Some(p) => c.check("paging RSS peak", p, num(b, "paging", "rss_mib_max")?, " MiB"),
        None => c.missing("paging RSS peak"),
    }
    // From the first sample after the window filled (125,000 rows) to the peak.
    let first = result["samples"].as_array().and_then(|s| s.get(1)).and_then(|s| s["rss_kb"].as_f64());
    match (first, peak) {
        (Some(first), Some(peak)) => {
            c.check("paging RSS growth", peak - first / MIB, num(b, "paging", "rss_growth_mib_max")?, " MiB")
        }
        _ => c.missing("paging RSS growth"),
    }
    if result["complete"] != true {
        c.missing("paging to the end of the result");
    }
    Ok(())
}

pub fn editor(c: &mut Checks, b: &Table, result: &Value) -> Result<(), String> {
    let max = num(b, "editor", "p95_ms_max")?;
    let keys = ["typing_ms", "movement_ms", "scrolling_ms", "normal_edit_ms", "vim_ms", "theme_switch_ms"];
    for what in keys.into_iter().chain(["search_miss_ms", "search_next_ms"]) {
        match f(result, &[what, "p95"]) {
            Some(m) => c.check(&format!("editor {what} p95"), m, max, " ms"),
            None => c.missing(&format!("editor {what} p95")),
        }
    }
    match f(result, &["rss_kb", "after_edits"]) {
        Some(k) => c.check("editor RSS", k / MIB, num(b, "editor", "rss_mib_max")?, " MiB"),
        None => c.missing("editor RSS"),
    }
    // Search work, counted: the bytes one key searched, in passes over the text, and the lines
    // one frame highlighted.
    let passes = num(b, "editor", "search_passes_max")?;
    for (what, key) in [("editor / miss", "miss_bytes_max"), ("editor n", "next_bytes_max")] {
        match (f(result, &["search", key]), f(result, &["search", "text_bytes"])) {
            // Nothing searched at all means the search did not run: not measured.
            (Some(b), Some(text)) if text > 0.0 && b > 0.0 => {
                c.check(&format!("{what} bytes searched per key"), b / text, passes, " passes")
            }
            _ => c.missing(&format!("{what} bytes searched per key")),
        }
    }
    match f(result, &["search", "highlight_lines_max"]) {
        // The matches of `n` are on screen: a frame that highlighted nothing did not draw them.
        Some(l) if l > 0.0 => {
            c.check("editor lines highlighted per frame", l, num(b, "editor", "highlight_lines_max")?, " lines")
        }
        _ => c.missing("editor lines highlighted per frame"),
    }
    Ok(())
}

/// Visual block operators over the whole text: the slowest key with its frame, and the bytes
/// one key walked, in passes over the text.
pub fn editor_block(c: &mut Checks, b: &Table, result: &Value) -> Result<(), String> {
    match f(result, &["keys_ms", "max"]) {
        Some(m) => {
            c.check("editor block over the whole text, slowest key", m, num(b, "editor", "block_ms_max")?, " ms")
        }
        None => c.missing("editor block over the whole text, slowest key"),
    }
    match (f(result, &["bytes_max"]), f(result, &["text_bytes"])) {
        // Nothing walked means no block operator ran: not measured.
        (Some(w), Some(text)) if text > 0.0 && w > 0.0 => {
            c.check("editor block bytes walked per key", w / text, num(b, "editor", "block_passes_max")?, " passes")
        }
        _ => c.missing("editor block bytes walked per key"),
    }
    Ok(())
}

pub fn idle(c: &mut Checks, b: &Table, result: &Value) -> Result<(), String> {
    if result["connected"] != true {
        c.missing("idle: the profile connected");
    }
    let checks = [
        ("idle RSS", f(result, &["rss_kb", "max"]).map(|k| k / MIB), "rss_mib_max", " MiB"),
        ("idle wakeups", f(result, &["wakeups_per_s"]), "wakeups_per_s_max", "/s"),
        ("idle CPU", f(result, &["cpu_pct"]), "cpu_pct_max", "%"),
        ("idle countdown frames", f(result, &["paging_countdown", "frames_per_s"]), "countdown_frames_per_s_max", "/s"),
    ];
    for (what, measured, key, unit) in checks {
        match measured {
            Some(m) => c.check(what, m, num(b, "idle", key)?, unit),
            None => c.missing(what),
        }
    }
    Ok(())
}

pub fn startup(c: &mut Checks, b: &Table, result: &Value) -> Result<(), String> {
    match f(result, &["first_frame_ms", "p95"]) {
        Some(m) => c.check("startup first frame p95", m, num(b, "startup", "first_frame_ms_p95_max")?, " ms"),
        None => c.missing("startup first frame p95"),
    }
    match f(result, &["binary_bytes"]) {
        Some(n) => c.check("binary size", n / MIB / MIB, num(b, "startup", "binary_mib_max")?, " MiB"),
        None => c.missing("binary size"),
    }
    Ok(())
}
