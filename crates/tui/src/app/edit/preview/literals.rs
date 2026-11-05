//! The text values a statement carries as their UTF-8 bytes (MySQL's `_utf8mb4 X'…'`), read
//! back for the preview: the column each is for and the text it is. Only shown, never sent.

use datarig_core::sql::dialect::Dialect;
use datarig_core::sql::lexer::{Tok, Token, lex_in};

/// What a literal's bytes are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app::edit) enum Decoded {
    Text(String),
    /// Not UTF-8: this many bytes.
    Bytes(usize),
}

/// Where in its statement a literal is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::edit) enum Part {
    /// An UPDATE's `SET`: the value written.
    Set,
    /// What a row is found by.
    Where,
    /// An INSERT's values.
    Values,
}

/// `column = 'text'` as the preview says it: a line break, a tab or another control character
/// (in either) as one character, the text cut at `max` columns with `…` (only its start is
/// looked at, however long it is).
pub(in crate::app::edit) fn text_pair(column: &str, text: &str, max: usize) -> String {
    format!("{} = {}", crate::text::sanitize_cell(column), quoted(text, max))
}

/// `'text'` as the preview says a text value ([`text_pair`]).
pub(in crate::app::edit) fn quoted(text: &str, max: usize) -> String {
    quoted_with(text, max, crate::text::sanitize_cell)
}

/// `'text'` cut at `max` columns with `…`, its start made one line by `line` (only its start is
/// looked at, however long it is).
pub(in crate::app::edit) fn quoted_with(text: &str, max: usize, line: fn(&str) -> String) -> String {
    use crate::text::{clip, width};
    let head: String = text.chars().take(max * 4).collect();
    let mut shown = clip(&line(&head), max);
    if head.len() < text.len() && width(&shown) <= max && !shown.ends_with('\u{2026}') {
        shown = clip(&format!("{shown}\u{2026}"), max);
    }
    format!("'{shown}'")
}

/// The `_utf8mb4 X'…'` literals of `sql` (in dialect `d`) a column is set to or compared with,
/// in order, with where each is and that column's name: `col = lit` and `col <=> lit` (in an
/// UPDATE's `SET` the value written, elsewhere what the row is found by), and an INSERT's values
/// by their place in its column list. A literal after anything else (the column converted and
/// compared again byte for byte) repeats the one before it and is left out.
pub(in crate::app::edit) fn text_literals(sql: &str, d: Dialect) -> Vec<(Part, String, Decoded)> {
    let toks: Vec<Token> = lex_in(sql, d).into_iter().filter(|t| !t.is_trivia()).collect();
    let word = |t: &Token, w: &str| t.is_word() && t.text(sql).eq_ignore_ascii_case(w);
    let insert = toks.first().is_some_and(|t| word(t, "INSERT"));
    let update = toks.first().is_some_and(|t| word(t, "UPDATE"));
    // Inside an UPDATE's `SET` (from it to its `WHERE`).
    let mut set = false;
    let mut columns: Vec<String> = Vec::new();
    let (mut depth, mut groups, mut place, mut values) = (0usize, 0usize, 0usize, false);
    let mut out = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        match t.kind {
            Tok::LParen => {
                depth += 1;
                if depth == 1 {
                    groups += 1;
                    place = 0;
                }
            }
            Tok::RParen => depth = depth.saturating_sub(1),
            Tok::Comma if depth == 1 => place += 1,
            _ if depth == 0 && word(t, "VALUES") => values = true,
            _ if depth == 0 && update && word(t, "SET") => set = true,
            _ if depth == 0 && word(t, "WHERE") => set = false,
            Tok::QuotedIdent if insert && depth == 1 && groups == 1 && !values => columns.push(unquote(t.text(sql))),
            Tok::Str if i > 0 && word(&toks[i - 1], "_utf8mb4") => {
                let Some(bytes) = hex_bytes(t.text(sql)) else { continue };
                let part = if set { Part::Set } else { Part::Where };
                let column = compared(sql, &toks[..i - 1]).map(|c| (part, c)).or_else(|| {
                    (insert && values && depth == 1)
                        .then(|| columns.get(place).map(|c| (Part::Values, c.clone())))
                        .flatten()
                });
                if let Some((part, c)) = column {
                    let v = String::from_utf8(bytes).map_or_else(|e| Decoded::Bytes(e.as_bytes().len()), Decoded::Text);
                    out.push((part, c, v));
                }
            }
            _ => {}
        }
    }
    out
}

/// The column `before` ends comparing or setting: a quoted name, then `=` or `<=>`.
fn compared(sql: &str, before: &[Token]) -> Option<String> {
    let ops = before.iter().rev().take_while(|t| t.kind == Tok::Op).count();
    let at = before.len() - ops;
    // The operator's characters, written together.
    let op = &before[at..];
    let joined = op.windows(2).all(|w| w[0].end == w[1].start);
    let text: String = op.iter().map(|t| t.text(sql)).collect();
    let name = before[..at].last().filter(|t| t.kind == Tok::QuotedIdent)?;
    (joined && (text == "=" || text == "<=>")).then(|| unquote(name.text(sql)))
}

/// A quoted name without its quotes (a doubled quote inside is one).
fn unquote(q: &str) -> String {
    let Some(open) = q.chars().next() else { return String::new() };
    let inner = q.strip_prefix(open).and_then(|s| s.strip_suffix(open)).unwrap_or(q);
    inner.replace(&format!("{open}{open}"), &open.to_string())
}

/// The bytes of a hexadecimal string literal `X'…'` (`None`: not one).
fn hex_bytes(lit: &str) -> Option<Vec<u8>> {
    let body = lit.strip_prefix(['X', 'x'])?.strip_prefix('\'')?.strip_suffix('\'')?;
    if body.len() % 2 != 0 || !body.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..body.len()).step_by(2).map(|i| u8::from_str_radix(body.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests;
