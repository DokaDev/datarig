//! The query bar's rows popup, in the frame of its popups: the range and the page of the shown
//! result on its top border (`rows 1–500 · page 1 of ~101`), its four items (the previous and
//! the next page, the planner's estimate, the exact count), each dimmed with why where it
//! would be refused now, else with what it does; under a rule, the last count with its kind
//! and the paging's state in full, wrapped.

use super::lists::{finish, list_frame};
use crate::app::App;
use crate::app::filter_dialog::DialogFocus;
use crate::app::filter_dialog::view::{PopupKind, View};
use crate::text::width;
use crate::theme;
use crate::widgets::filter_dialog::wrap_at;
use crate::widgets::put;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// The width the popup wants at least, border included.
const WANT: usize = 48;

/// The most rows the states take.
const STATES_MAX: usize = 4;

/// Draw the open filter dialog's rows popup inside `bounds`, under the part of the bar it
/// opened on.
pub(crate) fn draw_rows(app: &mut App, bounds: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let d = app.filter_dialog()?;
    let View::Popup { kind: PopupKind::Rows, anchor } = d.view() else { return None };
    if bounds.width < 30 || bounds.height < 8 {
        return None;
    }
    let m = app.rows_popup()?;
    let th = theme::cur();
    let muted = Style::new().fg(th.fg_muted);
    let bold = Style::new().fg(th.fg).add_modifier(Modifier::BOLD);
    let text_w = m.items.iter().map(|l| width(&l.text)).max().unwrap_or(0);
    let note_w = m.items.iter().map(|l| width(l.note.as_ref().unwrap_or_else(|e| e))).max().unwrap_or(0);
    // The border, then a line: the marker, a blank, the item, two blanks, its note, a blank
    // on the right.
    let want = (2 + 3 + text_w + 2 + note_w + 1).max(WANT).max(width(&m.title) + 6);
    // The last count, then the states (each with its style), wrapped at width `w`: the count
    // whole, at most [`STATES_MAX`] rows of the states.
    let count_style = Style::new().fg(th.fg);
    let wrap = |s: &str, w: usize, style: Style| -> Vec<(String, Style)> {
        wrap_at(s, w.max(1)).into_iter().map(|r| (s[r].trim_end().to_string(), style)).collect()
    };
    let wrapped = |w: usize| -> Vec<(String, Style)> {
        let states =
            m.states.iter().flat_map(|(s, warn)| wrap(s, w, if *warn { Style::new().fg(th.warning) } else { muted }));
        wrap(&m.count, w, count_style).into_iter().chain(states.take(STATES_MAX)).collect()
    };
    let title = [(" ".to_string(), Style::new()), (m.title.clone(), bold), (" ".to_string(), Style::new())];
    let below = |w: usize| 1 + wrapped(w).len();
    let (mut hits, inner) = list_frame(app, anchor, &title, want, m.items.len(), &below, bounds, buf)?;
    let lines = wrapped(usize::from(inner.width.saturating_sub(2)));
    let d = app.filter_dialog()?;
    let x0 = inner.x + 1;
    let end = inner.right().saturating_sub(1);
    let focus_style = Style::new().fg(th.fg).patch(th.selection);
    let text_x = x0 + 1;
    let note_x = text_x + text_w as u16 + 2;
    let mut y = inner.y;
    for line in &m.items {
        if y >= inner.bottom() {
            break;
        }
        let r = Rect::new(inner.x, y, inner.width, 1);
        let focused = d.focus == DialogFocus::RowsItem(line.item);
        if focused {
            buf.set_style(r, focus_style);
            put(buf, inner.x, y, "\u{25b8}", 1, Style::new().fg(th.accent));
        }
        hits.rows_items.push((r, line.item));
        let (style, note, note_style) = match &line.note {
            Ok(n) => (if focused { bold } else { Style::new().fg(th.fg) }, n, muted),
            Err(why) => (Style::new().fg(th.fg_dim), why, Style::new().fg(th.fg_dim)),
        };
        put(buf, text_x, y, &line.text, text_w, style);
        put(buf, note_x, y, note, usize::from(end.saturating_sub(note_x)), note_style);
        y += 1;
    }
    // The rule, the last count and the states.
    if y < inner.bottom() {
        let w = usize::from(inner.width);
        put(buf, inner.x, y, &"\u{2500}".repeat(w), w, Style::new().fg(th.border));
        y += 1;
    }
    for (text, style) in &lines {
        if y >= inner.bottom() {
            break;
        }
        put(buf, x0, y, text, usize::from(end.saturating_sub(x0)), *style);
        y += 1;
    }
    finish(app, hits, 0, None, buf)
}
