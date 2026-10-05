//! PostgreSQL's `EXPLAIN (FORMAT JSON)` as a [`Plan`] (PostgreSQL 13 to 18; keys a version adds
//! that are not known here are kept as the node's properties, never an error).
//!
//! **Times.** PostgreSQL reports a node's times and rows per loop, averaged over its loops. A
//! node's total time is `Actual Total Time × Actual Loops`; in a parallel part of the plan
//! (under `Gather` or `Gather Merge`) its loops count every process that ran it, which ran at
//! the same time, so the total is divided by the number of processes (the gather's child's
//! loops per gather loop). Its self time is its total less its children's totals, never below
//! zero. A CTE (an `InitPlan` named `CTE …`) runs inside the CTE scans that read it, not where
//! it hangs: it is not taken from its parent; its time is taken from those scans instead, in
//! plan order (which scan ran it is not reported). Another `InitPlan` runs when a node reads its
//! value, often a scan below its parent: its time is then also inside that scan's, so self times
//! can add up to a little more than the whole.
//! By cost, a node's self cost is its total cost less its children's (every child: a parent's
//! cost includes its subplans').

use super::json::{Doc, Value};
use super::{Actual, Buffers, MAX_NODES, Plan, PlanError, PlanNode, TriggerRun};
use crate::sql::ident::sql_ident;

/// A result column PostgreSQL names the JSON plan of `EXPLAIN (FORMAT JSON)` with.
pub fn is_plan_column(name: &str, type_name: &str) -> bool {
    name == "QUERY PLAN" && matches!(type_name, "json" | "jsonb")
}

/// Read the text of an `EXPLAIN (FORMAT JSON)` result.
pub fn parse(text: &str) -> Result<Plan, PlanError> {
    let doc = super::json::parse(text).map_err(|_| PlanError::Json)?;
    // An array of one object per plan (a rule may add statements), or one object.
    let plans: Vec<usize> = match doc.get(doc.root) {
        Value::Array(items) => items.clone(),
        Value::Object(_) => vec![doc.root],
        _ => return Err(PlanError::NotAPlan),
    };
    let first = *plans.first().ok_or(PlanError::NotAPlan)?;
    let root = doc.field(first, "Plan").ok_or(PlanError::NotAPlan)?;
    if !matches!(doc.get(root), Value::Object(_)) || doc.field(root, "Node Type").is_none() {
        return Err(PlanError::NotAPlan);
    }
    let mut plan = Plan { more_plans: plans.len() - 1, ..Plan::default() };
    read_nodes(&doc, root, &mut plan)?;
    plan.analyzed = plan.nodes.iter().any(|n| n.actual.is_some());
    plan.timed = plan.nodes.iter().any(|n| n.actual.is_some_and(|a| a.total_ms.is_some()));
    read_statement(&doc, first, &mut plan);
    derive(&mut plan);
    plan.finish();
    Ok(plan)
}

/// The nodes under `root`, depth first, without recursion.
fn read_nodes(doc: &Doc, root: usize, plan: &mut Plan) -> Result<(), PlanError> {
    // (JSON object, parent node)
    let mut stack: Vec<(usize, Option<usize>)> = vec![(root, None)];
    while let Some((obj, parent)) = stack.pop() {
        if plan.nodes.len() >= MAX_NODES {
            return Err(PlanError::TooBig);
        }
        let i = plan.nodes.len();
        let mut n = node(doc, obj);
        n.parent = parent;
        if let Some(p) = parent {
            n.depth = plan.nodes[p].depth + 1;
            plan.nodes[p].children.push(i);
        }
        plan.nodes.push(n);
        if let Some(children) = doc.field(obj, "Plans") {
            // Pushed in reverse so the first child is read next.
            for &c in doc.items(children).iter().rev() {
                if matches!(doc.get(c), Value::Object(_)) {
                    stack.push((c, Some(i)));
                }
            }
        }
    }
    Ok(())
}

