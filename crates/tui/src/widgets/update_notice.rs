//! The update notice ([`crate::app::updates`]): the new version, the command that updates this
//! install, where What's new is, and how to turn the checks off.

use crate::app::App;
use crate::app::overlay::OverlayKind;
use crate::text::{width, wrap_words};
use crate::theme;
use crate::widgets::dialog::{button_row, centered, modal};
use crate::widgets::put;
use datarig_core::update::install::{BREW_COMMAND, Install, RELEASES_URL, SOURCE_COMMANDS, WHATS_NEW_URL};
use datarig_i18n::{Label, Msg};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// The notice is at most this wide.
const MAX_W: u16 = 72;

pub(crate) fn draw_update_notice(app: &mut App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    let when = (app.overlays.top().map(|o| o.kind()) == Some(OverlayKind::UpdateNotice), app.now());
    let Some(n) = app.overlays.update_notice() else { return };
    let w = area.width.saturating_sub(4).min(MAX_W);
    let iw = (w as usize).saturating_sub(4);
    let surface = |fg| Style::new().fg(fg).bg(th.surface);
    let (plain, muted, strong, code) =
        (surface(th.fg), surface(th.fg_muted), surface(th.fg).add_modifier(Modifier::BOLD), surface(th.accent_warm));
    let mut rows: Vec<Vec<(String, Style)>> = Vec::new();
    let text = |t: &str, style: Style, rows: &mut Vec<Vec<(String, Style)>>| {
        rows.extend(wrap_words(t, iw).into_iter().map(|r| vec![(r, style)]));
    };
    let out = app.i18n.msg(&Msg::UpdateNoticeOut { version: n.version.to_string(), running: n.running.to_string() });
    text(&out.to_string(), strong, &mut rows);
    if let Some(s) = &n.summary {
        text(s, plain, &mut rows);
    }
    rows.push(Vec::new());
    // A label, then its values lined up after it; a value wraps under itself, at spaces only.
    let field = |label: &str, values: &[(String, Style)], rows: &mut Vec<Vec<(String, Style)>>| {
        let lw = width(label) + 2;
        for (i, (v, style)) in values.iter().enumerate() {
            for (j, part) in wrap_words(v, iw.saturating_sub(lw).max(8)).into_iter().enumerate() {
                let lead = if i == 0 && j == 0 { format!("{label}  ") } else { " ".repeat(lw) };
                rows.push(vec![(lead, muted), (part, *style)]);
            }
        }
    };
    let commands: Vec<(String, Style)> = match n.install {
        Install::Homebrew => vec![(BREW_COMMAND.to_string(), code)],
        Install::Source => SOURCE_COMMANDS.iter().map(|c| (c.to_string(), code)).collect(),
        Install::Other => {
            vec![(app.i18n.msg(&Msg::UpdateNoticeDownload { url: RELEASES_URL.to_string() }).to_string(), plain)]
        }
    };
    let to_update = app.i18n.label(Label::UpdateNoticeToUpdate).to_string();
    let whats_new = app.i18n.label(Label::UpdateNoticeWhatsNew).to_string();
    // Both labels as wide as the wider one, so the values line up.
    let lw = width(&to_update).max(width(&whats_new));
    let pad = |l: &str| format!("{l}{}", " ".repeat(lw - width(l)));
    field(&pad(&to_update), &commands, &mut rows);
    field(&pad(&whats_new), &[(WHATS_NEW_URL.to_string(), plain)], &mut rows);
    rows.push(Vec::new());
    text(&app.i18n.label(Label::UpdateNoticeOptOut).to_string(), muted, &mut rows);
    let title = app.i18n.label(Label::UpdateNoticeTitle);
    let keys = app.i18n.label(Label::UpdateNoticeKeys);
    let labels = [
        app.i18n.label(Label::DialogButtonClose),
        app.i18n.label(Label::UpdateNoticeSkip),
        app.i18n.label(Label::UpdateNoticeTurnOff),
    ];
    let focus = n.focus;
    // Border (2), the rows, a blank line, the buttons.
    let shown = rows.len().min((area.height as usize).saturating_sub(4));
    let rect = centered(area, w, (shown + 4) as u16);
    let inner = modal(rect, &title, &keys, buf);
    let x = inner.x + 1;
    let end = inner.x + inner.width.saturating_sub(1);
    for (k, row) in rows.iter().take(shown).enumerate() {
        let mut cx = x;
        for (t, style) in row {
            if cx >= end {
                break;
            }
            cx += put(buf, cx, inner.y + k as u16, t, (end - cx) as usize, *style) as u16;
        }
    }
    let y = inner.y + shown as u16 + 1;
    let Some(n) = app.overlays.update_notice_mut() else { return };
    button_row(buf, inner, y, &labels, Some(focus), &mut n.buttons, when);
}
