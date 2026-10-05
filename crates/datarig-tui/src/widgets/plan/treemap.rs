//! The treemap: every node a rectangle whose area is its own share of the whole (self time, by
//! cost: self cost), largest first, laid out squarified (Bruls, Huizing and van Wijk) with a
//! terminal cell counted twice as tall as it is wide, so rectangles look square. The
//! smallest nodes that would not get a cell share one rectangle at the end (`… more nodes`).

use super::{Look, fmt_share, fmt_weight, heat, info_line, work};
use crate::app::plan::PlanTab;
use crate::widgets::put;
use datarig_core::i18n::Msg;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// At most this many nodes get a rectangle of their own.
const MAX_RECTS: usize = 300;

/// A rectangle in floating cells.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Cells are about twice as tall as wide: the layout works in a space where they are square.
const CELL_ASPECT: f64 = 2.0;

/// The squarified layout of `items` (`(id, weight)`, largest first) in `r`.
pub(super) fn squarify(items: &[(usize, f64)], r: Area) -> Vec<(usize, Area)> {
    let total: f64 = items.iter().map(|i| i.1).sum();
    if total <= 0.0 || r.w <= 0.0 || r.h <= 0.0 {
        return Vec::new();
    }
    // Square space: x is stretched by the cell's aspect.
    let mut rect = Area { x: r.x * CELL_ASPECT, y: r.y, w: r.w * CELL_ASPECT, h: r.h };
    let scale = rect.w * rect.h / total;
    let areas: Vec<(usize, f64)> = items.iter().map(|&(id, w)| (id, w * scale)).collect();
    let mut out = Vec::with_capacity(areas.len());
    let mut row: Vec<(usize, f64)> = Vec::new();
    let worst = |row: &[(usize, f64)], side: f64| {
        let sum: f64 = row.iter().map(|r| r.1).sum();
        let (min, max) = row.iter().fold((f64::MAX, 0.0_f64), |(lo, hi), r| (lo.min(r.1), hi.max(r.1)));
        let (s2, w2) = (sum * sum, side * side);
        (w2 * max / s2).max(s2 / (w2 * min))
    };
    let mut i = 0;
    while i < areas.len() {
        let side = rect.w.min(rect.h);
        let mut next = row.clone();
        next.push(areas[i]);
        if row.is_empty() || worst(&next, side) <= worst(&row, side) {
            row = next;
            i += 1;
        } else {
            place(&row, &mut rect, &mut out);
            row.clear();
        }
    }
    if !row.is_empty() {
        place(&row, &mut rect, &mut out);
    }
    // Back to cells.
    out.into_iter().map(|(id, a)| (id, Area { x: a.x / CELL_ASPECT, w: a.w / CELL_ASPECT, ..a })).collect()
}

/// Lay `row` along the short side of `rect` and take its strip off `rect`.
fn place(row: &[(usize, f64)], rect: &mut Area, out: &mut Vec<(usize, Area)>) {
    let sum: f64 = row.iter().map(|r| r.1).sum();
    if rect.w >= rect.h {
        // A column at the left.
        let w = if rect.h > 0.0 { sum / rect.h } else { 0.0 };
        let mut y = rect.y;
        for &(id, a) in row {
            let h = if w > 0.0 { a / w } else { 0.0 };
            out.push((id, Area { x: rect.x, y, w, h }));
            y += h;
        }
        rect.x += w;
        rect.w -= w;
    } else {
        // A row at the top.
        let h = if rect.w > 0.0 { sum / rect.w } else { 0.0 };
        let mut x = rect.x;
        for &(id, a) in row {
            let w = if h > 0.0 { a / h } else { 0.0 };
            out.push((id, Area { x, y: rect.y, w, h }));
            x += w;
        }
        rect.y += h;
        rect.h -= h;
    }
}

/// The rectangle of the nodes too small for one of their own.
pub(super) const OTHERS: usize = usize::MAX;

pub(super) fn draw(cx: &Look, p: &mut PlanTab, area: Rect, buf: &mut Buffer) {
    let th = cx.th;
    let plan = p.plan.clone();
    if area.height < 2 || area.width < 4 {
        return;
    }
    let map = Rect { height: area.height - 1, ..area };
    info_line(cx, &plan, p.selected, Rect { y: area.y + area.height - 1, height: 1, ..area }, buf);
    let mut items: Vec<(usize, f64)> =
        (0..plan.nodes.len()).map(|i| (i, plan.weight(i))).filter(|i| i.1 > 0.0).collect();
    work(items.len() as u64);
    items.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    // Nodes that would get less than a cell, and those past the most rectangles: one together.
    let total: f64 = items.iter().map(|i| i.1).sum();
    let cells = f64::from(map.width.saturating_sub(1)) * f64::from(map.height);
    let keep = items.iter().take(MAX_RECTS).take_while(|i| total > 0.0 && i.1 / total * cells >= 1.0).count();
    let rest = items.len() - keep;
    let rest_weight: f64 = items[keep..].iter().map(|i| i.1).sum();
    items.truncate(keep);
    if rest > 0 && rest_weight > 0.0 {
        items.push((OTHERS, rest_weight));
    }
    let r = Area { x: 0.0, y: 0.0, w: f64::from(map.width.saturating_sub(1)), h: f64::from(map.height) };
    for (id, a) in squarify(&items, r) {
        work(1);
        let (x0, x1) = (a.x.round() as u16, (a.x + a.w).round() as u16);
        let (y0, y1) = (a.y.round() as u16, (a.y + a.h).round() as u16);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let cell = Rect { x: map.x + 1 + x0, y: map.y + y0, width: x1 - x0, height: y1 - y0 };
        // A gap at the right and the bottom between neighbours, when there is room for one.
        let inner = Rect {
            width: if cell.width >= 3 { cell.width - 1 } else { cell.width },
            height: if cell.height >= 3 { cell.height - 1 } else { cell.height },
            ..cell
        };
        let sel = id == p.selected;
        let style = match id {
            OTHERS => Style::new().fg(th.fg_dim).bg(th.bg).add_modifier(Modifier::REVERSED),
            _ if sel => th.base().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            _ => {
                let color = heat(th, plan.share(id)).fg.unwrap_or(th.accent);
                Style::new().fg(color).bg(th.bg).add_modifier(Modifier::REVERSED)
            }
        };
        buf.set_style(inner, style);
        // A thin line down the left edge, so neighbours stay apart without color too.
        for y in inner.y..inner.y + inner.height {
            buf.set_stringn(inner.x, y, "▏", 1, style);
        }
        let (x, w) = (inner.x + 1, (inner.width as usize).saturating_sub(1));
        if id == OTHERS {
            let count = rest as u64;
            put(buf, x, inner.y, &cx.i18n.msg(&Msg::PlanTreemapOthers { count }), w, style);
            continue;
        }
        p.hits.push((cell, id));
        put(buf, x, inner.y, &plan.label(id), w, style);
        if inner.height >= 2 {
            let line = format!("{} · {}", fmt_share(plan.share(id)), fmt_weight(&plan, plan.weight(id)));
            put(buf, x, inner.y + 1, &line, w, style);
        }
    }
}
