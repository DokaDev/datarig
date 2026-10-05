//! The timeline (waterfall): one line per node, its measured time on a common axis — until its
//! first row (`░`), then until its last (`█`), per loop as the server measures them (a node
//! that ran in several loops says so). Without `ANALYZE` the bars are the planner's startup
//! and total cost on a cost axis, and the legend says so: never as time.

use super::{Look, fmt_ms, fmt_num, heat, indented_name, label_column, on_line, selected_style, work};
use crate::app::plan::PlanTab;
use crate::widgets::put;
use datarig_core::i18n::{Label, Msg};
use datarig_core::sql::plan::Measure;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

pub(super) fn draw(cx: &Look, p: &mut PlanTab, area: Rect, buf: &mut Buffer) {
    let th = cx.th;
    let plan = p.plan.clone();
    if area.height < 3 || area.width < 20 {
        return;
    }
    let time = plan.measure() == Measure::Time;
    // Each node's span on the axis: (start, end), and the axis' end.
    let span = |i: usize| -> Option<(f64, f64)> {
        let n = &plan.nodes[i];
        match time {
            true => n.actual.filter(|a| a.loops > 0.0).and_then(|a| a.startup_ms.zip(a.total_ms)),
            false => n.cost,
        }
    };
    let max = (0..plan.nodes.len()).filter_map(span).map(|s| s.1).fold(0.0_f64, f64::max).max(f64::MIN_POSITIVE);
    work(plan.nodes.len() as u64);
    let name_w = label_column(area.width);
    let axis_x = area.x + 1 + name_w as u16 + 1;
    let axis_w = (area.x + area.width).saturating_sub(axis_x + 1) as usize;
    // The axis: 0, half way and the end, and the legend.
    let muted = Style::new().fg(th.fg_dim).bg(th.bg);
    let at = |v: f64| if time { fmt_ms(v) } else { fmt_num(v) };
    let legend = cx.i18n.label(if time { Label::PlanTimelineLegend } else { Label::PlanTimelineLegendCost });
    let legend_style = if time { muted } else { Style::new().fg(th.warning).bg(th.bg) };
    put(buf, area.x + 1, area.y, &legend, name_w, legend_style);
    if axis_w >= 12 {
        put(buf, axis_x, area.y, "0", 1, muted);
        let mid = at(max / 2.0);
        put(buf, axis_x + (axis_w / 2) as u16 - (mid.len() / 2) as u16, area.y, &mid, axis_w / 2, muted);
        let end = at(max);
        put(buf, axis_x + axis_w as u16 - end.len() as u16, area.y, &end, end.len(), muted);
    }
    let rows = Rect { y: area.y + 1, height: area.height - 1, ..area };
    let order = p.visible();
    let at_row = order.iter().position(|i| *i == p.selected).unwrap_or(0);
    let top = super::follow(p, at_row, order.len(), rows.height as usize);
    for (row, &i) in order.iter().skip(top).take(rows.height as usize).enumerate() {
        work(1);
        let y = rows.y + row as u16;
        let line = Rect { y, height: 1, ..rows };
        let sel = i == p.selected;
        let base = if sel { th.base().patch(selected_style(cx)) } else { th.base() };
        buf.set_style(line, base);
        p.hits.push((line, i));
        let n = &plan.nodes[i];
        let name_style = if plan.is_hot(i) { base.patch(on_line(cx, th.plan_hot, sel)) } else { base };
        put(buf, area.x + 1, y, &indented_name(&plan, i, name_w), name_w, name_style);
        let Some((start, end)) = span(i) else {
            let never = n.actual.is_some_and(|a| a.loops <= 0.0);
            if never {
                put(buf, axis_x, y, &cx.i18n.label(Label::PlanNeverRan), axis_w, base.fg(th.fg_dim));
            }
            continue;
        };
        let scale = axis_w as f64 / max;
        let (a, b) = ((start * scale).round() as usize, ((end * scale).round() as usize).max(1));
        let (a, b) = (a.min(axis_w), b.min(axis_w));
        let color = on_line(cx, heat(th, plan.share(i)), sel);
        if a > 0 {
            put(buf, axis_x, y, &"░".repeat(a), a, base.patch(color));
        }
        if b > a {
            put(buf, axis_x + a as u16, y, &"█".repeat(b - a), b - a, base.patch(color));
        }
        // Several loops: the bar is one loop's, said after it.
        if let Some(loops) = n.actual.map(|a| a.loops).filter(|l| *l > 1.0) {
            let note = cx.i18n.msg(&Msg::PlanTimelineLoops { loops: fmt_num(loops) }).to_string();
            let x = axis_x + b as u16 + 1;
            if (x as usize) < (axis_x as usize + axis_w) {
                put(buf, x, y, &note, axis_x as usize + axis_w - x as usize, base.fg(th.fg_muted));
            }
        }
    }
}
