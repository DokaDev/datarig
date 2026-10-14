//! A large plan in the Plan tab: hundreds of nodes (a deep chain of joins, parallel workers,
//! subplans, a CTE, an `Append` over hundreds of partitions), shown in every view at 160x45 while
//! the selection moves. Measured: the key + frame time, and the nodes the views walked per
//! frame (the work count of `widgets::plan`), as a multiple of the plan's nodes.

use crate::apps;
use crate::stats::{Summary, ms};
use datarig_core::driver::{ColumnMeta, DbEvent, ValueKind};
use datarig_tui::app::Focus;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// One node of the plan, measured.
fn node(kind: &str, extra: Value, total: f64, loops: f64, plans: Vec<Value>) -> Value {
    let mut n = json!({
        "Node Type": kind,
        "Startup Cost": 0.0,
        "Total Cost": total * 10.0,
        "Plan Rows": 1000,
        "Plan Width": 8,
        "Actual Startup Time": total / 10.0,
        "Actual Total Time": total,
        "Actual Rows": 120.0,
        "Actual Loops": loops,
        "Shared Hit Blocks": 64,
        "Shared Read Blocks": 2,
    });
    if let (Value::Object(m), Value::Object(e)) = (&mut n, extra) {
        m.extend(e);
    }
    if !plans.is_empty() {
        n["Plans"] = Value::Array(plans);
    }
    n
}

/// The plan's JSON as PostgreSQL writes it: `joins` joins deep, over `partitions` partitions.
pub fn json(joins: usize, partitions: usize) -> String {
    let scans: Vec<Value> = (0..partitions)
        .map(|p| {
            let extra = json!({"Relation Name": format!("events_p{p}"), "Alias": format!("events_{p}"),
                "Filter": "(at >= '2024-01-01'::date)", "Rows Removed by Filter": 12, "Parent Relationship": "Member"});
            node("Seq Scan", extra, 0.05, 3.0, Vec::new())
        })
        .collect();
    let append = node("Append", json!({"Parent Relationship": "Outer"}), 40.0, 3.0, scans);
    let mut below = node("Gather", json!({"Workers Planned": 2, "Workers Launched": 2}), 60.0, 1.0, vec![append]);
    for j in 0..joins {
        let scan = node(
            "Index Scan",
            json!({"Index Name": format!("t{j}_pkey"), "Relation Name": format!("t{j}"), "Alias": format!("t{j}"),
                "Index Cond": format!("(id = t{}.ref)", j + 1), "Parent Relationship": "Inner"}),
            0.01,
            120.0,
            Vec::new(),
        );
        let sub = node(
            "Aggregate",
            json!({"Parent Relationship": "SubPlan", "Subplan Name": format!("SubPlan {j}")}),
            0.002,
            120.0,
            vec![node(
                "Seq Scan",
                json!({"Relation Name": "small", "Parent Relationship": "Outer"}),
                0.001,
                120.0,
                Vec::new(),
            )],
        );
        let mut outer = below;
        outer["Parent Relationship"] = json!("Outer");
        below = node(
            "Nested Loop",
            json!({"Join Type": if j % 3 == 0 { "Left" } else { "Inner" }}),
            62.0 + j as f64 * 2.0,
            1.0,
            vec![outer, scan, sub],
        );
    }
    let cte = node(
        "Aggregate",
        json!({"Strategy": "Hashed", "Parent Relationship": "InitPlan", "Subplan Name": "CTE totals"}),
        5.0,
        1.0,
        vec![node(
            "Seq Scan",
            json!({"Relation Name": "orders", "Parent Relationship": "Outer"}),
            2.0,
            1.0,
            Vec::new(),
        )],
    );
    let scan = node(
        "CTE Scan",
        json!({"CTE Name": "totals", "Alias": "totals", "Parent Relationship": "Inner"}),
        5.5,
        1.0,
        Vec::new(),
    );
    below["Parent Relationship"] = json!("Outer");
    let root = node("Hash Join", json!({"Hash Cond": "(totals.id = t0.id)"}), 200.0, 1.0, vec![cte, below, scan]);
    json!([{ "Plan": root, "Planning Time": 3.2, "Triggers": [], "Execution Time": 210.0 }]).to_string()
}

pub fn run(joins: usize, partitions: usize, n: usize) -> Result<Value, String> {
    let text = json(joins, partitions);
    let nodes = datarig_core::sql::plan::pg::parse(&text).map_err(|e| format!("{e:?}"))?.nodes.len();
    let mut app = apps::offline("EXPLAIN (ANALYZE, FORMAT JSON) SELECT 1;", None);
    let columns = vec![ColumnMeta {
        name: "QUERY PLAN".into(),
        type_name: "json".into(),
        numeric: false,
        json: true,
        kind: ValueKind::Json,
        origin: None,
    }];
    app.on_db_event(DbEvent::Page {
        id: 0,
        columns: Some(columns),
        rows: vec![vec![Some(text)]],
        more: false,
        elapsed: Duration::ZERO,
    });
    if app.tab().exec.plan.is_none() {
        return Err("the plan was not read".into());
    }
    app.focus = Focus::Results;
    let mut term = apps::terminal();
    let mut views = serde_json::Map::new();
    let (mut all_frames, mut worst_work) = (Vec::new(), 0u64);
    let every = [
        ('1', "tree"),
        ('2', "summary"),
        ('3', "icicle"),
        ('4', "flame"),
        ('5', "timeline"),
        ('6', "rows"),
        ('7', "treemap"),
        ('8', "boxes"),
        ('9', "raw"),
    ];
    for (digit, view) in every {
        apps::key(&mut app, KeyCode::Char(digit), KeyModifiers::NONE);
        apps::key(&mut app, KeyCode::Char('g'), KeyModifiers::NONE);
        apps::key(&mut app, KeyCode::Char('g'), KeyModifiers::NONE);
        apps::draw(&mut term, &mut app);
        datarig_tui::widgets::plan::take_work();
        let mut frame = Vec::new();
        let mut work = Vec::new();
        for i in 0..n {
            let code = match i % 10 {
                0..=6 => KeyCode::Char('j'),
                7 => KeyCode::PageDown,
                8 => KeyCode::Enter,
                _ => KeyCode::Char('k'),
            };
            let t = Instant::now();
            apps::key(&mut app, code, KeyModifiers::NONE);
            apps::draw(&mut term, &mut app);
            frame.push(ms(t.elapsed()));
            let w = datarig_tui::widgets::plan::take_work();
            worst_work = worst_work.max(w);
            work.push(w as f64);
        }
        let f = Summary::of(&frame);
        let w = Summary::of(&work);
        println!("plan {view}: {nodes} nodes at {}x{}", apps::WIDTH, apps::HEIGHT);
        println!("  key+frame  {}", f.line(" ms"));
        println!("  nodes walked per frame  {}", w.line(""));
        views.insert(view.into(), json!({ "frame_ms": f.json(), "work": w.json() }));
        all_frames.extend(frame);
    }
    let f = Summary::of(&all_frames);
    Ok(json!({
        "nodes": nodes,
        "frame_ms": f.json(),
        "work_max": worst_work,
        "work_per_node_max": worst_work as f64 / nodes as f64,
        "views": views,
    }))
}
