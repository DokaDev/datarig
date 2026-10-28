//! MySQL plans: the tree text of `EXPLAIN FORMAT=TREE` (estimates) and `EXPLAIN ANALYZE`
//! (estimates and what was measured), as MySQL 8.0, 8.4 and 9.x write it, read into [`Plan`].
//!
//! One node per line: `-> ` after four spaces per level, the operation, then the groups the
//! server adds at the end of the line: `(cost=T rows=E)` or `(cost=S..T rows=E)`, then
//! `(actual time=F..L rows=R loops=N)` or `(never executed)`. The groups are read from the end of
//! the line only, and only in that exact shape: a string literal in a condition is written on
//! the same line (a newline in it as `\n`), so whatever it holds stays part of the operation.
//! A name the server writes unquoted (8.0 writes sort keys so) could still end a line that has
//! no group of its own like one: a measure is believed only when every node with an estimate
//! has one too, as in a real `EXPLAIN ANALYZE`.
//! Lines the server adds without `-> ` (none are known) are kept under the node above them.
//!
//! Times are per loop, like PostgreSQL's: a node's total time is its last-row time times its
//! loops, its self time the total less its children's. A node the server writes no measure for
//! (`Hash`, `Select #2 (subquery in condition; run only once)`, a sort's line in some versions)
//! takes its children's together as its own and adds no time or cost of its own; such a cost is
//! marked [`PlanNode::cost_derived`], never shown as the server's.

use super::{Actual, Plan, PlanError, PlanNode};

/// Read the tree text of a MySQL `EXPLAIN FORMAT=TREE` or `EXPLAIN ANALYZE`.
pub fn parse(text: &str) -> Result<Plan, PlanError> {
    let mut plan = Plan::default();
    // The node open at each depth (its index), for the parents of the lines that follow.
    let mut open: Vec<usize> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        let Some(rest) = line[indent..].strip_prefix("-> ") else {
            // Not a node: kept with the node above it, or the text is no plan at all.
            let Some(last) = plan.nodes.last_mut() else { return Err(PlanError::NotAPlan) };
            last.properties.push((String::new(), line.trim().to_string()));
            continue;
        };
        if indent % 4 != 0 {
            return Err(PlanError::NotAPlan);
        }
        let depth = indent / 4;
        // A node is at most one level under the one before it (the first at the top).
        if depth > open.len() {
            return Err(PlanError::NotAPlan);
        }
        if plan.nodes.len() >= super::MAX_NODES {
            return Err(PlanError::TooBig);
        }
        open.truncate(depth);
        let i = plan.nodes.len();
        let parent = open.last().copied();
        let mut n = node(rest);
        n.depth = depth;
        n.parent = parent;
        if let Some(p) = parent {
            plan.nodes[p].children.push(i);
        }
        plan.nodes.push(n);
        open.push(i);
    }
    // `-> <not executable by iterator executor>`: the server has no tree for the statement.
    if plan.nodes.is_empty() || plan.nodes.iter().any(|n| n.op.starts_with("<not executable")) {
        return Err(PlanError::NotAPlan);
    }
    // Measured when every node with an estimate is (a real `EXPLAIN ANALYZE`); otherwise what
    // looked like a measure is text of a node's own.
    let measured = plan.nodes.iter().any(|n| n.actual.is_some())
        && plan.nodes.iter().all(|n| n.cost.is_none() || n.actual.is_some());
    if !measured {
        for n in &mut plan.nodes {
            n.actual = None;
        }
    }
    plan.analyzed = measured;
    plan.timed = plan.analyzed;
    derive(&mut plan);
    plan.finish();
    Ok(plan)
}

/// `text` (a result's one cell) is MySQL tree text: its first line is a node.
pub fn is_tree(text: &str) -> bool {
    text.lines().find(|l| !l.trim().is_empty()).is_some_and(|l| l.starts_with("-> "))
}

/// One node from its line (after `-> `).
fn node(line: &str) -> PlanNode {
    let mut n = PlanNode::default();
    let mut rest = line.trim_end();
    if let Some(r) = rest.strip_suffix(" (never executed)") {
        n.actual = Some(Actual { startup_ms: Some(0.0), total_ms: Some(0.0), ..Actual::default() });
        rest = r;
    } else if let Some((r, a)) = group(rest, "(actual time=").and_then(|(r, g)| Some((r, actual(g)?))) {
        n.actual = Some(a);
        rest = r;
    }
    if let Some((r, (cost, rows))) = group(rest, "(cost=").and_then(|(r, g)| Some((r, cost_rows(g)?))) {
        n.cost = Some(cost);
        n.plan_rows = Some(rows);
        rest = r;
    }
    let rest = rest.trim_end();
    // `Select #2 (subquery in condition; run only once)`: a subquery and how it runs.
    if let Some(sel) = rest.strip_prefix("Select #") {
        let (num, how) = sel.split_once(' ').unwrap_or((sel, ""));
        if !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()) {
            n.subplan = Some(format!("Select #{num}"));
            let how = how.trim().strip_prefix('(').and_then(|h| h.strip_suffix(')'));
            n.relationship = how.map(|h| h.split(';').next().unwrap_or(h).trim().to_string());
        }
    }
    // `Filter: (cond)`, `Sort: x DESC`, `Aggregate: count(0)`: the operation and what it is
    // about; `Table scan on c`, `Index lookup on o using c_id (…)`: the operation and its target.
    match rest.split_once(": ") {
        Some((op, about)) if !op.contains(['(', '`', '\'']) => {
            n.op = op.to_string();
            n.properties.insert(0, (op.to_string(), about.to_string()));
        }
        _ => match rest.find(" on ") {
            Some(at) if !rest[..at].contains(['(', '`', '\'']) => {
                n.op = rest[..at].to_string();
                n.target = Some(rest[at + 1..].to_string());
            }
            _ => n.op = rest.to_string(),
        },
    }
    n
}

