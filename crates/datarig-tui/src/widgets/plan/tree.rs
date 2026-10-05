//! The tree view: one line per node under guides (`├─`, `└─`), closed nodes (`▸`) hiding their
//! children, and per node its estimated and actual rows, its self and total time (or cost),
//! a bar of its share of the whole colored by heat, and its buffers.
//!
//! Narrow panes drop columns from the least needed (buffers, total, estimate, actual rows); a
//! deep node's guides are cut at the left (`…`) so its name keeps room.

use super::{Look, bar, fmt_ms, fmt_num, fmt_share, fmt_weight, heat, markers, on_line, selected_style, work};
use crate::app::plan::PlanTab;
use crate::text::{Align, fit, width};
use crate::widgets::put;
use datarig_core::i18n::Label;
use datarig_core::sql::plan::{Measure, Plan};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Col {
    Est,
    Actual,
    SelfW,
    Total,
    Share,
    Buffers,
}

impl Col {
    fn width(self) -> usize {
        match self {
            Col::Est => 8,
            Col::Actual => 12,
            Col::SelfW => 10,
            Col::Total => 10,
            Col::Share => 14,
            Col::Buffers => 12,
        }
    }
}

/// The columns that fit `w` next to a name of at least 20 columns, in their order.
fn columns(plan: &Plan, w: usize) -> Vec<Col> {
    let analyzed = plan.analyzed;
    let buffers = plan.nodes.iter().any(|n| n.buffers.is_some());
    // Most needed first; a column is shown when it and every one before it fit.
    let mut wanted = vec![Col::SelfW, Col::Share];
    if analyzed {
        wanted.push(Col::Actual);
    }
    wanted.push(Col::Est);
    wanted.push(Col::Total);
    if buffers {
        wanted.push(Col::Buffers);
    }
    let mut shown = Vec::new();
    let mut used = 20;
    for c in wanted {
        if used + c.width() + 1 > w {
            break;
        }
        used += c.width() + 1;
        shown.push(c);
    }
    let order = [Col::Est, Col::Actual, Col::SelfW, Col::Total, Col::Share, Col::Buffers];
    order.into_iter().filter(|c| shown.contains(c)).collect()
}

pub(super) fn draw(cx: &Look, p: &mut PlanTab, area: Rect, buf: &mut Buffer) {
    let th = cx.th;
    let plan = p.plan.clone();
    let cols = columns(&plan, area.width as usize);
    let cols_w: usize = cols.iter().map(|c| c.width() + 1).sum();
    let name_w = (area.width as usize).saturating_sub(cols_w + 1);
    // The header.
    let muted = Style::new().fg(th.fg_dim).bg(th.bg);
    let cost = plan.measure() == Measure::Cost;
    put(buf, area.x + 1, area.y, &cx.i18n.label(Label::PlanColNode), name_w, muted);
    let mut x = area.x + 1 + name_w as u16;
    for c in &cols {
        let label = match c {
            Col::Est => Label::PlanColEst,
            Col::Actual => Label::PlanColActual,
            Col::SelfW if cost => Label::PlanColSelfCost,
            Col::SelfW => Label::PlanColSelf,
            Col::Total if cost => Label::PlanColTotalCost,
            Col::Total => Label::PlanColTotal,
            Col::Share => Label::PlanColShare,
            Col::Buffers => Label::PlanColBuffers,
        };
        let align = if *c == Col::Share { Align::Left } else { Align::Right };
        put(buf, x + 1, area.y, &fit(&cx.i18n.label(label), c.width(), align), c.width(), muted);
        x += c.width() as u16 + 1;
    }
    let rows = Rect { y: area.y + 1, height: area.height.saturating_sub(1), ..area };
    if rows.height == 0 {
        return;
    }
    let order = p.visible();
    let at = order.iter().position(|i| *i == p.selected).unwrap_or(0);
    let top = super::follow(p, at, order.len(), rows.height as usize);
    // Which nodes are the last of their parent's children (their guides end there).
    let last = last_children(&plan);
    for (row, &i) in order.iter().skip(top).take(rows.height as usize).enumerate() {
        work(1);
        let y = rows.y + row as u16;
        let line = Rect { y, height: 1, ..rows };
        let sel = i == p.selected;
        let base = if sel { th.base().patch(selected_style(cx)) } else { th.base() };
        buf.set_style(line, base);
        p.hits.push((line, i));
        let n = &plan.nodes[i];
        // Guides, then the fold mark.
        let guides = guides(&plan, &last, i, name_w / 4);
        let fold = match (n.children.is_empty(), p.collapsed[i]) {
            (true, _) => "─ ",
            (false, true) => "▸ ",
            (false, false) => "▾ ",
        };
        let fold = if n.parent.is_none() && n.children.is_empty() { "  " } else { fold };
        let lead = cut_left(&format!("{guides}{fold}"), name_w / 2);
        let mut x = area.x + 1;
        let end = area.x + 1 + name_w as u16;
        x += put(buf, x, y, &lead, name_w, base.fg(th.fg_dim));
        // The subplan's name, the operation and its target; markers after it, kept whole.
        let marks = markers(cx, &plan, i);
        let marks_w: usize = marks.iter().map(|(t, _)| width(t) + 1).sum();
        let room = (end.saturating_sub(x) as usize).saturating_sub(marks_w);
        if let Some(sub) = &n.subplan {
            x += put(buf, x, y, &format!("{sub}: "), room, base.fg(th.fg_muted));
        }
        let room = (end.saturating_sub(x) as usize).saturating_sub(marks_w);
        let name_style =
            if plan.is_hot(i) { base.patch(on_line(cx, th.plan_hot, sel)) } else { base.add_modifier(Modifier::BOLD) };
        x += put(buf, x, y, &n.op, room, name_style);
        if let Some(t) = &n.target {
            let room = (end.saturating_sub(x) as usize).saturating_sub(marks_w);
            if room > 2 {
                x += put(buf, x, y, &format!(" {t}"), room, base.fg(th.fg_muted));
            }
        }
        for (t, style) in marks {
            if x + 1 < end {
                x += 1 + put(buf, x + 1, y, &t, (end - x - 1) as usize, base.patch(on_line(cx, style, sel)));
            }
        }
        // The numbers.
        let mut cx_x = end;
        for c in &cols {
            let w = c.width();
            let (text, style) = cell(cx, &plan, i, *c, base);
            let style = if sel { base.patch(on_line(cx, style, true)) } else { style };
            put(buf, cx_x + 1, y, &fit(&text, w, if *c == Col::Share { Align::Left } else { Align::Right }), w, style);
            cx_x += w as u16 + 1;
        }
    }
}

