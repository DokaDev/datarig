//! Overlays anchored in the workspace: the completion popup and the cell viewer.

use crate::app::App;
use crate::text::{Align, clip, fit, width, wrap};
use crate::theme;
use datarig_core::i18n::{Localized, Msg};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget};

/// Before drawing an overlay, blank any wide grapheme that starts just left of `rect` and
/// would otherwise straddle its left border (the terminal would erase the border cell).
pub(crate) fn clear_overlay(rect: Rect, buf: &mut Buffer) {
    if rect.x > buf.area.x {
        for y in rect.y..rect.y + rect.height {
            let cell = &mut buf[(rect.x - 1, y)];
            if width(cell.symbol()) > 1 {
                cell.set_symbol(" ");
            }
        }
    }
    Clear.render(rect, buf);
}

pub(crate) fn draw_popup(app: &App, cursor: (u16, u16), screen: Rect, buf: &mut Buffer) {
    let Some(p) = &app.tab().popup else { return };
    let kinds: Vec<Localized> = p.items.iter().map(|c| app.i18n.label(c.kind.label())).collect();
    let name_w = p.items.iter().map(|c| width(&c.label)).max().unwrap_or(1).min(40);
    let kind_w = kinds.iter().map(|k| width(k)).max().unwrap_or(0);
    let type_w = p.items.iter().filter_map(|c| c.detail.as_deref()).map(width).max().unwrap_or(0).min(24);
    let mut inner_w = 1 + name_w + 2 + kind_w + if type_w > 0 { 2 + type_w } else { 0 } + 1;
    inner_w = inner_w.min(screen.width as usize - 2);
    let w = inner_w as u16 + 2;
    let h = p.items.len() as u16 + 2;
    let prefix_w =
        width(&app.tab().editor.text()[p.replace_start.min(app.tab().editor.offset())..app.tab().editor.offset()])
            as u16;
    let x = cursor.0.saturating_sub(prefix_w + 2).min(screen.width.saturating_sub(w));
    let bottom = screen.height - 1; // keep the status bar visible
    let y = if cursor.1 + 1 + h <= bottom { cursor.1 + 1 } else { cursor.1.saturating_sub(h) };
    let rect = Rect::new(x, y, w, h.min(screen.height));
    clear_overlay(rect, buf);
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme::BORDER).bg(theme::SURFACE))
        .style(Style::new().bg(theme::SURFACE).fg(theme::FG))
        .render(rect, buf);
    for (i, c) in p.items.iter().enumerate() {
        let yy = rect.y + 1 + i as u16;
        if yy >= rect.y + rect.height - 1 {
            break;
        }
        let bg = if i == p.selected { theme::SELECTION_BG } else { theme::SURFACE };
        let mut line = vec![
            (" ".to_string(), Style::new()),
            (fit(&c.label, name_w, Align::Left), Style::new().fg(theme::FG)),
            ("  ".to_string(), Style::new()),
            (fit(&kinds[i], kind_w, Align::Left), Style::new().fg(theme::FG_DIM)),
        ];
        if type_w > 0 {
            line.push(("  ".into(), Style::new()));
            line.push((fit(c.detail.as_deref().unwrap_or(""), type_w, Align::Left), Style::new().fg(theme::FG_MUTED)));
        }
        line.push((" ".into(), Style::new()));
        let mut xx = rect.x + 1;
        let end = rect.x + rect.width - 1;
        for (s, st) in line {
            if xx >= end {
                break;
            }
            let room = (end - xx) as usize;
            let s = if width(&s) > room { fit(&s, room, Align::Left) } else { s };
            buf.set_stringn(xx, yy, &s, room, st.bg(bg));
            xx += width(&s) as u16;
        }
    }
}

pub(crate) fn draw_viewer(app: &mut App, content: Rect, buf: &mut Buffer) {
    let Some(v) = app.overlays.viewer_mut() else { return };
    // Only the box is cleared; the panels stay visible (dimmed) around it.
    let w = (content.width as u32 * 8 / 10) as u16;
    let h = (content.height as u32 * 8 / 10) as u16;
    let rect = Rect::new(content.x + (content.width - w) / 2, content.y + (content.height - h) / 2, w, h);
    clear_overlay(rect, buf);
    let title = app.i18n.msg(&Msg::CellViewerTitle { column: v.column.clone() });
    let close = app.i18n.label(datarig_core::i18n::Label::CellViewerClose);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme::ACCENT).bg(theme::SURFACE))
        .style(Style::new().bg(theme::SURFACE).fg(theme::FG))
        .title(Line::from(Span::styled(
            format!(" {} ", clip(&title, w as usize - 6)),
            Style::new().fg(theme::FG).add_modifier(Modifier::BOLD),
        )))
        .title_bottom(Line::from(Span::styled(format!(" {close} "), Style::new().fg(theme::FG_MUTED))).right_aligned());
    let inner = block.inner(rect);
    block.render(rect, buf);
    let text_w = inner.width.saturating_sub(2) as usize;
    let lines = wrap(&v.text, text_w);
    let vh = inner.height as usize;
    v.view_h = vh;
    v.scroll = v.scroll.min(lines.len().saturating_sub(vh));
    for (i, l) in lines.iter().skip(v.scroll).take(vh).enumerate() {
        buf.set_stringn(inner.x + 1, inner.y + i as u16, l, text_w, Style::new().fg(theme::FG).bg(theme::SURFACE));
    }
}
