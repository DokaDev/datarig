//! The filter and sort dialog of a table tab ([`crate::app::filter_dialog`]), over the whole
//! screen: the title with what is loaded, the table's columns on the left, the selected
//! column's conditions and sort line on the right, and a footer of three rows: the `Where`
//! chips with `undo` and `clear all`, the `Order` keys with the `Limit` field and the `SQL`
//! toggle, and one hint line with `Enter apply` and `Esc cancel`. The operator picker is an
//! opaque box over the right pane.
//!
//! The accent marks only the focus and what is applied (filter and sort marks, chip glyphs);
//! the warning colour only an expression chip and a condition still waiting for its value;
//! counts, types and the hints' words are muted.

use crate::app::App;
use crate::app::PopupOwner;
use crate::app::action::Action;
use crate::app::filter_dialog::draft::{Op, Term};
use crate::app::filter_dialog::list::{Heading, NotTicked, Source, ValuesView};
use crate::app::filter_dialog::{
    ColumnSource, DialogAction, DialogFocus, DialogHit, DialogHits, FilterDialog, Mark, op_off_label,
};
use crate::icons;
use crate::keymap::Ctx;
use crate::text::{clip, width};
use crate::theme;
use crate::widgets::popup::draw_popup;
use crate::widgets::put;
use crate::widgets::statusbar::level_color;
use crate::widgets::text_input::TextInput;
use datarig_core::query::{self, Dir, RenderError};
use datarig_core::sql::dialect::Dialect;
use datarig_core::sql::lexer::{Tok, lex_in};
use datarig_i18n::{Label, Msg};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Widget};
use std::ops::Range;

/// The width the column list takes from below which the types are left out.
const TYPES_MIN: u16 = 34;

/// The width of an operator's box (`= ▾`, `≠ or NULL ▾`).
const OP_BOX: u16 = 14;

/// The highlight of `text` as the editor's: each token's byte range and style, after the part
/// `mark` (bytes) underlined in the error colour, which wins over them.
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

/// The key `a` is bound to in the dialog.
fn key(app: &App, a: crate::app::filter_dialog::DialogAction) -> String {
    app.key_for(Action::FilterDialog(a), Ctx::FilterDialog)
}

/// `ty` as the column list writes it: a PostgreSQL type by its short name where it has one
/// (`timestamptz`, `varchar(20)`, `float8`), so a cut does not take what tells types apart;
/// anything else as written.
fn short_type(d: Dialect, ty: &str) -> String {
    if d != Dialect::Postgres {
        return ty.to_string();
    }
    let base = ty.trim_end_matches("[]");
    let arrays = &ty[base.len()..];
    for head in ["timestamp", "time"] {
        let Some(rest) = base.strip_prefix(head) else { continue };
        let (precision, zone) = rest.split_at(rest.find(' ').unwrap_or(rest.len()));
        if !(precision.is_empty() || precision.starts_with('(')) {
            continue;
        }
        match zone {
            " with time zone" => return format!("{head}tz{precision}{arrays}"),
            " without time zone" => return format!("{head}{precision}{arrays}"),
            _ => {}
        }
    }
    for (long, short) in [
        ("character varying", "varchar"),
        ("character", "char"),
        ("double precision", "float8"),
        ("bit varying", "varbit"),
    ] {
        if let Some(rest) = base.strip_prefix(long)
            && (rest.is_empty() || rest.starts_with('('))
        {
            return format!("{short}{rest}{arrays}");
        }
    }
    ty.to_string()
}

/// Fills `r` with `style`, nothing of what was drawn under it kept (a style only adds
/// modifiers, so the grid's italics would show through).
fn fill(buf: &mut Buffer, r: Rect, style: Style) {
    for y in r.y..r.y + r.height {
        for x in r.x..r.x + r.width {
            let cell = &mut buf[(x, y)];
            cell.reset();
            cell.set_style(style);
        }
    }
}