/// A node's text and style in column `c`.
fn cell(cx: &Look, plan: &Plan, i: usize, c: Col, base: Style) -> (String, Style) {
    let th = cx.th;
    let n = &plan.nodes[i];
    let dim = base.fg(th.fg_dim);
    match c {
        Col::Est => (n.plan_rows.map(fmt_num).unwrap_or_default(), base.fg(th.fg_muted)),
        Col::Actual => match n.actual {
            Some(a) if a.loops <= 0.0 => (cx.i18n.label(Label::PlanNeverRan).to_string(), dim),
            Some(a) if a.loops > 1.0 => (format!("{} ×{}", fmt_num(a.rows), fmt_num(a.loops)), base),
            Some(a) => (fmt_num(a.rows), base),
            None => (String::new(), base),
        },
        Col::SelfW => {
            let style = if plan.is_hot(i) { base.patch(th.plan_hot) } else { base };
            (fmt_weight(plan, plan.weight(i)), style)
        }
        Col::Total => {
            let total = match plan.measure() {
                Measure::Time => n.total_ms.map(fmt_ms),
                Measure::Cost => n.cost.map(|c| fmt_num(c.1)),
            };
            (total.unwrap_or_default(), base.fg(th.fg_muted))
        }
        Col::Share => {
            let share = plan.share(i);
            (format!("{} {:>4}", bar(share, 9), fmt_share(share)), base.patch(heat(th, share)))
        }
        Col::Buffers => match n.buffers {
            Some(b) if !b.is_empty() => (format!("{}/{}", fmt_num(b.hits() as f64), fmt_num(b.reads() as f64)), dim),
            _ => (String::new(), dim),
        },
    }
}

/// For each node, whether it is the last child of its parent.
fn last_children(plan: &Plan) -> Vec<bool> {
    let mut last = vec![true; plan.nodes.len()];
    for n in &plan.nodes {
        work(1);
        for &c in n.children.iter().rev().skip(1) {
            last[c] = false;
        }
    }
    last
}

/// The guides in front of node `i`: for each ancestor below the root, `│ ` while it has a next
/// sibling, then `├─` or `└─` for the node itself; at most `levels` of them (`…` first when
/// there are more).
fn guides(plan: &Plan, last: &[bool], i: usize, levels: usize) -> String {
    let n = &plan.nodes[i];
    let Some(_) = n.parent else { return String::new() };
    let mut parts: Vec<&str> = vec![if last[i] { "└─" } else { "├─" }];
    let mut a = n.parent;
    while let Some(at) = a {
        if plan.nodes[at].parent.is_none() {
            break;
        }
        if parts.len() >= levels.max(1) {
            parts.push("…");
            break;
        }
        work(1);
        parts.push(if last[at] { "  " } else { "│ " });
        a = plan.nodes[at].parent;
    }
    parts.reverse();
    parts.concat()
}

/// `s` in at most `w` columns, cut at its left (`…` first) when longer.
pub(super) fn cut_left(s: &str, w: usize) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out: Vec<char> = Vec::new();
    let mut used = 1;
    for &c in chars.iter().rev() {
        let cw = width(&c.to_string());
        if used + cw > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.reverse();
    format!("…{}", out.into_iter().collect::<String>())
}
