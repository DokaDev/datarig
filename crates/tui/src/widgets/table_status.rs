//! A table tab's filter and sort on the bottom border of its results pane, on the left:
//! `▼ status = 'shipped' +1 · total_amount ↓ +1` (the first condition as it is written and how
//! many more, the first key of the order with its direction and how many more). Narrower, the
//! condition's text is cut (down to [`CUT_MIN`] columns), then each part is only counted
//! (`▼ 2 · ↓ 2`), then counted tighter (`▼2 ↓2`): that a filter or an order is on always shows.
//! A run of the query on its way puts the spinner first; a run that brought no rows says so in
//! the warning colour (`▼ failed`). With no condition and no order, nothing. A click on it
//! opens the filter dialog.

use crate::app::App;
use crate::app::filter_dialog::draft::Where;
use crate::app::hover::PointerOn;
use crate::app::table_query::{Unapplied, filter_text};
use crate::text::{clip, width};
use crate::theme;
use crate::widgets::put;
use datarig_core::query::{Dir, KeyOrigin};
use datarig_i18n::Label;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// The fewest columns the first condition's text is cut to before the parts are only counted.
pub(crate) const CUT_MIN: usize = 8;

/// What a piece of the status is, for its style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// A part's glyph or a key's arrow (the accent).
    Glyph,
    /// What a part is about: a condition's text, a key's column.
    Text,
    /// How many more there are, or how many in all (the accent).
    Count,
    /// Between two parts.
    Sep,
    /// The spinner of a run on its way.
    Spinner,
    /// A run that brought no rows (the warning colour).
    Warning,
}

/// A piece of the status: its text and what it is.
pub(crate) type Piece = (String, Role);

/// One part of the status (the conditions, the order; another kind of part joins the list
/// as one more [`Part`]) in its three forms, from the longest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Part {
    /// Whole: `▼ status = 'shipped' +1`.
    whole: Vec<Piece>,
    /// The piece of `whole` that may be cut with `…` where the whole part does not fit.
    cut: Option<usize>,
    /// Counted: `▼ 2`.
    counted: Vec<Piece>,
    /// Counted tighter: `▼2`.
    compact: Vec<Piece>,
}

impl Part {
    /// The conditions: `glyph`, the first one's text, how many there are in all.
    pub(crate) fn filter(glyph: &str, first: &str, count: usize) -> Self {
        let mut whole = vec![(glyph.to_string(), Role::Glyph), (format!(" {first}"), Role::Text)];
        if count > 1 {
            whole.push((format!(" +{}", count - 1), Role::Count));
        }
        Self::counted_as(whole, Some(1), glyph, count)
    }

    /// The order: the first key's column, its direction, how many keys there are in all.
    pub(crate) fn order(first: &str, dir: Dir, count: usize) -> Self {
        let arrow = if dir == Dir::Desc { "\u{2193}" } else { "\u{2191}" };
        let mut whole = vec![(first.to_string(), Role::Text), (format!(" {arrow}"), Role::Glyph)];
        if count > 1 {
            whole.push((format!(" +{}", count - 1), Role::Count));
        }
        Self::counted_as(whole, None, arrow, count)
    }

    /// A run that brought no rows: `glyph` and `word`, in the warning colour, in every form.
    pub(crate) fn failed(glyph: &str, word: &str) -> Self {
        let pieces = vec![(format!("{glyph} {word}"), Role::Warning)];
        Self { whole: pieces.clone(), cut: None, counted: pieces.clone(), compact: pieces }
    }

    fn counted_as(whole: Vec<Piece>, cut: Option<usize>, glyph: &str, count: usize) -> Self {
        let counted = vec![(glyph.to_string(), Role::Glyph), (format!(" {count}"), Role::Count)];
        let compact = vec![(glyph.to_string(), Role::Glyph), (count.to_string(), Role::Count)];
        Self { whole, cut, counted, compact }
    }
}

/// The columns `pieces` take.
fn width_of(pieces: &[Piece]) -> usize {
    pieces.iter().map(|(t, _)| width(t)).sum()
}

/// `parts` in the form `form` picks (given each part's place), joined by `sep`, after `first`.
fn joined(first: &[Piece], parts: &[Part], form: impl Fn(usize, &Part) -> Vec<Piece>, sep: &str) -> Vec<Piece> {
    let mut out = first.to_vec();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.push((sep.to_string(), Role::Sep));
        }
        out.extend(form(i, p));
    }
    out
}

