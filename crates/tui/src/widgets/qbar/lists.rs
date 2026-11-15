//! The query bar's list popups, in the frame of its popups ([`frame`]): the add popup
//! ([`draw_add_pick`]: the column of a new condition, one a line with its filter mark, its key
//! icon in the key's colour and its short type, the search on top while it is open), the
//! WHERE list ([`draw_where_list`]: the conditions one a line with their `×`, then `+ add
//! condition`) and the columns popup ([`draw_columns`]: each column shown or hidden). Their
//! borders hold the title, where the list is when it scrolls, the hint line and `⋯ Advanced`;
//! the order and rows popups share the frame.

use super::frame::{anchored, frame};
use crate::app::App;
use crate::app::filter_dialog::view::{Anchor, PopupKind, View};
use crate::app::filter_dialog::{DialogFocus, DialogHits};
use crate::app::overlay::OverlayKind;
use crate::icons;
use crate::text::{clip, escape_controls, width};
use crate::theme;
use crate::widgets::filter_dialog::{hint_parts, short_type};
use crate::widgets::put;
use crate::widgets::statusbar::level_color;
use datarig_i18n::{Label, Msg};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// The narrowest and the widest a list popup is (with its border).
const MIN_W: u16 = 40;
const MAX_W: u16 = 64;

/// The most lines a list popup shows before it scrolls.
pub(super) const LINES_MAX: usize = 12;

/// The most columns a column's name and its type take in a list of columns.
pub(super) const NAME_MAX: usize = 28;
pub(super) const TYPE_MAX: usize = 20;

/// Where the popup is placed by: the part of the bar it opened on where it is drawn now, else
/// where it was.
fn anchor_at(app: &App, anchor: &Anchor) -> Rect {
    anchor.seg.and_then(|s| app.layout.qbar.iter().find(|(_, h)| *h == s)).map_or(anchor.at, |(r, _)| *r)
}

/// The popup's frame at `w` (wider when its hint line wants it) by `lines` (at most
/// [`LINES_MAX`] of them, then the rows `fixed` says it takes below them at the width of text
/// inside the frame, a blank kept on both sides) inside `bounds`, its title `title` and its hint
/// line (what the dialog says, else the keys) with `⋯ Advanced` on its right. Returns what is
/// drawn for the pointer (the box and `⋯ Advanced`) and the cells inside the border; `None`
/// when it does not fit.
#[allow(clippy::too_many_arguments)]
pub(super) fn list_frame(
    app: &App,
    anchor: &Anchor,
    title: &[(String, Style)],
    w: usize,
    lines: usize,
    fixed: &dyn Fn(usize) -> usize,
    bounds: Rect,
    buf: &mut Buffer,
) -> Option<(DialogHits, Rect)> {
    let d = app.filter_dialog()?;
    let th = theme::cur();
    let i18n = &app.i18n;
    let advanced = format!(" \u{22ef} {} ", i18n.label(Label::TableBarAdvanced));
    let adv_w = width(&advanced) as u16;
    // As wide as the hint line the widest popup would keep (a long one keeps fewer keys).
    let most = usize::from(MAX_W.saturating_sub(adv_w + 7));
    let keys_w: usize = hint_parts(app, d, None, most).iter().map(|(t, _)| width(t)).sum();
    let title_w: usize = title.iter().map(|(t, _)| width(t)).sum();
    let w = (w as u16).max(keys_w as u16 + adv_w + 7).max(title_w as u16 + 4).clamp(MIN_W, MAX_W);
    let below = fixed(usize::from(w.min(bounds.width).saturating_sub(4)));
    let rect = anchored(anchor_at(app, anchor), w, (lines.min(LINES_MAX) + below) as u16 + 2, bounds);
    let adv_x = rect.right().saturating_sub(2 + adv_w);
    // A dash of the border kept between the hint line and `⋯ Advanced`.
    let hint_w = usize::from(adv_x.saturating_sub(rect.x + 5));
    let mut hint = vec![(" ".to_string(), Style::new())];
    match d.message.clone() {
        Some(n) => hint.push((clip(&n.render(i18n), hint_w), Style::new().fg(level_color(n.level)))),
        None => hint.extend(hint_parts(app, d, None, hint_w)),
    }
    hint.push((" ".to_string(), Style::new()));
    let inner = frame(buf, rect, title, &hint);
    if inner.width < 6 || inner.height < 1 {
        return None;
    }
    let by = rect.bottom() - 1;
    put(buf, adv_x, by, &advanced, usize::from(adv_w), Style::new().fg(th.fg));
    let hits = DialogHits { frame: rect, advanced: Rect::new(adv_x, by, adv_w, 1), ..DialogHits::default() };
    Some((hits, inner))
}

