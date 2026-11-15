//! The query bar's order popup, in the frame of its popups: the order's keys first, one a
//! line with its place and its direction (`1 ↓ total_amount  numeric`), a dotted rule, then the
//! other columns in the table's order with their key icon and short type; under a rule, the
//! `ORDER BY` clause the draft sends, coloured as the editor colours SQL, the key the app
//! appends to make the order total dimmed.

use super::lists::{NAME_MAX, TYPE_MAX, finish, list_frame, scroll_cue, window};
use crate::app::App;
use crate::app::filter_dialog::DialogFocus;
use crate::app::filter_dialog::view::{PopupKind, View};
use crate::icons;
use crate::text::{escape_controls, grapheme_width, width};
use crate::theme;
use crate::widgets::filter_dialog::{highlight, order_clause, short_type, wrap_at};
use crate::widgets::put;
use datarig_core::query::Dir;
use datarig_i18n::Label;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_segmentation::UnicodeSegmentation;

/// The most rows the `ORDER BY` clause takes.
const CLAUSE_MAX: usize = 3;

/// A line of the list: a column (its place among the popup's lines), or the dotted rule
/// between the keys and the other columns.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Line {
    Row(usize),
    Rule,
}

/// Draw the open filter dialog's order popup inside `bounds`, under the part of the bar it
/// opened on: the lines of [`crate::app::filter_dialog::FilterDialog::sort_rows`] (the focused
/// one lit), then the clause sent.
pub(crate) fn draw_order(app: &mut App, bounds: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let d = app.filter_dialog()?;
    let View::Popup { kind: PopupKind::Order, anchor } = d.view() else { return None };
    if bounds.width < 30 || bounds.height < 6 {
        return None;
    }
    let th = theme::cur();
    let i18n = &app.i18n;
    let on = app.icons_on();
    let muted = Style::new().fg(th.fg_muted);
    let bold = Style::new().fg(th.fg).add_modifier(Modifier::BOLD);
    let rows = d.sort_rows();
    let keys = d.draft.order.len();
    let info = |name: &str| d.columns.iter().find(|c| c.name == name);
    // The primary key's columns, as the catalogue or the table's keys read say them.
    let pk = app.table_target(d.tab).map(|t| t.primary_key).unwrap_or_default();
    let names: Vec<String> = rows.iter().map(|n| escape_controls(n)).collect();
    let types: Vec<String> =
        rows.iter().map(|n| info(n).map_or_else(String::new, |c| short_type(d.dialect, &c.type_name))).collect();
    let name_w = names.iter().map(|n| width(n)).max().unwrap_or(0).min(NAME_MAX);
    let type_w = types.iter().map(|t| width(t)).max().unwrap_or(0).min(TYPE_MAX);
    let mut lines: Vec<Line> = (0..keys).map(Line::Row).collect();
    if keys > 0 && rows.len() > keys {
        lines.push(Line::Rule);
    }
    lines.extend((keys..rows.len()).map(Line::Row));
    // The border, then a line: the marker, the place, the arrow, the key icon, the name, the
    // type, a blank between each and on the right.
    let want = 2 + 9 + name_w + 2 + type_w + 1;
    let clause = order_clause(app, d);
    // Under the list: a rule, then the clause on as many rows as it takes (at most three).
    let below = |w: usize| 1 + clause.as_ref().map_or(1, |(c, _)| wrap_at(c, w.max(1)).len().clamp(1, CLAUSE_MAX));
    let title = [(" ".to_string(), Style::new()), ("ORDER BY".to_string(), bold), (" ".to_string(), Style::new())];
    let (mut hits, inner) = list_frame(app, anchor, &title, want, lines.len().max(1), &below, bounds, buf)?;
    let rect = hits.frame;
    let d = app.filter_dialog()?;
    let at = match d.focus {
        DialogFocus::Sorted(i) => i.min(rows.len().saturating_sub(1)),
        _ => 0,
    };
    let focus_line = lines.iter().position(|l| *l == Line::Row(at)).unwrap_or(0);
    // The clause's rows (as many as fit in the popup's width now) and its rule under the list.
    let x0 = inner.x + 1;
    let end = inner.right().saturating_sub(1);
    let text_w = usize::from(end.saturating_sub(x0));
    let wrapped = clause.as_ref().map(|(c, _)| wrap_at(c, text_w.max(1)));
    let below = 1 + wrapped.as_ref().map_or(1, |w| w.len().clamp(1, CLAUSE_MAX)) as u16;
    let list_h = usize::from(inner.height.saturating_sub(below));
    let scroll = window(d.scroll, focus_line, lines.len(), list_h);
    let focus_style = Style::new().fg(th.fg).patch(th.selection);
    let name_x = inner.x + 9;
    let type_x = name_x + name_w as u16 + 2;
    for (li, line) in lines.iter().enumerate().skip(scroll).take(list_h) {
        let ly = inner.y + (li - scroll) as u16;
        let r = Rect::new(inner.x, ly, inner.width, 1);
        let Line::Row(i) = *line else {
            put(buf, x0, ly, &"\u{2504}".repeat(text_w), text_w, Style::new().fg(th.border));
            continue;
        };
        let focused = i == at;
        if focused {
            buf.set_style(r, focus_style);
            put(buf, inner.x, ly, "\u{25b8}", 1, Style::new().fg(th.accent));
        }
        hits.sorted.push((r, i));
        if let Some((dir, place)) = d.draft.sort_of(&rows[i]) {
            let arrow = if dir == Dir::Desc { "\u{2193}" } else { "\u{2191}" };
            put(buf, x0, ly, &format!("{:>2}", place + 1), 2, Style::new().fg(th.fg_muted));
            put(buf, x0 + 3, ly, arrow, 1, Style::new().fg(th.accent).add_modifier(Modifier::BOLD));
        }
        let (primary, foreign) = info(&rows[i]).map_or((false, false), |c| (c.primary_key, c.foreign_key));
        let key = match (primary || pk.contains(&rows[i]), foreign) {
            (true, _) => Some((icons::KeyMark::Pk.text(on), th.key_pk)),
            (false, true) => Some((icons::KeyMark::Fk.text(on), th.key_fk)),
            _ => None,
        };
        if let Some((k, color)) = key {
            put(buf, inner.x + 6, ly, k, 2, Style::new().fg(color));
        }
        let name_style = if focused || i < keys { bold } else { Style::new().fg(th.fg) };
        put(buf, name_x, ly, &names[i], name_w, name_style);
        put(buf, type_x, ly, &types[i], usize::from(end.saturating_sub(type_x)), muted);
    }
    if lines.len() > list_h {
        scroll_cue(buf, rect, focus_line, lines.len());
    }
    // The rule, then the clause.
    let mut y = inner.bottom().saturating_sub(below);
    put(
        buf,
        inner.x,
        y,
        &"\u{2500}".repeat(usize::from(inner.width)),
        usize::from(inner.width),
        Style::new().fg(th.border),
    );
    y += 1;
    match (&clause, &wrapped) {
        (Some((sql, tie)), Some(wrapped)) => {
            let mut spans = highlight(sql, d.dialect, None);
            if let Some(t) = tie {
                spans.insert(0, (t.clone(), Style::new().fg(th.fg_dim)));
            }
            let base = th.base();
            let shown = wrapped.len().min(CLAUSE_MAX);
            for (i, line) in wrapped.iter().take(shown).enumerate() {
                let cut = i + 1 == shown && wrapped.len() > shown;
                let mut x = x0;
                for (b, g) in sql[line.clone()].grapheme_indices(true) {
                    let pos = line.start + b;
                    let gw = grapheme_width(g) as u16;
                    if x + gw > end || (cut && x + gw + 1 > end) {
                        put(buf, x, y, "\u{2026}", 1, muted);
                        break;
                    }
                    let style = spans.iter().find(|(r, _)| r.contains(&pos)).map_or(base, |(_, st)| base.patch(*st));
                    buf.set_stringn(x, y, g, usize::from(gw.max(1)), style);
                    x += gw;
                }
                y += 1;
            }
        }
        _ => {
            // No key: the rows come in no order the app sets.
            let none = i18n.label(Label::TableBarNone).to_string();
            let x = x0 + put(buf, x0, y, "ORDER BY", text_w, Style::new().fg(th.fg_dim).add_modifier(Modifier::BOLD));
            put(buf, x + 1, y, &none, usize::from(end.saturating_sub(x + 1)), muted);
        }
    }
    finish(app, hits, scroll, None, buf)
}
