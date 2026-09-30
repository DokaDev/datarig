//! The which-key popup and the keyboard help overlay. Their content comes from
//! [`App::which_key_items`] and [`App::help_rows`]; this module only lays it out.

use crate::app::App;
use crate::app::guide::{HelpRow, WhichKeyItem};
use crate::text::{Align, clip, fit, width};
use crate::theme;
use crate::widgets::dialog::{centered, modal};
use crate::widgets::put;
use datarig_core::i18n::{Label, Msg};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// Widest column of the which-key popup (longer labels end in `…`).
const WHICH_KEY_COL: usize = 40;
/// Widest keyboard help box.
const HELP_MAX_W: u16 = 100;

/// The which-key popup: a box along the bottom of `area` (above the status bar) listing the
/// keys that may follow, column by column, and apart below them the key of this screen's
/// help. Fewer columns fit a narrow terminal; what does not fit ends with `…`.
pub(crate) fn draw_which_key(app: &App, area: Rect, buf: &mut Buffer) {
    let Some((title, all)) = app.which_key_items() else { return };
    let (items, footer): (Vec<_>, Vec<_>) = all.into_iter().partition(|i| !i.footer);
    let footer_h = if footer.is_empty() { 0 } else { 2 };
    let keys_footer = app.which_key_footer();
    let inner_w = area.width.saturating_sub(2) as usize;
    let key_w = items.iter().chain(&footer).map(|i| width(&i.key)).max().unwrap_or(1);
    let entry_w = |label: &str| key_w + 2 + width(label);
    let col_w = items.iter().map(|i| entry_w(&i.label)).max().unwrap_or(1).min(WHICH_KEY_COL) + 3;
    let cols = (inner_w / col_w).clamp(1, items.len().max(1));
    let max_rows = (area.height as usize / 2).saturating_sub(footer_h).max(1);
    let rows = items.len().div_ceil(cols).clamp(1, max_rows);
    let h = rows as u16 + 2 + footer_h as u16;
    let rect = Rect::new(area.x, area.y + area.height.saturating_sub(h), area.width, h.min(area.height));
    let inner = modal(rect, &title, &keys_footer, buf);
    let cell_w = inner.width as usize / cols;
    let fits = rows * cols;
    let entry = |buf: &mut Buffer, it: &WhichKeyItem, x: u16, y: u16, room: usize| {
        let dim = !it.enabled;
        let key_style = Style::new()
            .fg(if dim { theme::FG_DIM } else { theme::ACCENT })
            .bg(theme::SURFACE)
            .add_modifier(Modifier::BOLD);
        let label_color = if dim {
            theme::FG_DIM
        } else if it.group {
            theme::ACCENT_WARM
        } else {
            theme::FG
        };
        let used = put(buf, x, y, &fit(&it.key, key_w, Align::Left), room, key_style) as usize;
        let rest = room.saturating_sub(used + 2);
        put(buf, x + used as u16 + 2, y, &it.label, rest, Style::new().fg(label_color).bg(theme::SURFACE));
    };
    for (i, it) in items.iter().enumerate().take(fits) {
        let (col, row) = (i / rows, i % rows);
        let x = inner.x + (col * cell_w) as u16 + 1;
        let y = inner.y + row as u16;
        let room = cell_w.saturating_sub(2);
        if i + 1 == fits && items.len() > fits {
            put(buf, x, y, "…", room, Style::new().fg(theme::FG_MUTED).bg(theme::SURFACE));
            break;
        }
        entry(buf, it, x, y, room);
    }
    if let Some(it) = footer.first() {
        let iw = inner.width as usize;
        let y = inner.y + rows as u16;
        buf.set_stringn(inner.x, y, "─".repeat(iw), iw, Style::new().fg(theme::BORDER).bg(theme::SURFACE));
        entry(buf, it, inner.x + 1, y + 1, iw.saturating_sub(2));
    }
}

