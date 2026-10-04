//! The editor with a large file: keystroke-to-frame latency (the app handles the key, then
//! draws the frame at 160x45) for typing, cursor movement, scrolling and vim's other motions
//! and edits, the process's memory, what an autosave of the file costs, a theme switch
//! (`:set theme=`) up to its frame, search: `/` with a pattern that is nowhere (each key
//! searches the whole text) and `n`, with the bytes each key searched and the lines each frame
//! highlighted counted, and `:%s` over the whole text with the bytes it searched. `block` (the
//! `editor_block` scenario, in a process of its own): Visual block operators over the whole
//! text, with the bytes they walked counted.

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

/// The most work of one keystroke and its frame: bytes searched, lines highlighted.
#[derive(Default)]
struct MostWork {
    bytes: usize,
    highlighted: usize,
}

/// Time `n` keystrokes from `each(i)`, each followed by a frame, and count the search work
/// of each.
fn search_keys(
    app: &mut App,
    term: &mut Terminal<TestBackend>,
    n: usize,
    mut each: impl FnMut(&mut App, usize),
) -> (Vec<f64>, MostWork) {
    let mut out = Vec::with_capacity(n);
    let mut most = MostWork::default();
    app.tab_mut().editor.take_search_work();
    for i in 0..n {
        let t = Instant::now();
        each(app, i);
        apps::draw(term, app);
        out.push(ms(t.elapsed()));
        let w = app.tab_mut().editor.take_search_work();
        most.bytes = most.bytes.max(w.bytes);
        most.highlighted = most.highlighted.max(w.highlighted);
    }
    (out, most)
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
    // Vim's other motions (brackets and paragraphs lex or walk the lines around the cursor),
    // scrolling, a text object, `.` and undo.
    let vim_keys = [
        "}", "}", "{", "%", "%", "f", "(", ";", ",", "W", "B", "E", "g", "e", "H", "M", "L", "C-d", "C-u", "z", "z",
        "d", "i", "w", "u", ".", "u", "j", "j",
    ];
    let vim = keystrokes(&mut app, &mut term, n, |a, i| match vim_keys[i % vim_keys.len()] {
        k if k.starts_with("C-") => apps::key(a, KeyCode::Char(k.as_bytes()[2] as char), KeyModifiers::CONTROL),
        k => {
            let c = k.chars().next().unwrap_or(' ');
            let m = if c.is_ascii_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
            apps::key(a, KeyCode::Char(c), m);
        }
    });
    let rss_edits = rss_kb(pid).unwrap_or(0);

    // Search: `/`, a pattern found nowhere typed key by key (each key searches the whole text
    // for the cursor's preview), `Enter` (once more, and the notice); then `n` over a word on
    // every few lines, its matches highlighted.
    // A command the keys above left unfinished (`g` of `ge`) is dropped first.
    apps::key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    let nowhere = "zq_nowhere";
    let (search_miss, miss) = search_keys(&mut app, &mut term, n, |a, i| match i % (nowhere.len() + 2) {
        0 => apps::char(a, '/'),
        k if k <= nowhere.len() => apps::char(a, nowhere.as_bytes()[k - 1] as char),
        _ => apps::key(a, KeyCode::Enter, KeyModifiers::NONE),
    });
    if app.tab().editor.searching() {
        apps::key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    }
    for c in "/orders".chars() {
        apps::char(&mut app, c);
    }
    apps::key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let (search_next, next) = search_keys(&mut app, &mut term, n, |a, _| apps::char(a, 'n'));
    let rows = app.layout.editor_text.height;
    // What a search of the whole text searches: each line with its line break.
    let text_bytes = app.tab().editor.len_bytes() + 1;

    // A theme switch: every built-in theme in turn, each drawn at once.
    let names = datarig_tui::theme::NAMES;
    let themes = keystrokes(&mut app, &mut term, n.min(200), |a, i| {
        a.set_theme(names[i % names.len()]).expect("a built-in theme");
    });

    // `:%s` over the whole text through the command line (`:`, the command typed, `Enter`,
    // the frame), then `u`: the bytes one substitute searched, in passes over the text, and its
    // time. It replaces a word on every few lines, so the whole text is one undo step.
    let mut subst = Vec::new();
    let (mut subst_bytes, mut subst_changed) = (0, true);
    for _ in 0..3 {
        let version = app.tab().editor.version();
        app.tab_mut().editor.take_search_work();
        let t = Instant::now();
        apps::char(&mut app, ':');
        for c in "%s/orders/ORDERS/g".chars() {
            apps::char(&mut app, c);
        }
        apps::key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        apps::draw(&mut term, &mut app);
        subst.push(ms(t.elapsed()));
        subst_bytes = subst_bytes.max(app.tab_mut().editor.take_search_work().bytes);
        subst_changed &= app.tab().editor.version() != version;
        apps::char(&mut app, 'u');
        apps::draw(&mut term, &mut app);
    }

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
        "vim_ms": report("vim", &vim),
        "theme_switch_ms": report("theme", &themes),
        "search_miss_ms": report("/ miss", &search_miss),
        "search_next_ms": report("n", &search_next),
        "search": {
            "miss_bytes_max": miss.bytes,
            "next_bytes_max": next.bytes,
            "highlight_lines_max": miss.highlighted.max(next.highlighted),
            "editor_rows": rows,
            "text_bytes": text_bytes,
        },
        "subst_ms": report(":%s", &subst),
        "subst": {
            // Nothing searched means the command did not run: not measured.
            "bytes_max": if subst_changed { subst_bytes } else { 0 },
            "text_bytes": text_bytes,
        },
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

/// Set in the process [`block_in_own_process`] starts: it runs [`block`] itself.
const IN_PROCESS: &str = "DATARIG_BENCH_BLOCK_IN_PROCESS";

/// [`block`] in a process of its own (this binary again): the copies of the whole text it
/// leaves in the registers and the undo steps, and the memory the allocator keeps after them,
/// stay out of this process, whose memory the other scenarios measure.
pub fn block_in_own_process(scratch: &Path, bytes: usize) -> Result<Value, String> {
    if std::env::var(IN_PROCESS).is_ok_and(|v| v == "1") {
        return block(scratch, bytes);
    }
    let out = scratch.join("editor-block.jsonl");
    let _ = std::fs::remove_file(&out);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let status = std::process::Command::new(exe)
        .arg("editor_block")
        .arg("--scratch")
        .arg(scratch)
        .arg("--out")
        .arg(&out)
        .env(IN_PROCESS, "1")
        .status()
        .map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let record: Value =
        serde_json::from_str(text.lines().last().unwrap_or("")).map_err(|e| format!("{}: {e}", out.display()))?;
    match record.get("result") {
        Some(r) if status.success() => Ok(r.clone()),
        _ => Err(format!("editor_block: {}", record.get("error").unwrap_or(&Value::Null))),
    }
}

/// Visual block operators over the whole of a file of about `bytes` bytes (`gg Ctrl+V G`, to
/// the line ends with `$`): `y`, `d` and `u`, `I` on every line and `u`; each key with its
/// frame, and the bytes the block operators walked for it counted.
pub fn block(scratch: &Path, bytes: usize) -> Result<Value, String> {
    let text = generate(bytes);
    let lines = text.lines().count();
    let state = scratch.join("editor-block-state");
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).map_err(|e| e.to_string())?;
    let mut app = apps::offline(&text, Some(state));
    drop(text);
    let mut term = apps::terminal();
    apps::draw(&mut term, &mut app);
    println!("editor_block: {bytes} bytes, {lines} lines");
    let keys = [
        "g", "g", "C-v", "G", "$", "y", "g", "g", "C-v", "G", "$", "d", "u", "g", "g", "C-v", "G", "l", "I", "x",
        "Esc", "u",
    ];
    app.tab_mut().editor.take_block_work();
    let mut walked = 0;
    let times = keystrokes(&mut app, &mut term, keys.len(), |a, i| {
        match keys[i] {
            "C-v" => apps::key(a, KeyCode::Char('v'), KeyModifiers::CONTROL),
            "Esc" => apps::key(a, KeyCode::Esc, KeyModifiers::NONE),
            k => apps::char(a, k.chars().next().unwrap_or(' ')),
        }
        walked = walked.max(a.tab_mut().editor.take_block_work());
    });
    let s = Summary::of(&times);
    println!("  block      {}", s.line(" ms"));
    let text_bytes = app.tab().editor.len_bytes() + 1;
    println!("  bytes one key walked: {walked} ({:.2} passes over the text)", walked as f64 / text_bytes as f64);
    Ok(json!({ "bytes": bytes, "lines": lines, "keys_ms": s.json(), "bytes_max": walked, "text_bytes": text_bytes }))
}