/// The first of `n` lines shown in `rows` with line `at` in view, from `scroll`.
pub(super) fn window(scroll: usize, at: usize, n: usize, rows: usize) -> usize {
    let scroll = scroll.min(n.saturating_sub(rows));
    if at < scroll {
        at
    } else if rows > 0 && at >= scroll + rows {
        at + 1 - rows
    } else {
        scroll
    }
}

/// `i/n` on the top border's right, where the list is when it scrolls.
pub(super) fn scroll_cue(buf: &mut Buffer, rect: Rect, at: usize, n: usize) {
    let cue = format!(" {}/{n} ", at + 1);
    let cw = width(&cue) as u16;
    put(
        buf,
        rect.right().saturating_sub(2 + cw),
        rect.y,
        &cue,
        usize::from(cw),
        Style::new().fg(theme::cur().fg_muted),
    );
}

/// What was drawn kept for the pointer and the keys, what the pointer is over lit, and the
/// hardware cursor (`cursor`) while the dialog is on top.
pub(super) fn finish(
    app: &mut App,
    hits: DialogHits,
    scroll: usize,
    cursor: Option<(u16, u16)>,
    buf: &mut Buffer,
) -> Option<(u16, u16)> {
    let d = app.filter_dialog()?;
    let lit = d.hover.and_then(|h| hits.rect(h).map(|r| (h, r)));
    if let Some((h, r)) = lit {
        buf.set_style(r, if h.row() { crate::widgets::hover_style() } else { crate::widgets::pointer_style() });
    }
    let top = app.overlays.top().is_some_and(|o| o.kind() == OverlayKind::FilterDialog);
    let now = app.now();
    if let Some(d) = app.filter_dialog_mut() {
        d.scroll = scroll;
        d.hits = hits;
        d.hover = lit.map(|(h, _)| h);
        d.press.drawn(top, now);
    }
    cursor.filter(|_| top)
}

