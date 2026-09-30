//! The editor with a large file: keystroke-to-frame latency (the app handles the key, then
//! draws the frame at 160x45) for typing, cursor movement and scrolling, the process's memory,
//! what an autosave of the file costs, and a theme switch (`:set theme=`) up to its frame.

use crate::apps;
use crate::grid::wide;
use crate::stats::{Summary, ms, rss_kb};
use datarig_tui::app::App;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};

/// A SQL file of about `bytes` bytes: statements of the kinds people keep in consoles, with
/// comments, strings and some Hangul, about 70 characters per line.
pub fn generate(bytes: usize) -> String {
    let mut s = String::with_capacity(bytes + 256);
    let mut i = 0u64;
    while s.len() < bytes {
        i += 1;
        match i % 5 {
            0 => s.push_str(&format!(
                "-- {} {i}: {}\nSELECT o.status, count(*) AS n, sum(o.total_amount)\n  FROM shop.orders o\n WHERE o.created_at >= now() - interval '7 days' AND o.id > {i}\n GROUP BY o.status;\n\n",
                wide(i as usize, 4, false),
                wide(i as usize, 12, false)
            )),
            1 => s.push_str(&format!(
                "INSERT INTO shop.audit_log (actor, action, note) VALUES ('bench', 'update', '{} {i}; with a semicolon');\n",
                wide(i as usize, 2, false)
            )),
            2 => s.push_str(&format!(
                "UPDATE shop.products SET price = price * 1.05, name = '{} {i}' WHERE id = {i};\n",
                wide(i as usize, 2, false)
            )),
            3 => s.push_str(&format!(
                "SELECT $$a body; {i}$$ AS dollar, \"quoted ident\", 'it''s {i}' FROM analytics.events LIMIT 10;\n"
            )),
            _ => s.push_str(&format!(
                "/* block comment {i} */ SELECT e.id, e.event_type, e.payload->>'session' FROM analytics.events e WHERE e.user_id = {i};\n"
            )),
        }
    }
    s
}

/// Time `n` keystrokes from `each(i)`, each followed by a frame.
fn keystrokes(
    app: &mut App,
    term: &mut Terminal<TestBackend>,
    n: usize,
    mut each: impl FnMut(&mut App, usize),
) -> Vec<f64> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = Instant::now();
        each(app, i);
        apps::draw(term, app);
        out.push(ms(t.elapsed()));
    }
    out
}

pub fn run(scratch: &Path, bytes: usize, n: usize) -> Result<Value, String> {
    let text = generate(bytes);
    let path = scratch.join("big.sql");
    std::fs::write(&path, &text).map_err(|e| e.to_string())?;
    let lines = text.lines().count();
    let state = scratch.join("editor-state");
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).map_err(|e| e.to_string())?;
    let pid = std::process::id();
    let rss_before = rss_kb(pid).unwrap_or(0);
    let t = Instant::now();
    let mut app = apps::offline(&text, Some(state.clone()));
    drop(text);
    let mut term = apps::terminal();
    apps::draw(&mut term, &mut app);
    let open_ms = ms(t.elapsed());
    let rss_open = rss_kb(pid).unwrap_or(0);
    println!("editor: {bytes} bytes, {lines} lines; opened and drawn in {open_ms:.1} ms; RSS {rss_open} KiB");

    // Start in the middle of the file.
    app.tab_mut().editor.row = lines / 2;
    apps::draw(&mut term, &mut app);
    let typed = "select count(*) from shop.orders where id = 42 ";
    let typing = keystrokes(&mut app, &mut term, n, |a, i| {
        if i == 0 {
            apps::char(a, 'i');
        } else {
            let c = typed.as_bytes()[i % typed.len()] as char;
            apps::char(a, c);
        }
    });
    apps::key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    let rss_typed = rss_kb(pid).unwrap_or(0);
    let moves = ['j', 'j', 'l', 'w', 'k', 'b', 'e', 'j'];
    let movement = keystrokes(&mut app, &mut term, n, |a, i| apps::char(a, moves[i % moves.len()]));
    let editor_area = app.layout.editor;
    let (x, y) = (editor_area.x + 5, editor_area.y + 3);
    let scrolling = keystrokes(&mut app, &mut term, n, |a, i| apps::scroll(a, i % 10 != 9, x, y));
    // Normal-mode edits keep undo snapshots.
    let edits = keystrokes(&mut app, &mut term, n.min(100), |a, i| apps::char(a, if i % 2 == 0 { 'x' } else { 'u' }));
    let rss_edits = rss_kb(pid).unwrap_or(0);

    // A theme switch: every built-in theme in turn, each drawn at once.
    let names = datarig_tui::theme::NAMES;
    let themes = keystrokes(&mut app, &mut term, n.min(200), |a, i| {
        a.set_theme(names[i % names.len()]).expect("a built-in theme");
    });

    // Autosave: one edit, then the timer that saves it.
    let mut saves = Vec::new();
    for _ in 0..5 {
        apps::char(&mut app, 'x');
        let due = app.now() + Duration::from_secs(5);
        let t = Instant::now();
        app.on_tick(due);
        saves.push(ms(t.elapsed()));
    }
    let written = std::fs::read_dir(state.join("consoles")).map(|d| d.count()).unwrap_or(0);

    let report = |name: &str, xs: &[f64]| {
        let s = Summary::of(xs);
        println!("  {name:<10} {}", s.line(" ms"));
        s.json()
    };
    let out = json!({
        "bytes": bytes,
        "lines": lines,
        "open_ms": open_ms,
        "typing_ms": report("typing", &typing),
        "movement_ms": report("movement", &movement),
        "scrolling_ms": report("scrolling", &scrolling),
        "normal_edit_ms": report("x/u edits", &edits),
        "theme_switch_ms": report("theme", &themes),
        "autosave_ms": report("autosave", &saves),
        "rss_kb": { "before": rss_before, "open": rss_open, "after_typing": rss_typed, "after_edits": rss_edits },
        "console_files": written,
    });
    println!(
        "  RSS KiB: before {rss_before}, open {rss_open}, after typing {rss_typed}, after {} x/u edits {rss_edits}",
        n.min(100)
    );
    Ok(out)
}
