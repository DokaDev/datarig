//! The quick connect list (`Ctrl+O`): a filter line and the matching profiles with their state,
//! icon, name and folder.

use crate::app::App;
use crate::app::quick::{QuickNote, QuickPurpose, QuickRow};
use crate::icons;
use crate::text::{Align, fit};
use crate::theme;
use crate::widgets::dialog::{centered, modal};
use crate::widgets::explorer::{CONNECTED, DISCONNECTED, FAILED};
use crate::widgets::{put, spinner_at};
use datarig_core::i18n::Label;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// Draw the quick connect list; returns the hardware cursor (its input).
pub(crate) fn draw_quick_connect(app: &mut App, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let q = app.overlays.quick()?;
    let title = app.i18n.label(match q.purpose {
        _ if q.context_of.is_some() => Label::QuickTitleContext,
        QuickPurpose::Bind { .. } => Label::QuickTitleBind,
        _ => Label::QuickTitle,
    });
    let footer = app.i18n.label(Label::QuickKeys);
    let w = area.width.saturating_sub(8).min(72);
    let rows = q.items.len().clamp(1, (area.height as usize).saturating_sub(6).max(1));
    let rect = centered(area, w, rows as u16 + 4);
    let inner = modal(rect, &title, &footer, buf);
    let iw = inner.width as usize;
    let (items, selected) = (q.items.clone(), q.selected);
    put(
        buf,
        inner.x + 1,
        inner.y,
        "›",
        1,
        Style::new().fg(theme::ACCENT).bg(theme::SURFACE).add_modifier(Modifier::BOLD),
    );
    let input = Rect::new(inner.x + 3, inner.y, inner.width.saturating_sub(4), 1);
    let placeholder = app.i18n.label(Label::QuickPlaceholder);
    let icons_on = app.icons_on();
    let (open, open_db) = (q.open.clone(), q.open_db.clone());
    let arrow = |on: bool| if on { "▾ " } else { "▸ " };
    let dim = Style::new().fg(theme::FG_DIM);
    let lines: Vec<Vec<(String, Style)>> = items
        .iter()
        .map(|row| match row {
            QuickRow::Profile(id) => {
                let Some(p) = app.profile(*id) else { return Vec::new() };
                let color = theme::profile_color(p.display_color());
                let (mark, mark_color) = match app.conns.state(*id) {
                    crate::app::NodeState::Disconnected => (DISCONNECTED, color),
                    crate::app::NodeState::Connecting => (
                        app.conns
                            .get(*id)
                            .and_then(|c| c.connecting.as_ref())
                            .map_or("⠋", |c| spinner_at(c.started, app.now())),
                        theme::ACCENT,
                    ),
                    crate::app::NodeState::Connected => (CONNECTED, color),
                    crate::app::NodeState::Failed => (FAILED, theme::ERROR),
                };
                vec![
                    (arrow(open.contains(id)).to_string(), dim),
                    (format!("{mark} "), Style::new().fg(mark_color)),
                    (icons::cell(p, icons_on), Style::new().fg(color)),
                    (p.name.clone(), Style::new().fg(theme::FG)),
                    (p.folder.as_deref().map(|f| format!("  {f}/")).unwrap_or_default(), dim),
                ]
            }
            QuickRow::Database(id, db) => vec![
                ("    ".to_string(), dim),
                (arrow(open_db.contains(&(*id, db.clone()))).to_string(), dim),
                (db.clone(), Style::new().fg(theme::FG)),
            ],
            QuickRow::Schema(_, _, schema) => {
                vec![("        ".to_string(), dim), (schema.clone(), Style::new().fg(theme::FG))]
            }
            QuickRow::Note(_, db, note) => {
                let text = match note {
                    QuickNote::Loading => app.i18n.label(Label::TreeLoading).to_string(),
                    QuickNote::Empty => app.i18n.label(Label::TreeEmpty).to_string(),
                    QuickNote::Failed(e) => e.clone(),
                };
                let indent = if db.is_some() { "        " } else { "    " };
                let style = if matches!(note, QuickNote::Failed(_)) { Style::new().fg(theme::ERROR) } else { dim };
                vec![(indent.to_string(), dim), (text, style)]
            }
        })
        .collect();
    let first = selected.saturating_sub(rows - 1);
    for (row, parts) in lines.iter().enumerate().skip(first).take(rows) {
        let y = inner.y + 2 + (row - first) as u16;
        let bg = if row == selected { theme::SELECTION_BG } else { theme::SURFACE };
        buf.set_stringn(inner.x, y, fit("", iw, Align::Left), iw, Style::new().bg(bg));
        let mut x = inner.x + 1;
        for (text, style) in parts {
            let room = (inner.x + inner.width).saturating_sub(x + 1) as usize;
            x += put(buf, x, y, text, room, style.bg(bg));
        }
    }
    if items.is_empty() {
        let none = app.i18n.label(Label::QuickNone);
        put(
            buf,
            inner.x + 1,
            inner.y + 2,
            &none,
            iw.saturating_sub(2),
            Style::new().fg(theme::FG_DIM).bg(theme::SURFACE),
        );
    }
    let q = app.overlays.quick_mut()?;
    let cx = q.input.render(input, buf, Style::new().fg(theme::FG).bg(theme::SURFACE), true, false, None);
    if q.input.text().is_empty() {
        put(
            buf,
            input.x,
            input.y,
            &placeholder,
            input.width as usize,
            Style::new().fg(theme::FG_DIM).bg(theme::SURFACE),
        );
    }
    Some((cx, inner.y))
}
