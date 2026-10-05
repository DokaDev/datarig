//! A plan as `psql` shows a text `EXPLAIN`, written from the model: the node lines with their
//! costs and measurements, each node's properties under it, children after `->`, a subplan
//! under its name, then what the statement as a whole reported. Nothing is asked of the server.

use super::Plan;

/// At most this many spaces in front of a line: a plan deeper than about a hundred levels keeps
/// its deeper lines at this indent instead of growing without end.
const MAX_INDENT: usize = 600;

fn indent(n: usize) -> String {
    " ".repeat(n.min(MAX_INDENT))
}

/// The text's lines, each with the node it belongs to (`None`: the statement's lines after the
/// tree).
pub fn lines(plan: &Plan) -> Vec<(String, Option<usize>)> {
    let mut out: Vec<(String, Option<usize>)> = Vec::new();
    // (node, indent of its line in spaces): depth first, without recursion.
    let mut stack: Vec<(usize, Option<usize>)> = plan.roots().map(|r| (r, None)).collect();
    stack.reverse();
    while let Some((i, at)) = stack.pop() {
        let n = &plan.nodes[i];
        // `at`: where its `->` goes (the root has none); its properties go 6 further.
        let props = match at {
            None => 2,
            Some(at) => {
                let mut arrow = at;
                if let Some(name) = &n.subplan {
                    out.push((format!("{}{name}", indent(at)), Some(i)));
                    arrow += 2;
                }
                out.push((format!("{}->  {}", indent(arrow), node_line(plan, i)), Some(i)));
                arrow + 6
            }
        };
        if at.is_none() {
            out.push((node_line(plan, i), Some(i)));
        }
        for (k, v) in &n.properties {
            out.push((format!("{}{k}: {v}", indent(props)), Some(i)));
        }
        for &c in n.children.iter().rev() {
            stack.push((c, Some(props)));
        }
    }
    for (line, under) in &plan.footer {
        out.push((line.clone(), None));
        for u in under {
            out.push((format!("  {u}"), None));
        }
    }
    out
}

/// The whole text, one line each.
pub fn text(plan: &Plan) -> String {
    lines(plan).into_iter().map(|(l, _)| l).collect::<Vec<_>>().join("\n")
}

/// `Hash Join  (cost=1.09..2.20 rows=4 width=8) (actual time=0.030..0.035 rows=4 loops=1)`.
pub fn node_line(plan: &Plan, i: usize) -> String {
    let n = &plan.nodes[i];
    let mut s = plan.label(i);
    if let (Some((a, b)), Some(rows)) = (n.cost, n.plan_rows) {
        let width = n.plan_width.map_or(0.0, |w| w);
        s.push_str(&format!("  (cost={a:.2}..{b:.2} rows={rows:.0} width={width:.0})"));
    }
    match n.actual {
        Some(a) if a.loops <= 0.0 => s.push_str(" (never executed)"),
        Some(a) => {
            let rows = if a.rows_decimals { format!("{:.2}", a.rows) } else { format!("{:.0}", a.rows) };
            match (a.startup_ms, a.total_ms) {
                (Some(st), Some(t)) => {
                    s.push_str(&format!(" (actual time={st:.3}..{t:.3} rows={rows} loops={:.0})", a.loops))
                }
                _ => s.push_str(&format!(" (actual rows={rows} loops={:.0})", a.loops)),
            }
        }
        None => {}
    }
    s
}
