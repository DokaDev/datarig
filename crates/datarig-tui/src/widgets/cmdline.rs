//! The `:` command line. By default a popup near the top of the screen, in the
//! style of noice.nvim's `cmdline_popup`: a box whose first line is the `:` input, the matching
//! entries listed below it. With `[commands] position = "bottom"` it is drawn Neovim style:
//! one input line at the very bottom of the screen (in place of the status bar) with the
//! entries rising above it. Each entry shows what it completes to, its localized name and its
//! keys on the right. The rows come from [`App::command_rows`]; the screen behind is dimmed.

use crate::app::App;
use crate::app::cmdline::CommandRow;
use crate::text::{Align, fit, width, wrap_words};
use crate::theme;
use crate::widgets::popup::clear_overlay;
use crate::widgets::put;
use crate::widgets::statusbar::level_color;
use datarig_core::config::CommandsPosition;
use datarig_core::i18n::Label;
use datarig_core::i18n::Localized;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Widget};

/// Most entries shown at once (the list scrolls).
const MAX_ROWS: usize = 12;
/// Widest "completes to" column; longer names end in `…`.
const NAME_MAX: usize = 32;

/// Draw the command line over the whole frame `area`; returns the hardware cursor.
pub(crate) fn draw_command_line(app: &mut App, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    if area.height < 4 || area.width < 8 {
        return None;
    }
    match app.prefs.commands_position {
        CommandsPosition::Popup => {
            let parts = Parts::of(app, popup_width(area).saturating_sub(6) as usize)?;
            let cursor = draw_popup(app, parts, area, buf);
            // The COMMAND badge stands out of the dimmed status bar.
            draw_badge(app, area.x, area.y + area.height - 1, buf);
            cursor
        }
        CommandsPosition::Bottom => {
            let parts = Parts::of(app, area.width.saturating_sub(4) as usize)?;
            draw_bottom(app, parts, area, buf)
        }
    }
}

/// What both forms draw.
struct Parts {
    rows: Vec<CommandRow>,
    title: Localized,
    keys_hint: Localized,
    placeholder: Localized,
    empty: Localized,
    /// Why the last `Enter` did nothing, wrapped, and its level.
    error: Option<(Vec<String>, crate::app::Level)>,
    /// A note on what is being typed (the `icons` setting's description), wrapped.
    note: Vec<String>,
}

impl Parts {
    /// The parts, with the note and the error wrapped to `text_w` columns (three lines each at
    /// most).
    fn of(app: &App, text_w: usize) -> Option<Self> {
        let c = app.overlays.command_line()?;
        let wrap3 = |t: &str| wrap_words(t, text_w).into_iter().take(3).collect::<Vec<_>>();
        Some(Parts {
            rows: app.command_rows(),
            title: app.i18n.label(Label::CommandsTitle),
            keys_hint: app.i18n.label(Label::CommandsKeys),
            placeholder: app.i18n.label(Label::CommandsPlaceholder),
            empty: app.i18n.label(Label::CommandsEmpty),
            error: c.error.as_ref().map(|e| (wrap3(&e.render(&app.i18n)), e.level)),
            note: app.command_note().map(|n| wrap3(&n)).unwrap_or_default(),
        })
    }
}

/// Width of the popup: 60% of the screen, 60 to 100 columns, and never wider than the screen
/// less a margin.
fn popup_width(area: Rect) -> u16 {
    (area.width * 6 / 10).clamp(60, 100).min(area.width.saturating_sub(4)).max(8)
}

/// The entry shown selected: none while a typed `:use` argument is what `Enter` runs.
fn selected_entry(app: &App) -> Option<usize> {
    let c = app.overlays.command_line()?;
    app.typed_context().is_none().then_some(c.selected)
}

