//! The condition popup of the query bar: one column's conditions, values and sort, as the
//! filter dialog's pane draws them ([`draw_condition_pane`]), in a small box under the
//! condition it opened on, or under the column's header ([`frame`]). Its border names the column (and its type, muted), says
//! where the conditions are when they scroll, and holds the hint line: what the dialog says,
//! else the keys of the focused line, with `⋯ Advanced` on its right, which turns the popup
//! into the whole dialog (as `A` does). Opened on the column from the grid (`F`, a right click
//! on its header) it has lines above the pane, a rule under them: hiding the column, and on a
//! cell only its value or everything but it, each dimmed with why when it cannot run.

use super::frame::{anchored, frame};
use crate::app::App;
use crate::app::filter_dialog::list::ValuesView;
use crate::app::filter_dialog::view::{PopupKind, View};
use crate::app::filter_dialog::{DialogFocus, DialogHit, DialogHits};
use crate::app::overlay::OverlayKind;
use crate::text::width;
use crate::theme;
use crate::widgets::condition_pane::{PaneOpts, draw_condition_pane, draw_op_picker};
use crate::widgets::filter_dialog::{hint_parts, short_type};
use crate::widgets::put;
use crate::widgets::statusbar::level_color;
use datarig_i18n::Label;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// The narrowest and the widest the popup is (with its border).
const MIN_W: u16 = 40;
const MAX_W: u16 = 60;

/// The most values the popup lists before the list scrolls.
const VALUES_MAX: usize = 8;

/// The columns the sort line takes with its short words (`Sort  (•) none  ( ) ↑ asc  ( ) ↓
/// desc`), and a condition's line short of its value (marker, operator box, `×`).
const SORT_W: usize = 44;
const CONDITION_W: usize = 22;

/// The columns the pane wants for view `v`'s values (box, value, count, bar).
fn values_width(v: &ValuesView) -> usize {
    let name = v.items.iter().map(|it| it.value.as_deref().map_or(4, width)).max().unwrap_or(0).min(24);
    let count = v.items.iter().map(|it| crate::text::width(&datarig_i18n::fmt_count(it.rows)) + 1).max().unwrap_or(0);
    6 + name + 2 + count + 2 + 12
}

/// The lines under view `v`'s values: what is not listed.
fn notes(v: &ValuesView) -> usize {
    let mut n = if v.range.is_some() { 2 } else { 0 };
    n += usize::from(v.items.is_empty() && v.range.is_none() && v.narrow.is_some());
    n + usize::from(v.long > 0) + usize::from(v.more) + usize::from(v.kept_expr)
}

