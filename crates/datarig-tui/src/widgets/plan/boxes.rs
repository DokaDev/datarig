//! The box diagram: every node a box with its name and its own time (or cost), its children in
//! a row below it joined by lines, laid out as a tidy tree (leaves side by side, a parent over
//! the middle of its children). A large plan is larger than the screen: the view follows the
//! selected box, `<` `>` and the wheel move it. Only the boxes on screen are drawn.

use super::{Look, fmt_share, fmt_weight, heat, info_line, markers, work};
use crate::app::plan::PlanTab;
use datarig_core::sql::plan::Plan;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// A box's width and height, and the room between boxes across and down.
pub(super) const BOX_W: usize = 24;
const BOX_H: usize = 4;
const GAP_X: usize = 2;
const GAP_Y: usize = 2;

/// Each node's box: its left column and its top line on the whole diagram.
pub(super) fn layout(plan: &Plan) -> Vec<(usize, usize)> {
    let len = plan.nodes.len();
    // Centers in slots of one box: a leaf takes the next slot, a parent sits over the middle of
    // its first and last child. Children come after their parent, so the last node first.
    let mut center = vec![0.0_f64; len];
    let mut next = 0.0;
    // Leaves in depth-first order get the slots from the left.
    for (i, n) in plan.nodes.iter().enumerate() {
        work(1);
        if n.children.is_empty() {
            center[i] = next;
            next += 1.0;
        }
    }
    for i in (0..len).rev() {
        let n = &plan.nodes[i];
        if let (Some(&a), Some(&b)) = (n.children.first(), n.children.last()) {
            center[i] = (center[a] + center[b]) / 2.0;
        }
    }
    let pitch = (BOX_W + GAP_X) as f64;
    plan.nodes
        .iter()
        .enumerate()
        .map(|(i, n)| ((center[i] * pitch).round() as usize, n.depth * (BOX_H + GAP_Y)))
        .collect()
}

