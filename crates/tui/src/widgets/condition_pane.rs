//! The condition pane of the filter dialog ([`crate::app::filter_dialog`]): the selected
//! column's name and type, its conditions (operator box, value fields, `×`), `+ add condition`,
//! its values ([`ValuesView`]) and its sort line; and the operator picker, an opaque box drawn
//! over what is under it. The whole dialog draws it as its right pane; a smaller view can draw
//! it in a rectangle of its own ([`PaneOpts`]). Where the parts are drawn goes into the
//! dialog's [`DialogHits`], in the screen's cells.

use super::filter_dialog::key;
use crate::app::App;
use crate::app::filter_dialog::draft::{Arity, Op, Term};
use crate::app::filter_dialog::list::{Heading, NotTicked, Source, ValuesView};
use crate::app::filter_dialog::{DialogAction, DialogFocus, DialogHits, FilterDialog, op_off_label};
use crate::text::width;
use crate::theme;
use crate::widgets::put;
use datarig_core::query::{Dir, RenderError};
use datarig_i18n::{Label, Msg};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Widget};

/// The width of an operator's box (`= ▾`, `≠ or NULL ▾`).
const OP_BOX: u16 = 14;

/// How the pane is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PaneOpts {
    /// The column is named by the border around the pane: no name and type row, no `Filter`
    /// heading; the conditions start on the pane's first row.
    pub(crate) title_in_border: bool,
    /// Short words where the pane is small: the sort line's short labels, the values' short
    /// heading (`~ loaded`) and short `count exactly`, whatever the width.
    pub(crate) compact: bool,
}

/// What drawing the pane leaves for the caller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PaneOut {
    /// The hardware cursor: the focused value field's.
    pub(crate) cursor: Option<(u16, u16)>,
    /// Where the open operator picker goes (its top left corner).
    pub(crate) picker_at: Option<(u16, u16)>,
    /// The first value of the list drawn.
    pub(crate) values_scroll: usize,
    /// The first line of the conditions drawn.
    pub(crate) rows_scroll: usize,
    /// Where the conditions and `+ add condition` are when they do not fit, with no heading to
    /// say it on (the border names the column): the first and last lines drawn (from 1) and
    /// how many there are.
    pub(crate) rows_window: Option<(usize, usize, usize)>,
}

/// An operator as the dialog names it.
pub(crate) fn op_label(app: &App, op: Op) -> String {
    let word = |l: Label| app.i18n.label(l).to_string();
    match op {
        Op::Eq => "=".into(),
        Op::Between => word(Label::TableDialogOpBetween),
        Op::Ne => "\u{2260}".into(),
        Op::NeOrNull => word(Label::TableDialogOpNeOrNull),
        Op::Lt => "<".into(),
        Op::Le => "\u{2264}".into(),
        Op::Gt => ">".into(),
        Op::Ge => "\u{2265}".into(),
        Op::Contains => word(Label::TableDialogOpContains),
        Op::StartsWith => word(Label::TableDialogOpStartsWith),
        Op::InList => word(Label::TableDialogOpInList),
        Op::NotInList => word(Label::TableDialogOpNotInList),
        Op::IsNull => word(Label::TableDialogOpIsNull),
        Op::IsNotNull => word(Label::TableDialogOpIsNotNull),
    }
}

/// Fills `r` with `style`, nothing of what was drawn under it kept (a style only adds
/// modifiers, so the grid's italics would show through).
pub(crate) fn fill(buf: &mut Buffer, r: Rect, style: Style) {
    for y in r.y..r.y + r.height {
        for x in r.x..r.x + r.width {
            let cell = &mut buf[(x, y)];
            cell.reset();
            cell.set_style(style);
        }
    }
}