/// Keys a node's other fields are read from (not listed again as properties).
const STRUCTURE: &[&str] = &[
    "Node Type",
    "Strategy",
    "Partial Mode",
    "Operation",
    "Custom Plan Provider",
    "Parent Relationship",
    "Subplan Name",
    "Parallel Aware",
    "Async Capable",
    "Join Type",
    "Scan Direction",
    "Index Name",
    "Relation Name",
    "Schema",
    "Alias",
    "CTE Name",
    "Function Name",
    "Tuplestore Name",
    "Startup Cost",
    "Total Cost",
    "Plan Rows",
    "Plan Width",
    "Actual Startup Time",
    "Actual Total Time",
    "Actual Rows",
    "Actual Loops",
    "Plans",
    // Shown by the text form only in VERBOSE (or never): kept out of the properties.
    "Inner Unique",
];

/// Buffer counters: (key, field setter).
const BUFFER_KEYS: &[&str] = &[
    "Shared Hit Blocks",
    "Shared Read Blocks",
    "Shared Dirtied Blocks",
    "Shared Written Blocks",
    "Local Hit Blocks",
    "Local Read Blocks",
    "Local Dirtied Blocks",
    "Local Written Blocks",
    "Temp Read Blocks",
    "Temp Written Blocks",
];

/// Properties in the order the text form writes them; the rest follow in the JSON's order.
const ORDER: &[&str] = &[
    "Output",
    "Group Key",
    "Grouping Sets",
    "Sort Key",
    "Presorted Key",
    "Cache Key",
    "Cache Mode",
    "Hash Cond",
    "Merge Cond",
    "Join Filter",
    "Rows Removed by Join Filter",
    "Index Cond",
    "Order By",
    "Recheck Cond",
    "Rows Removed by Index Recheck",
    "TID Cond",
    "Function Call",
    "Table Function Call",
    "One-Time Filter",
    "Filter",
    "Rows Removed by Filter",
    "Index Searches",
    "Heap Fetches",
    "Workers Planned",
    "Workers Launched",
    "Params Evaluated",
];

/// Counters the text form writes only when they are not zero.
const ONLY_NONZERO: &[&str] =
    &["Rows Removed by Join Filter", "Rows Removed by Index Recheck", "Rows Removed by Filter", "Subplans Removed"];

fn num(doc: &Doc, obj: usize, key: &str) -> Option<f64> {
    doc.field(obj, key).and_then(|v| doc.num(v))
}

fn text(doc: &Doc, obj: usize, key: &str) -> Option<String> {
    doc.field(obj, key).and_then(|v| doc.str(v)).map(str::to_string)
}

fn flag(doc: &Doc, obj: usize, key: &str) -> bool {
    doc.field(obj, key).is_some_and(|v| matches!(doc.get(v), Value::Bool(true)))
}

/// A number as the text form writes a counter (`%.0f`).
fn whole(n: f64) -> String {
    format!("{n:.0}")
}

fn node(doc: &Doc, obj: usize) -> PlanNode {
    let kind = text(doc, obj, "Node Type").unwrap_or_default();
    let (op, target) = name(doc, obj, &kind);
    let cost = num(doc, obj, "Startup Cost").zip(num(doc, obj, "Total Cost"));
    let rows = doc.field(obj, "Actual Rows");
    let actual = match (rows.and_then(|r| doc.num(r)), num(doc, obj, "Actual Loops")) {
        (Some(r), Some(loops)) => Some(Actual {
            startup_ms: num(doc, obj, "Actual Startup Time"),
            total_ms: num(doc, obj, "Actual Total Time"),
            rows: r,
            rows_decimals: rows.is_some_and(|v| matches!(doc.get(v), Value::Number(t, _) if t.contains('.'))),
            loops,
        }),
        _ => None,
    };
    let buffers = read_buffers(doc, obj);
    let loops = actual.map_or(1.0, |a| a.loops);
    let removed = ["Rows Removed by Filter", "Rows Removed by Join Filter", "Rows Removed by Index Recheck"]
        .iter()
        .filter_map(|k| num(doc, obj, k))
        .reduce(|a, b| a + b)
        .map(|r| r * loops);
    let workers =
        num(doc, obj, "Workers Planned").map(|p| (p as u64, num(doc, obj, "Workers Launched").map(|l| l as u64)));
    PlanNode {
        op,
        target,
        relationship: text(doc, obj, "Parent Relationship"),
        subplan: text(doc, obj, "Subplan Name"),
        cost,
        plan_rows: num(doc, obj, "Plan Rows"),
        plan_width: num(doc, obj, "Plan Width"),
        actual,
        buffers,
        removed,
        workers,
        properties: properties(doc, obj, buffers),
        ..PlanNode::default()
    }
}