pub(super) fn draw(cx: &Look, p: &mut PlanTab, area: Rect, buf: &mut Buffer) {
    let th = cx.th;
    let plan = p.plan.clone();
    if area.height < BOX_H as u16 + 2 || area.width < BOX_W as u16 + 2 {
        return;
    }
    let view = Rect { height: area.height - 1, ..area };
    info_line(cx, &plan, p.selected, Rect { y: area.y + area.height - 1, height: 1, ..area }, buf);
    let at = layout(&plan);
    let (vw, vh) = (view.width.saturating_sub(1) as usize, view.height as usize);
    // The selected box in view (unless the wheel moved the view away).
    if let Some(&(x, y)) = at.get(p.selected)
        && !p.detached
    {
        if x < p.pan {
            p.pan = x;
        } else if x + BOX_W > p.pan + vw {
            p.pan = (x + BOX_W).saturating_sub(vw);
        }
        if y < p.scroll {
            p.scroll = y;
        } else if y + BOX_H > p.scroll + vh {
            p.scroll = (y + BOX_H).saturating_sub(vh);
        }
    }
    let width = at.iter().map(|a| a.0 + BOX_W).max().unwrap_or(0);
    let height = at.iter().map(|a| a.1 + BOX_H).max().unwrap_or(0);
    p.pan = p.pan.min(width.saturating_sub(vw));
    p.scroll = p.scroll.min(height.saturating_sub(vh));
    p.page = (vh / (BOX_H + GAP_Y)).max(1);
    let (ox, oy) = (p.pan as isize, p.scroll as isize);
    let line = Style::new().fg(th.border).bg(th.bg);
    // A cell of the diagram on screen, if it is.
    let cell = |x: usize, y: usize| -> Option<(u16, u16)> {
        let (sx, sy) = (x as isize - ox, y as isize - oy);
        (sx >= 0 && sy >= 0 && (sx as usize) < vw && (sy as usize) < vh)
            .then(|| (view.x + 1 + sx as u16, view.y + sy as u16))
    };
    let put_at = |buf: &mut Buffer, x: usize, y: usize, s: &str, style: Style| {
        if let Some((sx, sy)) = cell(x, y) {
            buf.set_stringn(sx, sy, s, 1, style);
        }
    };
    // The lines between a parent and its children, first (boxes go over them).
    for (i, n) in plan.nodes.iter().enumerate() {
        work(1);
        if n.children.is_empty() {
            continue;
        }
        let (px, py) = at[i];
        let parent_mid = px + BOX_W / 2;
        let (bus_y, below) = (py + BOX_H, py + BOX_H + 1);
        // Skip a family entirely off screen.
        let first = at[n.children[0]].0 + BOX_W / 2;
        let last = at[*n.children.last().unwrap_or(&n.children[0])].0 + BOX_W / 2;
        let (lo, hi) = (first.min(parent_mid), last.max(parent_mid));
        if (hi as isize) < ox
            || lo as isize > ox + vw as isize
            || (below as isize) < oy
            || bus_y as isize > oy + vh as isize
        {
            continue;
        }
        put_at(buf, parent_mid, bus_y, "│", line);
        // Only the part of the line on screen, the children's columns looked up.
        let centers: Vec<usize> = n.children.iter().map(|&c| at[c].0 + BOX_W / 2).collect();
        let (from, to) = (lo.max(ox.max(0) as usize), hi.min((ox + vw as isize).max(0) as usize));
        for x in from..=to.max(from) {
            if x > hi {
                break;
            }
            let child = centers.binary_search(&x).is_ok();
            let s = match (child, x == parent_mid, x == lo, x == hi) {
                (true, true, _, _) => "┼",
                (true, false, true, _) => "╭",
                (true, false, _, true) => "╮",
                (true, false, _, _) => "┬",
                (false, true, _, _) => "┴",
                _ => "─",
            };
            let s = if lo == hi { "│" } else { s };
            put_at(buf, x, below, s, line);
        }
    }
    // The boxes on screen.
    for (i, &(x, y)) in at.iter().enumerate() {
        work(1);
        let (sx, sy) = (x as isize - ox, y as isize - oy);
        if sx + BOX_W as isize <= 0 || sy + BOX_H as isize <= 0 || sx >= vw as isize || sy >= vh as isize {
            continue;
        }
        let sel = i == p.selected;
        let share = plan.share(i);
        let frame = if sel {
            Style::new().fg(th.accent).bg(th.bg).add_modifier(Modifier::BOLD)
        } else if plan.is_hot(i) {
            Style::new().patch(th.plan_hot).bg(th.bg)
        } else {
            line
        };
        let inner = BOX_W - 2;
        let label = plan.label(i);
        let numbers = format!("{} · {}", fmt_weight(&plan, plan.weight(i)), fmt_share(share));
        let marks: String = markers(cx, &plan, i).into_iter().map(|(t, _)| format!(" {t}")).collect();
        let rows = [
            format!("╭{}╮", "─".repeat(inner)),
            format!("│{}│", crate::text::fit(&label, inner, crate::text::Align::Left)),
            format!("│{}│", crate::text::fit(&format!("{numbers}{marks}"), inner, crate::text::Align::Left)),
            format!("╰{}╯", "─".repeat(inner)),
        ];
        let text_style = if sel { th.base().patch(super::selected_style(cx)) } else { th.base() };
        for (k, r) in rows.iter().enumerate() {
            for (j, ch) in r.chars().enumerate() {
                let edge = k == 0 || k == rows.len() - 1 || j == 0 || j + 1 == r.chars().count();
                let style = if edge { frame } else { text_style };
                let style = match (k, edge) {
                    (2, false) => style.patch(super::on_line(cx, heat(th, share), sel)),
                    _ => style,
                };
                put_at(buf, x + j, y + k, &ch.to_string(), style);
            }
        }
        // Where the pointer finds it: the part on screen.
        let x0 = sx.max(0) as u16;
        let y0 = sy.max(0) as u16;
        let x1 = ((sx + BOX_W as isize).min(vw as isize)) as u16;
        let y1 = ((sy + BOX_H as isize).min(vh as isize)) as u16;
        if x1 > x0 && y1 > y0 {
            p.hits.push((Rect { x: view.x + 1 + x0, y: view.y + y0, width: x1 - x0, height: y1 - y0 }, i));
        }
    }
}