/// Draw the open filter dialog's add popup inside `bounds`, under the part of the bar it
/// opened on: the columns the search leaves, one a line (the selected one lit), the search on
/// top while it is open. Returns the hardware cursor (the search's).
pub(crate) fn draw_add_pick(app: &mut App, bounds: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let d = app.filter_dialog()?;
    let View::Popup { kind: PopupKind::AddPick, anchor } = d.view() else { return None };
    if bounds.width < 30 || bounds.height < 5 {
        return None;
    }
    let th = theme::cur();
    let i18n = &app.i18n;
    let on = app.icons_on();
    let muted = Style::new().fg(th.fg_muted);
    let bold = Style::new().fg(th.fg).add_modifier(Modifier::BOLD);
    let shown = d.shown_columns();
    let names: Vec<String> = shown.iter().map(|&i| escape_controls(&d.columns[i].name)).collect();
    let types: Vec<String> = shown
        .iter()
        .map(|&i| match d.columns[i].invisible {
            true => i18n.label(Label::TableDialogInvisible).to_string(),
            false => short_type(d.dialect, &d.columns[i].type_name),
        })
        .collect();
    let name_w = names.iter().map(|n| width(n)).max().unwrap_or(0).min(NAME_MAX);
    let type_w = types.iter().map(|t| width(t)).max().unwrap_or(0).min(TYPE_MAX);
    // A line: the marker, the filter mark, the key icon, the name, the type, a blank between
    // each and on the right.
    let want = 7 + name_w + 2 + type_w + 1;
    let title = [
        (" ".to_string(), Style::new()),
        ("WHERE".to_string(), bold),
        (format!(" \u{2026} {} ", i18n.label(Label::TableBarNewCondition)), muted),
    ];
    let search = d.search.is_some();
    let lines = shown.len().clamp(1, LINES_MAX) + usize::from(search);
    let (mut hits, inner) = list_frame(app, anchor, &title, want, lines, &|_| 0, bounds, buf)?;
    let rect = hits.frame;
    let d = app.filter_dialog()?;
    let x0 = inner.x + 1;
    let mut y = inner.y;
    let mut cursor = None;
    if let Some(s) = &d.search {
        let cue = i18n
            .msg(&Msg::TableDialogSearchCount { shown: shown.len().to_string(), total: d.columns.len().to_string() })
            .to_string();
        let cw = width(&cue) as u16;
        let end = inner.right().saturating_sub(1);
        put(buf, x0, y, "/", 1, Style::new().fg(th.accent));
        let field_end = end.saturating_sub(cw + 1);
        put(buf, field_end + 1, y, &cue, usize::from(cw), muted);
        let mut s = s.clone();
        let r = Rect::new(x0 + 2, y, field_end.saturating_sub(x0 + 2), 1);
        let cx = s.render(r, buf, th.base(), d.picker.is_none(), false, None);
        cursor = Some((cx, y));
        y += 1;
    }
    let rows = usize::from(inner.bottom().saturating_sub(y));
    let scroll = window(d.scroll, d.cursor, shown.len(), rows);
    if shown.is_empty() {
        put(buf, x0 + 1, y, &i18n.label(Label::TableBarNoColumn), usize::from(inner.width - 2), muted);
    }
    let focus_style = Style::new().fg(th.fg).patch(th.selection);
    let name_x = inner.x + 7;
    let type_x = name_x + name_w as u16 + 2;
    for (row, &ci) in shown.iter().enumerate().skip(scroll).take(rows) {
        let c = &d.columns[ci];
        let ly = y + (row - scroll) as u16;
        let line = Rect::new(inner.x, ly, inner.width, 1);
        let selected = row == d.cursor;
        if selected {
            buf.set_style(line, focus_style);
            put(buf, inner.x, ly, "\u{25b8}", 1, Style::new().fg(th.accent));
        }
        hits.columns.push((line, row));
        // Filtered already: the column has a condition that is sent.
        if d.draft.where_.rows_of(&c.name).iter().any(|&t| d.draft.where_.terms()[t].complete()) {
            put(buf, inner.x + 2, ly, icons::column_filter(on), 1, Style::new().fg(th.accent));
        }
        let key = if c.primary_key {
            Some((icons::KeyMark::Pk.text(on), th.key_pk))
        } else if c.foreign_key {
            Some((icons::KeyMark::Fk.text(on), th.key_fk))
        } else {
            None
        };
        if let Some((k, color)) = key {
            put(buf, inner.x + 4, ly, k, 2, Style::new().fg(color));
        }
        let name_style = if selected { bold } else { Style::new().fg(th.fg) };
        put(buf, name_x, ly, &names[row], name_w, name_style);
        let end = inner.right().saturating_sub(1);
        put(buf, type_x, ly, &types[row], usize::from(end.saturating_sub(type_x)), muted);
    }
    if shown.len() > rows {
        scroll_cue(buf, rect, d.cursor, shown.len());
    }
    finish(app, hits, scroll, cursor, buf)
}