/// Draw dialog `d`'s condition pane in `rect` (its sort line on the last row), with the values
/// of `view` when there is room for them, as `opts` say.
pub(crate) fn draw_condition_pane(
    app: &App,
    d: &FilterDialog,
    view: Option<&ValuesView>,
    rect: Rect,
    opts: PaneOpts,
    buf: &mut Buffer,
    hits: &mut DialogHits,
) -> PaneOut {
    let th = theme::cur();
    let i18n = &app.i18n;
    let muted = Style::new().fg(th.fg_muted);
    let bold = Style::new().fg(th.fg).add_modifier(Modifier::BOLD);
    let accent = Style::new().fg(th.accent);
    let field = Style::new().fg(th.fg).bg(th.surface_alt);
    let focus_style = Style::new().fg(th.fg).patch(th.selection);
    let (px, right) = (rect.x, rect.right());
    let pw = rect.width;
    let pane_rows_y = rect.y;
    let sort_y = rect.bottom().saturating_sub(1);
    // The conditions start under the name and type row and the `Filter` heading.
    let first_y = if opts.title_in_border { pane_rows_y } else { pane_rows_y + 3 };
    let pane_focus = !matches!(
        d.focus,
        DialogFocus::Columns
            | DialogFocus::Chips(_)
            | DialogFocus::Order(_)
            | DialogFocus::Limit
            | DialogFocus::Sql
            | DialogFocus::SqlWhere
            | DialogFocus::SqlOrder
            | DialogFocus::SqlOpen
    );
    let keys_n = d.draft.order.len();
    let mut out = PaneOut { values_scroll: d.values_scroll, rows_scroll: d.rows_scroll, ..PaneOut::default() };
    if let Some(c) = d.column() {
        if !opts.title_in_border {
            let hx = px
                + put(
                    buf,
                    px,
                    pane_rows_y,
                    &crate::text::escape_controls(&c.name),
                    usize::from(pw),
                    if pane_focus { bold.fg(th.accent) } else { bold },
                );
            let tx = hx
                + 2
                + put(buf, hx + 2, pane_rows_y, &c.type_name, usize::from(pw.saturating_sub(hx + 2 - px)), muted);
            // Hidden from the grid: still read, which matters for a key the rows are named by.
            if d.draft.hidden.contains(&c.name) {
                let (l, style) = match c.primary_key {
                    true => (Label::TableDialogHiddenKey, Style::new().fg(th.warning)),
                    false => (Label::TableDialogHiddenNote, muted),
                };
                put(buf, tx + 2, pane_rows_y, &i18n.label(l), usize::from(right.saturating_sub(tx + 2)), style);
            }
            put(buf, px, pane_rows_y + 2, &i18n.label(Label::TableDialogFilter), usize::from(pw), bold);
        }
        let mut y = first_y;
        let terms = d.draft.where_.terms();
        // The conditions and `+ add condition` scroll when they do not fit above the sort
        // line, following the line with the keys.
        let rows = d.rows();
        let (room, lines) = (usize::from(sort_y.saturating_sub(y)), rows.len() + 1);
        let top = rows_top(&rows, d.focus, d.rows_scroll, room);
        out.rows_scroll = top;
        // Where they are, on the `Filter` heading's right (no heading, no cue).
        if lines > room && room > 0 {
            let (from, to) = (top + 1, (top + room).min(lines));
            if opts.title_in_border {
                out.rows_window = Some((from, to, lines));
            } else {
                let cue = format!("{from}-{to}/{lines}");
                let w = width(&cue) as u16;
                put(buf, right.saturating_sub(w), pane_rows_y + 2, &cue, usize::from(w), muted);
            }
        }
        for &t in rows.iter().skip(top) {
            if y >= sort_y {
                break;
            }
            let term: &Term = &terms[t];
            let Some(row) = term.row.as_ref() else { continue };
            let focused_op = d.focus == DialogFocus::Op(t);
            let focused_line = matches!(d.focus, DialogFocus::Op(x) | DialogFocus::Value(x, _) if x == t);
            if focused_line {
                put(buf, px, y, "\u{25b8}", 1, accent);
            }
            let ob = Rect::new(px + 2, y, OP_BOX.min(pw.saturating_sub(4)), 1);
            fill(buf, ob, if focused_op { focus_style } else { field });
            put(
                buf,
                ob.x + 1,
                y,
                &op_label(app, row.op),
                usize::from(ob.width.saturating_sub(3)),
                if focused_op { focus_style } else { field },
            );
            put(
                buf,
                ob.x + ob.width - 2,
                y,
                "\u{25be}",
                1,
                if focused_op { focus_style } else { field.fg(th.fg_muted) },
            );
            hits.ops.push((ob, t));
            if focused_op && d.picker.is_some() {
                out.picker_at = Some((ob.x, y + 1));
            }
            let remove_x = right.saturating_sub(1);
            let vx = ob.x + ob.width + 2;
            let incomplete = !term.complete();
            // Why a row with its values is not written, where it is the value's; else it waits.
            let note = match (&term.problem, incomplete) {
                (_, false) => String::new(),
                (Some(RenderError::NotNumber(_)), _) => i18n.label(Label::TableDialogProblemNotNumber).to_string(),
                (Some(RenderError::InexactEquality(_)), _) => i18n.label(Label::TableDialogProblemInexact).to_string(),
                (Some(RenderError::NotExact(_)), _) => i18n.label(Label::TableDialogProblemNotExact).to_string(),
                (Some(RenderError::NotInstant(_)), _) => i18n.label(Label::TableDialogProblemNotInstant).to_string(),
                _ => i18n.label(Label::TableDialogIncomplete).to_string(),
            };
            let note_w = if incomplete { width(&note) as u16 + 2 } else { 0 };
            let vw = remove_x.saturating_sub(vx + 2 + note_w);
            let n = match row.op.arity() {
                Arity::Zero => 0,
                Arity::Two => 2,
                _ => 1,
            };
            let values = row.field_texts(d.dialect, &d.column_info(&row.col));
            // The condition the last refusal points into: its values underlined.
            let refused = d.mark.as_ref().is_some_and(|m| m.term(t));
            let text_style =
                if refused { field.fg(th.error).add_modifier(Modifier::UNDERLINED | Modifier::BOLD) } else { field };
            let and = format!(" {} ", i18n.label(Label::TableDialogAnd));
            let each = if n == 2 { vw.saturating_sub(width(&and) as u16) / 2 } else { vw };
            let mut fx = vx;
            for k in 0..n {
                let r = Rect::new(fx, y, each, 1);
                fill(buf, r, field);
                let focused = d.focus == DialogFocus::Value(t, k) && d.picker.is_none();
                if focused_line && let Some(input) = d.inputs.get(k) {
                    let mut input = input.clone();
                    let cx = input.render(
                        Rect::new(r.x + 1, y, r.width.saturating_sub(2), 1),
                        buf,
                        text_style,
                        focused,
                        false,
                        None,
                    );
                    if focused {
                        out.cursor = Some((cx, y));
                    }
                } else {
                    let text = if n == 2 {
                        values.get(k).cloned().unwrap_or_default()
                    } else {
                        row.input_text(d.dialect, &d.column_info(&row.col))
                    };
                    put(buf, r.x + 1, y, &text, usize::from(r.width.saturating_sub(2)), text_style);
                }
                hits.values.push((r, t, k));
                fx += each;
                if n == 2 && k == 0 {
                    fx += put(buf, fx, y, &and, width(&and), muted);
                }
            }
            if incomplete {
                put(buf, remove_x.saturating_sub(note_w), y, &note, usize::from(note_w), Style::new().fg(th.warning));
            }
            put(buf, remove_x, y, "\u{00d7}", 1, muted);
            hits.removes.push((Rect::new(remove_x, y, 1, 1), t));
            y += 1;
        }
        if y < sort_y {
            if d.focus == DialogFocus::Add {
                put(buf, px, y, "\u{25b8}", 1, accent);
            }
            let add = i18n.label(Label::TableDialogAdd).to_string();
            let aw = put(
                buf,
                px + 2,
                y,
                &add,
                usize::from(pw.saturating_sub(2)),
                if d.focus == DialogFocus::Add { focus_style } else { Style::new().fg(th.fg) },
            );
            hits.add = Rect::new(px + 2, y, aw, 1);
        }
        // The values, a blank line under `+ add condition`, up to the line above the sort line.
        if let Some(v) = view
            && y + 3 < sort_y
        {
            out.values_scroll = {
                let area = Rect::new(px, y + 2, right - px, sort_y - 1 - (y + 2));
                draw_values(app, d, v, area, opts.compact, buf, hits)
            };
        }
        // The sort line, on the pane's last row.
        if d.focus == DialogFocus::Sort {
            put(buf, px, sort_y, "\u{25b8}", 1, accent);
        }
        let mut sx = px + 2;
        sx += put(buf, sx, sort_y, &i18n.label(Label::TableDialogSort), 8, bold) + 3;
        let now = d.draft.sort_of(&c.name);
        let narrow = opts.compact || pw < 70;
        let choices = [
            (None, i18n.label(Label::TableDialogSortNone).to_string()),
            (
                Some(Dir::Asc),
                format!(
                    "\u{2191} {}",
                    i18n.label(if narrow { Label::TableDialogSortAscShort } else { Label::TableDialogSortAsc })
                ),
            ),
            (
                Some(Dir::Desc),
                format!(
                    "\u{2193} {}",
                    i18n.label(if narrow { Label::TableDialogSortDescShort } else { Label::TableDialogSortDesc })
                ),
            ),
        ];
        for (dir, label) in choices {
            let chosen = now.map(|(d, _)| d) == dir;
            let text = format!("({}) {label}", if chosen { "\u{2022}" } else { " " });
            let style = match (chosen, d.focus == DialogFocus::Sort) {
                (true, true) => focus_style.add_modifier(Modifier::BOLD),
                (true, false) => accent.add_modifier(Modifier::BOLD),
                _ => Style::new().fg(th.fg),
            };
            let w = put(buf, sx, sort_y, &text, usize::from(right.saturating_sub(sx)), style);
            hits.sort.push((Rect::new(sx, sort_y, w, 1), dir));
            sx += w + 3;
        }
        if let Some((_, place)) = now.filter(|_| keys_n > 1) {
            let p = i18n
                .msg(&Msg::TableDialogSortPlace { place: (place + 1).to_string(), keys: keys_n.to_string() })
                .to_string();
            sx += put(buf, sx, sort_y, &p, usize::from(right.saturating_sub(sx)), muted) + 3;
            if d.focus == DialogFocus::Sort {
                sx += put(
                    buf,
                    sx,
                    sort_y,
                    &i18n.label(Label::TableDialogSortMove),
                    usize::from(right.saturating_sub(sx)),
                    muted,
                ) + 3;
            }
        }
        // Where this server puts NULLs in the order chosen, where it fits whole.
        if let Some(k) = d.draft.order.iter().find(|k| k.col == c.name) {
            let l = if k.nulls_first(d.dialect) { Label::TableDialogNullsFirst } else { Label::TableDialogNullsLast };
            let note = i18n.label(l).to_string();
            if sx + width(&note) as u16 <= right {
                put(buf, sx, sort_y, &note, width(&note), muted);
            }
        }
    }
    // An operator picker whose row has no room is drawn where the conditions start.
    out.picker_at = out.picker_at.or_else(|| {
        d.picker.and_then(|_| matches!(d.focus, DialogFocus::Op(_)).then_some((px + 2, first_y.min(sort_y))))
    });
    out
}

