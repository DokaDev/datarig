//! The filter line of a table tab: one row above the grid with the `WHERE` condition on the
//! left, the `ORDER BY` keys on the right (at most two fifths of the row), the `LIMIT` on the
//! rows (`-` for none) and the refresh mark at the right end.
//!
//! What the line shows is what the tab holds as applied, highlighted as the editor highlights
//! SQL; a field edited but not applied puts `●` at the left and its keyword in the warning
//! color. The keys are shown as the app writes them (normalised), the keys appended to make the
//! order total dimmed after them (`+ id`), or `order not unique` while no key is known to be.
//! Where the two do not fit, the order shrinks to its first column and the count of the others
//! (`↓id+1`) and a long condition is cut with `…` (scrolled while it is edited); the limit
//! shrinks to `L100` unless it is edited, and is left out when there is none. With nothing applied and
//! nothing typed, the line says which key edits it.

use crate::app::App;
use crate::app::table_query::Field;
use crate::icons;
use crate::text::width;
use crate::theme;
use crate::widgets::put;
use crate::widgets::text_input::TextInput;
use datarig_core::query::{Dir, KeyOrigin, OrderKey};
use datarig_core::sql::dialect::Dialect;
use datarig_core::sql::lexer::{Tok, lex_in};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use std::ops::Range;

/// The most a pasted text makes a field hold (bytes).
pub const FIELD_CAP: usize = 16 * 1024;

/// The fields of a table tab's filter line: their texts (what the user writes) and which one
/// has the focus.
#[derive(Clone, Debug, Default)]
pub struct LineState {
    pub where_: TextInput,
    pub order: TextInput,
    pub limit: TextInput,
    /// The field keys go to; `None` while the grid has them.
    pub focus: Option<Field>,
    /// The applied texts the fields were last set from: a field still holding its own is not
    /// edited and follows what is applied (a command, a clear).
    base: (String, String, String),
}

impl LineState {
    pub fn field(&self, f: Field) -> &TextInput {
        match f {
            Field::Where => &self.where_,
            Field::Order => &self.order,
            Field::Limit => &self.limit,
        }
    }

    pub fn field_mut(&mut self, f: Field) -> &mut TextInput {
        match f {
            Field::Where => &mut self.where_,
            Field::Order => &mut self.order,
            Field::Limit => &mut self.limit,
        }
    }

    /// The fields follow the applied texts `applied` (condition, order, limit) where they are not
    /// edited (they hold the texts they were last set from) and have no focus. A field that
    /// holds the applied text again counts as not edited from then on.
    pub fn follow(&mut self, applied: (&str, &str, &str)) {
        let focus = self.focus;
        let (b0, b1, b2) = (&mut self.base.0, &mut self.base.1, &mut self.base.2);
        for (f, base, now) in
            [(Field::Where, b0, applied.0), (Field::Order, b1, applied.1), (Field::Limit, b2, applied.2)]
        {
            let input = match f {
                Field::Where => &mut self.where_,
                Field::Order => &mut self.order,
                Field::Limit => &mut self.limit,
            };
            if norm(input.text()) == now {
                *base = now.to_string();
            } else if focus != Some(f) && norm(input.text()) == base.as_str() {
                input.set(now);
                *base = now.to_string();
            }
        }
    }

    /// The fields back to the applied texts (`Esc`).
    pub fn restore(&mut self, applied: (&str, &str, &str)) {
        self.where_.set(applied.0);
        self.order.set(applied.1);
        self.limit.set(applied.2);
        self.base = (applied.0.to_string(), applied.1.to_string(), applied.2.to_string());
    }

    /// Every field is empty.
    pub fn empty(&self) -> bool {
        self.where_.text().is_empty() && self.order.text().is_empty() && self.limit.text().is_empty()
    }

    /// Whether field `f` holds another text than `applied`.
    pub fn edited(&self, f: Field, applied: &str) -> bool {
        norm(self.field(f).text()) != applied
    }
}

