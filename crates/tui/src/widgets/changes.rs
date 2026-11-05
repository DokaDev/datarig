//! The changes of a result drawn: the statements that apply them, in the editor's colors, each
//! wrapped under itself with the count it must give and the text values it carries read back;
//! and the Changes tab of the results pane, with its list of the changed cells.

use crate::app::App;
use crate::app::edit::{ChangesModel, ListEntry, Mark, Pointer, PreviewLine};
use crate::text::wrap_words;
use crate::theme;
use crate::widgets::put;
use datarig_core::i18n::{Label, Msg};
use datarig_core::sql::dialect::Dialect;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

/// The rows of a preview's body `lines` (statements in dialect `d`) in `iw` columns on background
/// `bg`: a statement from column 2, wrapped under itself from column 4 (a line break in a value as
/// one character, a long literal shortened with `cut` saying how many bytes are not shown), its
/// count at the end of its first row; its values read back from column 4, wrapped from column 6;
/// words wrapped.
pub(crate) fn preview_rows(
    lines: &[PreviewLine],
    d: Dialect,
    iw: usize,
    cut: &dyn Fn(usize) -> String,
    bg: Color,
) -> Vec<Vec<(String, Style)>> {
    use crate::text::width;
    let th = theme::cur();
    let surface = |fg| Style::new().fg(fg).bg(bg);
    let risk = surface(th.warning).add_modifier(Modifier::BOLD);
    let dim = surface(th.fg_dim).add_modifier(Modifier::ITALIC);
    let mut rows: Vec<Vec<(String, Style)>> = Vec::new();
    let wrapped = |text: &str, style: Style, rows: &mut Vec<Vec<(String, Style)>>| {
        rows.extend(wrap_words(text, iw).into_iter().map(|l| vec![(l, style)]));
    };
    // A statement after its mark, wrapped under itself, its count at the end of its first row.
    let step = |sql: &str, expect: &str, mark: &str, tone: Style, rows: &mut Vec<Vec<(String, Style)>>| {
        let tail = format!("  {expect}");
        let first = iw.saturating_sub(2 + width(&tail)).max(8);
        let more = iw.saturating_sub(4).max(8);
        let cells = sql_cells(sql, d, bg, &|n| (cut(n), dim));
        let mut parts = wrap_cells(cells, first, more).into_iter();
        let head = parts.next().unwrap_or_default();
        let pad = first.saturating_sub(head.iter().map(|g| g.width).sum());
        let mut row = vec![(mark.to_string(), tone)];
        row.extend(runs(head));
        row.extend([(" ".repeat(pad), surface(th.fg)), (tail, tone)]);
        rows.push(row);
        for p in parts {
            rows.push(std::iter::once(("    ".to_string(), surface(th.fg))).chain(runs(p)).collect());
        }
    };
    for l in lines {
        match l {
            PreviewLine::Head(text) => rows.push(vec![(text.clone(), surface(th.fg).add_modifier(Modifier::BOLD))]),
            PreviewLine::Step { sql, expect, risky } => {
                let (mark, tone) = if *risky { ("! ", risk) } else { ("  ", surface(th.fg_muted)) };
                step(sql, expect, mark, tone, &mut rows);
            }
            PreviewLine::Failed { sql, expect } => {
                step(sql, expect, "! ", surface(th.error).add_modifier(Modifier::BOLD), &mut rows)
            }
            PreviewLine::Values(text) => {
                for (k, r) in crate::text::wrap_exact(text, iw.saturating_sub(6)).into_iter().enumerate() {
                    let lead = if k == 0 { "    \u{21B3} " } else { "      " };
                    rows.push(vec![(lead.to_string(), dim), (r.trim_start().to_string(), dim)]);
                }
            }
            PreviewLine::Choose { text, keys } => {
                wrapped(&format!("! {text}"), risk, &mut rows);
                rows.push(vec![(format!("  {keys}"), surface(th.fg))]);
            }
            PreviewLine::Note(text) => wrapped(text, surface(th.fg_muted), &mut rows),
            PreviewLine::Problem(text) => wrapped(text, surface(th.error), &mut rows),
        }
    }
    rows
}

/// A grapheme of a statement as drawn (or the note of what a long literal leaves out): its
/// text, style and width.
struct Glyph<'a> {
    text: std::borrow::Cow<'a, str>,
    style: Style,
    width: usize,
}