/// The group that starts with `open` and ends the line, exactly: the text before it (its
/// separating blanks dropped) and the group's inside.
fn group<'a>(line: &'a str, open: &str) -> Option<(&'a str, &'a str)> {
    let inner = line.strip_suffix(')')?;
    let at = inner.rfind(open)?;
    let before = &inner[..at];
    // The server puts at least one blank before a group.
    if !before.ends_with(' ') {
        return None;
    }
    Some((before.trim_end(), &inner[at + 1..]))
}

/// `actual time=F..L rows=R loops=N`.
fn actual(g: &str) -> Option<Actual> {
    let rest = g.strip_prefix("actual time=")?;
    let (time, rest) = rest.split_once(' ')?;
    let (first, last) = time.split_once("..")?;
    let (rows, loops) = rest.strip_prefix("rows=")?.split_once(" loops=")?;
    Some(Actual {
        startup_ms: Some(number(first)?),
        total_ms: Some(number(last)?),
        rows: number(rows)?,
        rows_decimals: number(rows)?.fract() != 0.0,
        loops: number(loops)?,
    })
}

/// `cost=T rows=E` or `cost=S..T rows=E`: startup and total cost (a single cost is both), rows.
fn cost_rows(g: &str) -> Option<((f64, f64), f64)> {
    let (cost, rows) = g.strip_prefix("cost=")?.split_once(" rows=")?;
    let cost = match cost.split_once("..") {
        Some((s, t)) => (number(s)?, number(t)?),
        None => (number(cost)?, number(cost)?),
    };
    Some((cost, number(rows)?))
}

/// A number as the server writes it: `12`, `2.52`, `0.116`, `82e-6`, `1.2e+06`. Nothing else.
fn number(s: &str) -> Option<f64> {
    let ok = !s.is_empty()
        && s.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
        && s.as_bytes()[0].is_ascii_digit();
    ok.then(|| s.parse::<f64>().ok()).flatten().filter(|v| v.is_finite() && *v >= 0.0)
}

/// Operations that read their input to its end before they give a row: what is under one is
/// read in full, whatever stops reading it. `Hash` is the build side of a hash join (not a
/// `Hash semijoin`, which is a join).
const READS_ALL: &[&str] = &["Sort", "Materialize", "Aggregate", "Temporary table"];

/// Whether node `i`'s parent may stop reading it before its end, given whether its parent may
/// itself be stopped (`parent_stops`): under a limit (within its own subquery, with nothing
/// between that reads its input in full), or the inner side of a nested loop semi or anti join.
/// Worked out from the root down, one step per node.
fn stops_early(plan: &Plan, i: usize, parent_stops: bool) -> bool {
    let node = &plan.nodes[i];
    let Some(p) = node.parent else { return false };
    let parent = &plan.nodes[p];
    // The operation without what it is about (`Inner hash join (t.a = 'semijoin')`).
    let kind = parent.op.split('(').next().unwrap_or_default().trim_end();
    if kind.starts_with("Limit") {
        return true;
    }
    if kind == "Hash" || READS_ALL.iter().any(|r| kind.starts_with(r)) {
        return false;
    }
    let semi = kind.starts_with("Nested loop") && (kind.contains("semijoin") || kind.contains("antijoin"));
    if semi && parent.children.last() == Some(&i) {
        return true;
    }
    // A subquery runs on its own: what is above it does not stop it.
    node.subplan.is_none() && parent_stops
}

/// Total and self times and costs (see the module's comment), children before parents.
fn derive(plan: &mut Plan) {
    let len = plan.nodes.len();
    // Parents come before their children.
    for i in 0..len {
        let parent_stops = plan.nodes[i].parent.is_some_and(|p| plan.nodes[p].early_stop);
        plan.nodes[i].early_stop = stops_early(plan, i, parent_stops);
    }
    for i in (0..len).rev() {
        let kids = plan.nodes[i].children.clone();
        let below_ms: f64 = kids.iter().filter_map(|&c| plan.nodes[c].total_ms).sum();
        let below_cost: f64 = kids.iter().filter_map(|&c| plan.nodes[c].cost.map(|c| c.1)).sum();
        let n = &mut plan.nodes[i];
        n.total_ms = match n.actual {
            Some(a) => a.total_ms.map(|t| t * a.loops),
            // Not measured, but what is under it was: it holds that.
            None if plan.analyzed => Some(below_ms),
            None => None,
        };
        n.self_ms = n.total_ms.map(|t| (t - below_ms).max(0.0));
        if n.cost.is_none() && !kids.is_empty() {
            n.cost = Some((0.0, below_cost));
            n.cost_derived = true;
            n.self_cost = Some(0.0);
        } else {
            n.self_cost = n.cost.map(|(_, t)| (t - below_cost).max(0.0));
        }
    }
}

#[cfg(test)]
mod tests;