/// Draw the open filter dialog's WHERE list inside `bounds`, under the part of the bar it
/// opened on: the conditions of the draft one a line (the focused one lit, an expression in
/// the warning colour, a refused one underlined in the error colour), each with its `×`, then
/// `+ add condition`.
pub(crate) fn draw_where_list(app: &mut App, bounds: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let d = app.filter_dialog()?;
    let View::Popup { kind: PopupKind::WhereList, anchor } = d.view() else { return None };
    if bounds.width < 30 || bounds.height < 5 {
        return None;
    }
    let th = theme::cur();
    let i18n = &app.i18n;
    let on = app.icons_on();
    let muted = Style::new().fg(th.fg_muted);
    let bold = Style::new().fg(th.fg).add_modifier(Modifier::BOLD);
    let chips = d.draft.where_.chips();
    let expr = format!("{} ", i18n.label(Label::TableDialogExpr));
    let texts: Vec<String> = chips.iter().map(|c| escape_controls(c.text)).collect();
    let text_w =
        chips.iter().zip(&texts).map(|(c, t)| width(t) + if c.expr { width(&expr) } else { 0 }).max().unwrap_or(0);
    let add = i18n.label(Label::TableDialogAdd).to_string();
    // A line: the marker, the filter glyph, the text, `×`, a blank between each and on the
    // right.
    let want = (4 + text_w + 3).max(2 + width(&add) + 1);
    let count = i18n.msg(&Msg::TableBarConditions { count: chips.len() as u64 }).to_string();
    let title = [(" ".to_string(), Style::new()), ("WHERE".to_string(), bold), (format!("  {count} "), muted)];
    let n = chips.len();
    let (mut hits, inner) = list_frame(app, anchor, &title, want, n + 1, &|_| 0, bounds, buf)?;
    let rect = hits.frame;
    let d = app.filter_dialog()?;
    let chips = d.draft.where_.chips();
    let at = match d.focus {
        DialogFocus::Chips(i) => i.min(n),
        _ => n,
    };
    let rows = usize::from(inner.height);
    let scroll = window(0, at, n + 1, rows);
    let focus_style = Style::new().fg(th.fg).patch(th.selection);
    let x0 = inner.x + 1;
    let end = inner.right().saturating_sub(1);
    let glyph = icons::column_filter(on);
    for line in scroll..(n + 1).min(scroll + rows) {
        let ly = inner.y + (line - scroll) as u16;
        let r = Rect::new(inner.x, ly, inner.width, 1);
        let focused = line == at;
        if focused {
            buf.set_style(r, focus_style);
            put(buf, inner.x, ly, "\u{25b8}", 1, Style::new().fg(th.accent));
        }
        let (Some(chip), Some(text)) = (chips.get(line), texts.get(line)) else {
            let style = if focused { bold.fg(th.accent) } else { Style::new().fg(th.accent) };
            put(buf, x0 + 1, ly, &add, usize::from(end.saturating_sub(x0 + 1)), style);
            hits.add = r;
            continue;
        };
        let x_at = end.saturating_sub(1);
        let mut cx = x0 + 1;
        cx += put(buf, cx, ly, glyph, 1, Style::new().fg(if chip.expr { th.warning } else { th.accent }));
        cx += 1;
        if chip.expr {
            cx += put(buf, cx, ly, &expr, usize::from(x_at.saturating_sub(cx + 1)), Style::new().fg(th.warning));
        }
        let mut ts = if chip.expr { Style::new().fg(th.warning) } else { Style::new().fg(th.fg) };
        if focused {
            ts = ts.add_modifier(Modifier::BOLD);
        }
        if d.mark.as_ref().is_some_and(|m| m.term(chip.index)) {
            ts = ts.fg(th.error).add_modifier(Modifier::UNDERLINED | Modifier::BOLD);
        }
        put(buf, cx, ly, text, usize::from(x_at.saturating_sub(cx + 1)), ts);
        put(buf, x_at, ly, "\u{00d7}", 1, muted);
        hits.chips.push((Rect::new(inner.x, ly, x_at - inner.x, 1), line));
        hits.chip_removes.push((Rect::new(x_at, ly, 1, 1), line));
    }
    if n + 1 > rows {
        scroll_cue(buf, rect, at, n + 1);
    }
    let scroll_kept = d.scroll;
    finish(app, hits, scroll_kept, None, buf)
}

