//! The editor with a large file: keystroke-to-frame latency (the app handles the key, then
//! draws the frame at 160x45) for typing, cursor movement, scrolling and vim's other motions
//! and edits, moving and scrolling among the hints of finished runs (each statement around the
//! cursor has one, as many as the editor keeps), the process's memory, what an autosave of the file costs, a theme switch
//! (`:set theme=`) up to its frame, search: `/` with a pattern that is nowhere (each key
//! searches the whole text) and `n`, with the bytes each key searched and the lines each frame
//! highlighted counted. `block` (the `editor_block` scenario, in a process of its own): Visual
//! block operators over the whole text, with the bytes they walked counted, and `:%s` over the
//! whole text with the bytes it searched, and typing next to a hinted statement of about 4.7 MB
//! (after a 100,000-line INSERT, above the same INSERT on one line, above a statement under a
//! 100,000-line comment header) with the bytes the hint checks lexed per key counted.
//! `editor_mysql`: the same keys in a 5 MB MySQL script with `DELIMITER` blocks.

use crate::apps;
use crate::grid::wide;
use crate::stats::{Summary, ms, rss_kb};
use datarig_tui::app::App;
use datarig_tui::widgets::editor::{HintKind, RunHint};
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

    // Run hints: every statement around the cursor ran (as `Ctrl+E` marks a run and its end),
    // so each has a hint after its last line; then moving and scrolling among them. They stay
    // for the scenarios below.
    apps::key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    let ed = &mut app.tab_mut().editor;
    let middle = ed.row;
    for (q, row) in (middle.saturating_sub(400)..middle + 400).step_by(2).enumerate() {
        ed.row = row.min(lines.saturating_sub(1));
        let (stmts, spans) = ed.run_statements();
        ed.stage_run(&stmts, spans);
        ed.start_run(q as u64, &stmts);
        let hint = RunHint { kind: HintKind::Ok, text: "128 rows \u{b7} 42ms \u{b7} 14:03".into() };
        ed.finish_run(q as u64, vec![Some(hint); stmts.len()]);
    }
    ed.row = middle;
    let hinted = ed.run_hints().count();
    let hint_keys = ['j', 'j', 'k', 'w', 'j', 'b', 'j', 'j'];
    let hints = keystrokes(&mut app, &mut term, n, |a, i| match i % 10 {
        8 => apps::key(a, KeyCode::Char('d'), KeyModifiers::CONTROL),
        9 => apps::key(a, KeyCode::Char('u'), KeyModifiers::CONTROL),
        k => apps::char(a, hint_keys[k % hint_keys.len()]),
    });

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
        "run_hints_ms": report("run hints", &hints),
        "run_hints": hinted,
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

/// A MySQL script of about `bytes` bytes: statements with backtick names, `#` and `-- `
/// comments, backslash escapes, executable comments and variables, and every few statements a
/// stored procedure between `DELIMITER $$` lines, about 70 characters per line.
pub fn generate_mysql(bytes: usize) -> String {
    let mut s = String::with_capacity(bytes + 256);
    let mut i = 0u64;
    while s.len() < bytes {
        i += 1;
        match i % 6 {
            0 => s.push_str(&format!(
                "DELIMITER $$\nCREATE PROCEDURE `p_{i}`(IN n INT)\nBEGIN\n  DECLARE v INT DEFAULT 0; -- {i}; a body\n  SET v = n + 1; # still; the body\n  SELECT v, 'a$$b; {i}' AS s;\nEND$$\nDELIMITER ;\n\n"
            )),
            1 => s.push_str(&format!(
                "INSERT INTO `shop`.`audit_log` (actor, action, note) VALUES ('bench', 'update', 'it\\'s {} {i}; with a semicolon');\n",
                wide(i as usize, 2, false)
            )),
            2 => s.push_str(&format!(
                "UPDATE shop.products SET price = price * 1.05, name = \"{} {i}\" WHERE id = {i}; # why\n",
                wide(i as usize, 2, false)
            )),
            3 => s.push_str(&format!(
                "SELECT /*!80000 SQL_NO_CACHE */ o.status, count(*) AS n FROM shop.orders o WHERE o.id > {i}\n GROUP BY o.status;\n"
            )),
            4 => s.push_str(&format!("SET @last_{i} := (SELECT max(id) FROM `shop`.`orders`), @@session.sql_select_limit = {i};\n")),
            _ => s.push_str(&format!(
                "/* block comment {i} */ SELECT e.id, e.event_type, e.payload->>'$.session' FROM shop.events e WHERE e.user_id = {i};\n"
            )),
        }
    }
    s
}

