//! The detail of the selected node: its name, its time (self, total, per loop) or cost, its rows
//! (estimated, actual, removed by filters, how far off), its buffers and workers, then every
//! property it has as its text says it (conditions, filters, output, sort and hash details).

use super::{Look, fmt_ms, fmt_num, fmt_share, work};
use crate::app::plan::PlanTab;
use crate::text::{width, wrap_words};
use crate::widgets::put;
use datarig_core::i18n::{Label, Msg};
use datarig_core::sql::plan::Measure;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// Properties the lines above them already say.
const SAID: &[&str] = &["Workers Planned", "Workers Launched"];

/// Draw it in `area`: `beside` the view (a line on its left), else below it (a line above).
pub(super) fn draw(cx: &Look, p: &PlanTab, area: Rect, beside: bool, buf: &mut Buffer) {
    let th = cx.th;
    let plan = &p.plan;
    let i = p.selected.min(plan.nodes.len().saturating_sub(1));
    let Some(n) = plan.nodes.get(i) else { return };
    work(1);
    let line = Style::new().fg(th.border).bg(th.bg);
    let content = if beside {
        for y in area.y..area.y + area.height {
            buf.set_stringn(area.x, y, "│", 1, line);
        }
        Rect { x: area.x + 2, width: area.width.saturating_sub(3), ..area }
    } else {
        buf.set_stringn(area.x, area.y, "─".repeat(area.width as usize), area.width as usize, line);
        Rect {
            y: area.y + 1,
            height: area.height.saturating_sub(1),
            x: area.x + 1,
            width: area.width.saturating_sub(2),
        }
    };
    let w = content.width as usize;
    if w < 4 {
        return;
    }
    let base = th.base();
    let muted = Style::new().fg(th.fg_muted).bg(th.bg);
    let mut lines: Vec<Vec<(String, Style)>> = Vec::new();
    let text = |lines: &mut Vec<Vec<(String, Style)>>, s: String, style: Style| {
        for l in wrap_words(&s, w) {
            lines.push(vec![(l, style)]);
        }
    };
    text(&mut lines, plan.label(i), base.add_modifier(Modifier::BOLD));
    if let Some(sub) = &n.subplan {
        text(&mut lines, sub.clone(), muted);
    }
    // Labelled lines: the label in its own column.
    let rows: Vec<(Label, String, Style)> = {
        let mut v = Vec::new();
        let share = fmt_share(plan.share(i));
        if plan.measure() == Measure::Time {
            let own = n.self_ms.map_or_else(String::new, fmt_ms);
            let total = n.total_ms.map_or_else(String::new, fmt_ms);
            v.push((
                Label::PlanDetailTime,
                cx.i18n.msg(&Msg::PlanDetailTimeLine { own, share: share.clone(), total }).to_string(),
                base,
            ));
            if let Some(a) = n.actual.filter(|a| a.loops > 0.0)
                && let (Some(first), Some(last)) = (a.startup_ms, a.total_ms)
            {
                let (first, last, loops) = (fmt_ms(first), fmt_ms(last), fmt_num(a.loops));
                v.push((
                    Label::PlanDetailPerLoop,
                    cx.i18n.msg(&Msg::PlanDetailPerLoopLine { first, last, loops }).to_string(),
                    muted,
                ));
            }
        }
        if let Some((startup, total)) = n.cost {
            let own = n.self_cost.map_or_else(String::new, fmt_num);
            let (startup, total) = (format!("{startup:.2}"), format!("{total:.2}"));
            let msg = if plan.measure() == Measure::Cost {
                Msg::PlanDetailCostShare { startup, total, own, share }
            } else {
                Msg::PlanDetailCostLine { startup, total, own }
            };
            v.push((Label::PlanDetailCostLabel, cx.i18n.msg(&msg).to_string(), muted));
        }
        let est = n.plan_rows.map_or_else(String::new, fmt_num);
        match n.actual {
            Some(a) if a.loops <= 0.0 => {
                v.push((Label::PlanDetailRows, cx.i18n.label(Label::PlanDetailNeverExecuted).to_string(), muted))
            }
            Some(a) => {
                let (actual, loops, all) = (fmt_num(a.rows), fmt_num(a.loops), fmt_num(a.rows * a.loops));
                let msg = Msg::PlanDetailRowsMeasured { est, actual, loops, all };
                v.push((Label::PlanDetailRows, cx.i18n.msg(&msg).to_string(), base));
            }
            None => {
                v.push((Label::PlanDetailRows, cx.i18n.msg(&Msg::PlanDetailRowsEstimated { est }).to_string(), base))
            }
        }
        if let Some(off) = plan.misestimate(i) {
            let ratio = fmt_num(off.ratio);
            let msg = if off.under { Msg::PlanDetailUnder { ratio } } else { Msg::PlanDetailOver { ratio } };
            v.push((Label::PlanDetailOff, cx.i18n.msg(&msg).to_string(), base.patch(th.plan_misestimate)));
        }
        if let Some(r) = n.removed.filter(|r| *r > 0.0) {
            let rows = fmt_num(r);
            v.push((Label::PlanDetailRemoved, cx.i18n.msg(&Msg::PlanDetailRemovedRows { rows }).to_string(), base));
        }
        if let Some(b) = n.buffers.filter(|b| !b.is_empty()) {
            let (hit, read) = (fmt_num(b.hits() as f64), fmt_num(b.reads() as f64));
            let ratio = b.hit_ratio().map_or_else(|| "-".into(), fmt_share);
            v.push((
                Label::PlanDetailBuffers,
                cx.i18n.msg(&Msg::PlanDetailBuffersLine { hit, read, ratio }).to_string(),
                base,
            ));
        }
        if let Some((planned, launched)) = n.workers {
            let planned = planned.to_string();
            let launched = launched.map_or_else(|| "-".into(), |l| l.to_string());
            v.push((
                Label::PlanDetailWorkers,
                cx.i18n.msg(&Msg::PlanDetailWorkersLine { planned, launched }).to_string(),
                base,
            ));
        }
        v
    };
    let label_w = rows.iter().map(|(l, ..)| width(&cx.i18n.label(*l))).max().unwrap_or(0) + 2;
    for (label, value, style) in rows {
        let label = cx.i18n.label(label).to_string();
        let vw = w.saturating_sub(label_w).max(8);
        for (k, l) in wrap_words(&value, vw).into_iter().enumerate() {
            let head = if k == 0 { format!("{label:<label_w$}", label_w = label_w) } else { " ".repeat(label_w) };
            lines.push(vec![(head, muted), (l, style)]);
        }
    }
    // Everything the node says, as its text says it.
    let props: Vec<&(String, String)> = n.properties.iter().filter(|(k, _)| !SAID.contains(&k.as_str())).collect();
    if !props.is_empty() {
        lines.push(vec![(String::new(), base)]);
    }
    for (k, v) in props {
        let first = format!("{k}: ");
        let kw = width(&first);
        for (j, l) in wrap_words(v, w.saturating_sub(kw).max(8)).into_iter().enumerate() {
            let head = if j == 0 { first.clone() } else { " ".repeat(kw) };
            lines.push(vec![(head, muted), (l, base)]);
        }
    }
    for (row, parts) in lines.iter().take(content.height as usize).enumerate() {
        let y = content.y + row as u16;
        let mut x = content.x;
        let end = content.x + content.width;
        for (t, style) in parts {
            if x >= end {
                break;
            }
            x += put(buf, x, y, t, (end - x) as usize, *style);
        }
    }
}
