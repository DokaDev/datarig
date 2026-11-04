//! The changes of a result drawn: the statements that apply them, in the editor's colors, each
//! wrapped under itself with the count it must give and the text values it carries read back.

use crate::app::edit::PreviewLine;
use crate::text::wrap_words;
use crate::theme;
use datarig_core::sql::dialect::Dialect;
use ratatui::style::{Modifier, Style};

/// The rows of a preview's body `lines` (statements in dialect `d`) in `iw` columns: a statement
/// from column 2, wrapped under itself from column 4 (a line break in a value as one character,
/// a long literal shortened with `cut` saying how many bytes are not shown), its count at the end
/// of its first row; its values read back from column 4, wrapped from column 6; words wrapped.
pub(crate) fn preview_rows(
    lines: &[PreviewLine],
    d: Dialect,
    iw: usize,
    cut: &dyn Fn(usize) -> String,
) -> Vec<Vec<(String, Style)>> {
    use crate::text::width;
    let th = theme::cur();
    let surface = |fg| Style::new().fg(fg).bg(th.surface);
    let risk = surface(th.warning).add_modifier(Modifier::BOLD);
    let dim = surface(th.fg_dim).add_modifier(Modifier::ITALIC);
    let mut rows: Vec<Vec<(String, Style)>> = Vec::new();
    let wrapped = |text: &str, style: Style, rows: &mut Vec<Vec<(String, Style)>>| {
        rows.extend(wrap_words(text, iw).into_iter().map(|l| vec![(l, style)]));
    };
    for l in lines {
        match l {
            PreviewLine::Head(text) => rows.push(vec![(text.clone(), surface(th.fg).add_modifier(Modifier::BOLD))]),
            PreviewLine::Step { sql, expect, risky } => {
                let tail = format!("  {expect}");
                let first = iw.saturating_sub(2 + width(&tail)).max(8);
                let more = iw.saturating_sub(4).max(8);
                let cells = sql_cells(sql, d, th.surface, &|n| (cut(n), dim));
                let mut parts = wrap_cells(cells, first, more).into_iter();
                let head = parts.next().unwrap_or_default();
                let pad = first.saturating_sub(head.iter().map(|g| g.width).sum());
                let (mark, tone) = if *risky { ("! ", risk) } else { ("  ", surface(th.fg_muted)) };
                let mut row = vec![(mark.to_string(), tone)];
                row.extend(runs(head));
                row.extend([(" ".repeat(pad), surface(th.fg)), (tail, tone)]);
                rows.push(row);
                for p in parts {
                    rows.push(std::iter::once(("    ".to_string(), surface(th.fg))).chain(runs(p)).collect());
                }
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