/// The editor with a 5 MB MySQL script (its text read as MySQL, `DELIMITER` blocks and all):
/// keystroke-to-frame time for typing, cursor movement, scrolling and vim's other motions, the
/// keys of the `editor` scenario; and jumps to the end and back after an edit (`x`, `G`, `gg`,
/// `u`), each jump lexing the line states of the whole text again.
pub fn run_mysql(bytes: usize, n: usize) -> Result<Value, String> {
    use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
    let text = generate_mysql(bytes);
    let lines = text.lines().count();
    let mut app = apps::offline(&text, None);
    drop(text);
    app.tab_mut().editor.set_language(Language::Sql(Dialect::MySql(MySqlMode::default())));
    let mut term = apps::terminal();
    let t = Instant::now();
    apps::draw(&mut term, &mut app);
    let open_ms = ms(t.elapsed());
    println!("editor_mysql: {bytes} bytes, {lines} lines; drawn in {open_ms:.1} ms");
    app.tab_mut().editor.row = lines / 2;
    apps::draw(&mut term, &mut app);
    let typed = "select count(*) from shop.orders where id = @n ";
    let typing = keystrokes(&mut app, &mut term, n, |a, i| {
        if i == 0 {
            apps::char(a, 'i');
        } else {
            apps::char(a, typed.as_bytes()[i % typed.len()] as char);
        }
    });
    apps::key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    let moves = ['j', 'j', 'l', 'w', 'k', 'b', 'e', 'j'];
    let movement = keystrokes(&mut app, &mut term, n, |a, i| apps::char(a, moves[i % moves.len()]));
    let editor_area = app.layout.editor;
    let (x, y) = (editor_area.x + 5, editor_area.y + 3);
    let scrolling = keystrokes(&mut app, &mut term, n, |a, i| apps::scroll(a, i % 10 != 9, x, y));
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
    apps::key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    let jump_keys = ["x", "G", "g", "g", "u"];
    let jumps = keystrokes(&mut app, &mut term, n.min(50), |a, i| match jump_keys[i % jump_keys.len()] {
        "G" => apps::key(a, KeyCode::Char('G'), KeyModifiers::SHIFT),
        k => apps::char(a, k.chars().next().unwrap_or(' ')),
    });
    let report = |name: &str, xs: &[f64]| {
        let s = Summary::of(xs);
        println!("  {name:<10} {}", s.line(" ms"));
        s.json()
    };
    Ok(json!({
        "bytes": bytes,
        "lines": lines,
        "open_ms": open_ms,
        "typing_ms": report("typing", &typing),
        "movement_ms": report("movement", &movement),
        "scrolling_ms": report("scrolling", &scrolling),
        "vim_ms": report("vim", &vim),
        "jump_ms": report("jumps", &jumps),
    }))
}

/// Typing next to a hinted statement, `n` keys each with its frame: the key times and the most
/// bytes one key's frame lexed to check the hint against the text; `None` when the hint is not
/// there after the keys (then nothing was checked, and the bytes would say nothing).
///
/// `text` is the editor's text; the statement on its line `run_line` is run and gets its hint;
/// then the keys go at the end of line `type_line`.
fn hinted_typing(text: &str, run_line: usize, type_line: usize, n: usize, hints: bool) -> Option<(Vec<f64>, usize)> {
    let mut app = apps::offline(text, None);
    if !hints {
        app.prefs.run_hints = datarig_core::config::RunHints::Off;
    }
    let mut term = apps::terminal();
    let ed = &mut app.tab_mut().editor;
    ed.row = run_line;
    let (stmts, spans) = ed.run_statements();
    ed.stage_run(&stmts, spans);
    ed.start_run(1, &stmts);
    let hint = RunHint { kind: HintKind::Ok, text: "100,000 rows affected \u{b7} 3.1s \u{b7} 14:03".into() };
    ed.finish_run(1, vec![Some(hint)]);
    ed.row = type_line;
    // The statement's first lines on screen too, below the line typed on.
    ed.top = type_line.saturating_sub(5);
    apps::char(&mut app, 'A');
    apps::draw(&mut term, &mut app);
    app.tab_mut().editor.take_check_work();
    let typed = " -- a comment typed here";
    let (mut times, mut most) = (Vec::with_capacity(n), 0);
    for i in 0..n {
        let t = Instant::now();
        apps::char(&mut app, typed.as_bytes()[i % typed.len()] as char);
        apps::draw(&mut term, &mut app);
        times.push(ms(t.elapsed()));
        most = most.max(app.tab_mut().editor.take_check_work());
    }
    let kept = app.tab_mut().editor.run_hints().count() == 1;
    (kept || !hints).then_some((times, most))
}