/// Draw the filter dialog over `area` (the whole screen); returns the hardware cursor (the
/// focused text field's).
pub(crate) fn draw_filter_dialog(app: &mut App, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    // The selected column's values as things are now.
    let view = app.filter_dialog_list();
    let d = app.filter_dialog()?;
    if area.width < 30 || area.height < 10 {
        return None;
    }
    let th = theme::cur();
    let tab = d.tab;
    let target = app.table_target(tab);
    let ops = app.filter_dialog_ops();
    let i18n = &app.i18n;
    let on = app.icons_on();
    let base = th.base();
    let muted = Style::new().fg(th.fg_muted);
    let bold = Style::new().fg(th.fg).add_modifier(Modifier::BOLD);
    let accent = Style::new().fg(th.accent);
    let field = Style::new().fg(th.fg).bg(th.surface_alt);
    let focus_style = Style::new().fg(th.fg).patch(th.selection);
    fill(buf, area, base);
    Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(th.border)).render(area, buf);
    let inner = Rect::new(area.x + 1, area.y + 1, area.width - 2, area.height - 2);
    let mut hits = DialogHits::default();
    let mut cursor = None;
    let (x0, right) = (inner.x + 1, inner.x + inner.width - 1);

    // The title on the top border, what is loaded on its right.
    let title = i18n.msg(&Msg::TableDialogTitle { table: d.table.clone() }).to_string();
    let mut tx = area.x + 2;
    tx += put(buf, tx, area.y, &format!(" {title} "), usize::from(area.width.saturating_sub(4)), bold);
    if d.edited() {
        put(buf, tx, area.y, &format!("{} ", i18n.label(Label::TableDialogEdited)), 12, muted);
    }
    if let Some(n) = d.loaded {
        let loaded = format!(" {} ", i18n.msg(&Msg::TableDialogLoaded { count: n as u64 }));
        let lw = width(&loaded) as u16;
        if tx + 14 + lw < area.x + area.width {
            put(buf, area.x + area.width - 2 - lw, area.y, &loaded, usize::from(lw), muted);
        }
    }

    // The footer: 3 rows, the SQL section between its second and its last while open.
    let base_q = app.table_query(tab).map(|tq| tq.applied().clone());
    let preview = match (d.sql.as_ref(), target.as_ref(), base_q.as_ref()) {
        (Some(_), Some(t), Some(q)) => Some(sql_preview(d, t, q)),
        _ => None,
    };
    let sql_lay =
        preview.as_ref().map(|p| sql_layout(d, p.as_ref().ok().map(|p| p.0.as_str()), inner, x0, right, i18n));
    let sql_rows = sql_lay.as_ref().map_or(0, |l| l.rows);
    let hy = inner.y + inner.height - 1;
    let foot_y = hy - 2 - sql_rows;
    let sep_y = foot_y - 1;
    let body = Rect::new(inner.x, inner.y, inner.width, sep_y - inner.y);
    for x in area.x..area.x + area.width {
        buf[(x, sep_y)].set_symbol("\u{2500}").set_style(Style::new().fg(th.border));
    }
    buf[(area.x, sep_y)].set_symbol("\u{251c}");
    buf[(area.x + area.width - 1, sep_y)].set_symbol("\u{2524}");

    // The panes.
    let left_w = (inner.width * 34 / 100).clamp(22, 44);
    let split = inner.x + left_w;
    for y in body.y..sep_y {
        buf[(split, y)].set_symbol("\u{2502}").set_style(Style::new().fg(th.border));
    }
    buf[(split, sep_y)].set_symbol("\u{2534}");
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
    let list_focus = d.focus == DialogFocus::Columns;

    // Left: the columns.
    let lw = usize::from(left_w.saturating_sub(2));
    let shown = d.shown_columns();
    let heading = i18n.msg(&Msg::TableDialogColumns { total: d.columns.len().to_string() }).to_string();
    put(buf, x0, body.y, &heading, lw, if list_focus { bold.fg(th.accent) } else { bold });
    let side = match &d.search {
        Some(_) => i18n
            .msg(&Msg::TableDialogSearchCount { shown: shown.len().to_string(), total: d.columns.len().to_string() })
            .to_string(),
        None => i18n.label(Label::TableDialogSearch).to_string(),
    };
    let sw = width(&side) as u16;
    if x0 + width(&heading) as u16 + 2 + sw < split {
        put(buf, split - 1 - sw, body.y, &side, usize::from(sw), muted);
    }
    if let Some(s) = &d.search {
        put(buf, x0, body.y + 1, "/", 1, accent);
        let mut s = s.clone();
        let r = Rect::new(x0 + 2, body.y + 1, left_w.saturating_sub(4), 1);
        let cx = s.render(r, buf, base, list_focus, false, None);
        if list_focus && d.picker.is_none() {
            cursor = Some((cx, r.y));
        }
    } else {
        let note = match &d.source {
            ColumnSource::Catalogue => None,
            ColumnSource::Result => Some(i18n.label(Label::TableDialogFromResult).to_string()),
            ColumnSource::ResultFailed(why) => {
                Some(i18n.msg(&Msg::TableDialogFromResultFailed { why: why.clone() }).to_string())
            }
        };
        if let Some(n) = note {
            put(buf, x0, body.y + 1, &n, lw, muted);
        }
    }
    let list_y = body.y + 2;
    let mut rows_h = usize::from(body.height.saturating_sub(2));
    let scrolls = shown.len() > rows_h;
    if scrolls {
        rows_h = rows_h.saturating_sub(1);
    }
    let mut scroll = d.scroll.min(shown.len().saturating_sub(rows_h));
    if d.cursor < scroll {
        scroll = d.cursor;
    } else if rows_h > 0 && d.cursor >= scroll + rows_h {
        scroll = d.cursor + 1 - rows_h;
    }
    let types = left_w >= TYPES_MIN;
    let marks_x = split - 6;
    let name_w = if types { usize::from(left_w) * 52 / 100 - 5 } else { usize::from(marks_x.saturating_sub(x0 + 4)) };
    let keys_n = d.draft.order.len();
    for (row, &ci) in shown.iter().enumerate().skip(scroll).take(rows_h) {
        let c = &d.columns[ci];
        let y = list_y + (row - scroll) as u16;
        let selected = row == d.cursor;
        let line = Rect::new(inner.x, y, left_w, 1);
        if selected {
            buf.set_style(line, if list_focus { focus_style } else { th.selection_unfocused() });
            put(buf, inner.x, y, "\u{25b8}", 1, accent);
        }
        hits.columns.push((line, row));
        let key = if c.primary_key {
            Some((icons::KeyMark::Pk.text(on), th.key_pk))
        } else if c.foreign_key {
            Some((icons::KeyMark::Fk.text(on), th.key_fk))
        } else {
            None
        };
        if let Some((k, color)) = key {
            put(buf, x0 + 1, y, k, 2, Style::new().fg(color));
        }
        let name_style = if selected { bold } else { Style::new().fg(th.fg) };
        put(buf, x0 + 4, y, &crate::text::escape_controls(&c.name), name_w, name_style);
        if types {
            let tx = x0 + 4 + name_w as u16 + 1;
            // A column `SELECT *` leaves out (MySQL INVISIBLE): filtered and sorted, never loaded.
            let ty = if c.invisible {
                i18n.label(Label::TableDialogInvisible).to_string()
            } else {
                short_type(d.dialect, &c.type_name)
            };
            put(buf, tx, y, &ty, usize::from(marks_x.saturating_sub(tx + 1)), muted);
        }
        if !d.draft.where_.rows_of(&c.name).iter().all(|&t| !d.draft.where_.terms()[t].complete()) {
            put(buf, marks_x, y, icons::column_filter(on), 1, accent);
        }
        if let Some((dir, place)) = d.draft.sort_of(&c.name) {
            let arrow = icons::sort_mark(dir == Dir::Asc, 1, on);
            let mark = if keys_n > 1 { format!("{arrow}{}", place + 1) } else { arrow };
            put(buf, marks_x + 2, y, &mark, 3, accent);
        }
    }
    if scrolls {
        let cue = format!("{}/{}", d.cursor + 1, shown.len());
        let w = width(&cue) as u16;
        put(buf, split.saturating_sub(w + 1), list_y + rows_h as u16, &cue, usize::from(w), muted);
    }

    // Right: the selected column's conditions and sort line.
    let px = split + 2;
    let pw = right.saturating_sub(px);
    let pane_rows_y = body.y;
    let sort_y = sep_y - 1;
    let mut picker_at = None;
    let mut values_scroll = d.values_scroll;
    let mut rows_scroll = d.rows_scroll;
    if let Some(c) = d.column() {
        let hx = px
            + put(
                buf,
                px,
                pane_rows_y,
                &crate::text::escape_controls(&c.name),
                usize::from(pw),
                if pane_focus { bold.fg(th.accent) } else { bold },
            );
        put(buf, hx + 2, pane_rows_y, &c.type_name, usize::from(pw.saturating_sub(hx + 2 - px)), muted);
        put(buf, px, pane_rows_y + 2, &i18n.label(Label::TableDialogFilter), usize::from(pw), bold);
        let mut y = pane_rows_y + 3;
        let terms = d.draft.where_.terms();
        // The conditions and `+ add condition` scroll when they do not fit above the sort
        // line, following the line with the keys.
        let rows = d.rows();
        let (room, lines) = (usize::from(sort_y.saturating_sub(y)), rows.len() + 1);
        let top = rows_top(&rows, d.focus, d.rows_scroll, room);
        rows_scroll = top;
        if lines > room && room > 0 {
            let cue = format!("{}-{}/{lines}", top + 1, (top + room).min(lines));
            let w = width(&cue) as u16;
            put(buf, right.saturating_sub(w), pane_rows_y + 2, &cue, usize::from(w), muted);
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
                picker_at = Some((ob.x, y + 1));
            }
            let remove_x = right.saturating_sub(1);
            let vx = ob.x + ob.width + 2;
            let incomplete = !term.complete();
            // Why a row with its values is not written, where it is the value's; else it waits.
            let note = match (&term.problem, incomplete) {
                (_, false) => String::new(),
                (Some(RenderError::NotNumber(_)), _) => i18n.label(Label::TableDialogProblemNotNumber).to_string(),
                (Some(RenderError::InexactEquality(_)), _) => i18n.label(Label::TableDialogProblemInexact).to_string(),
                _ => i18n.label(Label::TableDialogIncomplete).to_string(),
            };
            let note_w = if incomplete { width(&note) as u16 + 2 } else { 0 };
            let vw = remove_x.saturating_sub(vx + 2 + note_w);
            let n = match row.op.arity() {
                crate::app::filter_dialog::draft::Arity::Zero => 0,
                crate::app::filter_dialog::draft::Arity::Two => 2,
                _ => 1,
            };
            let values = row.field_texts(d.dialect, &d.column_info(&row.col));
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
                        field,
                        focused,
                        false,
                        None,
                    );
                    if focused {
                        cursor = Some((cx, y));
                    }
                } else {
                    let text = if n == 2 {
                        values.get(k).cloned().unwrap_or_default()
                    } else {
                        row.input_text(d.dialect, &d.column_info(&row.col))
                    };
                    put(buf, r.x + 1, y, &text, usize::from(r.width.saturating_sub(2)), field);
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
        if let Some(v) = &view
            && y + 3 < sort_y
        {
            values_scroll = draw_values(app, d, v, (px, right), (y + 2, sort_y - 1), buf, &mut hits);
        }
        // The sort line, on the pane's last row.
        if d.focus == DialogFocus::Sort {
            put(buf, px, sort_y, "\u{25b8}", 1, accent);
        }
        let mut sx = px + 2;
        sx += put(buf, sx, sort_y, &i18n.label(Label::TableDialogSort), 8, bold) + 3;
        let now = d.draft.sort_of(&c.name);
        let narrow = pw < 70;
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

    // The footer: `Where` chips, `undo`, `clear all`.
    let (wy, oy) = (foot_y, foot_y + 1);
    let label_w =
        [Label::TableDialogWhere, Label::TableDialogOrder].iter().map(|l| width(&i18n.label(*l))).max().unwrap_or(5)
            as u16
            + 2;
    put(buf, x0, wy, &i18n.label(Label::TableDialogWhere), usize::from(label_w), bold);
    let undo = format!(" {} ", i18n.label(Label::TableDialogUndo));
    let clear = format!(" {} ", i18n.label(Label::TableDialogClearAll));
    let (uw, cw) = (width(&undo) as u16, width(&clear) as u16);
    let clear_x = right.saturating_sub(cw);
    let undo_x = clear_x.saturating_sub(uw + 1);
    let button = Style::new().fg(th.fg).bg(th.surface_alt);
    let undoable = !d.undo.is_empty() || d.before_last.is_some();
    put(buf, undo_x, wy, &undo, usize::from(uw), if undoable { button } else { button.fg(th.fg_dim) });
    put(buf, clear_x, wy, &clear, usize::from(cw), button);
    hits.undo = Rect::new(undo_x, wy, uw, 1);
    hits.clear_all = Rect::new(clear_x, wy, cw, 1);
    let chips = d.draft.where_.chips();
    let mut cx = x0 + label_w;
    let room_end = undo_x.saturating_sub(2);
    let glyph = icons::column_filter(on);
    for (i, chip) in chips.iter().enumerate() {
        let left = chips.len() - i;
        let more = i18n.msg(&Msg::TableDialogMore { hidden: left.to_string() }).to_string();
        let room = room_end.saturating_sub(cx);
        let expr = if chip.expr { format!("{} ", i18n.label(Label::TableDialogExpr)) } else { String::new() };
        let text = crate::text::escape_controls(chip.text);
        // ` ▼ ` + text + ` × `, then two blanks to the next.
        let fixed = (4 + width(&expr) + 3) as u16;
        let want = fixed + width(&text) as u16;
        let reserve = if left > 1 { width(&more) as u16 + 1 } else { 0 };
        let fits_cut = room > fixed + 8 + reserve;
        if want + reserve > room && !fits_cut {
            put(buf, cx, wy, &more, usize::from(room), muted);
            break;
        }
        let tw = if want + reserve <= room { width(&text) } else { usize::from(room - fixed - reserve) };
        let focused = d.focus == DialogFocus::Chips(i);
        let marked = d.mark.as_ref().is_some_and(|m| m.term(chip.index));
        let bg = if focused { focus_style } else { button };
        let start = cx;
        cx += put(buf, cx, wy, &format!(" {glyph} "), 3, bg.fg(if focused { th.fg } else { th.accent }));
        if chip.expr {
            cx += put(buf, cx, wy, &expr, width(&expr), bg.fg(th.warning));
        }
        let mut ts = bg;
        if marked {
            ts = ts.fg(th.error).add_modifier(Modifier::UNDERLINED | Modifier::BOLD);
        }
        cx += put(buf, cx, wy, &clip(&text, tw), tw, ts);
        let x_at = cx + 1;
        cx += put(buf, cx, wy, " \u{00d7} ", 3, bg.fg(th.fg_muted));
        hits.chips.push((Rect::new(start, wy, cx - start, 1), i));
        hits.chip_removes.push((Rect::new(x_at, wy, 1, 1), i));
        cx += 2;
    }

    // `Order` keys, `Limit` field, `SQL` toggle.
    put(buf, x0, oy, &i18n.label(Label::TableDialogOrder), usize::from(label_w), bold);
    let sql =
        format!(" {} {} ", i18n.label(Label::TableDialogSql), if d.sql.is_some() { "\u{25be}" } else { "\u{25b8}" });
    let qw = width(&sql) as u16;
    let sql_x = right.saturating_sub(qw);
    put(buf, sql_x, oy, &sql, usize::from(qw), if d.focus == DialogFocus::Sql { focus_style } else { button });
    hits.sql = Rect::new(sql_x, oy, qw, 1);
    let limit_w: u16 = 10;
    let limit_x = sql_x.saturating_sub(limit_w + 2);
    let limit_label = i18n.label(Label::TableDialogLimit).to_string();
    let llw = width(&limit_label) as u16;
    put(buf, limit_x.saturating_sub(llw + 1), oy, &limit_label, usize::from(llw), bold);
    let lr = Rect::new(limit_x, oy, limit_w, 1);
    let limit_focus = d.focus == DialogFocus::Limit && d.picker.is_none();
    fill(buf, lr, field);
    // The part of the limit refused, underlined.
    let limit_marks: Vec<(Range<usize>, Style)> = match &d.mark {
        Some(Mark::Limit(at)) => {
            let at = if at.is_empty() { 0..d.limit.text().len() } else { at.clone() };
            vec![(at, Style::new().fg(th.error).add_modifier(Modifier::UNDERLINED | Modifier::BOLD))]
        }
        _ => Vec::new(),
    };
    let mut limit = d.limit.clone();
    let lx = limit.render_spans(Rect::new(lr.x + 1, oy, lr.width - 2, 1), buf, field, limit_focus, &limit_marks);
    if limit_focus {
        cursor = Some((lx, oy));
    }
    hits.limit = lr;
    let mut kx = x0 + label_w;
    let keys_end = limit_x.saturating_sub(llw + 3);
    // What makes the order total (`+ id`), or that nothing does: kept in view, the keys cut.
    let note = (!d.draft.order.is_empty())
        .then(|| {
            let (target, q) = (target.as_ref()?, app.table_query(tab).map(|tq| tq.applied().clone())?);
            let c = query::compile(&d.draft.to_query(&q, target).ok()?, target);
            let ties: Vec<&str> =
                c.order.iter().filter(|k| k.origin == query::KeyOrigin::TieBreak).map(|k| k.col.as_str()).collect();
            let long = i18n.label(Label::TableOrderNotUnique).to_string();
            match (ties.is_empty(), c.total) {
                (false, _) => Some(format!("+ {}", ties.join(", "))),
                (true, true) => None,
                (true, false) if usize::from(keys_end.saturating_sub(kx)) >= 30 + width(&long) => Some(long),
                (true, false) => Some(i18n.label(Label::TableOrderNotUniqueShort).to_string()),
            }
        })
        .flatten();
    let note_w = note.as_ref().map_or(0, |n| width(n) as u16 + 2);
    let names_end = keys_end.saturating_sub(note_w);
    for (i, k) in d.draft.order.iter().enumerate() {
        let arrow = if k.dir == Dir::Desc { "\u{2193}" } else { "\u{2191}" };
        let name = crate::text::escape_controls(&k.col);
        let room = usize::from(names_end.saturating_sub(kx + 2));
        if room < 2 {
            put(buf, kx, oy, "\u{2026}", 1, muted);
            kx += 2;
            break;
        }
        let shown = clip(&name, room);
        let focused = d.focus == DialogFocus::Order(i);
        let start = kx;
        kx += put(buf, kx, oy, &shown, width(&shown), if focused { focus_style } else { Style::new().fg(th.fg) });
        kx += put(buf, kx, oy, &format!(" {arrow}"), 2, if focused { focus_style } else { accent });
        hits.keys.push((Rect::new(start, oy, kx - start, 1), i));
        kx += 3;
        if shown != name {
            break;
        }
    }
    if let Some(n) = note {
        put(buf, kx.min(names_end + 1), oy, &n, usize::from(keys_end.saturating_sub(kx.min(names_end + 1))), muted);
    }

    // The SQL section, while open.
    if let (Some(lay), Some(p)) = (sql_lay.as_ref(), preview.as_ref()) {
        let at = draw_sql(app, d, lay, p.as_ref(), (x0, right, oy + 1), buf, &mut hits);
        cursor = at.or(cursor);
    }

    // The hint line: what the dialog says, else the keys of the focused part; then the
    // buttons.
    let apply = format!(" {} ", i18n.msg(&Msg::TableDialogApply { key: key(app, DialogAction::Apply) }));
    let cancel = format!(" {} ", i18n.msg(&Msg::TableDialogCancel { key: key(app, DialogAction::Cancel) }));
    let (aw, xw) = (width(&apply) as u16, width(&cancel) as u16);
    let cancel_x = right.saturating_sub(xw);
    let apply_x = cancel_x.saturating_sub(aw + 1);
    put(buf, apply_x, hy, &apply, usize::from(aw), Style::new().fg(th.bg).bg(th.accent).add_modifier(Modifier::BOLD));
    put(buf, cancel_x, hy, &cancel, usize::from(xw), button);
    hits.apply = Rect::new(apply_x, hy, aw, 1);
    hits.cancel = Rect::new(cancel_x, hy, xw, 1);
    let hint_w = usize::from(apply_x.saturating_sub(x0 + 1));
    let said = d.message.clone().or_else(|| d.applying.as_ref().and(app.status.clone()));
    match said {
        Some(n) => {
            put(buf, x0, hy, &n.render(i18n), hint_w, Style::new().fg(level_color(n.level)));
        }
        None => draw_hints(app, d, view.as_ref(), x0, hy, hint_w, buf),
    }

    // The operator picker, opaque, over the pane.
    // An operator picker whose row has no room is drawn under the pane's heading.
    let picker_at = picker_at.or_else(|| {
        d.picker.and_then(|_| matches!(d.focus, DialogFocus::Op(_)).then_some((px + 2, (pane_rows_y + 3).min(sort_y))))
    });
    if let (Some((bx, by)), Some(at), Some(ops)) = (picker_at, d.picker, ops) {
        let labels: Vec<(String, Option<&str>)> =
            ops.iter().map(|(op, _)| (op_label(app, *op), op.sql_hint(d.dialect))).collect();
        let lw = labels.iter().map(|(l, _)| width(l)).max().unwrap_or(4);
        let sw = labels.iter().filter_map(|(_, s)| s.map(width)).max().unwrap_or(0);
        let reason = ops.get(at).and_then(|(_, ok)| ok.err()).map(|off| i18n.label(op_off_label(off)).to_string());
        let rw = reason.as_ref().map_or(0, |r| width(r));
        let w = ((lw + 3 + sw).max(rw) + 4) as u16;
        let h = ops.len() as u16 + 2 + u16::from(reason.is_some());
        // Under its operator, down to the line above the hint line (over the footer), else above
        // it where there is more room there: the operator itself stays in view. Where the box
        // is shorter than its entries, they scroll with the selection.
        let op_y = by - 1;
        let (below, above) = (hy.saturating_sub(by), op_y.saturating_sub(body.y));
        let (y, room) = if h <= below || below >= above { (by, below) } else { (op_y - h.min(above), above) };
        let h = h.min(room);
        let x = bx.min(right.saturating_sub(w));
        let r = Rect::new(x, y, w.min(right.saturating_sub(x) + 1), h);
        fill(buf, r, base);
        Block::bordered().border_type(BorderType::Rounded).border_style(accent).render(r, buf);
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

    // What the pointer is over, lit: a row as a list's row under the pointer, a button or a
    // mark as a small target; none once it is not drawn any more (over the picker, only its
    // entries).
    let lit = d.hover.filter(|h| d.picker.is_none() || matches!(h, DialogHit::Pick(_)));
    let lit = lit.and_then(|h| hits.rect(h).map(|r| (h, r)));
    if let Some((h, r)) = lit {
        buf.set_style(r, if h.row() { crate::widgets::hover_style() } else { crate::widgets::pointer_style() });
    }
    let top = app.overlays.top().is_some_and(|o| o.kind() == crate::app::overlay::OverlayKind::FilterDialog);
    let now = app.now();
    if let Some(d) = app.filter_dialog_mut() {
        d.hits = hits;
        d.scroll = scroll;
        d.values_scroll = values_scroll;
        d.rows_scroll = rows_scroll;
        d.hover = lit.map(|(h, _)| h);
        d.press.drawn(top, now);
    }
    // The completion popup of the SQL section's field with the keys, at its cursor.
    let field = app.filter_dialog_sql_field();
    if let (Some(at), Some(f)) = (cursor, field)
        && app.tab().popup.as_ref().is_some_and(|p| p.owner == PopupOwner::Dialog(f))
    {
        draw_popup(app, at, area, buf);
    }
    cursor.filter(|_| top)
}

/// The SQL section's statement, as [`sql_preview`] writes it: `Err` when the `LIMIT` field is
/// no cap.
type Preview = Result<(String, Option<Range<usize>>), ()>;

/// The statement the draft sends with what the SQL section's fields hold now (a field typed in
/// is taken as it would be read back; an order that does not read leaves the draft's keys), and
/// the bytes of the key the app appends to make the order total (the primary key's columns).
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

fn sql_preview(d: &FilterDialog, target: &query::Target, base: &query::Query) -> Preview {
    let mut draft = d.draft.clone();
    if let Some(s) = &d.sql {
        if s.where_.edited() {
            draft.where_.set_text(s.where_.input.text(), target);
        }
        if s.order.edited()
            && let Ok(keys) = query::parse_order(s.order.input.text(), target)
        {
            draft.order = keys;
        }
    }
    let q = draft.to_query(base, target).map_err(|_| ())?;
    let c = query::compile(&q, target);
    let ties: Vec<String> = c
        .order
        .iter()
        .filter(|k| k.origin == query::KeyOrigin::TieBreak)
        .map(|k| format!("{} ASC", target.dialect.force_quote_ident(&k.col)))
        .collect();
    let suffix = ties.join(", ");
    let tie =
        (!ties.is_empty() && c.sql.ends_with(&format!(", {suffix}"))).then(|| c.sql.len() - suffix.len()..c.sql.len());
    Ok((c.sql, tie))
}

/// The byte ranges of `text`'s lines when it is wrapped at `w` columns: a line ends after its
/// last blank that fits (the blank kept on it), else where its next grapheme does not fit; one
/// empty line for an empty text. Every byte is on one line, in order.
fn wrap_at(text: &str, w: usize) -> Vec<Range<usize>> {
    use unicode_segmentation::UnicodeSegmentation;
    let w = w.max(1);
    let gs: Vec<(usize, usize, bool)> = text
        .grapheme_indices(true)
        .map(|(at, g)| (at, crate::text::grapheme_width(g), g.chars().all(char::is_whitespace)))
        .collect();
    let mut lines = Vec::new();
    let (mut start, mut cols) = (0, 0);
    // The byte after the last blank of the line, and the columns up to it.
    let mut blank: Option<(usize, usize)> = None;
    for (i, &(at, gw, space)) in gs.iter().enumerate() {
        if cols + gw > w && at > start {
            match blank.filter(|&(b, _)| b > start && b <= at) {
                Some((b, upto)) => {
                    lines.push(start..b);
                    (start, cols) = (b, cols - upto);
                }
                None => {
                    lines.push(start..at);
                    (start, cols) = (at, 0);
                }
            }
            blank = None;
            // A word longer than the line: cut where it does not fit.
            if cols + gw > w && at > start {
                lines.push(start..at);
                (start, cols) = (at, 0);
            }
        }
        cols += gw;
        if space {
            let end = gs.get(i + 1).map_or(text.len(), |n| n.0);
            blank = Some((end, cols));
        }
    }
    lines.push(start..text.len());
    lines
}

/// How the SQL section is laid out: its rows, the statement's and the `WHERE` field's, the
/// labels' width.
struct SqlLayout {
    rows: u16,
    statement: u16,
    where_: u16,
    label: u16,
}

/// The rest of the dialog keeps at least these rows while the SQL section is open.
const BODY_MIN: u16 = 8;

/// The layout of the SQL section of `d` (statement `sql`, `None` when there is none) in
/// `inner`: the statement up to 4 rows, a note, the `WHERE` field up to 3 rows, the `ORDER BY`
/// field; the statement then the field given fewer rows where the dialog is short.
fn sql_layout(
    d: &FilterDialog,
    sql: Option<&str>,
    inner: Rect,
    x0: u16,
    right: u16,
    i18n: &datarig_i18n::I18n,
) -> SqlLayout {
    let label = [Label::TableDialogSqlWhere, Label::TableDialogSqlOrder]
        .iter()
        .map(|l| width(&i18n.label(*l)))
        .max()
        .unwrap_or(8) as u16
        + 2;
    let statement_w = usize::from(right.saturating_sub(x0));
    let field_w = usize::from(right.saturating_sub(x0 + label + 2));
    let mut statement = sql.map_or(1, |s| wrap_at(s, statement_w).len()).clamp(1, 4) as u16;
    let text = d.sql.as_ref().map_or("", |s| s.where_.input.text());
    let mut where_ = wrap_at(text, field_w).len().clamp(1, 3) as u16;
    let room = inner.height.saturating_sub(3 + 1 + BODY_MIN);
    while statement + where_ + 2 > room && statement > 1 {
        statement -= 1;
    }
    while statement + where_ + 2 > room && where_ > 1 {
        where_ -= 1;
    }
    SqlLayout { rows: (statement + where_ + 2).min(room), statement, where_, label }
}

/// Draw the SQL section of `d` (laid out as `lay`, statement `p`) from row `at.2` between
/// columns `at.0` and `at.1`: the statement (read-only, coloured as the editor colours SQL,
/// the tie-breaker dimmed, wrapped), a note with `Open in editor` on its right, the `WHERE`
/// field (wrapped) and the `ORDER BY` field, the part refused underlined. Returns the cursor
/// of its field with the keys.
fn draw_sql(
    app: &App,
    d: &FilterDialog,
    lay: &SqlLayout,
    p: Result<&(String, Option<Range<usize>>), &()>,
    at: (u16, u16, u16),
    buf: &mut Buffer,
    hits: &mut DialogHits,
) -> Option<(u16, u16)> {
    let th = theme::cur();
    let i18n = &app.i18n;
    let (x0, right, mut y) = at;
    let end = y + lay.rows;
    let base = th.base();
    let muted = Style::new().fg(th.fg_muted);
    let bold = Style::new().fg(th.fg).add_modifier(Modifier::BOLD);
    let field = Style::new().fg(th.fg).bg(th.surface_alt);
    let focus_style = Style::new().fg(th.fg).patch(th.selection);
    let error = Style::new().fg(th.error).add_modifier(Modifier::UNDERLINED | Modifier::BOLD);
    let mut cursor = None;
    let s = d.sql.as_ref()?;
    // The statement.
    let w = usize::from(right.saturating_sub(x0));
    match p {
        Ok((sql, tie)) => {
            let mut spans = highlight(sql, d.dialect, None);
            if let Some(t) = tie {
                spans.insert(0, (t.clone(), Style::new().fg(th.fg_dim)));
            }
            let lines = wrap_at(sql, w);
            let shown = usize::from(lay.statement).min(lines.len());
            for (i, line) in lines.iter().take(shown).enumerate() {
                let cut = i + 1 == shown && lines.len() > shown;
                let mut x = x0;
                for (b, g) in unicode_segmentation::UnicodeSegmentation::grapheme_indices(&sql[line.clone()], true) {
                    let pos = line.start + b;
                    let gw = crate::text::grapheme_width(g) as u16;
                    if cut && x + gw + 1 > right {
                        put(buf, x, y, "\u{2026}", 1, muted);
                        break;
                    }
                    let style = spans.iter().find(|(r, _)| r.contains(&pos)).map_or(base, |(_, st)| base.patch(*st));
                    buf.set_stringn(x, y, g, usize::from(gw.max(1)), style);
                    x += gw;
                }
                y += 1;
            }
            y += lay.statement - shown as u16;
        }
        Err(()) => {
            put(buf, x0, y, &i18n.label(Label::TableDialogSqlLimit), w, Style::new().fg(th.warning));
            y += lay.statement;
        }
    }
    if y >= end {
        return None;
    }
    // The note, `Open in editor` on its right.
    let open = format!(" {} ", i18n.label(Label::TableDialogOpenEditor));
    let ow = width(&open) as u16;
    let open_x = right.saturating_sub(ow);
    let button = Style::new().fg(th.fg).bg(th.surface_alt);
    put(buf, open_x, y, &open, usize::from(ow), if d.focus == DialogFocus::SqlOpen { focus_style } else { button });
    hits.open_sql = Rect::new(open_x, y, ow, 1);
    put(buf, x0, y, &i18n.label(Label::TableDialogSqlNote), usize::from(open_x.saturating_sub(x0 + 2)), muted);
    y += 1;
    // The `WHERE` field, wrapped over its rows.
    let fx = x0 + lay.label;
    let fw = right.saturating_sub(fx);
    if y < end {
        put(buf, x0, y, &i18n.label(Label::TableDialogSqlWhere), usize::from(lay.label), bold);
        let rows = lay.where_.min(end - y - u16::from(end - y > 1));
        let r = Rect::new(fx, y, fw, rows.max(1));
        fill(buf, r, field);
        hits.sql_where = r;
        let focused = d.focus == DialogFocus::SqlWhere && d.picker.is_none();
        let mut spans = highlight(s.where_.input.text(), d.dialect, None);
        // Where the last refusal points in the text sent, while the field holds that text.
        if let Some(Mark::Where { at, .. }) = &d.mark
            && !s.where_.edited()
        {
            spans.insert(0, (at.clone(), error));
        }
        let c = draw_wrapped(
            &s.where_.input,
            Rect::new(fx + 1, y, fw.saturating_sub(2), r.height),
            field,
            &spans,
            focused,
            buf,
        );
        if focused {
            cursor = c;
        }
        y += r.height;
    }
    // The `ORDER BY` field.
    if y < end {
        put(buf, x0, y, &i18n.label(Label::TableDialogSqlOrder), usize::from(lay.label), bold);
        let r = Rect::new(fx, y, fw, 1);
        fill(buf, r, field);
        hits.sql_order = r;
        let focused = d.focus == DialogFocus::SqlOrder && d.picker.is_none();
        let mut spans = highlight(s.order.input.text(), d.dialect, None);
        if let Some(Mark::Order(at)) = &d.mark {
            spans.insert(0, (at.clone(), error));
        }
        let mut input = s.order.input.clone();
        let cx = input.render_spans(Rect::new(fx + 1, y, fw.saturating_sub(2), 1), buf, field, focused, &spans);
        if focused {
            cursor = Some((cx, y));
        }
    }
    cursor
}

/// How [`draw_wrapped`] lays `input` out over `height` rows `w` columns wide: the lines (byte
/// ranges of its text), the first line shown (`focused`: so that the cursor shows, else the
/// first), the cursor's line and column.
fn wrapped_view(input: &TextInput, w: usize, height: u16, focused: bool) -> (Vec<Range<usize>>, usize, usize, usize) {
    use unicode_segmentation::UnicodeSegmentation;
    let text = input.text();
    // The cursor's line: one past a full line's end starts the next one.
    let mut lines = wrap_at(text, w);
    let at = input.cursor_byte();
    let mut cl = lines.iter().rposition(|l| l.start <= at).unwrap_or(0);
    let mut cx = text[lines[cl].start..at].graphemes(true).map(crate::text::grapheme_width).sum::<usize>();
    if cx >= w {
        lines.push(at..at);
        cl = lines.len() - 1;
        cx = 0;
    }
    let first = if focused { (cl + 1).saturating_sub(usize::from(height)) } else { 0 };
    (lines, first, cl, cx)
}

/// Draw `input` wrapped over the rows of `r` (each grapheme in `style`, patched by the first
/// span holding its first byte): from its start, or (`focused`) so that its cursor shows.
/// Returns where the cursor is when `focused`.
fn draw_wrapped(
    input: &TextInput,
    r: Rect,
    style: Style,
    spans: &[(Range<usize>, Style)],
    focused: bool,
    buf: &mut Buffer,
) -> Option<(u16, u16)> {
    use unicode_segmentation::UnicodeSegmentation;
    let w = usize::from(r.width);
    if w == 0 || r.height == 0 {
        return None;
    }
    let text = input.text();
    let (lines, first, cl, cx) = wrapped_view(input, w, r.height, focused);
    for (row, line) in lines.iter().skip(first).take(usize::from(r.height)).enumerate() {
        let y = r.y + row as u16;
        let mut x = r.x;
        for (b, g) in text[line.clone()].grapheme_indices(true) {
            let pos = line.start + b;
            let gw = crate::text::grapheme_width(g) as u16;
            let st = spans.iter().find(|(s, _)| s.contains(&pos)).map_or(style, |(_, s)| style.patch(*s));
            buf.set_stringn(x, y, g, usize::from(gw.max(1)), st);
            x += gw;
        }
    }
    focused.then(|| (r.x + cx as u16, r.y + (cl - first) as u16))
}

/// The byte of `input`'s text a click at (x, y) puts the cursor before, where
/// [`draw_wrapped`] drew it into `r` (`focused` as it was drawn): the grapheme under the
/// pointer, the end of its line past the line's end, the end of the text under the last line.
pub(crate) fn wrapped_byte_at(input: &TextInput, r: Rect, focused: bool, x: u16, y: u16) -> Option<usize> {
    use unicode_segmentation::UnicodeSegmentation;
    if !r.contains(ratatui::layout::Position::new(x, y)) || r.width == 0 {
        return None;
    }
    let text = input.text();
    let (lines, first, ..) = wrapped_view(input, usize::from(r.width), r.height, focused);
    let Some(line) = lines.get(first + usize::from(y - r.y)) else { return Some(text.len()) };
    let col = usize::from(x - r.x);
    let mut acc = 0;
    for (b, g) in text[line.clone()].grapheme_indices(true) {
        let gw = crate::text::grapheme_width(g);
        if col < acc + gw {
            return Some(line.start + b);
        }
        acc += gw;
    }
    Some(line.end)
}

/// The values of view `v` from row `rows.0` (its heading) to the row before `rows.1`, between
/// `cols.0` and `cols.1`: the heading with what they are and `# count exactly` on its right,
/// then one value a line (tick box, value, rows, a bar or where it comes from), then what is
/// not listed. Returns the first value shown (the focused one kept in view).
fn draw_values(
    app: &App,
    d: &FilterDialog,
    v: &ValuesView,
    (px, right): (u16, u16),
    (y, end): (u16, u16),
    buf: &mut Buffer,
    hits: &mut DialogHits,
) -> usize {
    let th = theme::cur();
    let i18n = &app.i18n;
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
        false if pw < 60 => (i18n.msg(&Msg::TableDialogCountExactShort { key: count_key }).to_string(), muted),
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

/// The keys of the focused part of the dialog, at most five, in the hint line.
fn draw_hints(app: &App, d: &FilterDialog, view: Option<&ValuesView>, x: u16, y: u16, w: usize, buf: &mut Buffer) {
    let th = theme::cur();
    let i18n = &app.i18n;
    let l = |k: &str, label: Label| (k.to_string(), i18n.label(label).to_string());
    let undo = key(app, DialogAction::Undo);
    let (apply, cancel) = (key(app, DialogAction::Apply), key(app, DialogAction::Cancel));
    let mut items: Vec<(String, String)> = match d.focus {
        _ if d.picker.is_some() => vec![
            l("\u{2191}\u{2193}", Label::TableDialogHintOperator),
            l("Space", Label::TableDialogHintPick),
            l(&cancel, Label::TableDialogHintClose),
        ],
        DialogFocus::Columns if d.search.is_some() => vec![
            l("\u{2191}\u{2193}", Label::TableDialogHintColumn),
            l(&apply, Label::TableDialogHintPick),
            l(&cancel, Label::TableDialogHintClose),
        ],
        DialogFocus::Columns => vec![
            l("\u{2191}\u{2193}", Label::TableDialogHintColumn),
            l("\u{2192}", Label::TableDialogHintFilter),
            l("s", Label::TableDialogHintSort),
            l("/", Label::TableDialogHintSearch),
            l("Tab", Label::TableDialogHintNextArea),
        ],
        DialogFocus::Op(_) => vec![
            l("Space", Label::TableDialogHintOperator),
            l("\u{2192}", Label::TableDialogHintValue),
            l("x", Label::TableDialogHintRemove),
            (undo, i18n.label(Label::TableDialogHintUndo).to_string()),
            l("Tab", Label::TableDialogHintNextArea),
        ],
        DialogFocus::Value(..) if d.listed > 0 => vec![
            l("\u{2193}", Label::TableDialogHintValues),
            l("\u{2190}", Label::TableDialogHintOperator),
            (key(app, DialogAction::Undo), i18n.label(Label::TableDialogHintUndo).to_string()),
            l("Tab", Label::TableDialogHintNextArea),
        ],
        DialogFocus::Value(..) => vec![
            l("\u{2190}", Label::TableDialogHintOperator),
            l("\u{2191}\u{2193}", Label::TableDialogHintLines),
            (key(app, DialogAction::Undo), i18n.label(Label::TableDialogHintUndo).to_string()),
            l("Tab", Label::TableDialogHintNextArea),
        ],
        DialogFocus::Values(i) => {
            // A value that is not ticked says why instead.
            if let Some(Err(why)) = view.and_then(|v| v.items.get(i)).map(|it| &it.tick) {
                let n = app.not_ticked_notice(why);
                put(buf, x, y, &n.render(i18n), w, Style::new().fg(level_color(n.level)));
                return;
            }
            vec![
                l("Space", Label::TableDialogHintTick),
                (key(app, DialogAction::Count), i18n.label(Label::TableDialogHintCount).to_string()),
                (undo, i18n.label(Label::TableDialogHintUndo).to_string()),
                l("Tab", Label::TableDialogHintNextArea),
            ]
        }
        DialogFocus::Add => vec![
            l("Space", Label::TableDialogHintAdd),
            l("\u{2191}\u{2193}", Label::TableDialogHintLines),
            l("Tab", Label::TableDialogHintNextArea),
        ],
        DialogFocus::Sort => vec![
            l("\u{2190}\u{2192}", Label::TableDialogHintSortDirs),
            l("< >", Label::TableDialogHintMove),
            (undo, i18n.label(Label::TableDialogHintUndo).to_string()),
            l("Tab", Label::TableDialogHintNextArea),
        ],
        DialogFocus::Chips(_) => vec![
            l("\u{2190}\u{2192}", Label::TableDialogHintChip),
            l("x", Label::TableDialogHintRemove),
            l("Space", Label::TableDialogHintGoTo),
            l("Tab", Label::TableDialogHintNextArea),
        ],
        DialogFocus::Order(_) => vec![
            l("\u{2190}\u{2192}", Label::TableDialogHintKey),
            l("< >", Label::TableDialogHintMove),
            l("x", Label::TableDialogHintRemove),
            l("Space", Label::TableDialogHintGoTo),
            l("Tab", Label::TableDialogHintNextArea),
        ],
        DialogFocus::Limit => vec![l("0-9", Label::TableDialogHintLimit), l("Tab", Label::TableDialogHintNextArea)],
        DialogFocus::Sql => {
            let what = if d.sql.is_some() { Label::TableDialogHintSqlClose } else { Label::TableDialogHintSql };
            vec![
                l("Space", what),
                (key(app, DialogAction::Sql), i18n.label(what).to_string()),
                l("Tab", Label::TableDialogHintNextArea),
            ]
        }
        DialogFocus::SqlWhere | DialogFocus::SqlOrder => vec![
            (
                app.key_for(Action::ShowCompletions, Ctx::FilterDialogText),
                i18n.label(Label::TableDialogHintComplete).to_string(),
            ),
            (key(app, DialogAction::Cancel), i18n.label(Label::TableDialogHintSqlClose).to_string()),
            l("\u{2191}\u{2193}", Label::TableDialogHintField),
            l("Tab", Label::TableDialogHintNextArea),
        ],
        DialogFocus::SqlOpen => vec![
            l("Space", Label::TableDialogHintOpenSql),
            l("\u{2191}", Label::TableDialogHintField),
            l("Tab", Label::TableDialogHintNextArea),
        ],
    };
    items.truncate(5);
    let mut cx = x;
    let end = x + w as u16;
    if d.draft.where_.has_incomplete() {
        let text = i18n.label(Label::TableDialogIncomplete).to_string();
        let tw = width(&text) as u16;
        if cx + tw + 3 > end {
            return;
        }
        cx += put(buf, cx, y, &text, usize::from(tw), Style::new().fg(th.warning)) + 3;
    }
    let keyst = Style::new().fg(th.fg).add_modifier(Modifier::BOLD);
    let muted = Style::new().fg(th.fg_muted);
    for (i, (k, label)) in items.iter().enumerate() {
        let need = (width(k) + 1 + width(label) + if i > 0 { 3 } else { 0 }) as u16;
        if cx + need > end {
            break;
        }
        if i > 0 {
            cx += put(buf, cx, y, " \u{00b7} ", 3, muted);
        }
        cx += put(buf, cx, y, k, width(k), keyst);
        cx += put(buf, cx, y, &format!(" {label}"), width(label) + 1, muted);
    }
}

#[cfg(test)]
mod tests;