/// The operation and its target as `EXPLAIN`'s text names them (`src/backend/commands/
/// explain.c`).
fn name(doc: &Doc, obj: usize, kind: &str) -> (String, Option<String>) {
    let strategy = text(doc, obj, "Strategy");
    let mut op = match kind {
        "Aggregate" => match strategy.as_deref() {
            Some("Sorted") => "GroupAggregate".to_string(),
            Some("Hashed") => "HashAggregate".to_string(),
            Some("Mixed") => "MixedAggregate".to_string(),
            _ => "Aggregate".to_string(),
        },
        "SetOp" => match strategy.as_deref() {
            Some("Hashed") => "HashSetOp".to_string(),
            _ => "SetOp".to_string(),
        },
        "ModifyTable" => text(doc, obj, "Operation").unwrap_or_else(|| kind.to_string()),
        "Foreign Scan" => match text(doc, obj, "Operation").as_deref() {
            Some(o @ ("Insert" | "Update" | "Delete")) => format!("Foreign {o}"),
            _ => kind.to_string(),
        },
        "Custom Scan" => match text(doc, obj, "Custom Plan Provider") {
            Some(p) => format!("Custom Scan ({p})"),
            None => kind.to_string(),
        },
        _ => kind.to_string(),
    };
    if let Some(mode @ ("Partial" | "Finalize")) = text(doc, obj, "Partial Mode").as_deref() {
        op = format!("{mode} {op}");
    }
    if let Some(jt) = text(doc, obj, "Join Type").filter(|j| j != "Inner") {
        op = match op.strip_suffix(" Join") {
            Some(base) => format!("{base} {jt} Join"),
            None => format!("{op} {jt} Join"),
        };
    }
    if flag(doc, obj, "Async Capable") {
        op = format!("Async {op}");
    }
    if flag(doc, obj, "Parallel Aware") {
        op = format!("Parallel {op}");
    }
    let mut target = String::new();
    if matches!(kind, "Index Scan" | "Index Only Scan") {
        if text(doc, obj, "Scan Direction").as_deref() == Some("Backward") {
            op.push_str(" Backward");
        }
        if let Some(index) = text(doc, obj, "Index Name") {
            target.push_str(&format!("using {} ", sql_ident(&index)));
        }
    }
    if kind == "Bitmap Index Scan" {
        return (op, text(doc, obj, "Index Name").map(|i| format!("on {}", sql_ident(&i))));
    }
    let object = text(doc, obj, "Relation Name")
        .or_else(|| text(doc, obj, "Function Name"))
        .or_else(|| text(doc, obj, "CTE Name"))
        .or_else(|| text(doc, obj, "Tuplestore Name"));
    let alias = text(doc, obj, "Alias");
    match (object, alias) {
        (Some(o), alias) => {
            let qualified = match text(doc, obj, "Schema") {
                Some(s) => format!("{}.{}", sql_ident(&s), sql_ident(&o)),
                None => sql_ident(&o),
            };
            target.push_str(&format!("on {qualified}"));
            if let Some(a) = alias.filter(|a| *a != o) {
                target.push_str(&format!(" {}", sql_ident(&a)));
            }
        }
        (None, Some(a)) => target.push_str(&format!("on {}", sql_ident(&a))),
        (None, None) => {}
    }
    let target = target.trim_end().to_string();
    (op, (!target.is_empty()).then_some(target))
}

fn read_buffers(doc: &Doc, obj: usize) -> Option<Buffers> {
    if !BUFFER_KEYS.iter().any(|k| doc.field(obj, k).is_some()) {
        return None;
    }
    let g = |k: &str| num(doc, obj, k).map_or(0, |n| n.max(0.0) as u64);
    Some(Buffers {
        shared_hit: g("Shared Hit Blocks"),
        shared_read: g("Shared Read Blocks"),
        shared_dirtied: g("Shared Dirtied Blocks"),
        shared_written: g("Shared Written Blocks"),
        local_hit: g("Local Hit Blocks"),
        local_read: g("Local Read Blocks"),
        local_dirtied: g("Local Dirtied Blocks"),
        local_written: g("Local Written Blocks"),
        temp_read: g("Temp Read Blocks"),
        temp_written: g("Temp Written Blocks"),
    })
}

