//! The result grid with wide rows of CJK text: the time to draw a frame at 160x45 while the
//! cursor moves down, pages and moves right.

use crate::apps;
use crate::stats::{Summary, ms};
use datarig_core::driver::{ColumnMeta, DbEvent};
use datarig_tui::app::Focus;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// `n` wide characters starting at `seed`: Hangul syllables (U+AC00…) or, with `han`, CJK
/// ideographs (U+4E00…). Built from code points, so the source stays ASCII.
pub fn wide(seed: usize, n: usize, han: bool) -> String {
    let (base, span) = if han { (0x4E00, 20_000) } else { (0xAC00, 11_000) };
    (0..n).filter_map(|i| char::from_u32(base + ((seed * 31 + i * 7) % span) as u32)).collect()
}

/// An emoji (two columns wide).
pub const EMOJI: char = '\u{1F600}';

/// `n` rows of `cols` columns: Hangul, CJK ideographs, emoji, long text, JSON and numbers.
pub fn rows(n: usize, cols: usize) -> (Vec<ColumnMeta>, Vec<Vec<Option<String>>>) {
    let columns = (0..cols)
        .map(|c| ColumnMeta {
            name: format!("{}_{c}", wide(c, 3, false)),
            type_name: if c % 4 == 3 { "bigint".into() } else { "text".into() },
            numeric: c % 4 == 3,
            json: c % 4 == 2,
            origin: None,
        })
        .collect();
    let rows = (0..n)
        .map(|r| {
            (0..cols)
                .map(|c| match (c % 4, r % 11) {
                    (_, 0) if c == 1 => None,
                    (0, _) => Some(format!(
                        "{} {r} {} {c} ({r}) {} {EMOJI}",
                        wide(r, 6, false),
                        wide(r + c, 4, false),
                        wide(r, 5, true)
                    )),
                    (1, _) => Some(format!("{} {r}", wide(r, 14 * (1 + r % 5), false))),
                    (2, _) => Some(format!(
                        "{{\"{}\": {r}, \"path\": \"/{}?q={}\", \"ms\": {c}, \"tags\": [\"a\", \"b\"]}}",
                        wide(c, 2, false),
                        wide(r, 2, false),
                        wide(r, 3, false)
                    )),
                    _ => Some(format!("{}", r as u64 * 7919 + c as u64)),
                })
                .collect()
        })
        .collect();
    (columns, rows)
}

pub fn run(n_rows: usize, cols: usize, n: usize) -> Result<Value, String> {
    let mut app = apps::offline("SELECT 1;", None);
    let (columns, data) = rows(n_rows, cols);
    app.on_db_event(DbEvent::Page { id: 0, columns: Some(columns), rows: data, more: false, elapsed: Duration::ZERO });
    app.focus = Focus::Results;
    let mut term = apps::terminal();
    apps::draw(&mut term, &mut app);
    let mut frame = Vec::new();
    let mut draw_only = Vec::new();
    let mut step = |app: &mut datarig_tui::app::App, code: KeyCode| {
        let t = Instant::now();
        apps::key(app, code, KeyModifiers::NONE);
        let d = Instant::now();
        apps::draw(&mut term, app);
        draw_only.push(ms(d.elapsed()));
        frame.push(ms(t.elapsed()));
    };
    for i in 0..n {
        let code = match i % 20 {
            0..=11 => KeyCode::Char('j'),
            12..=15 => KeyCode::Char('l'),
            16 | 17 => KeyCode::PageDown,
            _ => KeyCode::Char('h'),
        };
        step(&mut app, code);
    }
    let f = Summary::of(&frame);
    let d = Summary::of(&draw_only);
    println!("grid: {n_rows} rows x {cols} columns at {}x{}", apps::WIDTH, apps::HEIGHT);
    println!("  key+frame  {}", f.line(" ms"));
    println!("  draw only  {}", d.line(" ms"));
    Ok(json!({ "rows": n_rows, "cols": cols, "frame_ms": f.json(), "draw_ms": d.json() }))
}
