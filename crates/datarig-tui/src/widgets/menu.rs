//! Context menus (the explorer's, the grid's): a box at the pointer with one item per row and
//! its keys on the right; an open submenu (the grid's copy formats) is a second box next to
//! its row.

use crate::app::App;
use crate::text::{Align, fit, width};
use crate::theme;
use crate::widgets::popup::clear_overlay;
use crate::widgets::put;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Widget};

pub(crate) fn draw_context_menu(app: &mut App, area: Rect, buf: &mut Buffer) {
    let rows = app.menu_rows();
    let Some(m) = app.overlays.menu() else { return };
    let (selected, (ax, ay), open) = (m.selected, m.at, m.sub.as_ref().map(|s| s.selected));
    // At the pointer, moved in to stay on the screen.
    let (rect, list) = draw_box(&rows, (ax, ay + 1), selected, open.is_none(), area, buf);
    if let Some(m) = app.overlays.menu_mut() {
        m.list = list;
    }
    // The open submenu: next to its row, on the right (on the left when there is no room).
    let Some(sub_selected) = open else { return };
    let sub_rows = app.sub_menu_rows();
    let w = box_width(&sub_rows).min(area.width);
    let y = rect.y + 1 + selected as u16;
    let right = rect.x + rect.width;
    let x = if right + w <= area.x + area.width { right } else { rect.x.saturating_sub(w) };
    let (_, list) = draw_box(&sub_rows, (x, y.saturating_sub(1)), sub_selected, true, area, buf);
    if let Some(sub) = app.overlays.menu_mut().and_then(|m| m.sub.as_mut()) {
        sub.list = list;
    }
}

/// Columns a box of `rows` takes: a label, its keys and the border.
fn box_width(rows: &[(datarig_core::i18n::Localized, String)]) -> u16 {
    let label_w = rows.iter().map(|(l, _)| width(l)).max().unwrap_or(0);
    let key_w = rows.iter().map(|(_, k)| width(k)).max().unwrap_or(0);
    (label_w + key_w + 5) as u16
}

/// A menu box of `rows` with its top-left corner at `at` (moved in to stay in `area`), row
/// `selected` highlighted (dimmer when the keys are in a submenu); returns the box and the area
/// of its rows.
fn draw_box(
    rows: &[(datarig_core::i18n::Localized, String)],
    (ax, ay): (u16, u16),
    selected: usize,
    active: bool,
    area: Rect,
    buf: &mut Buffer,
) -> (Rect, Rect) {
    let key_w = rows.iter().map(|(_, k)| width(k)).max().unwrap_or(0);
    let w = box_width(rows).min(area.width);
    let h = (rows.len() as u16 + 2).min(area.height);
    let x = ax.min(area.x + area.width - w);
    let y = ay.min(area.y + area.height - h);
    let rect = Rect::new(x, y, w, h);
    clear_overlay(rect, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme::ACCENT).bg(theme::SURFACE))
        .style(Style::new().bg(theme::SURFACE).fg(theme::FG));
    let inner = block.inner(rect);
    block.render(rect, buf);
    let iw = inner.width as usize;
    for (i, (label, keys)) in rows.iter().enumerate().take(inner.height as usize) {
        let ry = inner.y + i as u16;
        let bg = match (i == selected, active) {
            (true, true) => theme::SELECTION_BG,
            (true, false) => theme::CURSOR_LINE_BG,
            _ => theme::SURFACE,
        };
        buf.set_stringn(inner.x, ry, fit("", iw, Align::Left), iw, Style::new().bg(bg));
        put(buf, inner.x + 1, ry, label, iw.saturating_sub(key_w + 3), Style::new().fg(theme::FG).bg(bg));
        let kx = inner.x + inner.width - 1 - width(keys) as u16;
        let style = Style::new().fg(theme::ACCENT_WARM).bg(bg).add_modifier(Modifier::BOLD);
        put(buf, kx, ry, keys, key_w, style);
    }
    (rect, Rect::new(inner.x, inner.y, inner.width, inner.height.min(rows.len() as u16)))
}