/// `Buffers: shared hit=1 read=2, temp written=3` as the text form writes it (`None`: nothing
/// to say).
pub fn buffers_text(b: &Buffers) -> Option<String> {
    let part = |label: &str, fields: &[(&str, u64)]| {
        let set: Vec<String> = fields.iter().filter(|(_, n)| *n > 0).map(|(k, n)| format!("{k}={n}")).collect();
        (!set.is_empty()).then(|| format!("{label} {}", set.join(" ")))
    };
    let parts: Vec<String> = [
        part(
            "shared",
            &[
                ("hit", b.shared_hit),
                ("read", b.shared_read),
                ("dirtied", b.shared_dirtied),
                ("written", b.shared_written),
            ],
        ),
        part(
            "local",
            &[("hit", b.local_hit), ("read", b.local_read), ("dirtied", b.local_dirtied), ("written", b.local_written)],
        ),
        part("temp", &[("read", b.temp_read), ("written", b.temp_written)]),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// A node's properties as its text form says them, in that order.
fn properties(doc: &Doc, obj: usize, buffers: Option<Buffers>) -> Vec<(String, String)> {
    let members = doc.members(obj);
    let mut used: Vec<&str> = STRUCTURE.iter().chain(BUFFER_KEYS).copied().collect();
    let mut out: Vec<(String, String)> = Vec::new();
    let put = |out: &mut Vec<(String, String)>, key: &str| {
        let Some(v) = doc.field(obj, key) else { return };
        if ONLY_NONZERO.contains(&key) && doc.num(v) == Some(0.0) {
            return;
        }
        let shown = match doc.get(v) {
            // A false flag says nothing (the text form leaves it out).
            Value::Bool(false) | Value::Null => None,
            Value::Number(_, n) => Some(whole_or_text(doc, v, *n)),
            _ => doc.scalar_text(v),
        };
        if let Some(s) = shown {
            out.push((key.to_string(), s));
        }
    };
    for &key in ORDER {
        put(&mut out, key);
        used.push(key);
    }
    // Grouped counters, one line each as the text form writes them.
    let n = |k: &str| num(doc, obj, k);
    if let Some(method) = text(doc, obj, "Sort Method") {
        let space = match (text(doc, obj, "Sort Space Type"), n("Sort Space Used")) {
            (Some(t), Some(u)) => format!("  {t}: {}kB", whole(u)),
            _ => String::new(),
        };
        out.push(("Sort Method".into(), format!("{method}{space}")));
    }
    if let (Some(b), Some(batches)) = (n("Hash Buckets"), n("Hash Batches")) {
        let ob = n("Original Hash Buckets").unwrap_or(b);
        let obt = n("Original Hash Batches").unwrap_or(batches);
        let buckets = if ob != b { format!("{} (originally {})", whole(b), whole(ob)) } else { whole(b) };
        let batch =
            if obt != batches { format!("{} (originally {})", whole(batches), whole(obt)) } else { whole(batches) };
        let mem = n("Peak Memory Usage").map(|m| format!("  Memory Usage: {}kB", whole(m))).unwrap_or_default();
        out.push(("Buckets".into(), format!("{buckets}  Batches: {batch}{mem}")));
        used.push("Peak Memory Usage");
    }
    if let Some(batches) = n("HashAgg Batches") {
        let mut line = String::new();
        if let Some(p) = n("Planned Partitions").filter(|p| *p > 0.0) {
            line.push_str(&format!("Planned Partitions: {}  ", whole(p)));
        }
        line.push_str(&format!("Batches: {}", whole(batches)));
        if let Some(m) = n("Peak Memory Usage") {
            line.push_str(&format!("  Memory Usage: {}kB", whole(m)));
        }
        if let Some(d) = n("Disk Usage").filter(|_| batches > 1.0) {
            line.push_str(&format!("  Disk Usage: {}kB", whole(d)));
        }
        let (key, value) = line.split_once(": ").map(|(k, v)| (k.to_string(), v.to_string())).unwrap_or_default();
        out.push((key, value));
        used.extend(["Peak Memory Usage", "Disk Usage"]);
    } else if let Some(p) = n("Planned Partitions").filter(|p| *p > 0.0) {
        out.push(("Planned Partitions".into(), whole(p)));
    }
    if let (Some(h), Some(m)) = (n("Cache Hits"), n("Cache Misses")) {
        let ev = n("Cache Evictions").unwrap_or(0.0);
        let ov = n("Cache Overflows").unwrap_or(0.0);
        let mem = n("Peak Memory Usage").map(|m| format!("  Memory Usage: {}kB", whole(m))).unwrap_or_default();
        let value =
            format!("{}  Misses: {}  Evictions: {}  Overflows: {}{mem}", whole(h), whole(m), whole(ev), whole(ov));
        out.push(("Hits".into(), value));
        used.push("Peak Memory Usage");
    }
    let (exact, lossy) = (n("Exact Heap Blocks").unwrap_or(0.0), n("Lossy Heap Blocks").unwrap_or(0.0));
    if exact > 0.0 || lossy > 0.0 {
        let mut parts = Vec::new();
        if exact > 0.0 {
            parts.push(format!("exact={}", whole(exact)));
        }
        if lossy > 0.0 {
            parts.push(format!("lossy={}", whole(lossy)));
        }
        out.push(("Heap Blocks".into(), parts.join(" ")));
    }
    if let Some(s) = text(doc, obj, "Storage") {
        let max = n("Maximum Storage").map(|m| format!("  Maximum Storage: {}kB", whole(m))).unwrap_or_default();
        out.push(("Storage".into(), format!("{s}{max}")));
    }
    if let Some(s) = buffers.as_ref().and_then(buffers_text) {
        out.push(("Buffers".into(), s));
    }
    // `I/O Timings: shared read=1.000 write=2.000, temp read=3.000` (PostgreSQL 16 on), or
    // `read=… write=…` before it.
    let io: Vec<String> = ["", "Shared ", "Local ", "Temp "]
        .iter()
        .filter_map(|group| {
            let t = |what: &str| n(&format!("{group}I/O {what} Time")).filter(|ms| *ms > 0.0);
            let parts: Vec<String> = [("read", t("Read")), ("write", t("Write"))]
                .into_iter()
                .filter_map(|(w, ms)| ms.map(|ms| format!("{w}={ms:.3}")))
                .collect();
            let label = group.trim().to_lowercase();
            (!parts.is_empty()).then(|| [label, parts.join(" ")].join(" ").trim().to_string())
        })
        .collect();
    if !io.is_empty() {
        out.push(("I/O Timings".into(), io.join(", ")));
    }
    // Incremental sort: `Full-sort Groups: 2  Sort Method: quicksort  Average Memory: 26kB …`.
    for key in ["Full-sort Groups", "Pre-sorted Groups"] {
        let Some(g) = doc.field(obj, key) else { continue };
        let mut line = num(doc, g, "Group Count").map_or_else(String::new, whole);
        if let Some(m) = doc.field(g, "Sort Methods Used").and_then(|m| doc.scalar_text(m)) {
            let plural = if m.contains(", ") { "s" } else { "" };
            line.push_str(&format!("  Sort Method{plural}: {m}"));
        }
        for (space, label) in [("Sort Space Memory", "Memory"), ("Sort Space Disk", "Disk")] {
            if let Some(sp) = doc.field(g, space) {
                for (k, l) in [("Average Sort Space Used", "Average"), ("Peak Sort Space Used", "Peak")] {
                    if let Some(v) = num(doc, sp, k) {
                        line.push_str(&format!("  {l} {label}: {}kB", whole(v)));
                    }
                }
            }
        }
        out.push((key.to_string(), line));
        used.push(key);
    }
    if let Some(records) = n("WAL Records") {
        let mut wal = format!("records={}", whole(records));
        for (k, label) in [("WAL FPI", "fpi"), ("WAL Bytes", "bytes"), ("WAL Buffers Full", "buffers full")] {
            if let Some(v) = n(k).filter(|v| *v > 0.0) {
                wal.push_str(&format!(" {label}={}", whole(v)));
            }
        }
        out.push(("WAL".into(), wal));
    }
    used.extend([
        "Sort Method",
        "Sort Space Type",
        "Sort Space Used",
        "Hash Buckets",
        "Hash Batches",
        "Original Hash Buckets",
        "Original Hash Batches",
        "HashAgg Batches",
        "Planned Partitions",
        "Cache Hits",
        "Cache Misses",
        "Cache Evictions",
        "Cache Overflows",
        "Exact Heap Blocks",
        "Lossy Heap Blocks",
        "Storage",
        "Maximum Storage",
        "WAL Records",
        "WAL FPI",
        "WAL Bytes",
        "WAL Buffers Full",
        "Workers",
    ]);
    // Whatever else the node says (a key of a newer version), as it says it.
    for (k, v) in members {
        if used.contains(&k.as_str())
            || k.ends_with("I/O Read Time")
            || k.ends_with("I/O Write Time")
            || k.ends_with(" Groups")
        {
            continue;
        }
        let shown = match doc.get(*v) {
            Value::Bool(false) | Value::Null => None,
            Value::Number(_, n) if *n == 0.0 && ONLY_NONZERO.contains(&k.as_str()) => None,
            Value::Number(_, n) => Some(whole_or_text(doc, *v, *n)),
            Value::Object(_) => Some(flat(doc, *v)),
            _ => doc.scalar_text(*v),
        };
        if let Some(s) = shown.filter(|s| !s.is_empty()) {
            out.push((k.clone(), s));
        }
    }
    // Each parallel worker's own numbers.
    if let Some(w) = doc.field(obj, "Workers") {
        for &item in doc.items(w) {
            let no = num(doc, item, "Worker Number").map_or_else(String::new, whole);
            out.push((format!("Worker {no}"), worker(doc, item)));
        }
    }
    out
}

/// A parallel worker's own numbers as psql writes them after `Worker N:`: its measurement, its
/// sort or hash aggregate, then anything else it says.
fn worker(doc: &Doc, w: usize) -> String {
    let n = |k: &str| num(doc, w, k);
    let mut parts: Vec<String> = Vec::new();
    let mut used = vec!["Worker Number"];
    if let (Some(rows), Some(loops)) = (n("Actual Rows"), n("Actual Loops")) {
        let rows = match doc.field(w, "Actual Rows").map(|v| doc.get(v)) {
            Some(Value::Number(t, _)) => t.clone(),
            _ => whole(rows),
        };
        let time = match (n("Actual Startup Time"), n("Actual Total Time")) {
            (Some(a), Some(b)) => format!("time={a:.3}..{b:.3} "),
            _ => String::new(),
        };
        parts.push(format!(" actual {time}rows={rows} loops={}", whole(loops)));
        used.extend(["Actual Rows", "Actual Loops", "Actual Startup Time", "Actual Total Time"]);
    }
    if let Some(method) = text(doc, w, "Sort Method") {
        let space = match (text(doc, w, "Sort Space Type"), n("Sort Space Used")) {
            (Some(t), Some(u)) => format!("  {t}: {}kB", whole(u)),
            _ => String::new(),
        };
        parts.push(format!("Sort Method: {method}{space}"));
        used.extend(["Sort Method", "Sort Space Type", "Sort Space Used"]);
    }
    if let Some(b) = n("HashAgg Batches") {
        let mem = n("Peak Memory Usage").map(|m| format!("  Memory Usage: {}kB", whole(m))).unwrap_or_default();
        let disk =
            n("Disk Usage").filter(|_| b > 1.0).map(|d| format!("  Disk Usage: {}kB", whole(d))).unwrap_or_default();
        parts.push(format!("Batches: {}{mem}{disk}", whole(b)));
        used.extend(["HashAgg Batches", "Peak Memory Usage", "Disk Usage"]);
    }
    if let Some(b) = read_buffers(doc, w).as_ref().and_then(buffers_text) {
        parts.push(format!("Buffers: {b}"));
    }
    used.extend(BUFFER_KEYS);
    for (k, v) in doc.members(w) {
        if used.contains(&k.as_str()) {
            continue;
        }
        if let Some(s) = doc.scalar_text(*v) {
            parts.push(format!("{k}: {s}"));
        }
    }
    parts.join("  ")
}

/// A number as the server wrote it.
fn whole_or_text(doc: &Doc, v: usize, n: f64) -> String {
    match doc.get(v) {
        Value::Number(t, _) => t.clone(),
        _ => whole(n),
    }
}

/// An object's members on one line: `Key: value  Key: value`; objects in it as deep as
/// [`FLAT_DEPTH`], `…` past that (a value can nest deeper than any stack).
fn flat(doc: &Doc, obj: usize) -> String {
    flat_at(doc, obj, 0)
}

const FLAT_DEPTH: usize = 32;

fn flat_at(doc: &Doc, obj: usize, depth: usize) -> String {
    if depth >= FLAT_DEPTH {
        return "…".into();
    }
    doc.members(obj)
        .iter()
        .filter_map(|(k, v)| match doc.get(*v) {
            Value::Object(_) => Some(format!("{k}: {}", flat_at(doc, *v, depth + 1))),
            _ => doc.scalar_text(*v).map(|s| format!("{k}: {s}")),
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// What the statement as a whole reported, after the tree.
fn read_statement(doc: &Doc, top: usize, plan: &mut Plan) {
    plan.planning_ms = num(doc, top, "Planning Time");
    plan.execution_ms = num(doc, top, "Execution Time");
    let mut footer: Vec<(String, Vec<String>)> = Vec::new();
    for (key, v) in doc.members(top) {
        match key.as_str() {
            "Plan" => {}
            "Settings" => {
                let s: Vec<String> = doc
                    .members(*v)
                    .iter()
                    .filter_map(|(k, v)| doc.scalar_text(*v).map(|s| format!("{k} = '{s}'")))
                    .collect();
                if !s.is_empty() {
                    footer.push((format!("Settings: {}", s.join(", ")), Vec::new()));
                }
            }
            "Planning" => {
                let b = read_buffers(doc, *v);
                plan.planning_buffers = b;
                let mut under = Vec::new();
                if let Some(s) = b.as_ref().and_then(buffers_text) {
                    under.push(format!("Buffers: {s}"));
                }
                if let (Some(used), Some(alloc)) = (num(doc, *v, "Memory Used"), num(doc, *v, "Memory Allocated")) {
                    under.push(format!("Memory: used={}kB  allocated={}kB", whole(used), whole(alloc)));
                }
                if !under.is_empty() {
                    footer.push(("Planning:".into(), under));
                }
            }
            "Planning Time" | "Execution Time" => {
                if let Some(t) = doc.scalar_text(*v) {
                    footer.push((format!("{key}: {t} ms"), Vec::new()));
                }
            }
            "Triggers" => {
                for &t in doc.items(*v) {
                    let run = TriggerRun {
                        name: text(doc, t, "Trigger Name").unwrap_or_default(),
                        relation: text(doc, t, "Relation"),
                        ms: num(doc, t, "Time").unwrap_or(0.0),
                        calls: num(doc, t, "Calls").unwrap_or(0.0),
                    };
                    let name = match text(doc, t, "Constraint Name") {
                        Some(c) => format!("Trigger {} for constraint {c}", run.name),
                        None => format!("Trigger {}", run.name),
                    };
                    footer.push((format!("{name}: time={:.3} calls={}", run.ms, whole(run.calls)), Vec::new()));
                    plan.triggers.push(run);
                }
            }
            "JIT" => {
                let mut under = Vec::new();
                for (k, j) in doc.members(*v) {
                    match doc.get(*j) {
                        Value::Object(_) if k == "Options" => {
                            let o: Vec<String> = doc
                                .members(*j)
                                .iter()
                                .filter_map(|(k, v)| doc.scalar_text(*v).map(|s| format!("{k} {s}")))
                                .collect();
                            under.push(format!("Options: {}", o.join(", ")));
                        }
                        Value::Object(_) => under.push(format!("{k}: {}", timings(doc, *j))),
                        _ => {
                            if let Some(s) = doc.scalar_text(*j) {
                                under.push(format!("{k}: {s}"));
                            }
                        }
                    }
                }
                footer.push(("JIT:".into(), under));
            }
            _ => match doc.get(*v) {
                Value::Object(_) => footer.push((format!("{key}: {}", flat(doc, *v)), Vec::new())),
                _ => {
                    if let Some(s) = doc.scalar_text(*v) {
                        footer.push((format!("{key}: {s}"), Vec::new()));
                    }
                }
            },
        }
    }
    plan.footer = footer;
}

/// JIT timings: `Generation 0.460 ms (Deform 0.173 ms), Inlining 151.862 ms, …`.
fn timings(doc: &Doc, obj: usize) -> String {
    doc.members(obj)
        .iter()
        .filter_map(|(k, v)| match doc.get(*v) {
            Value::Number(_, n) => Some(format!("{k} {n:.3} ms")),
            Value::Object(_) => {
                let total = num(doc, *v, "Total")?;
                let parts: Vec<String> = doc
                    .members(*v)
                    .iter()
                    .filter(|(k, _)| k != "Total")
                    .filter_map(|(k, v)| doc.num(*v).map(|n| format!("{k} {n:.3} ms")))
                    .collect();
                Some(format!("{k} {total:.3} ms ({})", parts.join(", ")))
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether node `i`'s parent may stop reading it before its end: under a `Limit` (within the
/// same subplan), or the inner side of a semi or anti join.
fn stops_early(plan: &Plan, i: usize) -> bool {
    let mut at = i;
    while let Some(p) = plan.nodes[at].parent {
        let (node, parent) = (&plan.nodes[at], &plan.nodes[p]);
        if parent.op.ends_with("Limit") {
            return true;
        }
        let semi = parent.op.contains("Semi Join") || parent.op.contains("Anti Join");
        if semi && node.relationship.as_deref() == Some("Inner") {
            return true;
        }
        // A subplan runs on its own: what is above it does not stop it.
        if matches!(node.relationship.as_deref(), Some("InitPlan" | "SubPlan")) {
            return false;
        }
        at = p;
    }
    false
}

/// Total and self times and self costs (see the module's comment).
fn derive(plan: &mut Plan) {
    let len = plan.nodes.len();
    for i in 0..len {
        plan.nodes[i].early_stop = stops_early(plan, i);
    }
    // Processes that ran each node at the same time: 1 outside a parallel part.
    let mut processes = vec![1.0_f64; len];
    for i in 0..len {
        let Some(p) = plan.nodes[i].parent else { continue };
        processes[i] = processes[p];
        let parent = &plan.nodes[p];
        if matches!(parent.op.as_str(), "Gather" | "Gather Merge") {
            let gather_loops = parent.actual.map_or(1.0, |a| a.loops).max(1.0);
            if let Some(a) = plan.nodes[i].actual.filter(|a| a.loops > 0.0) {
                processes[i] = (a.loops / gather_loops).round().max(1.0);
            }
        }
    }
    for (i, n) in plan.nodes.iter_mut().enumerate() {
        n.total_ms = n.actual.and_then(|a| a.total_ms.map(|t| t * a.loops / processes[i]));
    }
    let cte = |n: &PlanNode| {
        n.relationship.as_deref() == Some("InitPlan") && n.subplan.as_ref().is_some_and(|s| s.starts_with("CTE "))
    };
    for n in plan.nodes.iter_mut() {
        n.elsewhere = cte(n);
    }
    for i in 0..len {
        let n = &plan.nodes[i];
        let kids = n.children.clone();
        let self_ms = n.total_ms.map(|t| {
            let below: f64 =
                kids.iter().filter(|&&c| !cte(&plan.nodes[c])).filter_map(|&c| plan.nodes[c].total_ms).sum();
            (t - below).max(0.0)
        });
        let self_cost = n.cost.map(|(_, t)| {
            let below: f64 = kids.iter().filter_map(|&c| plan.nodes[c].cost.map(|c| c.1)).sum();
            (t - below).max(0.0)
        });
        plan.nodes[i].self_ms = self_ms;
        plan.nodes[i].self_cost = self_cost;
    }
    // A CTE's time is spent in the scans that read it: taken from them, in plan order.
    for c in 0..len {
        if !cte(&plan.nodes[c]) {
            continue;
        }
        let Some(name) = plan.nodes[c].subplan.as_ref().and_then(|s| s.strip_prefix("CTE ")).map(str::to_string) else {
            continue;
        };
        let mut left = plan.nodes[c].total_ms.unwrap_or(0.0);
        for s in 0..len {
            if left <= 0.0 {
                break;
            }
            let n = &mut plan.nodes[s];
            let on = format!("on {}", sql_ident(&name));
            let reads = n.op.ends_with("CTE Scan")
                && n.target.as_deref().is_some_and(|t| t == on || t.starts_with(&format!("{on} ")));
            if let (true, Some(own)) = (reads, n.self_ms) {
                let take = own.min(left);
                n.self_ms = Some(own - take);
                left -= take;
            }
        }
    }
}

#[cfg(test)]
mod tests;