/// Draw dialog `d`'s operator picker, opaque, with its top left corner at `at` (under the
/// operator's box): within rows `bound.y` to `bound.bottom()`, and left of `bound.right()`.
pub(crate) fn draw_op_picker(
    app: &App,
    d: &FilterDialog,
    (bx, by): (u16, u16),
    bound: Rect,
    buf: &mut Buffer,
    hits: &mut DialogHits,
) {
    let (Some(at), Some(ops)) = (d.picker, app.filter_dialog_ops()) else { return };
    let th = theme::cur();
    let i18n = &app.i18n;
    let muted = Style::new().fg(th.fg_muted);
    let accent = Style::new().fg(th.accent);
    let focus_style = Style::new().fg(th.fg).patch(th.selection);
    let right = bound.right().saturating_sub(1);
    let labels: Vec<(String, Option<&str>)> =
        ops.iter().map(|(op, _)| (op_label(app, *op), op.sql_hint(d.dialect))).collect();
    let lw = labels.iter().map(|(l, _)| width(l)).max().unwrap_or(4);
    let sw = labels.iter().filter_map(|(_, s)| s.map(width)).max().unwrap_or(0);
    let reason = ops.get(at).and_then(|(_, ok)| ok.err()).map(|off| i18n.label(op_off_label(off)).to_string());
    let rw = reason.as_ref().map_or(0, |r| width(r));
    let w = ((lw + 3 + sw).max(rw) + 4) as u16;
    let h = ops.len() as u16 + 2 + u16::from(reason.is_some());
    // Under its operator, down to the bound's last row, else above it where there is more
    // room there: the operator itself stays in view. Where the box is shorter than its
    // entries, they scroll with the selection.
    let op_y = by - 1;
    let (below, above) = (bound.bottom().saturating_sub(by), op_y.saturating_sub(bound.y));
    let (y, room) = if h <= below || below >= above { (by, below) } else { (op_y - h.min(above), above) };
    let h = h.min(room);
    let x = bx.min(right.saturating_sub(w));
    let r = Rect::new(x, y, w.min(right.saturating_sub(x) + 1), h);
    fill(buf, r, th.base());
    Block::bordered().border_type(BorderType::Rounded).border_style(accent).render(r, buf);
    hits.picker_box = r;
    let rows = usize::from(r.height.saturating_sub(2 + u16::from(reason.is_some())));
    let first = (at + 1).saturating_sub(rows);
    let entries = labels.iter().zip(ops.iter()).enumerate().skip(first).take(rows);
    for (k, (i, ((label, sql), (_, ok)))) in entries.enumerate() {
        let y = r.y + 1 + k as u16;
        let row = Rect::new(r.x + 1, y, r.width - 2, 1);
        let mut style = if ok.is_ok() { Style::new().fg(th.fg) } else { Style::new().fg(th.fg_dim) };
        if i == at {
            buf.set_style(row, focus_style);
            style = style.patch(focus_style);
        }
        put(buf, r.x + 2, y, label, lw, style);
        if let Some(s) = sql {
            put(buf, r.x + 2 + lw as u16 + 3, y, s, sw, if i == at { style } else { muted });
        }
        hits.picker.push((row, i));
    }
    if let Some(reason) = reason {
        put(
            buf,
            r.x + 2,
            r.y + r.height - 2,
            &reason,
            usize::from(r.width.saturating_sub(4)),
            Style::new().fg(th.warning),
        );
    }
}

