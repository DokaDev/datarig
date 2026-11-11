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
use crate::app::action::Action;
use crate::app::filter_dialog::draft::{Op, Term};
use crate::app::filter_dialog::{
    ColumnSource, DialogAction, DialogFocus, DialogHits, FilterDialog, Mark, op_off_label,
};
use crate::icons;
use crate::keymap::Ctx;
use crate::text::{clip, width};
use crate::theme;
use crate::widgets::put;
use crate::widgets::statusbar::level_color;
use datarig_core::query::{self, Dir};
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

/// Fills `r` with `style`.
fn fill(buf: &mut Buffer, r: Rect, style: Style) {
    buf.set_style(r, style);
    for y in r.y..r.y + r.height {
        for x in r.x..r.x + r.width {
            buf[(x, y)].set_symbol(" ");
        }
    }
}

/// Draw the filter dialog over `area` (the whole screen); returns the hardware cursor (the
/// focused text field's).
pub(crate) fn draw_filter_dialog(app: &mut App, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
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

    // The footer: 3 rows, and the statement above it while shown.
    let sql_rows: u16 = if d.sql_open { 5.min(inner.height.saturating_sub(10)) } else { 0 };
    let foot_y = inner.y + inner.height - 3;
    let sep_y = foot_y - 1 - sql_rows;
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
        DialogFocus::Columns | DialogFocus::Chips(_) | DialogFocus::Order(_) | DialogFocus::Limit | DialogFocus::Sql
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
            let ty =
                if c.invisible { i18n.label(Label::TableDialogInvisible).to_string() } else { c.type_name.clone() };
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
        for t in d.rows() {
            if y + 1 >= sort_y {
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
            let note = if incomplete { i18n.label(Label::TableDialogIncomplete).to_string() } else { String::new() };
            let note_w = if incomplete { width(&note) as u16 + 2 } else { 0 };
            let vw = remove_x.saturating_sub(vx + 2 + note_w);
            let n = match row.op.arity() {
                crate::app::filter_dialog::draft::Arity::Zero => 0,
                crate::app::filter_dialog::draft::Arity::Two => 2,
                _ => 1,
            };
            let values = row.value_texts(d.dialect);
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
                    let text =
                        if n == 2 { values.get(k).cloned().unwrap_or_default() } else { row.input_text(d.dialect) };
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
                put(
                    buf,
                    sx,
                    sort_y,
                    &i18n.label(Label::TableDialogSortMove),
                    usize::from(right.saturating_sub(sx)),
                    muted,
                );
            }
        }
    }

    // The statement, while shown.
    if sql_rows > 0
        && let (Some(target), Some(q)) = (target.as_ref(), app.table_query(tab).map(|tq| tq.applied().clone()))
    {
        let y0 = sep_y + 1;
        put(buf, x0, y0, &i18n.label(Label::TableDialogSql), 8, bold);
        match d.draft.to_query(&q, target) {
            Ok(q) => {
                let sql = query::compile(&q, target).sql;
                let w = usize::from(inner.width.saturating_sub(4));
                let mut at = 0;
                let spans = highlight(&sql, d.dialect, None);
                for i in 0..sql_rows.saturating_sub(1) {
                    if at >= sql.len() {
                        break;
                    }
                    let mut end = at;
                    let mut cols = 0;
                    for (b, ch) in sql[at..].char_indices() {
                        let cw = width(ch.encode_utf8(&mut [0; 4]));
                        if cols + cw > w {
                            break;
                        }
                        cols += cw;
                        end = at + b + ch.len_utf8();
                    }
                    let y = y0 + 1 + i;
                    let mut x = x0 + 2;
                    for (b, ch) in sql[at..end].char_indices() {
                        let pos = at + b;
                        let style = spans.iter().find(|(r, _)| r.contains(&pos)).map_or(base, |(_, s)| base.patch(*s));
                        buf[(x, y)].set_symbol(ch.encode_utf8(&mut [0; 4])).set_style(style);
                        x += width(ch.encode_utf8(&mut [0; 4])) as u16;
                    }
                    at = end.max(at + 1);
                }
            }
            Err(_) => {
                put(
                    buf,
                    x0 + 2,
                    y0 + 1,
                    &i18n.label(Label::TableDialogSqlLimit),
                    usize::from(inner.width.saturating_sub(4)),
                    Style::new().fg(th.warning),
                );
            }
        }
    }

    // The footer: `Where` chips, `undo`, `clear all`.
    let (wy, oy, hy) = (foot_y, foot_y + 1, foot_y + 2);
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
        let marked = d.mark == Some(Mark::Term(chip.index));
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
    let sql = format!(" {} {} ", i18n.label(Label::TableDialogSql), if d.sql_open { "\u{25be}" } else { "\u{25b8}" });
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
    let mut ls = field;
    if d.mark == Some(Mark::Limit) {
        ls = ls.fg(th.error).add_modifier(Modifier::UNDERLINED);
    }
    fill(buf, lr, ls);
    let mut limit = d.limit.clone();
    let lx = limit.render(Rect::new(lr.x + 1, oy, lr.width - 2, 1), buf, ls, limit_focus, false, None);
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
        None => draw_hints(app, d, x0, hy, hint_w, buf),
    }

    // The operator picker, opaque, over the pane.
    if let (Some((bx, by)), Some(at), Some(ops)) = (picker_at, d.picker, ops) {
        let labels: Vec<(String, Option<&str>)> =
            ops.iter().map(|(op, _)| (op_label(app, *op), op.sql_hint(d.dialect))).collect();
        let lw = labels.iter().map(|(l, _)| width(l)).max().unwrap_or(4);
        let sw = labels.iter().filter_map(|(_, s)| s.map(width)).max().unwrap_or(0);
        let reason = ops.get(at).and_then(|(_, ok)| ok.err()).map(|off| i18n.label(op_off_label(off)).to_string());
        let rw = reason.as_ref().map_or(0, |r| width(r));
        let w = ((lw + 3 + sw).max(rw) + 4) as u16;
        let h = ops.len() as u16 + 2 + u16::from(reason.is_some());
        let room_below = sep_y.saturating_sub(by);
        let y = if h <= room_below { by } else { by.saturating_sub(h + 1).max(body.y) };
        let x = bx.min(right.saturating_sub(w));
        let r = Rect::new(x, y, w.min(right.saturating_sub(x) + 1), h.min(sep_y.saturating_sub(y)));
        fill(buf, r, base);
        Block::bordered().border_type(BorderType::Rounded).border_style(accent).render(r, buf);
        for (i, ((label, sql), (_, ok))) in labels.iter().zip(ops.iter()).enumerate() {
            let y = r.y + 1 + i as u16;
            if y + 1 >= r.y + r.height {
                break;
            }
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

    let top = app.overlays.top().is_some_and(|o| o.kind() == crate::app::overlay::OverlayKind::FilterDialog);
    if let Some(d) = app.filter_dialog_mut() {
        d.hits = hits;
        d.scroll = scroll;
    }
    cursor.filter(|_| top)
}

/// The keys of the focused part of the dialog, at most five, in the hint line.
fn draw_hints(app: &App, d: &FilterDialog, x: u16, y: u16, w: usize, buf: &mut Buffer) {
    let th = theme::cur();
    let i18n = &app.i18n;
    let l = |k: &str, label: Label| (k.to_string(), i18n.label(label).to_string());
    let undo = key(app, DialogAction::Undo);
    let mut items: Vec<(String, String)> = match d.focus {
        _ if d.picker.is_some() => vec![
            l("\u{2191}\u{2193}", Label::TableDialogHintOperator),
            l("Space", Label::TableDialogHintPick),
            l("Esc", Label::TableDialogHintClose),
        ],
        DialogFocus::Columns if d.search.is_some() => vec![
            l("\u{2191}\u{2193}", Label::TableDialogHintColumn),
            l("Enter", Label::TableDialogHintPick),
            l("Esc", Label::TableDialogHintClose),
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
        DialogFocus::Value(..) => vec![
            l("\u{2190}", Label::TableDialogHintOperator),
            l("\u{2191}\u{2193}", Label::TableDialogHintLines),
            (key(app, DialogAction::Undo), i18n.label(Label::TableDialogHintUndo).to_string()),
            l("Tab", Label::TableDialogHintNextArea),
        ],
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
        DialogFocus::Sql => vec![
            l("Space", Label::TableDialogHintSql),
            (key(app, DialogAction::Sql), i18n.label(Label::TableDialogHintSql).to_string()),
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
