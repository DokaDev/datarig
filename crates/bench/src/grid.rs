//! The result grid with wide rows of CJK text: the time to draw a frame at 160x45 while the
//! cursor moves down, pages and moves right; then its search (`/` typed key by key, `Enter`,
//! `n`): the time of each key with its frame (the jobs answered first), and the cells one frame
//! looked up for the matches' highlight (only the cells on screen; none without a search); then
//! bursts of sideways wheel events (a trackpad swipe): the frames each burst drew (the event loop
//! draws a burst once, `input::burst`) and the cells one frame drew (the columns on screen, the
//! ones cut by the edges included, times the rows), and that each burst moved the view by all of
//! its events.

use crate::apps;
use crate::stats::{Summary, ms};
use datarig_core::driver::{ColumnMeta, DbEvent, ValueKind};
use datarig_tui::app::Focus;
use datarig_tui::app::Results;
use datarig_tui::input::burst;
use ratatui::crossterm::event::{Event, KeyCode, KeyModifiers, MouseEvent, MouseEventKind};
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
            kind: match c % 4 {
                3 => ValueKind::Integer,
                2 => ValueKind::Json,
                _ => ValueKind::Text,
            },
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
    // Without a search, the highlight looks nothing up.
    app.take_grid_search_work();
    let mut idle_lookups = 0;
    for i in 0..n {
        let code = match i % 20 {
            0..=11 => KeyCode::Char('j'),
            12..=15 => KeyCode::Char('l'),
            16 | 17 => KeyCode::PageDown,
            _ => KeyCode::Char('h'),
        };
        step(&mut app, code);
        idle_lookups = idle_lookups.max(app.take_grid_search_work());
    }
    let f = Summary::of(&frame);
    let d = Summary::of(&draw_only);
    println!("grid: {n_rows} rows x {cols} columns at {}x{}", apps::WIDTH, apps::HEIGHT);
    println!("  key+frame  {}", f.line(" ms"));
    println!("  draw only  {}", d.line(" ms"));
    // The search: a pattern of the numbers column typed key by key, `Enter`, then `n`.
    let mut search = Vec::new();
    let mut lookups = 0;
    let mut timed = |app: &mut datarig_tui::app::App, code: KeyCode| {
        let t = Instant::now();
        apps::key(app, code, KeyModifiers::NONE);
        app.pump_grid_search(Duration::from_secs(60));
        apps::draw(&mut term, app);
        search.push(ms(t.elapsed()));
        lookups = lookups.max(app.take_grid_search_work());
    };
    timed(&mut app, KeyCode::Char('/'));
    for c in "7919".chars() {
        timed(&mut app, KeyCode::Char(c));
    }
    timed(&mut app, KeyCode::Enter);
    for _ in 0..n.min(100) {
        timed(&mut app, KeyCode::Char('n'));
    }
    let on = app.grid_search_pattern().is_some();
    let s = Summary::of(&search);
    println!(
        "  search     {} (cells looked up per frame: at most {lookups}; without a search {idle_lookups})",
        s.line(" ms")
    );
    // Sideways: the search ended, bursts of `PER_BURST` wheel events right then left (and with
    // Shift on the vertical wheel), each burst handed over as the event loop does.
    apps::key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    apps::draw(&mut term, &mut app);
    let r = app.layout.results;
    let (x, y) = (r.x + r.width / 2, r.y + r.height / 2);
    let ev = |kind, modifiers| Event::Mouse(MouseEvent { kind, column: x, row: y, modifiers });
    let kinds = [
        ev(MouseEventKind::ScrollRight, KeyModifiers::NONE),
        ev(MouseEventKind::ScrollDown, KeyModifiers::SHIFT),
        ev(MouseEventKind::ScrollLeft, KeyModifiers::NONE),
        ev(MouseEventKind::ScrollUp, KeyModifiers::SHIFT),
    ];
    let (mut frames_max, mut cells_max, mut short) = (0, 0, 0);
    let mut side = Vec::new();
    let bursts = n.min(100);
    for i in 0..bursts {
        let events: Vec<Event> = std::iter::repeat_n(kinds[(i / 4) % 4].clone(), PER_BURST).collect();
        let was = app.tab().grid.x_off;
        let t = Instant::now();
        let mut frames = 0;
        let mut at = 0;
        while at < events.len() {
            // A frame: the events of one burst (as the event loop reads them), then one draw.
            let kind = burst::burst(&events[at]);
            let mut end = at + 1;
            while kind.is_some()
                && end < events.len()
                && end - at < burst::PER_FRAME
                && burst::burst(&events[end]) == kind
            {
                end += 1;
            }
            for e in &events[at..end] {
                app.handle_event(e.clone());
            }
            apps::draw(&mut term, &mut app);
            frames += 1;
            cells_max = cells_max.max(cells_drawn(&app));
            at = end;
        }
        side.push(ms(t.elapsed()));
        frames_max = frames_max.max(frames);
        // Each burst moves the view by all of its events, unless it stopped at an end.
        let g = &app.tab().grid;
        let at_end = g.x_off == 0 || g.x_off == g.x_max();
        if g.x_off.abs_diff(was) != PER_BURST * datarig_tui::widgets::grid::SIDE_STEP && !at_end {
            short += 1;
        }
    }
    let sd = Summary::of(&side);
    println!(
        "  sideways   {} a burst of {PER_BURST} (frames per burst: at most {frames_max}; cells drawn per frame: at most {cells_max}; bursts that moved short: {short})",
        sd.line(" ms")
    );
    Ok(json!({
        "rows": n_rows,
        "cols": cols,
        "frame_ms": f.json(),
        "draw_ms": d.json(),
        "search": { "on": on, "key_ms": s.json(), "lookups_per_frame_max": lookups, "idle_lookups_max": idle_lookups },
        "hscroll": {
            "bursts": bursts,
            "events_per_burst": PER_BURST,
            "frames_per_burst_max": frames_max,
            "cells_per_frame_max": cells_max,
            "short_bursts": short,
            "burst_ms": sd.json(),
        },
    }))
}

/// Wheel events in one sideways burst (a trackpad sends a few dozen a swipe).
const PER_BURST: usize = 24;

/// The cells the grid drew in the last frame: the columns on screen (also those cut by an edge)
/// times the rows drawn.
fn cells_drawn(app: &datarig_tui::app::App) -> usize {
    let t = app.tab();
    let Results::Rows(rs) = &t.results else { return 0 };
    let g = &t.grid;
    let rows = g.window(rs.rows.len()).end.saturating_sub(g.top).min(g.page_rows);
    g.hit_cols.len() * rows
}
