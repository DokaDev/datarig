//! The row flow (a funnel): one line per node, the rows it passed up drawn as a band as thick
//! as their number on a log scale (every loop together), centered, so rows narrowing through
//! filters and joins show as a funnel; the rows its filters removed, and a misestimate with how
//! far off it was. Without `ANALYZE` the bands are the planner's estimated rows per loop, and
//! the legend says so.

use super::{Look, fmt_num, indented_name, label_column, markers, on_line, selected_style, work};
use crate::app::plan::PlanTab;
use crate::text::{Align, fit, width};
use crate::widgets::put;
use datarig_core::i18n::{Label, Msg};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

/// The band's share of its room for `rows` when the most is `max` (log scale; a row or more
/// shows).
pub(super) fn thickness(rows: f64, max: f64) -> f64 {
    if rows <= 0.0 || max <= 0.0 {
        return 0.0;
    }
    ((rows + 1.0).log10() / (max + 1.0).log10()).clamp(0.0, 1.0)
}

pub(super) fn draw(cx: &Look, p: &mut PlanTab, area: Rect, buf: &mut Buffer) {
    let th = cx.th;
    let plan = p.plan.clone();
    if area.height < 3 || area.width < 24 {
        return;
    }
    let rows_of = |i: usize| plan.rows_out(i).unwrap_or(0.0);
    let max = (0..plan.nodes.len()).map(rows_of).fold(0.0_f64, f64::max);
    work(plan.nodes.len() as u64);
    let name_w = label_column(area.width);
    let band_x = area.x + 1 + name_w as u16 + 1;
    // The band, then the numbers (rows, removed, marks) on the right.
    let nums_w = 28.min((area.width as usize).saturating_sub(name_w + 12));
    let band_w = (area.x + area.width).saturating_sub(band_x + 1 + nums_w as u16 + 1) as usize;
    let legend = cx.i18n.label(if plan.analyzed { Label::PlanFunnelLegend } else { Label::PlanFunnelLegendEstimated });
    let legend_style =
        if plan.analyzed { Style::new().fg(th.fg_dim).bg(th.bg) } else { Style::new().fg(th.warning).bg(th.bg) };
    put(buf, area.x + 1, area.y, &legend, area.width.saturating_sub(2) as usize, legend_style);
    let rows = Rect { y: area.y + 1, height: area.height - 1, ..area };
    let order = p.visible();
    let at = order.iter().position(|i| *i == p.selected).unwrap_or(0);
    let top = super::follow(p, at, order.len(), rows.height as usize);
    for (row, &i) in order.iter().skip(top).take(rows.height as usize).enumerate() {
        work(1);
        let y = rows.y + row as u16;
        let line = Rect { y, height: 1, ..rows };
        let sel = i == p.selected;
        let base = if sel { th.base().patch(selected_style(cx)) } else { th.base() };
        buf.set_style(line, base);
        p.hits.push((line, i));
        let n = &plan.nodes[i];
        put(buf, area.x + 1, y, &indented_name(&plan, i, name_w), name_w, base);
        let never = n.actual.is_some_and(|a| a.loops <= 0.0);
        if never {
            put(buf, band_x, y, &cx.i18n.label(Label::PlanNeverRan), band_w, base.fg(th.fg_dim));
            continue;
        }
        let r = rows_of(i);
        let w = ((thickness(r, max) * band_w as f64).round() as usize).max(usize::from(r > 0.0)).min(band_w);
        let off = plan.misestimate(i).is_some();
        let color = if off { th.plan_misestimate } else { Style::new().fg(th.accent) };
        let pad = (band_w - w) / 2;
        put(buf, band_x + pad as u16, y, &"█".repeat(w), w, base.patch(on_line(cx, color, sel)));
        // The numbers, as far as they fit.
        let mut x = band_x + band_w as u16 + 1;
        let end = area.x + area.width - 1;
        if x + 7 > end {
            continue;
        }
        x += put(buf, x, y, &fit(&fmt_num(r), 7, Align::Right), 7, base);
        if let Some(removed) = n.removed.filter(|r| *r > 0.0) {
            let t = cx.i18n.msg(&Msg::PlanFunnelRemoved { rows: fmt_num(removed) }).to_string();
            if x + 1 + width(&t) as u16 <= end {
                x += 1 + put(
                    buf,
                    x + 1,
                    y,
                    &t,
                    (end - x - 1) as usize,
                    base.patch(on_line(cx, Style::new().fg(th.warning), sel)),
                );
            }
        }
        for (t, style) in markers(cx, &plan, i) {
            if x + 1 < end {
                x += 1 + put(buf, x + 1, y, &t, (end - x - 1) as usize, base.patch(on_line(cx, style, sel)));
            }
        }
    }
}
