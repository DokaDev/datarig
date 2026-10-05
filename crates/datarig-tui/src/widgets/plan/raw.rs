//! The raw view: the text `psql` shows for `EXPLAIN`, written from the plan (never asked of the
//! server again). The selected node's lines are marked, its node line most; `<` and `>` move
//! long lines sideways.

use super::{Look, on_line, selected_style, work};
use crate::app::plan::PlanTab;
use crate::text::{grapheme_width, width};
use crate::widgets::put;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_segmentation::UnicodeSegmentation;

pub(super) fn draw(cx: &Look, p: &mut PlanTab, area: Rect, buf: &mut Buffer) {
    let th = cx.th;
    let plan = p.plan.clone();
    let selected = p.selected;
    let pan = p.pan;
    let raw = p.raw();
    let lines = &raw.lines;
    let head = raw.node_line(&plan, selected);
    let top = super::follow(p, head, lines.len(), area.height as usize);
    for (row, (text, node)) in lines.iter().enumerate().skip(top).take(area.height as usize) {
        work(1);
        let y = area.y + (row - top) as u16;
        let line = Rect { y, height: 1, ..area };
        let mine = *node == Some(selected);
        let base = match (row == head, mine) {
            (true, _) => th.base().patch(selected_style(cx)),
            (false, true) => th.base().patch(th.cursor_line),
            _ => th.base(),
        };
        buf.set_style(line, base);
        if let Some(n) = node {
            p.hits.push((line, *n));
        }
        let shown = skip_cols(text, pan);
        let w = area.width.saturating_sub(1) as usize;
        let header = node.is_some_and(|n| row == raw.node_line(&plan, n));
        if header {
            let hot = node.is_some_and(|n| plan.is_hot(n));
            let style =
                if hot { base.patch(on_line(cx, th.plan_hot, row == head)) } else { base.add_modifier(Modifier::BOLD) };
            put(buf, area.x + 1, y, &shown, w, style);
        } else if let Some((key, value)) = shown.split_once(": ").filter(|_| node.is_some()) {
            let kw = put(buf, area.x + 1, y, &format!("{key}: "), w, base.fg(th.fg_muted));
            let rest = w.saturating_sub(kw as usize);
            put(buf, area.x + 1 + kw, y, value, rest, base);
        } else {
            put(buf, area.x + 1, y, &shown, w, Style::new().fg(th.fg).patch(base));
        }
    }
}

/// `s` without its first `n` columns (a wide character cut in two becomes a space).
pub(super) fn skip_cols(s: &str, n: usize) -> String {
    if n == 0 {
        return s.to_string();
    }
    let mut out = String::new();
    let mut at = 0;
    for g in s.graphemes(true) {
        let gw = grapheme_width(g);
        if at >= n {
            out.push_str(g);
        } else if at + gw > n {
            out.push_str(&" ".repeat(at + gw - n));
        }
        at += gw;
    }
    debug_assert!(width(&out) <= width(s));
    out
}