/// Draw the open filter dialog's condition popup inside `bounds`, under the part of the query
/// bar or the column's header it opened on (where it is drawn now, else where it was). Returns
/// the hardware cursor.
pub(crate) fn draw_condition_popup(app: &mut App, bounds: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let view = app.filter_dialog_list();
    let d = app.filter_dialog()?;
    let View::Popup { kind: PopupKind::Condition { col }, anchor } = d.view() else { return None };
    if bounds.width < 30 || bounds.height < 6 {
        return None;
    }
    // Under its part of the bar, or under its column's header (one opened there has no part).
    let at = match anchor.seg {
        Some(s) => app.layout.qbar.iter().find(|(_, h)| *h == s).map(|(r, _)| *r),
        None => app.popup_header(d.tab, col),
    };
    let at = at.unwrap_or(anchor.at);
    let th = theme::cur();
    let i18n = &app.i18n;
    let muted = Style::new().fg(th.fg_muted);
    let bold = Style::new().fg(th.fg).add_modifier(Modifier::BOLD);
    let c = d.column()?;
    let name = crate::text::escape_controls(&c.name);
    let ty = short_type(d.dialect, &c.type_name);

    // As wide as its parts want, within its bounds; as tall as its lines.
    let field = d
        .rows()
        .iter()
        .filter_map(|&t| d.draft.where_.terms()[t].row.as_ref())
        .map(|r| width(&r.input_text(d.dialect, &d.column_info(&r.col))) + 2)
        .max()
        .unwrap_or(0)
        .max(14);
    // The lines above the pane: label, and the key that runs it or (dimmed) why it cannot.
    let above = app.column_lines_shown();
    let lines_w = above.iter().map(|(_, a, b, _)| width(a) + width(b) + 6).max().unwrap_or(0);
    let want =
        [SORT_W, CONDITION_W + field, view.as_ref().map_or(0, values_width), width(&name) + width(&ty) + 4, lines_w]
            .into_iter()
            .max()
            .unwrap_or(0);
    // The hint line: what the dialog says, else the keys; `⋯ Advanced` on its right, a blank on
    // each side (the keys of the hint line widen the popup as its parts do).
    let advanced = format!(" \u{22ef} {} ", i18n.label(Label::TableBarAdvanced));
    let adv_w = width(&advanced) as u16;
    let keys_w: usize = hint_parts(app, d, view.as_ref(), usize::MAX).iter().map(|(t, _)| width(t)).sum();
    // Never narrower than it was drawn in this view: the hint line changes with the keys.
    let w = (want as u16 + 4).max(keys_w as u16 + adv_w + 5).clamp(MIN_W, MAX_W).max(d.popup_w);
    let lines = d.rows().len() + 1;
    let values = view.as_ref().map_or(0, |v| 1 + v.items.len().min(VALUES_MAX) + notes(v));
    let top_h = if above.is_empty() { 0 } else { above.len() + 1 };
    let inner_h = top_h + lines + if values > 0 { 1 + values } else { 0 } + 2;
    let rect = anchored(at, w, inner_h as u16 + 2, bounds);
    let adv_x = rect.right().saturating_sub(2 + adv_w);
    let hint_w = usize::from(adv_x.saturating_sub(rect.x + 3));
    let said = d.message.clone().or_else(|| d.applying.as_ref().and(app.status.clone()));
    let mut hint = vec![(" ".to_string(), Style::new())];
    match said {
        Some(n) => hint.push((crate::text::clip(&n.render(i18n), hint_w), Style::new().fg(level_color(n.level)))),
        None => hint.extend(hint_parts(app, d, view.as_ref(), hint_w)),
    }
    hint.push((" ".to_string(), Style::new()));
    let title =
        [(" ".to_string(), Style::new()), (name, bold), ("  ".to_string(), Style::new()), (format!("{ty} "), muted)];
    let inner = frame(buf, rect, &title, &hint);
    if inner.width < 4 || inner.height < 2 {
        return None;
    }
    let mut hits = DialogHits { frame: rect, ..DialogHits::default() };
    let by = rect.bottom() - 1;
    put(buf, adv_x, by, &advanced, usize::from(adv_w), Style::new().fg(th.fg));
    hits.advanced = Rect::new(adv_x, by, adv_w, 1);

    // The lines above the pane, the focused one lit, a rule under them.
    let top_h = (top_h as u16).min(inner.height.saturating_sub(1));
    for (i, (l, label, right, dim)) in above.iter().enumerate().take(usize::from(top_h.saturating_sub(1))) {
        let y = inner.y + i as u16;
        let row = Rect::new(inner.x, y, inner.width, 1);
        let base =
            if d.focus == DialogFocus::Line(*l) { Style::new().fg(th.fg).patch(th.selection) } else { th.base() };
        crate::widgets::condition_pane::fill(buf, row, base);
        let fg = if *dim { muted } else { Style::new().fg(th.fg) };
        // The marker of the focused line, where the pane's lines have theirs.
        if d.focus == DialogFocus::Line(*l) {
            put(buf, inner.x + 1, y, "\u{25b8}", 1, base.patch(Style::new().fg(th.accent)));
        }
        let rw = width(right) as u16;
        let room = usize::from(inner.width.saturating_sub(rw + 5));
        put(buf, inner.x + 3, y, label, room, base.patch(fg));
        put(buf, inner.right().saturating_sub(rw + 1), y, right, usize::from(rw), base.patch(muted));
        hits.lines.push((row, *l));
    }
    if top_h > 0 {
        let rule = "\u{2500}".repeat(usize::from(inner.width));
        put(buf, inner.x, inner.y + top_h - 1, &rule, usize::from(inner.width), Style::new().fg(th.border));
    }
    // The pane, a blank column on each side.
    let pane = Rect::new(inner.x + 1, inner.y + top_h, inner.width - 2, inner.height - top_h);
    let out = draw_condition_pane(
        app,
        d,
        view.as_ref(),
        pane,
        PaneOpts { title_in_border: true, compact: true },
        buf,
        &mut hits,
    );
    // Where the conditions are when they scroll: on the top border's right.
    if let Some((from, to, n)) = out.rows_window {
        let cue = format!(" {from}-{to}/{n} ");
        let cw = width(&cue) as u16;
        put(buf, rect.right().saturating_sub(2 + cw), rect.y, &cue, usize::from(cw), muted);
    }
    // The operator picker, opaque, anywhere in the bounds.
    if let Some(p) = out.picker_at {
        draw_op_picker(app, d, p, bounds, buf, &mut hits);
    }

    // What the pointer is over, lit (over the picker, only its entries).
    let lit = d.hover.filter(|h| d.picker.is_none() || matches!(h, DialogHit::Pick(_)));
    let lit = lit.and_then(|h| hits.rect(h).map(|r| (h, r)));
    if let Some((h, r)) = lit {
        buf.set_style(r, if h.row() { crate::widgets::hover_style() } else { crate::widgets::pointer_style() });
    }
    let top = app.overlays.top().is_some_and(|o| o.kind() == OverlayKind::FilterDialog);
    let now = app.now();
    if let Some(d) = app.filter_dialog_mut() {
        d.hits = hits;
        d.values_scroll = out.values_scroll;
        d.rows_scroll = out.rows_scroll;
        d.set_values_drawn(out.values_drawn);
        d.hover = lit.map(|(h, _)| h);
        d.press.drawn(top, now);
        d.popup_w = w;
    }
    out.cursor.filter(|_| top)
}