/// The status of `parts` in `room` columns (`spinner`: a run on its way, its frame first): the
/// longest form that fits ([`Part`]); where even the tightest does not, what fits of it.
pub(crate) fn fit(parts: &[Part], spinner: Option<&str>, room: usize) -> Vec<Piece> {
    if parts.is_empty() {
        return Vec::new();
    }
    let first: Vec<Piece> = spinner.map(|s| vec![(format!("{s} "), Role::Spinner)]).unwrap_or_default();
    let sep = " \u{00b7} ";
    let whole = joined(&first, parts, |_, p| p.whole.clone(), sep);
    if width_of(&whole) <= room {
        return whole;
    }
    // The first part that may be cut, cut to what is left of the room.
    if let Some((k, at)) = parts.iter().enumerate().find_map(|(k, p)| p.cut.map(|at| (k, at))) {
        let over = width_of(&whole) - room;
        let text = &parts[k].whole[at].0;
        let keep = width(text).saturating_sub(over);
        // The text keeps its leading blank: at least that and `CUT_MIN` columns of it.
        if keep > CUT_MIN {
            let cut = joined(
                &first,
                parts,
                |i, p| {
                    let mut w = p.whole.clone();
                    if i == k {
                        w[at].0 = clip(text, keep);
                    }
                    w
                },
                sep,
            );
            if width_of(&cut) <= room {
                return cut;
            }
        }
    }
    let counted = joined(&first, parts, |_, p| p.counted.clone(), sep);
    if width_of(&counted) <= room {
        return counted;
    }
    let compact = joined(&first, parts, |_, p| p.compact.clone(), " ");
    if width_of(&compact) <= room {
        return compact;
    }
    // What fits of the tightest form.
    let mut out = Vec::new();
    let mut left = room;
    for (text, role) in compact {
        if left == 0 {
            break;
        }
        let t = clip(&text, left);
        left -= width(&t);
        out.push((t, role));
    }
    out
}

/// The parts of the status of the active tab, when it is a table tab whose query (the one
/// applied, or on its way) has a condition or an order, or whose last run brought no rows; and
/// the spinner's frame while the run of its query is on its way.
fn status_of(app: &App) -> Option<(Vec<Part>, Option<&'static str>)> {
    let t = app.tab();
    if !t.is_table() {
        return None;
    }
    let tq = t.doc.table_query.as_ref()?;
    let glyph = crate::icons::filter_status(app.icons_on());
    if let Some(f) = tq.failure() {
        let word = match f.how {
            Unapplied::Refused => Label::TableStatusFailed,
            Unapplied::Cancelled => Label::TableStatusCancelled,
            Unapplied::Ended => Label::TableStatusUnapplied,
        };
        return Some((vec![Part::failed(glyph, &app.i18n.label(word))], None));
    }
    let q = tq.applied();
    let mut parts = Vec::new();
    if let Some(text) = filter_text(q) {
        // As the filter dialog's chips: each term of a proven conjunction, else the whole text.
        let target = app.table_target(t.id);
        let w = target.as_ref().map(|target| Where::from_text(text, target));
        let chips = w.as_ref().map(Where::chips).unwrap_or_default();
        let (first, count) = match chips.first() {
            Some(c) => (c.text, chips.len()),
            None => (text, 1),
        };
        parts.push(Part::filter(glyph, &crate::text::escape_controls(first.trim()), count));
    }
    let keys: Vec<_> = q.order.iter().filter(|k| k.origin == KeyOrigin::User).collect();
    if let Some(k) = keys.first() {
        parts.push(Part::order(&crate::text::escape_controls(&k.col), k.dir, keys.len()));
    }
    let running = t
        .exec
        .running
        .filter(|r| !r.count && !r.fetch && tq.requested.as_ref().is_some_and(|(id, _)| *id == r.id))
        .filter(|_| !tq.shows_applied())
        .map(|r| crate::widgets::spinner_at(r.started, app.now()));
    if parts.is_empty() && running.is_none() {
        return None;
    }
    Some((parts, running))
}

/// Draw the active table tab's status on the bottom border of its results pane `pane` (the
/// pane with its border), and keep where it is for the pointer.
pub(crate) fn draw_table_status(app: &mut App, pane: Rect, buf: &mut Buffer) {
    app.layout.table_status = Rect::default();
    if pane.height < 3 || pane.width < 8 {
        return;
    }
    let Some((parts, spinner)) = status_of(app) else { return };
    // Between the corners: a blank on both sides, and a dash of the border left on the right.
    let room = usize::from(pane.width - 5);
    let mut pieces = fit(&parts, spinner, room);
    if parts.is_empty()
        && let Some(s) = spinner
    {
        pieces = vec![(s.to_string(), Role::Spinner)];
    }
    let th = theme::cur();
    let hovered = app.pointer_hover() == Some(PointerOn::TableStatus);
    let style = |role: Role| {
        let s = match role {
            Role::Glyph | Role::Count | Role::Spinner => Style::new().fg(th.accent),
            Role::Text => Style::new().fg(th.fg),
            Role::Sep => Style::new().fg(th.fg_muted),
            Role::Warning => Style::new().fg(th.warning).add_modifier(Modifier::BOLD),
        };
        if hovered { s.patch(crate::widgets::pointer_style()) } else { s }
    };
    let y = pane.y + pane.height - 1;
    let x0 = pane.x + 1;
    let mut x = x0;
    x += put(buf, x, y, " ", 1, style(Role::Text));
    for (text, role) in &pieces {
        let left = usize::from((pane.x + pane.width - 2).saturating_sub(x));
        x += put(buf, x, y, text, left, style(*role));
    }
    if x < pane.x + pane.width - 2 {
        x += put(buf, x, y, " ", 1, style(Role::Text));
    }
    app.layout.table_status = Rect::new(x0, y, x - x0, 1);
}

#[cfg(test)]
mod tests;