/// The entries from `list_y` down, `n` of them, the selected one kept in view, across
/// `x..x + iw`; `bg` is the list's background.
#[allow(clippy::too_many_arguments)]
fn draw_entries(
    p: &Parts,
    selected: Option<usize>,
    x: u16,
    list_y: u16,
    iw: usize,
    n: usize,
    bg: ratatui::style::Color,
    buf: &mut Buffer,
) {
    if p.rows.is_empty() {
        put(buf, x + 2, list_y, &p.empty, iw.saturating_sub(4), Style::new().fg(theme::FG_DIM).bg(bg));
    }
    let name_w = p.rows.iter().map(|r| width(&r.name)).max().unwrap_or(0).min(NAME_MAX).min(iw / 2);
    let offset = selected.unwrap_or(0).saturating_sub(n - 1);
    for (row, (i, r)) in p.rows.iter().enumerate().skip(offset).take(n).enumerate() {
        let y = list_y + row as u16;
        let rbg = if Some(i) == selected { theme::SELECTION_BG } else { bg };
        buf.set_style(Rect::new(x, y, iw as u16, 1), Style::new().bg(rbg));
        let kw = width(&r.keys);
        let keys_room = if kw > 0 && kw + name_w + 12 <= iw { kw + 2 } else { 0 };
        let mut cx = x + 2;
        if name_w > 0 {
            let style = Style::new().fg(theme::ACCENT).bg(rbg).add_modifier(Modifier::BOLD);
            put(buf, cx, y, &fit(&r.name, name_w, Align::Left), name_w, style);
            cx += name_w as u16 + 2;
        }
        let label_w = (x as usize + iw).saturating_sub(cx as usize + keys_room + 1);
        put(buf, cx, y, &fit(&r.label, label_w, Align::Left), label_w, Style::new().fg(theme::FG).bg(rbg));
        if keys_room > 0 {
            let kx = x + iw as u16 - 1 - kw as u16;
            put(buf, kx, y, &r.keys, kw, Style::new().fg(theme::FG_MUTED).bg(rbg));
        }
    }
}

/// The note and the error under the entries, from `y` down.
fn draw_notes(p: &Parts, x: u16, mut y: u16, iw: usize, bg: ratatui::style::Color, buf: &mut Buffer) {
    for l in &p.note {
        put(buf, x + 2, y, l, iw.saturating_sub(4), Style::new().fg(theme::FG_MUTED).bg(bg));
        y += 1;
    }
    if let Some((lines, level)) = &p.error {
        for l in lines {
            put(buf, x + 2, y, l, iw.saturating_sub(4), Style::new().fg(level_color(*level)).bg(bg));
            y += 1;
        }
    }
}

impl Parts {
    /// Lines under the entries: the note and the error.
    fn extra(&self) -> u16 {
        (self.note.len() + self.error.as_ref().map_or(0, |(l, _)| l.len())) as u16
    }
}

/// The `:` prompt (at `pad` columns from `x`, the input one column after it), the input (or
/// what can be typed) and, with `hint`, the keys on the right, on the line `x..x + iw` at `y`;
/// returns the hardware cursor.
#[allow(clippy::too_many_arguments)]
fn draw_input(
    app: &mut App,
    p: &Parts,
    (x, y): (u16, u16),
    iw: usize,
    pad: u16,
    hint: bool,
    bg: ratatui::style::Color,
    buf: &mut Buffer,
) -> Option<(u16, u16)> {
    let c = app.overlays.command_line_mut()?;
    let line = Style::new().fg(theme::FG).bg(bg);
    buf.set_style(Rect::new(x, y, iw as u16, 1), line);
    buf.set_stringn(x + pad, y, ":", 1, Style::new().fg(theme::ACCENT).bg(bg).add_modifier(Modifier::BOLD));
    let hint_w = width(&p.keys_hint);
    let show_hint = hint && (c.input.text().is_empty() || iw >= 60 + hint_w);
    let hint_room = if show_hint && hint_w + 24 <= iw { hint_w + 2 } else { 0 };
    let start = 2 * pad + 1;
    let input_area = Rect::new(x + start, y, (iw - usize::from(start) - hint_room) as u16, 1);
    let cx = c.input.render(input_area, buf, line, true, false, None);
    if c.input.text().is_empty() {
        put(
            buf,
            input_area.x,
            y,
            &p.placeholder,
            input_area.width.saturating_sub(1) as usize,
            Style::new().fg(theme::FG_DIM).bg(bg),
        );
    }
    if hint_room > 0 {
        let hx = x + iw as u16 - hint_w as u16;
        put(buf, hx, y, &p.keys_hint, hint_w, Style::new().fg(theme::FG_MUTED).bg(bg));
    }
    Some((cx, y))
}