impl<'a> Glyph<'a> {
    /// Grapheme `g` in `style`; a line break, a tab or another control character as one character.
    fn new(g: &'a str, style: Style) -> Self {
        let text = if g.chars().any(char::is_control) {
            std::borrow::Cow::Owned(crate::text::sanitize_cell(g))
        } else {
            std::borrow::Cow::Borrowed(g)
        };
        let width = crate::text::width(&text);
        Glyph { text, style, width }
    }
}

/// A literal or quoted name longer than this many bytes is shown shortened.
const LONG_TOKEN: usize = 256;
/// The graphemes shown of a shortened literal's start, and of its end.
const LONG_HEAD: usize = 64;
const LONG_TAIL: usize = 16;

/// The graphemes of statement `sql` (in dialect `d`) on background `bg`, each in its syntax
/// color as the editor gives it. A literal longer than [`LONG_TOKEN`] bytes shows its start and
/// its end around `cut` of how many characters are between (what it says, in its style), so
/// that a huge value is only looked at once; what is copied and sent is `sql` itself.
fn sql_cells<'a>(
    sql: &'a str,
    d: Dialect,
    bg: ratatui::style::Color,
    cut: &dyn Fn(usize) -> (String, Style),
) -> Vec<Glyph<'a>> {
    use datarig_core::sql::lexer::{Tok, lex_in};
    use unicode_segmentation::UnicodeSegmentation;
    let th = theme::cur();
    let toks = lex_in(sql, d);
    let mut out = Vec::new();
    let push = |text: &'a str, style: Style, out: &mut Vec<Glyph<'a>>| {
        out.extend(text.graphemes(true).map(|g| Glyph::new(g, style)));
    };
    let mut at = 0;
    for (i, t) in toks.iter().enumerate() {
        // Anything the lexer left between tokens, as plain text.
        if t.start > at {
            push(&sql[at..t.start], Style::new().fg(th.fg).bg(bg), &mut out);
        }
        at = t.end;
        let call = t.kind == Tok::Ident && toks.get(i + 1).is_some_and(|n| n.kind == Tok::LParen);
        let style = th.syntax(t.kind, call).bg(bg);
        let text = t.text(sql);
        if text.len() > LONG_TOKEN && matches!(t.kind, Tok::Str | Tok::Dollar | Tok::QuotedIdent) {
            let head = text.grapheme_indices(true).nth(LONG_HEAD).map_or(text.len(), |(k, _)| k);
            let tail = text.grapheme_indices(true).nth_back(LONG_TAIL - 1).map_or(0, |(k, _)| k);
            if tail > head {
                push(&text[..head], style, &mut out);
                let (note, note_style) = cut(text[head..tail].graphemes(true).count());
                let width = crate::text::width(&note);
                out.push(Glyph { text: std::borrow::Cow::Owned(note), style: note_style, width });
                push(&text[tail..], style, &mut out);
                continue;
            }
        }
        push(text, style, &mut out);
    }
    if at < sql.len() {
        push(&sql[at..], Style::new().fg(th.fg).bg(bg), &mut out);
    }
    out
}