/// Typing next to a hinted statement of about 4.7 MB, three ways: after an `INSERT` of 100,000
/// lines (on the line after it), above the same `INSERT` written on one line (on the line before
/// it), and above a hinted `SELECT 1;` under a comment header of 100,000 lines (on the header's
/// last line). The key times of the first (the other two draw a 4.7 MB line or lex it for the
/// highlighter, as without hints), and per way the most bytes one key's hint checks lexed.
fn hinted_statement(n: usize) -> Result<(Vec<f64>, Value), String> {
    let mut rows = String::from("INSERT INTO shop.audit_log (id, note) VALUES\n");
    for i in 0..100_000 {
        rows.push_str(&format!("  ({i}, 'a note of row {i}, written for the bench'),\n"));
    }
    rows.truncate(rows.len() - 2);
    rows.push(';');
    let after = format!("{rows}\nSELECT 1;");
    let lines = after.lines().count();
    let one_line = format!("SELECT 0;\n-- the rows\n{}", rows.replace('\n', " "));
    let header = format!("{}SELECT 1;", "-- a line of the header that says what the script does\n".repeat(100_000));
    let mut out = serde_json::Map::new();
    let mut times = Vec::new();
    for (name, text, run_line, type_line) in [
        ("after", &after, 0, lines - 1),
        ("above_one_line", &one_line, 2, 1),
        ("above_header", &header, 100_000, 99_999),
    ] {
        let (t, most) =
            hinted_typing(text, run_line, type_line, n, true).ok_or(format!("hinted {name}: the hint went"))?;
        // Typing above the statement checks its hint each key: nothing lexed means no check ran.
        if name != "after" && most == 0 {
            return Err(format!("hinted {name}: no key checked the hint"));
        }
        let s = Summary::of(&t);
        println!("  hinted {name}: {}; the most bytes one key's hint checks lexed: {most}", s.line(" ms"));
        // The same keys with the hints off: what the frame costs without them.
        if let Some((off, _)) = hinted_typing(text, run_line, type_line, n, false) {
            println!("  hinted {name}, hints off: {}", Summary::of(&off).line(" ms"));
            out.insert(format!("{name}_hints_off_ms"), Summary::of(&off).json());
        }
        out.insert(format!("{name}_check_bytes_max"), json!(most));
        out.insert(format!("{name}_ms"), s.json());
        if name == "after" {
            times = t;
        }
    }
    Ok((times, Value::Object(out)))
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

    // Here too, for the copies it leaves in the undo steps: `:%s` over the whole text through the command line (`:`, the command typed, `Enter`,
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

    let subst = Summary::of(&subst);
    println!("  :%s        {}", subst.line(" ms"));
    drop(app);
    // Here too, for the copies of the big statement it lexes once (its run's end).
    let (hinted, hinted_ways) = hinted_statement(100)?;
    let hinted = Summary::of(&hinted);
    println!("  hinted     {}", hinted.line(" ms"));
    Ok(json!({
        "bytes": bytes,
        "lines": lines,
        "keys_ms": s.json(),
        "bytes_max": walked,
        "text_bytes": text_bytes,
        "subst_ms": subst.json(),
        // Nothing searched means the command did not run: not measured.
        "subst_bytes_max": if subst_changed { subst_bytes } else { 0 },
        "hinted_typing_ms": hinted.json(),
        "hinted": hinted_ways,
    }))
}