/// A blank text is no text.
fn norm(s: &str) -> &str {
    if s.trim().is_empty() { "" } else { s }
}

/// What the line shows of the applied query.
pub struct Applied {
    /// The condition, empty for none.
    pub filter: String,
    /// The user's keys as the app writes them, empty for none.
    pub order: String,
    /// The user's keys.
    pub keys: Vec<OrderKey>,
    /// The cap on the rows, empty for none.
    pub limit: String,
    /// After the keys, dimmed: the columns appended to make the order total (`+ id`), or why
    /// it may not be.
    pub tie: Option<String>,
    /// When the order is not known total: [`Applied::tie`] in a few columns. It is never left
    /// out while the keys show in any form (they are cut, or shrunk to `↑rating not unique`);
    /// the columns appended (`+ id`) give way to the keys instead.
    pub tie_short: Option<String>,
    pub dialect: Dialect,
    /// The server refused it: the rows shown are of an earlier query.
    pub failed: bool,
    /// What a field holds that is wrong (bytes of its text): our refusal's part, or where the
    /// server said its error is. Underlined in the error color.
    pub mark: Option<(Field, Range<usize>)>,
}

/// Where the parts of the line were drawn, for the mouse and the terminal cursor.
#[derive(Clone, Copy, Debug, Default)]
pub struct Drawn {
    pub where_: Rect,
    pub order: Rect,
    pub limit: Rect,
    pub refresh: Rect,
    pub cursor: Option<(u16, u16)>,
}

/// The highlight of `text` as the editor's: each token's byte range and style, after the part
/// `mark` (bytes) underlined in the error color, which wins over them.
fn highlight(text: &str, d: Dialect, mark: Option<&Range<usize>>) -> Vec<(Range<usize>, Style)> {
    let th = theme::cur();
    let toks = lex_in(text, d);
    let error = Style::new().fg(th.error).add_modifier(Modifier::UNDERLINED | Modifier::BOLD);
    mark.map(|m| (m.clone(), error))
        .into_iter()
        .chain(toks.iter().enumerate().filter(|(_, t)| t.kind != Tok::Whitespace).map(|(i, t)| {
            let is_fn = t.kind == Tok::Ident && toks.get(i + 1).is_some_and(|n| n.kind == Tok::LParen);
            (t.start..t.end, th.syntax(t.kind, is_fn))
        }))
        .collect()
}

/// The order shrunk to its first key: its direction, its column and how many keys follow.
pub fn collapsed(keys: &[OrderKey]) -> String {
    let user: Vec<&OrderKey> = keys.iter().filter(|k| k.origin == KeyOrigin::User).collect();
    let Some(first) = user.first() else { return String::new() };
    let arrow = if first.dir == Dir::Desc { "\u{2193}" } else { "\u{2191}" };
    match user.len() - 1 {
        0 => format!("{arrow}{}", first.col),
        n => format!("{arrow}{}+{n}", first.col),
    }
}

/// The fewest columns the keys are cut to, to keep what is said of their order next to them.
const KEYS_MIN: usize = 4;

/// The same for the shrunk keys (`↑v…`): the arrow and a letter of the column, then `…`.
const KEYS_SHRUNK_MIN: usize = 3;

const WHERE: &str = "WHERE";
const ORDER_BY: &str = "ORDER BY";
const LIMIT: &str = "LIMIT";

/// The fewest columns the limit's text has while it is edited.
const LIMIT_FIELD: usize = 8;

/// The limit's part of the line (`LIMIT 100`, `LIMIT -`), or its shrunk form (`L100`).
fn limit_part(text: &str, focused: bool) -> (usize, usize) {
    let value = if focused { width(text).max(LIMIT_FIELD) + 1 } else { width(text).max(1) };
    let short = if text.trim().is_empty() { 0 } else { 1 + width(text) };
    (LIMIT.len() + 1 + value, short)
}

