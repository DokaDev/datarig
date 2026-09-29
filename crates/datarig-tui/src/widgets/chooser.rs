//! The chooser (a list to pick from, with a `/` filter) and the name input dialog.

use crate::app::App;
use crate::app::chooser::ChooserPurpose;
use crate::app::profiles::Field;
use crate::icons;
use crate::text::{Align, fit};
use crate::theme;
use crate::widgets::dialog::{centered, modal};
use crate::widgets::put;
use datarig_core::i18n::Label;
use datarig_core::profile::color::ProfileColor;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// Draw the chooser; returns the hardware cursor while its filter is typed.
pub(crate) fn draw_chooser(app: &mut App, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let icons_on = app.icons_on();
    let c = app.overlays.chooser()?;
    let title = app.i18n.label(c.title);
    let footer = app.i18n.label(Label::ChooserKeys);
    let visible = c.visible();
    let w = area.width.saturating_sub(8).min(56);
    let rows = visible.len().clamp(1, (area.height as usize).saturating_sub(6).max(1));
    let rect = centered(area, w, rows as u16 + 4);
    let inner = modal(rect, &title, &footer, buf);
    let iw = inner.width as usize;
    let filter_style = Style::new().fg(theme::FG).bg(theme::SURFACE);
    put(
        buf,
        inner.x + 1,
        inner.y,
        "/",
        1,
        Style::new().fg(theme::ACCENT).bg(theme::SURFACE).add_modifier(Modifier::BOLD),
    );
    let first = c.selected.saturating_sub(rows - 1);
    for (row, &i) in visible.iter().enumerate().skip(first).take(rows) {
        let y = inner.y + 2 + (row - first) as u16;
        let bg = if row == c.selected { theme::SELECTION_BG } else { theme::SURFACE };
        buf.set_stringn(inner.x, y, fit("", iw, Align::Left), iw, Style::new().bg(bg));
        let (value, text) = &c.items[i];
        // What the item looks like: the color's dot, the icon's glyph.
        let lead = match (&c.purpose, value) {
            (ChooserPurpose::Form(Field::Color), Some(v)) => {
                ProfileColor::parse(v).map(|pc| ("● ", theme::profile_color(pc)))
            }
            (ChooserPurpose::Form(Field::Color), None) => Some(("○ ", theme::FG_MUTED)),
            _ => None,
        };
        let mut x = inner.x + 1;
        if let Some((dot, color)) = lead {
            x += put(buf, x, y, dot, 2, Style::new().fg(color).bg(bg));
        }
        if let (ChooserPurpose::Form(Field::Icon), true) = (&c.purpose, icons_on) {
            let glyph = value.as_deref().and_then(icons::by_name).unwrap_or(icons::UNKNOWN_DRIVER);
            x += put(buf, x, y, &format!("{glyph} "), 2, Style::new().fg(theme::FG).bg(bg));
        }
        put(buf, x, y, text, (inner.x + inner.width).saturating_sub(x + 1) as usize, Style::new().fg(theme::FG).bg(bg));
    }
    if visible.is_empty() {
        let none = app.i18n.label(Label::ChooserNone);
        put(
            buf,
            inner.x + 1,
            inner.y + 2,
            &none,
            iw.saturating_sub(2),
            Style::new().fg(theme::FG_DIM).bg(theme::SURFACE),
        );
    }
    let c = app.overlays.chooser_mut()?;
    let filtering = c.filtering;
    let input = Rect::new(inner.x + 3, inner.y, inner.width.saturating_sub(4), 1);
    let cx = c.filter.render(input, buf, filter_style, filtering, false, None);
    filtering.then_some((cx, inner.y))
}

/// Draw the name input; returns the hardware cursor (its input).
pub(crate) fn draw_name_input(app: &mut App, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let n = app.overlays.name_input()?;
    let title = app.i18n.msg(&n.title);
    let footer = app.i18n.label(Label::NameKeys);
    let error = n.error.map(|e| app.i18n.label(e));
    let w = area.width.saturating_sub(8).min(56);
    // A long error wraps (up to three lines) and the box grows for it.
    let lines = error.as_ref().map(|e| crate::text::wrap_words(e, (w as usize).saturating_sub(4))).unwrap_or_default();
    let lines = &lines[..lines.len().min(3)];
    let rect = centered(area, w, 5 + lines.len().saturating_sub(1) as u16);
    let inner = modal(rect, &title, &footer, buf);
    for (i, l) in lines.iter().enumerate() {
        put(
            buf,
            inner.x + 1,
            inner.y + 2 + i as u16,
            l,
            (inner.width as usize).saturating_sub(2),
            Style::new().fg(theme::ERROR).bg(theme::SURFACE),
        );
    }
    let n = app.overlays.name_input_mut()?;
    let input = Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), 1);
    let cx = n.input.render(input, buf, Style::new().fg(theme::FG).bg(theme::SELECTION_BG), true, false, None);
    Some((cx, inner.y))
}