/// The popup: a box near the top, the input on its first line, a rule, then the entries, the
/// note and the error.
fn draw_popup(app: &mut App, p: Parts, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let w = popup_width(area);
    let x = area.x + (area.width - w) / 2;
    let top = area.y + (area.height / 6).max(1);
    let extra = p.extra();
    // Borders, input and rule take 4 lines; leave the last line of the screen alone.
    let room = (area.y + area.height).saturating_sub(top + 4 + extra + 1) as usize;
    let n = p.rows.len().clamp(1, MAX_ROWS.min(room.max(1)));
    let h = 4 + n as u16 + extra;
    let rect = Rect::new(x, top, w, h);
    clear_overlay(rect, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme::ACCENT).bg(theme::SURFACE))
        .style(Style::new().fg(theme::FG).bg(theme::SURFACE))
        .title(ratatui::text::Line::from(ratatui::text::Span::styled(
            format!(" {} ", p.title),
            Style::new().fg(theme::FG).add_modifier(Modifier::BOLD),
        )));
    // The keys on the bottom border, when they fit.
    let block = if width(&p.keys_hint) + 6 <= w as usize {
        let keys = ratatui::text::Span::styled(format!(" {} ", p.keys_hint), Style::new().fg(theme::FG_MUTED));
        block.title_bottom(ratatui::text::Line::from(keys).right_aligned())
    } else {
        block
    };
    let inner = block.inner(rect);
    block.render(rect, buf);
    let iw = inner.width as usize;
    let cursor = draw_input(app, &p, (inner.x, inner.y), iw, 1, false, theme::SURFACE, buf);
    // A rule under the input, joined to the box's sides.
    let rule = Style::new().fg(theme::ACCENT).bg(theme::SURFACE);
    buf.set_stringn(rect.x, inner.y + 1, format!("├{}┤", "─".repeat(iw)), iw + 2, rule);
    let selected = selected_entry(app);
    draw_entries(&p, selected, inner.x, inner.y + 2, iw, n, theme::SURFACE, buf);
    draw_notes(&p, inner.x, inner.y + 2 + n as u16, iw, theme::SURFACE, buf);
    cursor
}

/// Neovim style: the input on the last line, the entries rising above it.
fn draw_bottom(app: &mut App, p: Parts, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let max_rows = ((area.height as usize).saturating_sub(2) / 2).clamp(1, MAX_ROWS);
    let n = p.rows.len().clamp(1, max_rows);
    let extra = p.extra();
    let box_h = 1 + n as u16 + extra + 1;
    let top = area.y + area.height - box_h;
    let rect = Rect::new(area.x, top, area.width, box_h);
    clear_overlay(rect, buf);
    let surface = Style::new().fg(theme::FG).bg(theme::SURFACE);
    buf.set_style(rect, surface);
    let iw = area.width as usize;

    // Top edge with the title.
    let edge = Style::new().fg(theme::BORDER).bg(theme::SURFACE);
    buf.set_stringn(area.x, top, "─".repeat(iw), iw, edge);
    put(
        buf,
        area.x + 2,
        top,
        &format!(" {} ", p.title),
        iw.saturating_sub(4),
        Style::new().fg(theme::FG).bg(theme::SURFACE).add_modifier(Modifier::BOLD),
    );
    let selected = selected_entry(app);
    draw_entries(&p, selected, area.x, top + 1, iw, n, theme::SURFACE, buf);
    draw_notes(&p, area.x, top + 1 + n as u16, iw, theme::SURFACE, buf);
    // The COMMAND badge where the status bar starts, then the input.
    let y = area.y + area.height - 1;
    let bw = draw_badge(app, area.x, y, buf);
    draw_input(app, &p, (area.x + bw, y), iw - usize::from(bw), u16::from(bw > 0), true, theme::BG, buf)
}

/// The mode badge (COMMAND) at (x, y), as the status bar draws it; returns its width.
fn draw_badge(app: &App, x: u16, y: u16, buf: &mut Buffer) -> u16 {
    let Some((label, style)) = crate::widgets::statusbar::mode_badge(app) else { return 0 };
    let text = format!(" {label} ");
    put(buf, x, y, &text, width(&text), style)
}
