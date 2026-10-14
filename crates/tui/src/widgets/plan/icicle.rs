//! The icicle and the flame graph: one row per depth, each node a bar as wide as its share of
//! its parent's total time (by cost: its total cost), children side by side under (icicle) or
//! above (flame) their parent; what a parent did itself is the room its children leave.
//! Colored by the heat of the node's own share; the selected node is drawn the other way
//! round. A line names the selected node, which may be too narrow to name itself.
//!
//! The layout is one pass over the nodes. Depths that do not fit scroll so the selected node
//! stays on screen.

use super::{Look, fmt_share, heat, info_line, work};
use crate::app::plan::PlanTab;
use crate::text::width;
use crate::widgets::put;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// Each node's span on a row, in columns from the left of the view: `(start, end)` as
/// fractions, laid out once per frame.
pub(super) fn spans(p: &PlanTab, w: f64) -> Vec<(f64, f64)> {
    let plan = &p.plan;
    let mut span = vec![(0.0, 0.0); plan.nodes.len()];
    let roots: Vec<usize> = plan.roots().collect();
    let total: f64 = roots.iter().map(|&r| plan.inclusive(r)).sum::<f64>().max(f64::MIN_POSITIVE);
    let mut at = 0.0;
    for &r in &roots {
        let len = w * plan.inclusive(r) / total;
        span[r] = (at, at + len);
        at += len;
    }
    // Parents come before their children (depth first).
    for i in 0..plan.nodes.len() {
        work(1);
        let n = &plan.nodes[i];
        let (start, end) = span[i];
        // A child whose time is counted elsewhere (a CTE, inside the scans that read it) does
        // not shrink the others: they share the parent as if it were not there, and it gets
        // what room they leave, at the same scale.
        let counted = |c: &usize| !plan.nodes[*c].elsewhere;
        let kids: f64 = n.children.iter().filter(|c| counted(c)).map(|&c| plan.inclusive(c)).sum();
        // Children never take more than their parent (a measurement or a cost can say so).
        let whole = plan.inclusive(i).max(kids).max(f64::MIN_POSITIVE);
        let mut x = start;
        for &c in n.children.iter().filter(|c| counted(c)).chain(n.children.iter().filter(|c| !counted(c))) {
            let len = ((end - start) * plan.inclusive(c) / whole).min(end - x).max(0.0);
            span[c] = (x, x + len);
            x += len;
        }
    }
    span
}

/// `flame`: the root at the bottom, children above.
pub(super) fn draw(cx: &Look, p: &mut PlanTab, area: Rect, flame: bool, buf: &mut Buffer) {
    let th = cx.th;
    let plan = p.plan.clone();
    if area.height < 2 || area.width < 4 {
        return;
    }
    // The line about the selected node: below an icicle, above a flame graph.
    let (rows, info) = if flame {
        (Rect { y: area.y + 1, height: area.height - 1, ..area }, Rect { height: 1, ..area })
    } else {
        (Rect { height: area.height - 1, ..area }, Rect { y: area.y + area.height - 1, height: 1, ..area })
    };
    info_line(cx, &plan, p.selected, info, buf);
    let w = rows.width.saturating_sub(1) as f64;
    let span = spans(p, w);
    let depth = plan.nodes.iter().map(|n| n.depth).max().unwrap_or(0) + 1;
    let h = rows.height as usize;
    let at = plan.nodes.get(p.selected).map_or(0, |n| n.depth);
    let top = super::follow(p, at, depth, h);
    for (i, n) in plan.nodes.iter().enumerate() {
        work(1);
        if n.depth < top || n.depth >= top + h {
            continue;
        }
        let (start, end) = span[i];
        let (x0, x1) = (start.round() as u16, end.round() as u16);
        if x1 <= x0 {
            continue;
        }
        let row = (n.depth - top) as u16;
        let y = if flame { rows.y + rows.height - 1 - row } else { rows.y + row };
        let x = rows.x + 1 + x0;
        let len = (x1 - x0).min(rows.x + rows.width - x);
        let cell = Rect { x, y, width: len, height: 1 };
        p.hits.push((cell, i));
        let sel = i == p.selected;
        let share = plan.share(i);
        // A bar in the heat's color with the background's text; the selected one the other way.
        let color = heat(th, share).fg.unwrap_or(th.accent);
        let style = if sel {
            th.base().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::new().fg(color).bg(th.bg).add_modifier(Modifier::REVERSED)
        };
        // A column free at the right between neighbours, and thin lines at both edges, so the
        // bars stay apart (and show their width) without color too.
        let bar = Rect { width: if len >= 2 { len - 1 } else { len }, ..cell };
        buf.set_style(bar, style);
        buf.set_stringn(x, y, "▏", 1, style);
        if bar.width >= 3 {
            buf.set_stringn(x + bar.width - 1, y, "▕", 1, style);
        }
        let room = (bar.width as usize).saturating_sub(2);
        if room >= 2 {
            let label = plan.label(i);
            let text = if width(&label) + 6 <= room { format!("{label} {}", fmt_share(share)) } else { label };
            put(buf, x + 1, y, &text, room, style);
        }
    }
}