/// The values of view `v` in `area` (its first row the heading, up to the row before its
/// bottom): the heading with what they are and `# count exactly` on its right,
/// then one value a line (tick box, value, rows, a bar or where it comes from), then what is
/// not listed; `compact`: the short heading (`~ loaded`) and `count exactly` whatever the
/// width. Returns the first value
/// shown (the focused one kept in view).
fn draw_values(
    app: &App,
    d: &FilterDialog,
    v: &ValuesView,
    area: Rect,
    compact: bool,
    buf: &mut Buffer,
    hits: &mut DialogHits,
) -> usize {
    let th = theme::cur();
    let i18n = &app.i18n;
    let (px, right, y, end) = (area.x, area.right(), area.y, area.bottom());
    let muted = Style::new().fg(th.fg_muted);
    let bold = Style::new().fg(th.fg).add_modifier(Modifier::BOLD);
    let focus_style = Style::new().fg(th.fg).patch(th.selection);
    let pw = right.saturating_sub(px);
    let focused = match d.focus {
        DialogFocus::Values(i) => Some(i),
        _ => None,
    };
    // The heading.
    let title = i18n.label(Label::TableDialogValues).to_string();
    let tw = put(buf, px, y, &title, usize::from(pw), if focused.is_some() { bold.fg(th.accent) } else { bold });
    let count_key = key(app, DialogAction::Count);
    let (action, action_style) = match v.counting {
        true => (i18n.label(Label::TableDialogValuesCounting).to_string(), Style::new().fg(th.accent)),
        false if compact || pw < 60 => {
            (i18n.msg(&Msg::TableDialogCountExactShort { key: count_key }).to_string(), muted)
        }
        false => (i18n.msg(&Msg::TableDialogCountExactly { key: count_key }).to_string(), muted),
    };
    // Dimmed where the server cannot count the column (its key says why).
    let action_style = if v.count_off.is_some() { Style::new().fg(th.fg_dim) } else { action_style };
    let aw = width(&action) as u16;
    let ax = right.saturating_sub(aw);
    put(buf, ax, y, &action, usize::from(aw), action_style);
    hits.count = Rect::new(ax, y, aw, 1);
    let (heading, heading_style) = match (&v.narrow, &v.heading) {
        (Some(text), _) => (i18n.msg(&Msg::TableDialogValuesContaining { text: text.clone() }).to_string(), muted),
        (None, Heading::Nothing(l)) => (i18n.label(*l).to_string(), muted),
        (None, Heading::Loaded { rows, held: Some(held), .. }) => (
            i18n.msg(&Msg::TableDialogValuesLoadedHeld { count: *held, total: datarig_i18n::fmt_count(*rows) })
                .to_string(),
            muted,
        ),
        // A popup's short words: `~ loaded`.
        (None, Heading::Loaded { earlier: false, .. }) if compact => {
            (i18n.label(Label::TableDialogValuesLoadedShort).to_string(), muted)
        }
        (None, Heading::Loaded { earlier: true, .. }) if compact => {
            (i18n.label(Label::TableDialogValuesLoadedEarlierShort).to_string(), muted)
        }
        (None, Heading::Loaded { rows, earlier: false, .. }) => {
            (i18n.msg(&Msg::TableDialogValuesLoaded { count: *rows }).to_string(), muted)
        }
        (None, Heading::Loaded { rows, earlier: true, .. }) => {
            (i18n.msg(&Msg::TableDialogValuesLoadedEarlier { count: *rows }).to_string(), muted)
        }
        (None, Heading::Exact { others: true }) => (i18n.label(Label::TableDialogValuesExact).to_string(), muted),
        (None, Heading::Exact { others: false }) => (i18n.label(Label::TableDialogValuesExactAll).to_string(), muted),
        (None, Heading::EarlierCount { others }) => {
            let l = if *others {
                Label::TableDialogValuesEarlierCount
            } else {
                Label::TableDialogValuesEarlierCountSession
            };
            (i18n.label(l).to_string(), Style::new().fg(th.warning))
        }
    };
    let hx = px + tw + 2;
    // What is not listed, under the values.
    let mut notes: Vec<(String, Style)> = Vec::new();
    if let Some((lo, hi, n)) = &v.range {
        let range = i18n.msg(&Msg::TableDialogValuesRange { min: lo.clone(), max: hi.clone(), count: *n });
        notes.push((range.to_string(), Style::new().fg(th.fg)));
        notes.push((i18n.label(Label::TableDialogValuesMostlyDistinct).to_string(), muted));
    }
    if v.items.is_empty() && v.range.is_none() && v.narrow.is_some() {
        notes.push((i18n.label(Label::TableDialogValuesNoMatch).to_string(), muted));
    }
    if v.long > 0 {
        notes.push((i18n.msg(&Msg::TableDialogValuesLongRows { count: v.long }).to_string(), muted));
    }
    if v.more {
        notes.push((i18n.label(Label::TableDialogValuesMore).to_string(), muted));
    }
    if v.kept_expr {
        notes.push((i18n.label(Label::TableDialogValuesKeptExpr).to_string(), muted));
    }
    let rows = usize::from(end.saturating_sub(y + 1));
    let shown = rows.saturating_sub(notes.len()).max(rows.min(1));
    let n = v.items.len();
    let mut scroll = d.values_scroll.min(n.saturating_sub(shown));
    if let Some(i) = focused {
        if i < scroll {
            scroll = i;
        } else if shown > 0 && i >= scroll + shown {
            scroll = i + 1 - shown;
        }
    }
    // Where the list is when it does not fit: `3-9 of 14`.
    let mut heading_end = ax.saturating_sub(2);
    if n > shown && shown > 0 {
        let cue = i18n
            .msg(&Msg::TableDialogValuesWindow {
                from: (scroll + 1).to_string(),
                to: (scroll + shown).min(n).to_string(),
                total: n.to_string(),
            })
            .to_string();
        let cw = width(&cue) as u16;
        if heading_end > hx + cw + 2 {
            put(buf, heading_end - cw, y, &cue, usize::from(cw), muted);
            heading_end -= cw + 2;
        }
    }
    put(buf, hx, y, &heading, usize::from(heading_end.saturating_sub(hx)), heading_style);
    // One value a line: the box at `px + 2`, the value, the rows right-aligned, then a bar.
    let counts: Vec<String> = v
        .items
        .iter()
        .map(|it| {
            let n = datarig_i18n::fmt_count(it.rows);
            if it.source == Source::Exact { n } else { format!("~{n}") }
        })
        .collect();
    let cw = counts.iter().skip(scroll).take(shown).map(|c| width(c)).max().unwrap_or(0) as u16;
    let name_w = (pw.saturating_sub(6) * 45 / 100).max(8);
    let count_end = (px + 6 + name_w + 1 + cw).min(right);
    let bar_x = count_end + 2;
    let bar_w = right.saturating_sub(bar_x + 1).min(12);
    let max_rows = v.items.iter().filter(|it| it.source != Source::Earlier).map(|it| it.rows).max().unwrap_or(1).max(1);
    for (k, it) in v.items.iter().enumerate().skip(scroll).take(shown) {
        let ly = y + 1 + (k - scroll) as u16;
        let line = Rect::new(px + 2, ly, right.saturating_sub(px + 3), 1);
        let foc = focused == Some(k);
        if foc {
            put(buf, px, ly, "\u{25b8}", 1, Style::new().fg(th.accent));
            buf.set_style(line, focus_style);
        }
        let on = it.tick.as_ref().is_ok_and(|val| v.ticked.contains(val));
        let base = if foc { focus_style } else { Style::new().fg(th.fg) };
        let box_style = match (on, it.tick.is_ok()) {
            (true, _) => base.fg(th.accent).add_modifier(Modifier::BOLD),
            (false, false) => base.fg(th.fg_dim),
            (false, true) => base,
        };
        put(buf, px + 2, ly, if on { "[x]" } else { "[ ]" }, 3, box_style);
        let (text, style) = match (&it.value, &it.tick) {
            (Some(s), _) => (crate::text::escape_controls(s), base),
            (None, Err(NotTicked::Long(bytes))) => {
                (i18n.msg(&Msg::TableDialogValuesLong { bytes: bytes.to_string() }).to_string(), base.fg(th.fg_muted))
            }
            (None, Err(NotTicked::Inexact(bytes))) => (
                i18n.msg(&Msg::TableDialogValuesInexact { bytes: bytes.to_string() }).to_string(),
                base.fg(th.fg_muted),
            ),
            (None, _) => ("NULL".to_string(), base.fg(th.fg_muted)),
        };
        put(buf, px + 6, ly, &text, usize::from(name_w), style);
        let c = &counts[k];
        let w = width(c) as u16;
        put(buf, count_end.saturating_sub(w), ly, c, usize::from(w), base.fg(th.fg_muted));
        let mark = match (&it.source, &it.tick) {
            (Source::Earlier, _) => Some(i18n.label(Label::TableDialogValuesEarlierLoad).to_string()),
            (_, Err(NotTicked::Prefix)) => Some(i18n.label(Label::TableDialogValuesPrefix).to_string()),
            _ => None,
        };
        match mark {
            Some(m) => {
                put(buf, bar_x, ly, &m, usize::from(right.saturating_sub(bar_x + 1)), base.fg(th.fg_muted));
            }
            None if bar_w > 0 => {
                let len = ((it.rows * u64::from(bar_w)).div_ceil(max_rows)).clamp(1, u64::from(bar_w)) as usize;
                put(buf, bar_x, ly, &"\u{2584}".repeat(len), len, base.fg(th.fg_dim));
            }
            None => {}
        }
        hits.items.push((line, k));
    }
    for (i, (text, style)) in notes.iter().enumerate() {
        let ny = y + 1 + (n.saturating_sub(scroll).min(shown) + i) as u16;
        if ny >= end {
            break;
        }
        put(buf, px + 2, ny, text, usize::from(pw.saturating_sub(2)), *style);
    }
    scroll
}

/// The first of the lines of the selected column's conditions (its rows `rows`, then
/// `+ add condition`) drawn in `room` rows: from `top` (where they were drawn from), moved as
/// little as it takes for the line with the keys (`focus`) to be drawn, and no further down
/// than the last lines filling the room.
pub(crate) fn rows_top(rows: &[usize], focus: DialogFocus, top: usize, room: usize) -> usize {
    let lines = rows.len() + 1;
    if lines <= room || room == 0 {
        return 0;
    }
    let mut top = top.min(lines - room);
    let at = match focus {
        DialogFocus::Op(t) | DialogFocus::Value(t, _) => rows.iter().position(|&r| r == t),
        DialogFocus::Add => Some(rows.len()),
        _ => None,
    };
    if let Some(at) = at {
        if at < top {
            top = at;
        } else if at >= top + room {
            top = at + 1 - room;
        }
    }
    top
}

#[cfg(test)]
mod tests;
