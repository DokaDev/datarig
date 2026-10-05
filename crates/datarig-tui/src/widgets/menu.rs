//! The action menu: a box at the pointer or next to the selection with the filter on its first
//! row, then headings and one item per row with its keys on the right; an open submenu (the
//! grid's copy formats) is a second box next to its row.

use crate::app::App;
use crate::text::{Align, fit, width};
use crate::theme;
use crate::widgets::popup::clear_overlay;
use crate::widgets::put;
use datarig_core::i18n::{Label, Localized};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Widget};

/// The filter's mark at the start of its row.
const PROMPT: &str = "› ";

/// Draw the menu; returns where the cursor goes (the filter's end) while the menu has the keys.
pub(crate) fn draw_context_menu(app: &mut App, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let th = theme::cur();
    let lines = app.menu_lines();
    let (label_w, heading_w, key_w) = app.menu_widths();
    let hint = app.i18n.label(Label::MenuFilterHint);
    let m = app.overlays.menu_mut()?;
    let (ax, ay) = m.at;
    // Items are indented under their headings; the keys keep a gap after the labels.
    let label_w =
        lines.iter().map(|(h, l, _)| width(l) + usize::from(!*h)).max().unwrap_or(0).max(label_w + 1).max(heading_w);
    let key_w = lines.iter().map(|(_, _, k)| width(k)).max().unwrap_or(0).max(key_w);
    let inner_w = (label_w + key_w + 4).max(width(PROMPT) + width(&hint) + 2).max(width(m.filter.text()) + 4);
    let w = (inner_w as u16 + 2).min(area.width);
    let h = (lines.len() as u16 + 3).min(area.height);
    // Below the row it was opened on, moved in to stay on the screen.
    let x = ax.min(area.x + area.width - w);
    let y = (ay + 1).min(area.y + area.height - h);
    let rect = Rect::new(x, y, w, h);
    m.area = rect;
    clear_overlay(rect, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(th.accent).bg(th.surface))
        .style(Style::new().bg(th.surface).fg(th.fg));
    let inner = block.inner(rect);
    block.render(rect, buf);
    if inner.height == 0 || inner.width < 3 {
        return None;
    }
    // The filter, or what typing does.
    let iw = inner.width as usize;
    let base = Style::new().bg(th.surface);
    put(buf, inner.x + 1, inner.y, PROMPT, iw - 1, base.fg(th.accent));
    let fx = inner.x + 1 + width(PROMPT) as u16;
    let room = Rect::new(fx, inner.y, inner.width.saturating_sub(fx - inner.x + 1), 1);
    let typing = m.sub.is_none();
    let cursor = if m.filter.text().is_empty() {
        put(buf, fx, inner.y, &hint, room.width as usize, base.fg(th.fg_dim));
        typing.then_some((fx, inner.y))
    } else {
        let cx = m.filter.render(room, buf, base.fg(th.fg), typing, false, None);
        typing.then_some((cx, inner.y))
    };
    // The lines, scrolled so the selected one is on screen.
    let list = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
    let visible = list.height as usize;
    if m.selected < m.scroll {
        // Its section's heading stays in sight above the first item.
        let heading = m.selected > 0 && lines.get(m.selected - 1).is_some_and(|l| l.0);
        m.scroll = m.selected - usize::from(heading);
    } else if visible > 0 && m.selected >= m.scroll + visible {
        m.scroll = m.selected + 1 - visible;
    }
    m.scroll = m.scroll.min(lines.len().saturating_sub(visible));
    let active = m.sub.is_none();
    for (i, (heading, label, keys)) in lines.iter().enumerate().skip(m.scroll).take(visible) {
        let ry = list.y + (i - m.scroll) as u16;
        let bg = match (i == m.selected && !heading, active) {
            (true, true) => th.selection,
            (true, false) => th.cursor_line,
            _ => base,
        };
        buf.set_stringn(list.x, ry, fit("", iw, Align::Left), iw, bg);
        if *heading {
            put(
                buf,
                list.x + 1,
                ry,
                label,
                iw - 1,
                Style::new().fg(th.fg_muted).patch(bg).add_modifier(Modifier::BOLD),
            );
            continue;
        }
        put(buf, list.x + 2, ry, label, iw.saturating_sub(key_w + 4), Style::new().fg(th.fg).patch(bg));
        let kx = list.x + list.width - 1 - width(keys) as u16;
        let style = Style::new().fg(th.accent_warm).patch(bg).add_modifier(Modifier::BOLD);
        put(buf, kx, ry, keys, key_w, style);
    }
    m.list = Rect::new(list.x, list.y, list.width, list.height.min((lines.len() - m.scroll) as u16));
    // The open submenu: next to its row, on the right (on the left when there is no room).
    let (selected, scroll) = (m.selected, m.scroll);
    let Some(sub_selected) = m.sub.as_ref().map(|s| s.selected) else { return cursor };
    let sub_rows = app.sub_menu_rows();
    let w = box_width(&sub_rows).min(area.width);
    let row_y = list.y + selected.saturating_sub(scroll) as u16;
    let right = rect.x + rect.width;
    let x = if right + w <= area.x + area.width { right } else { rect.x.saturating_sub(w) };
    let sub_list = draw_box(&sub_rows, (x, row_y.saturating_sub(1)), sub_selected, area, buf);
    if let Some(sub) = app.overlays.menu_mut().and_then(|m| m.sub.as_mut()) {
        sub.list = sub_list;
    }
    None
}

/// Columns a box of `rows` takes: a label, its keys and the border.
fn box_width(rows: &[(Localized, String)]) -> u16 {
    let label_w = rows.iter().map(|(l, _)| width(l)).max().unwrap_or(0);
    let key_w = rows.iter().map(|(_, k)| width(k)).max().unwrap_or(0);
    (label_w + key_w + 5) as u16
}

/// The submenu's box of `rows` with its top-left corner at `at` (moved in to stay in `area`),
/// row `selected` highlighted; returns the area of its rows.
fn draw_box(rows: &[(Localized, String)], (ax, ay): (u16, u16), selected: usize, area: Rect, buf: &mut Buffer) -> Rect {
    let th = theme::cur();
    let key_w = rows.iter().map(|(_, k)| width(k)).max().unwrap_or(0);
    let w = box_width(rows).min(area.width);
    let h = (rows.len() as u16 + 2).min(area.height);
    let x = ax.min(area.x + area.width - w);
    let y = ay.min(area.y + area.height - h);
    let rect = Rect::new(x, y, w, h);
    clear_overlay(rect, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(th.accent).bg(th.surface))
        .style(Style::new().bg(th.surface).fg(th.fg));
    let inner = block.inner(rect);
    block.render(rect, buf);
    let iw = inner.width as usize;
    for (i, (label, keys)) in rows.iter().enumerate().take(inner.height as usize) {
        let ry = inner.y + i as u16;
        let bg = if i == selected { th.selection } else { Style::new().bg(th.surface) };
        buf.set_stringn(inner.x, ry, fit("", iw, Align::Left), iw, bg);
        put(buf, inner.x + 1, ry, label, iw.saturating_sub(key_w + 3), Style::new().fg(th.fg).patch(bg));
        let kx = inner.x + inner.width - 1 - width(keys) as u16;
        let style = Style::new().fg(th.accent_warm).patch(bg).add_modifier(Modifier::BOLD);
        put(buf, kx, ry, keys, key_w, style);
    }
    Rect::new(inner.x, inner.y, inner.width, inner.height.min(rows.len() as u16))
}