/// `cells` in rows of `first` columns, then of `more`: broken at the last blank that fits (the
/// blank dropped), a run without one where it no longer fits.
fn wrap_cells(cells: Vec<Glyph<'_>>, first: usize, more: usize) -> Vec<Vec<Glyph<'_>>> {
    let mut out: Vec<Vec<Glyph>> = Vec::new();
    let mut line: Vec<Glyph> = Vec::new();
    // The width of `line`.
    let mut w = 0;
    for g in cells {
        let blank = g.text == " ";
        // A blank that does not fit is the break itself.
        if blank && !line.is_empty() && w + g.width > if out.is_empty() { first } else { more } {
            out.push(std::mem::take(&mut line));
            w = 0;
            continue;
        }
        while !line.is_empty() && w + g.width > if out.is_empty() { first } else { more } {
            let at = line.iter().rposition(|c| c.text == " ").filter(|&b| b > 0);
            let rest: Vec<Glyph> = at.map(|b| line.split_off(b).into_iter().skip(1).collect()).unwrap_or_default();
            w = rest.iter().map(|c| c.width).sum();
            out.push(std::mem::replace(&mut line, rest));
        }
        // A row after a break starts with what follows the blanks.
        if blank && line.is_empty() && !out.is_empty() {
            continue;
        }
        w += g.width;
        line.push(g);
    }
    out.push(line);
    out
}

/// Graphemes of one style joined: the spans of a row.
fn runs(cells: Vec<Glyph<'_>>) -> Vec<(String, Style)> {
    let mut out: Vec<(String, Style)> = Vec::new();
    for g in cells {
        match out.last_mut() {
            Some((text, s)) if *s == g.style => text.push_str(&g.text),
            _ => out.push((g.text.into_owned(), g.style)),
        }
    }
    out
}

/// `spans` cut to `w` columns, `…` where something is left out.
fn cut_spans(spans: Vec<(String, Style)>, w: usize) -> Vec<(String, Style)> {
    use crate::text::{clip, width};
    let mut left = w;
    let mut out = Vec::new();
    let n = spans.len();
    for (k, (text, style)) in spans.into_iter().enumerate() {
        let tw = width(&text);
        if tw < left || (tw == left && k + 1 == n) {
            left -= tw;
            out.push((text, style));
            continue;
        }
        // The last column that fits says something follows.
        let cut = if tw > left { clip(&text, left) } else { clip(&format!("{text}\u{2026}"), left) };
        out.push((cut, style));
        break;
    }
    out
}

/// How a line of the list names its row: by its key, its row in the grid, or as a new row.
fn pointer_text(app: &App, p: &Pointer) -> String {
    crate::app::edit::pointer_text(&app.i18n, p)
}

/// The rows of the Changes tab's list in `w` columns, each with the line of the list it shows
/// ([`ChangesModel::entries`]): a problem about no row (`!`, its words wrapped), one row per
/// changed cell (`~`), deleted row (`-`) and new row (`+`), its row named on its first line, cut
/// to the width; a row's problems under its lines (`!`, indented, without the row's name).
fn list_rows(app: &App, m: &ChangesModel, w: usize) -> Vec<(usize, Vec<(String, Style)>)> {
    use crate::text::{fit, width, wrap_words};
    let th = theme::cur();
    let base = |fg| Style::new().fg(fg).bg(th.bg);
    let token = |s: Style| s.bg(th.bg).remove_modifier(Modifier::CROSSED_OUT);
    let mut rows = Vec::new();
    let pointers: Vec<Option<String>> = m.list.iter().map(|l| l.row.as_ref().map(|p| pointer_text(app, p))).collect();
    let heads = m.entries.iter().filter_map(|e| match e {
        ListEntry::Row(_, p) => Some(width(&pointer_text(app, p))),
        _ => None,
    });
    let pw = pointers.iter().flatten().map(|p| width(p)).chain(heads).max().unwrap_or(0);
    let cw = m.list.iter().filter(|l| l.col.is_some()).map(|l| width(&l.column)).max().unwrap_or(0).min(24);
    let by_values = app.i18n.label(Label::ChangesByValues).to_string();
    let delete = app.i18n.label(Label::ChangesDelete).to_string();
    for (k, e) in m.entries.iter().enumerate() {
        let i = match e {
            ListEntry::Problem(i) => {
                let p = &m.problems[*i];
                let (lead, more, text) = if p.at.is_some() { ("  ! ", "    ", &p.bare) } else { ("! ", "  ", &p.text) };
                for (j, r) in wrap_words(text, w.saturating_sub(lead.len()).max(8)).into_iter().enumerate() {
                    let lead = if j == 0 { lead } else { more };
                    rows.push((
                        k,
                        vec![(lead.to_string(), base(th.error).add_modifier(Modifier::BOLD)), (r, base(th.error))],
                    ));
                }
                continue;
            }
            ListEntry::Row(_, p) => {
                rows.push((
                    k,
                    cut_spans(vec![("  ".to_string(), base(th.fg)), (pointer_text(app, p), base(th.fg))], w),
                ));
                continue;
            }
            ListEntry::Line(i) => *i,
        };
        let (l, pointer) = (&m.list[i], &pointers[i]);
        let (mark, tone) = match l.mark {
            Mark::Changed => ("~", token(th.edit_changed)),
            Mark::Inserted => ("+", token(th.edit_inserted)),
            Mark::Deleted => ("-", token(th.edit_deleted)),
        };
        let ptr = fit(pointer.as_deref().unwrap_or(""), pw, crate::text::Align::Left);
        let mut spans = vec![(format!("{mark} "), tone), (format!("{ptr}  "), base(th.fg))];
        match (l.mark, l.col) {
            (Mark::Deleted, _) => spans.push((delete.clone(), base(th.fg_muted))),
            (_, Some(_)) => {
                let column = fit(&l.column, cw, crate::text::Align::Left);
                let (old, new) = (l.old.as_deref().unwrap_or(""), l.new.as_deref().unwrap_or(""));
                spans.push((format!("{column}  "), base(th.fg_muted)));
                spans.push((format!("{old} \u{2192} {new}"), base(th.fg)));
            }
            (_, None) => spans.push((l.new.clone().unwrap_or_default(), base(th.fg))),
        }
        if matches!(l.row, Some(Pointer::Row(_))) {
            spans.push((format!("  {by_values}"), base(th.fg_dim).add_modifier(Modifier::ITALIC)));
        }
        rows.push((k, cut_spans(spans, w)));
    }
    rows
}

/// The Changes tab of the shown result in `area`: its first line (the result and the table,
/// how many changes of each kind and how many problems), what the Apply is doing, another
/// result's pending changes, then the list or the statements, and under them how the Apply
/// stands to a transaction with the tab's keys. In a short pane the keys go first, then the line
/// above them; the first line always stays.
pub(crate) fn draw_changes(app: &mut App, area: Rect, buf: &mut Buffer) {
    use crate::text::width;
    let th = theme::cur();
    buf.set_style(area, th.base());
    let Some(m) = app.shown_changes() else { return };
    if area.height == 0 || area.width < 4 {
        return;
    }
    let base = |fg| Style::new().fg(fg).bg(th.bg);
    let iw = area.width as usize - 2;
    let x = area.x + 1;
    let line = |buf: &mut Buffer, y: u16, parts: &[(String, Style)]| {
        let mut cx = x;
        for (text, style) in parts {
            let room = (x as usize + iw).saturating_sub(cx as usize);
            if room == 0 {
                break;
            }
            cx += put(buf, cx, y, text, room, *style);
        }
    };
    let bottom = area.y + area.height;
    let mut y = area.y;
    // The first line: the problems' count first to stay, then the summary, then the title.
    let (title, summary, problems) = app.changes_head(&m);
    let mut right: Vec<(String, Style)> = Vec::new();
    if !summary.is_empty() {
        right.push((summary, base(th.fg_muted)));
    }
    if let Some(p) = problems {
        if !right.is_empty() {
            right.push((" \u{B7} ".into(), base(th.fg_muted)));
        }
        right.push((p, base(th.error).add_modifier(Modifier::BOLD)));
    }
    let rw = |r: &[(String, Style)]| r.iter().map(|(t, _)| width(t)).sum::<usize>();
    if rw(&right) + 2 + width(&title).min(12) > iw {
        right.retain(|(_, s)| s.fg == Some(th.error));
    }
    let room = iw.saturating_sub(rw(&right) + if right.is_empty() { 0 } else { 2 });
    line(buf, y, &cut_spans(vec![(title, base(th.fg).add_modifier(Modifier::BOLD))], room));
    let w = rw(&right);
    if w > 0 && w <= iw {
        let rx = x + (iw - w) as u16;
        let mut cx = rx;
        for (text, style) in &right {
            cx += put(buf, cx, y, text, (x as usize + iw).saturating_sub(cx as usize), *style);
        }
    }
    y += 1;
    let mut extra: Vec<Vec<(String, Style)>> = Vec::new();
    if let Some((text, level)) = app.changes_status() {
        let tone = match level {
            crate::app::Level::Info => base(th.accent),
            l => base(crate::widgets::statusbar::level_color(l)),
        };
        extra.push(cut_spans(vec![(text, tone)], iw));
    }
    if let Some(note) = app.changes_note() {
        extra.push(cut_spans(vec![(note, base(th.warning))], iw));
    }
    let avail = bottom.saturating_sub(y) as usize;
    let footer = match avail.saturating_sub(extra.len()) {
        n if n >= 6 => 2,
        n if n >= 3 => 1,
        _ => 0,
    };
    for e in extra.iter().take(avail.saturating_sub(footer + 1)) {
        line(buf, y, e);
        y += 1;
    }
    let body_h = bottom.saturating_sub(y) as usize - footer;
    let focused = app.focus == crate::app::Focus::Results;
    let mut c = app.changes_cursor().unwrap_or_default();
    c.page = body_h;
    if app.changes_sql() {
        // Laid out again only for another model of the changes (made once for each state of
        // them and what was counted), width or theme.
        let current = app.changes_body.as_ref().is_some_and(|(made, b)| {
            std::sync::Arc::ptr_eq(made, &m) && b.width == iw && std::sync::Arc::ptr_eq(&b.theme, &app.theme)
        });
        if !current {
            let lines = app.changes_sql_lines(&m);
            let cut = |n: usize| app.i18n.msg(&Msg::EditPreviewCut { count: n as u64 }).to_string();
            let rows = preview_rows(&lines, m.dialect, iw, &cut, th.bg);
            let theme = app.theme.clone();
            app.changes_body = Some((m.clone(), crate::app::edit::PreviewBody { width: iw, theme, lines, rows }));
        }
        // The statement the last Apply failed at on screen, once.
        if std::mem::take(&mut c.seek)
            && let Some((_, b)) = &app.changes_body
            && let Some(k) = b.lines.iter().position(|l| matches!(l, PreviewLine::Failed { .. }))
        {
            let cut = |n: usize| app.i18n.msg(&Msg::EditPreviewCut { count: n as u64 }).to_string();
            c.sql_scroll = preview_rows(&b.lines[..k], m.dialect, iw, &cut, th.bg).len();
        }
        let rows = app.changes_body.as_ref().map(|(_, b)| b.rows.as_slice()).unwrap_or_default();
        c.sql_scroll = c.sql_scroll.min(rows.len().saturating_sub(body_h));
        for (k, parts) in rows.iter().skip(c.sql_scroll).take(body_h).enumerate() {
            line(buf, y + k as u16, parts);
        }
    } else {
        let rows = list_rows(app, &m, iw);
        let last = m.entries.len().saturating_sub(1);
        c.line = c.line.min(last);
        // The selected line on screen, whole when it fits; a row's problem with the row's
        // first line too when they fit.
        let first = rows.iter().position(|(k, _)| *k == c.line).unwrap_or(0);
        let end = rows.iter().rposition(|(k, _)| *k == c.line).map_or(first, |e| e + 1);
        let row_at = match m.entries.get(c.line) {
            Some(ListEntry::Problem(i)) => m.problems.get(*i).and_then(|p| p.at).map(|(at, _)| at),
            _ => None,
        };
        let head = row_at.and_then(|at| {
            m.entries.iter().position(|e| match e {
                ListEntry::Line(j) => m.list.get(*j).is_some_and(|l| l.at == at),
                ListEntry::Row(a, _) => *a == at,
                ListEntry::Problem(_) => false,
            })
        });
        let first = head
            .and_then(|e| rows.iter().position(|(k, _)| *k == e))
            .filter(|&top| end - top <= body_h)
            .unwrap_or(first);
        if first < c.scroll {
            c.scroll = first;
        } else if end > c.scroll + body_h {
            c.scroll = end.saturating_sub(body_h).min(first);
        }
        c.scroll = c.scroll.min(rows.len().saturating_sub(body_h));
        let selected = th.selection;
        for (k, (entry, parts)) in rows.iter().skip(c.scroll).take(body_h).enumerate() {
            let yy = y + k as u16;
            if focused && *entry == c.line {
                let lit: Vec<(String, Style)> = parts.iter().map(|(t, s)| (t.clone(), s.patch(selected))).collect();
                buf.set_style(Rect { x, y: yy, width: iw as u16, height: 1 }, Style::new().bg(th.bg).patch(selected));
                line(buf, yy, &lit);
            } else {
                line(buf, yy, parts);
            }
        }
    }
    if let Some(cursor) = app.changes_cursor_mut() {
        *cursor = c;
    }
    y += body_h as u16;
    if footer == 2 {
        line(buf, y, &[("\u{2500}".repeat(iw), base(th.border))]);
        y += 1;
    }
    if footer > 0 {
        let tx = app.changes_tx_text();
        let keys =
            [app.changes_keys(false), app.changes_keys(true)].into_iter().find(|k| width(&tx) + 3 + width(k) <= iw);
        line(buf, y, &cut_spans(vec![(tx, base(th.fg_muted))], iw));
        if let Some(k) = keys.filter(|k| !k.is_empty()) {
            put(buf, x + (iw - width(&k)) as u16, y, &k, width(&k), base(th.fg_dim));
        }
    }
}