/// The keyboard help: a search line that says how to search and close, then one list of
/// every context. Section headers show `▾` (open) or `▸` and the number of keys. Returns the
/// cursor position while the filter is being typed.
pub(crate) fn draw_help(app: &mut App, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let rows = app.help_rows();
    let h = app.overlays.help()?;
    let (origin, filtering, selected) = (h.origin, h.filtering, h.selected.min(rows.len().saturating_sub(1)));
    let query = h.filter.text().to_string();
    let title = app.i18n.msg(&Msg::HelpTitle { context: app.i18n.label(origin.title()).to_string() });
    let footer = app.i18n.label(Label::HelpKeys);
    let placeholder = app.i18n.label(Label::HelpFilterPlaceholder);
    let none = app.i18n.msg(&Msg::HelpFilterNone { query: query.clone() });
    // A centered box with a margin, so the screen stays visible (dimmed) around it.
    let rect = centered(area, area.width.saturating_sub(4).min(HELP_MAX_W), area.height.saturating_sub(2));
    let inner = modal(rect, &title, &footer, buf);
    let iw = inner.width as usize;
    let surface = |fg| Style::new().fg(fg).bg(theme::SURFACE);

    // Search line: says how to search and close until something is typed.
    let input_area = Rect::new(inner.x + 3, inner.y, inner.width.saturating_sub(4), 1);
    let cx = app.overlays.help_mut()?.filter.render(input_area, buf, surface(theme::FG), filtering, false, None);
    if query.is_empty() && !filtering {
        put(buf, inner.x + 1, inner.y, &placeholder, iw.saturating_sub(2), surface(theme::FG_MUTED));
    } else {
        buf.set_stringn(inner.x + 1, inner.y, "/", 1, surface(theme::ACCENT).add_modifier(Modifier::BOLD));
    }
    buf.set_stringn(inner.x, inner.y + 1, "─".repeat(iw), iw, surface(theme::BORDER));

    let view_h = inner.height.saturating_sub(2) as usize;
    if rows.is_empty() {
        put(buf, inner.x + 1, inner.y + 2, &none, iw.saturating_sub(2), surface(theme::FG_DIM));
    }
    // Keys column: as wide as the widest keys, at most half the box.
    let keys_text = |keys: &str, editor: &str| {
        let note = if editor.is_empty() {
            String::new()
        } else {
            app.i18n.msg(&Msg::HelpEditorKeys { keys: editor.to_string() }).to_string()
        };
        (keys.to_string(), note)
    };
    let keys_w = rows
        .iter()
        .filter_map(|r| if let HelpRow::Entry(e) = r { Some(e) } else { None })
        .map(|e| {
            let (k, n) = keys_text(&e.keys, &e.editor_keys);
            width(&k) + if n.is_empty() { 0 } else { width(&n) + 1 }
        })
        .max()
        .unwrap_or(0)
        .min(iw / 2);
    let h = app.overlays.help_mut()?;
    h.selected = selected;
    h.view_h = view_h.max(1);
    h.list = Rect::new(inner.x, inner.y + 2, inner.width, view_h as u16);
    // Keep the selected row in view, with the header above a section's first entry.
    if selected < h.scroll {
        h.scroll = selected.saturating_sub(1);
    } else if selected >= h.scroll + view_h {
        h.scroll = selected + 1 - view_h;
    }
    h.scroll = h.scroll.min(rows.len().saturating_sub(view_h));
    let scroll = h.scroll;

    for (i, row) in rows.iter().enumerate().skip(scroll).take(view_h) {
        let y = inner.y + 2 + (i - scroll) as u16;
        let bg = if i == selected { theme::SELECTION_BG } else { theme::SURFACE };
        buf.set_style(Rect::new(inner.x, y, inner.width, 1), Style::new().bg(bg));
        match row {
            HelpRow::Section { ctx, open, count } => {
                let mark = if *open { "▾" } else { "▸" };
                let t = format!("{mark} {}", app.i18n.label(ctx.title()));
                let head = Style::new().fg(theme::ACCENT).bg(bg).add_modifier(Modifier::BOLD);
                // A closed section shows how many keys it holds, in the keys column.
                let n = if *open { String::new() } else { count.to_string() };
                let used = put(buf, inner.x + 1, y, &t, iw.saturating_sub(3 + width(&n)), head) as usize;
                if !n.is_empty() && used + 3 + width(&n) <= iw {
                    let x = inner.x + inner.width - 1 - keys_w.max(width(&n)) as u16;
                    put(buf, x, y, &n, width(&n), Style::new().fg(theme::FG_DIM).bg(bg));
                }
            }
            HelpRow::Entry(e) => {
                let fg = if e.enabled { theme::FG } else { theme::FG_DIM };
                let label_w = iw.saturating_sub(keys_w + 5);
                put(buf, inner.x + 3, y, &fit(&e.label, label_w, Align::Left), label_w, Style::new().fg(fg).bg(bg));
                let (k, note) = keys_text(&e.keys, &e.editor_keys);
                let x = inner.x + inner.width - 1 - keys_w as u16;
                let kc = if e.enabled { theme::ACCENT } else { theme::FG_DIM };
                let used = put(buf, x, y, &clip(&k, keys_w), keys_w, Style::new().fg(kc).bg(bg));
                if !note.is_empty() {
                    let off = if used > 0 { used + 1 } else { 0 };
                    let room = keys_w.saturating_sub(off as usize);
                    put(buf, x + off, y, &note, room, Style::new().fg(theme::FG_MUTED).bg(bg));
                }
            }
        }
    }
    filtering.then_some((cx, inner.y))
}