/// Draw the open filter dialog's columns popup inside `bounds`, under the part of the bar it
/// opened on: the table's columns one a line (the selected one lit), `◉` shown or `○` hidden
/// (its name crossed out), with its key icon and short type; under them, while a column of the
/// primary key is hidden, that it is still read.
pub(crate) fn draw_columns(app: &mut App, bounds: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let d = app.filter_dialog()?;
    let View::Popup { kind: PopupKind::Columns, anchor } = d.view() else { return None };
    if bounds.width < 30 || bounds.height < 5 {
        return None;
    }
    let th = theme::cur();
    let i18n = &app.i18n;
    let on = app.icons_on();
    let muted = Style::new().fg(th.fg_muted);
    let bold = Style::new().fg(th.fg).add_modifier(Modifier::BOLD);
    let shown = d.shown_columns();
    let hidden = |name: &str| d.draft.hidden.iter().any(|h| h == name);
    let names: Vec<String> = shown.iter().map(|&i| escape_controls(&d.columns[i].name)).collect();
    let types: Vec<String> = shown.iter().map(|&i| short_type(d.dialect, &d.columns[i].type_name)).collect();
    let name_w = names.iter().map(|n| width(n)).max().unwrap_or(0).min(NAME_MAX);
    let type_w = types.iter().map(|t| width(t)).max().unwrap_or(0).min(TYPE_MAX);
    let off = d.columns.iter().filter(|c| hidden(&c.name)).count();
    let total = d.columns.len();
    // The primary key's columns, as the catalogue or the table's keys read say them.
    let pk = app.table_target(d.tab).map(|t| t.primary_key).unwrap_or_default();
    let in_pk = |c: &crate::app::filter_dialog::draft::ColumnInfo| c.primary_key || pk.contains(&c.name);
    // A key column hidden is still read: the grid's edits find their rows by it.
    let key_note = d.columns.iter().any(|c| in_pk(c) && hidden(&c.name));
    let note = i18n.label(Label::TableDialogHiddenKey).to_string();
    // The border, then a line: the marker, the mark, the key icon, the name, the type, a blank
    // between each and on the right.
    let want = 2 + (7 + name_w + 2 + type_w + 1).max(if key_note { width(&note) + 2 } else { 0 });
    let count =
        i18n.msg(&Msg::TableBarColumnsTitle { shown: (total - off).to_string(), total: total.to_string() }).to_string();
    let title_style = if off > 0 { Style::new().fg(th.warning).add_modifier(Modifier::BOLD) } else { bold };
    let title = [(" ".to_string(), Style::new()), (count, title_style), (" ".to_string(), Style::new())];
    let (mut hits, inner) =
        list_frame(app, anchor, &title, want, shown.len().max(1), &|_| usize::from(key_note), bounds, buf)?;
    let rect = hits.frame;
    let d = app.filter_dialog()?;
    let hidden = |name: &str| d.draft.hidden.iter().any(|h| h == name);
    let rows = usize::from(inner.height).saturating_sub(usize::from(key_note));
    let scroll = window(d.scroll, d.cursor, shown.len(), rows);
    let focus_style = Style::new().fg(th.fg).patch(th.selection);
    let name_x = inner.x + 7;
    let type_x = name_x + name_w as u16 + 2;
    let end = inner.right().saturating_sub(1);
    for (row, &ci) in shown.iter().enumerate().skip(scroll).take(rows) {
        let c = &d.columns[ci];
        let ly = inner.y + (row - scroll) as u16;
        let line = Rect::new(inner.x, ly, inner.width, 1);
        let selected = row == d.cursor;
        if selected {
            buf.set_style(line, focus_style);
            put(buf, inner.x, ly, "\u{25b8}", 1, Style::new().fg(th.accent));
        }
        hits.columns.push((line, row));
        let off = hidden(&c.name);
        let (mark, mark_style) = match off {
            true => ("\u{25cb}", muted),
            false => ("\u{25c9}", Style::new().fg(th.accent)),
        };
        put(buf, inner.x + 2, ly, mark, 1, mark_style);
        let key = if in_pk(c) {
            Some((icons::KeyMark::Pk.text(on), th.key_pk))
        } else if c.foreign_key {
            Some((icons::KeyMark::Fk.text(on), th.key_fk))
        } else {
            None
        };
        if let Some((k, color)) = key {
            put(buf, inner.x + 4, ly, k, 2, Style::new().fg(color));
        }
        let name_style = match (off, selected) {
            (true, _) => muted.add_modifier(Modifier::CROSSED_OUT),
            (false, true) => bold,
            (false, false) => Style::new().fg(th.fg),
        };
        put(buf, name_x, ly, &names[row], name_w, name_style);
        put(buf, type_x, ly, &types[row], usize::from(end.saturating_sub(type_x)), muted);
    }
    if key_note {
        put(buf, inner.x + 1, inner.bottom() - 1, &note, usize::from(end.saturating_sub(inner.x + 1)), muted);
    }
    if shown.len() > rows {
        scroll_cue(buf, rect, d.cursor, shown.len());
    }
    finish(app, hits, scroll, None, buf)
}

#[cfg(test)]
mod tests {
    use super::window;

    #[test]
    fn the_line_shown_stays_in_view() {
        assert_eq!(window(0, 0, 20, 5), 0);
        assert_eq!(window(0, 7, 20, 5), 3);
        assert_eq!(window(6, 2, 20, 5), 2);
        // Past the end: as far as the last line allows.
        assert_eq!(window(30, 19, 20, 5), 15);
    }
}
