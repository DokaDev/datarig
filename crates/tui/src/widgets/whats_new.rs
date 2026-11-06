//! The What's new dialog ([`crate::app::whats_new`]): the notes, scrolled, with where the full
//! notes are and how to stop it showing by itself.

use crate::app::App;
use crate::app::overlay::OverlayKind;
use crate::app::whats_new::{Ink, layout};
use crate::text::{width, wrap_words};
use crate::theme;
use crate::widgets::dialog::{button_row, centered, modal};
use crate::widgets::put;
use datarig_core::i18n::{Label, Msg};
use datarig_core::update::install::WHATS_NEW_URL;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// The dialog is at most this wide.
const MAX_W: u16 = 100;

pub(crate) fn draw_whats_new(app: &mut App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    let when = (app.overlays.top().map(|o| o.kind()) == Some(OverlayKind::WhatsNew), app.now());
    let Some(v) = app.overlays.whats_new() else { return };
    let w = area.width.saturating_sub(4).min(MAX_W);
    let iw = (w as usize).saturating_sub(4);
    let surface = |fg| Style::new().fg(fg).bg(th.surface);
    let ink = |i: Ink| match i {
        Ink::Version => surface(th.accent).add_modifier(Modifier::BOLD),
        Ink::Date => surface(th.fg_dim),
        Ink::Heading | Ink::Bold => surface(th.fg).add_modifier(Modifier::BOLD),
        Ink::Text => surface(th.fg),
        Ink::Code => surface(th.accent_warm),
    };
    let (rows, heads) = layout(&v.sections, iw);
    let foot: Vec<String> = [
        app.i18n.msg(&Msg::WhatsNewMore { url: WHATS_NEW_URL.to_string() }).to_string(),
        app.i18n.label(Label::WhatsNewOptOut).to_string(),
    ]
    .iter()
    .flat_map(|t| wrap_words(t, iw))
    .collect();
    let title = v.title.clone();
    let keys = app.i18n.label(Label::WhatsNewKeys);
    // Border (2), the notes, a blank line, the footer, a blank line, the button.
    let fixed = 5 + foot.len();
    let height = area.height.saturating_sub(4).max(fixed as u16 + 1);
    let shown = (height as usize).saturating_sub(fixed).max(1);
    let rect = centered(area, w, (fixed + shown) as u16);
    let inner = modal(rect, &title, &keys, buf);
    let x = inner.x + 1;
    let end = inner.x + inner.width.saturating_sub(1);
    let max_scroll = rows.len().saturating_sub(shown);
    // Opened at a version: its heading at the top, once.
    let start = v.start.and_then(|s| v.sections.iter().position(|x| x.version == s)).and_then(|i| heads.get(i));
    let scroll = start.copied().unwrap_or(v.scroll).min(max_scroll);
    let mut y = inner.y;
    for (k, row) in rows.iter().skip(scroll).take(shown).enumerate() {
        let mut cx = x;
        for (text, i) in row {
            if cx >= end {
                break;
            }
            cx += put(buf, cx, y + k as u16, text, (end - cx) as usize, ink(*i)) as u16;
        }
    }
    // Which rows show, when not all do, at the right end of the blank row below the notes.
    y += shown as u16;
    if rows.len() > shown {
        let (from, to) = (scroll + 1, (scroll + shown).min(rows.len()));
        let m = Msg::WhatsNewPosition { from: from.to_string(), to: to.to_string(), total: rows.len().to_string() };
        let pos = app.i18n.msg(&m).to_string();
        let cx = end.saturating_sub(width(&pos) as u16).max(x);
        put(buf, cx, y, &pos, (end - cx) as usize, surface(th.fg_dim));
    }
    y += 1;
    for line in &foot {
        put(buf, x, y, line, (end - x) as usize, surface(th.fg_muted));
        y += 1;
    }
    y += 1;
    let labels = [app.i18n.label(Label::DialogButtonClose)];
    let Some(v) = app.overlays.whats_new_mut() else { return };
    (v.scroll, v.max_scroll, v.page, v.heads, v.start) = (scroll, max_scroll, shown, heads, None);
    button_row(buf, inner, y, &labels, Some(0), &mut v.buttons, when);
}
