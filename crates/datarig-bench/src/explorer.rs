//! The explorer's `/` filter over a large schema tree: one connected profile whose schemas are
//! all listed and open (`schemas` × `per_schema` relations, with estimates), the filter typed
//! and erased a key at a time at 160x45. Measured: each key with its frame (the filter edit, the
//! rows it shows and the explorer drawn).

use crate::apps;
use crate::stats::{Summary, ms};
use datarig_core::driver::SchemaObjects;
use datarig_core::driver::structure::RelationStats;
use datarig_tui::app::Focus;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Instant;

/// The objects of schema `s`: `per_schema` relations, a tenth of them views, every table with
/// estimates.
fn objects(s: usize, per_schema: usize) -> SchemaObjects {
    let kinds = ["orders", "events", "users", "invoices", "audit_log"];
    let name = |j: usize| format!("{}_{s:02}_{j:03}", kinds[j % kinds.len()]);
    let (tables, views): (Vec<usize>, Vec<usize>) = (0..per_schema).partition(|j| j % 10 != 9);
    let stats: BTreeMap<String, RelationStats> = tables
        .iter()
        .map(|&j| (name(j), RelationStats { rows: Some(j as u64 * 1_000), bytes: Some(j as u64 * 65_536) }))
        .collect();
    SchemaObjects {
        tables: tables.into_iter().map(name).collect(),
        views: views.into_iter().map(name).collect(),
        materialized: Default::default(),
        stats,
    }
}

pub fn run(schemas: usize, per_schema: usize, n: usize) -> Result<Value, String> {
    let mut app = apps::offline("SELECT 1;", None);
    let id = app.profiles[0].id;
    let c = app.conns.entry(id);
    c.connected = true;
    c.expanded = true;
    let names: Vec<String> = (0..schemas).map(|s| format!("schema_{s:02}")).collect();
    c.tree.set_schemas(names.clone());
    for (s, name) in names.iter().enumerate() {
        c.tree.set_objects(name, Ok(objects(s, per_schema)));
    }
    c.tree.schemas.iter_mut().for_each(|s| s.expanded = true);
    let unfiltered = app.explorer_rows().len();
    app.focus = Focus::Tree;
    let mut term = apps::terminal();
    apps::draw(&mut term, &mut app);
    apps::char(&mut app, '/');
    apps::draw(&mut term, &mut app);
    // Typed and erased again: from one letter matching nearly every relation to one matching
    // a few, and back to no filter at all (every row).
    let typed = "events_07_1";
    let mut keys = Vec::new();
    for c in typed.chars() {
        keys.push(KeyCode::Char(c));
    }
    keys.extend(std::iter::repeat_n(KeyCode::Backspace, typed.len()));
    let mut frame = Vec::new();
    let mut most = 0;
    for i in 0..n {
        let code = keys[i % keys.len()];
        let t = Instant::now();
        apps::key(&mut app, code, KeyModifiers::NONE);
        apps::draw(&mut term, &mut app);
        frame.push(ms(t.elapsed()));
        most = most.max(app.explorer_rows().len());
    }
    let f = Summary::of(&frame);
    println!(
        "explorer filter: {} relations in {schemas} open schemas ({unfiltered} rows) at {}x{}",
        schemas * per_schema,
        apps::WIDTH,
        apps::HEIGHT
    );
    println!("  key+frame  {}", f.line(" ms"));
    Ok(json!({
        "relations": schemas * per_schema,
        "rows_unfiltered": unfiltered,
        "rows_most": most,
        "frame_ms": f.json(),
    }))
}