/// The rows the line takes in `width` columns: two while the order is edited where fewer than
/// a dozen columns are left for its keys next to the condition, else one.
pub fn rows(width: u16, focused: Option<Field>) -> u16 {
    // The line's room for its fields (as `draw` reckons it), less a few columns of the
    // condition and the `ORDER BY` keyword.
    let room = usize::from(width).saturating_sub(11);
    let keys = room.saturating_sub(2 + 6 + ORDER_BY.len() + 1);
    if focused == Some(Field::Order) && keys < 12 { 2 } else { 1 }
}

/// Below this many rows of the results pane an empty line without the focus gives its row to
/// the grid.
pub const SHORT: u16 = 12;

/// Whether the line is left out: nothing applied or typed (`blank`), no focus, a results pane
/// of `pane_h` rows that is short.
pub fn hidden(blank: bool, focused: bool, pane_h: u16) -> bool {
    blank && !focused && pane_h < SHORT
}

/// The refresh mark: the glyph, or `⟳` with icons off.
pub fn refresh_mark(on: bool) -> &'static str {
    if on { icons::REFRESH } else { "\u{27f3}" }
}

/// Draw the line of the active table tab into the one-row `area`: `state` holds the fields,
/// `applied` what the tab holds as applied, `focused` the field with the focus (the results
/// pane focused), `hint` the line's text with nothing applied or typed.
pub(crate) fn draw(
    app: &App,
    state: &mut LineState,
    applied: &Applied,
    focused: Option<Field>,
    hint: &str,
    area: Rect,
    buf: &mut Buffer,
) -> Drawn {
    let th = theme::cur();
    let base = Style::new().fg(th.fg).bg(th.bg);
    buf.set_style(area, base);
    let mut drawn = Drawn::default();
    let w = area.width as usize;
    if w < 12 {
        return drawn;
    }
    let (x0, y) = (area.x, area.y);
    let muted = Style::new().fg(th.fg_muted).bg(th.bg);
    let dim = Style::new().fg(th.fg_dim).bg(th.bg);
    let label = muted.add_modifier(Modifier::BOLD);
    let tint = Style::new().fg(th.warning).bg(th.bg).add_modifier(Modifier::BOLD);
    // The refresh mark, one blank after it.
    let rx = x0 + area.width - 2;
    put(buf, rx, y, refresh_mark(app.icons_on()), 1, muted);
    drawn.refresh = Rect::new(rx, y, 1, 1);
    let end = usize::from(rx - x0).saturating_sub(2);
    let where_edited = state.edited(Field::Where, &applied.filter);
    let order_edited = state.edited(Field::Order, &applied.order);
    let limit_edited = state.edited(Field::Limit, &applied.limit);
    let blank = applied.filter.is_empty() && applied.order.is_empty() && applied.limit.is_empty();
    if focused.is_none() && state.empty() && blank {
        // Whole parts only: a key cut in its middle says nothing.
        let room = end.saturating_sub(1);
        put(buf, x0 + 1, y, &crate::text::whole_parts(hint, room), room, dim);
        drawn.where_ = Rect::new(x0, y, end as u16, 1);
        return drawn;
    }
    let failed = Style::new().fg(th.error).bg(th.bg).add_modifier(Modifier::BOLD);
    if where_edited || order_edited || limit_edited {
        put(buf, x0, y, "\u{25cf}", 1, Style::new().fg(th.warning).bg(th.bg));
    } else if applied.failed {
        // The server refused the condition (or the order) as it stands.
        put(buf, x0, y, "\u{2717}", 1, failed);
    }
    let where_label = match (where_edited, applied.failed) {
        (true, _) => tint,
        (false, true) => failed,
        (false, false) => label,
    };
    put(buf, x0 + 1, y, WHERE, 5, where_label);
    let mark = |f: Field| applied.mark.as_ref().filter(|(mf, _)| *mf == f).map(|(_, r)| r);
    let field_x = 7usize;
    // The order's part: whole (`ORDER BY` and the keys, then what makes them total), or shrunk.
    let order_text = state.order.text().to_string();
    let tie = applied.tie.as_deref().filter(|_| !order_edited && focused != Some(Field::Order));
    let tie_short = applied.tie_short.as_deref().filter(|_| tie.is_some());
    let cap = w * 2 / 5;
    let label_w = width(ORDER_BY) + 1;
    let keys_w = width(&order_text);
    let tie_w = tie.map_or(0, |t| width(t) + 1);
    let where_need = width(state.where_.text()) + usize::from(focused == Some(Field::Where));
    // The limit's part, right of the order: whole where the condition and the whole order
    // fit next to it (or while it is edited), else shrunk to `L100`, and left out when none
    // is set. The order and the condition share what is left.
    let limit_focused = focused == Some(Field::Limit);
    let limit_text = state.limit.text().to_string();
    let (limit_full, limit_short) = limit_part(&limit_text, limit_focused);
    let whole_fits = field_x + where_need + 2 + label_w + keys_w + tie_w + 2 + limit_full <= end;
    let limit_whole = limit_focused || limit_edited || whole_fits;
    let limit_w = if limit_whole { limit_full } else { limit_short }.min(end.saturating_sub(field_x + 4));
    let end = if limit_w > 0 { end - limit_w - 2 } else { end };
    let room = end.saturating_sub(field_x);
    let fits = |order_w: usize| order_w <= cap && where_need + 2 + order_w <= room;
    let shrunk = collapsed(&applied.keys);
    // (width of the order's part, whether whole, whether with what makes it total)
    // Too narrow to edit the order next to the condition: it has a second row of its own.
    let below = focused == Some(Field::Order) && area.height >= 2 && rows(area.width, focused) == 2;
    let (order_w, whole, with_tie) = if below {
        (0, true, false)
    } else if focused == Some(Field::Order) {
        // Edited: as wide as its text, and up to two fifths where the condition does not need
        // the columns; the condition keeping a few columns.
        let spare = room.saturating_sub(where_need + 2);
        ((label_w + width(&order_text) + 1).max(cap.min(spare)).min(room.saturating_sub(2 + 6)), true, false)
    } else if fits(label_w + keys_w + tie_w) {
        (label_w + keys_w + tie_w, true, tie.is_some())
    } else if tie.is_some() && tie_short.is_none() && fits(label_w + keys_w) {
        (label_w + keys_w, true, false)
    } else if tie_short.is_some() && fits(label_w + KEYS_MIN + tie_w) {
        // The keys cut rather than what is said of them: an order not known total says so.
        let w = cap.min(room.saturating_sub(where_need + 2));
        (w, true, true)
    } else if order_edited || shrunk.is_empty() {
        // Nothing to shrink to: the label and what of the keys fits.
        let w = (label_w + keys_w).min(cap).min(room.saturating_sub(4));
        (if w > label_w { w } else { 0 }, true, false)
    } else {
        let short = tie_short.map_or(0, |t| width(t) + 1);
        let w = width(&shrunk).saturating_add(short).min(room.saturating_sub(4));
        // What is said of the order wins over the condition's last columns too: the arrow and
        // it, at least.
        let need = tie_short.map_or(0, |t| width(t) + 2).min(room);
        (w.max(need), false, false)
    };
    let where_w = if order_w > 0 { room.saturating_sub(order_w + 2) } else { room };
    let where_rect = Rect::new(x0 + field_x as u16, y, where_w as u16, 1);
    drawn.where_ = Rect::new(x0, y, (field_x + where_w) as u16, 1);
    let d = applied.dialect;
    if limit_w > 0 {
        let lx = x0 + (end + 2) as u16;
        drawn.limit = Rect::new(lx, y, limit_w as u16, 1);
        let limit_label = if limit_edited { tint } else { label };
        let input = &mut state.limit;
        let spans = highlight(input.text(), d, mark(Field::Limit));
        if limit_whole {
            put(buf, lx, y, LIMIT, LIMIT.len(), limit_label);
            let field = Rect::new(lx + LIMIT.len() as u16 + 1, y, (limit_w - LIMIT.len() - 1) as u16, 1);
            if limit_focused {
                drawn.cursor = Some((input.render_spans(field, buf, base, true, &spans), y));
            } else if limit_text.trim().is_empty() {
                put(buf, field.x, y, "-", 1, dim);
            } else {
                input.render_spans(field, buf, base, false, &spans);
            }
        } else {
            put(buf, lx, y, "L", 1, limit_label);
            input.render_spans(Rect::new(lx + 1, y, (limit_w - 1) as u16, 1), buf, base, false, &spans);
        }
    }
    if where_w > 0 {
        let input = &mut state.where_;
        let spans = highlight(input.text(), d, mark(Field::Where));
        let cx = input.render_spans(where_rect, buf, base, focused == Some(Field::Where), &spans);
        if focused == Some(Field::Where) {
            drawn.cursor = Some((cx, y));
        } else if input.cut().1 {
            put(buf, where_rect.x + where_rect.width - 1, y, "\u{2026}", 1, muted);
        }
    }
    if below {
        let y = y + 1;
        buf.set_style(Rect::new(x0, y, area.width, 1), base);
        put(buf, x0 + 1, y, ORDER_BY, label_w - 1, if order_edited { tint } else { label });
        let fx = x0 + 1 + label_w as u16;
        let field = Rect::new(fx, y, (x0 + area.width).saturating_sub(fx + 1), 1);
        let input = &mut state.order;
        let spans = highlight(input.text(), d, mark(Field::Order));
        drawn.cursor = Some((input.render_spans(field, buf, base, true, &spans), y));
        drawn.order = Rect::new(x0, y, area.width, 1);
        return drawn;
    }
    if order_w == 0 {
        return drawn;
    }
    let ox = x0 + (field_x + where_w + 2) as u16;
    drawn.order = Rect::new(ox, y, order_w as u16, 1);
    if !whole {
        // What is said of the order has its room first, at every width: the keys are cut (`…`)
        // to what is left, down to their arrow alone, and left out where not even that fits.
        let said = tie_short.filter(|t| width(t) <= order_w);
        let keys_room = match said {
            Some(t) => order_w.saturating_sub(width(t) + 1),
            None => order_w,
        };
        let arrow: String = shrunk.chars().take(1).collect();
        let keys = if width(&shrunk) <= keys_room {
            shrunk.clone()
        } else if keys_room >= KEYS_SHRUNK_MIN {
            format!("{}\u{2026}", crate::text::clip(&shrunk, keys_room - 1))
        } else if keys_room >= width(&arrow) {
            arrow
        } else {
            String::new()
        };
        let keys_w = width(&keys);
        put(buf, ox, y, &keys, keys_w, base);
        if let Some(t) = said {
            let at = if keys_w == 0 { 0 } else { keys_w + 1 };
            put(buf, ox + at as u16, y, t, order_w - at, dim);
        }
        return drawn;
    }
    put(buf, ox, y, ORDER_BY, label_w - 1, if order_edited { tint } else { label });
    let fx = ox + label_w as u16;
    let field_w = order_w - label_w - if with_tie { tie_w } else { 0 };
    let field = Rect::new(fx, y, field_w as u16, 1);
    let input = &mut state.order;
    let spans = highlight(input.text(), d, mark(Field::Order));
    let cx = input.render_spans(field, buf, base, focused == Some(Field::Order), &spans);
    if focused == Some(Field::Order) {
        drawn.cursor = Some((cx, y));
    } else if input.cut().1 {
        put(buf, fx + field.width - 1, y, "\u{2026}", 1, muted);
    }
    if let Some(t) = tie.filter(|_| with_tie) {
        put(buf, fx + field_w as u16 + 1, y, t, tie_w - 1, dim);
    }
    drawn
}

#[cfg(test)]
mod tests;
