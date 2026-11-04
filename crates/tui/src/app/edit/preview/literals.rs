//! The text values a statement carries as their UTF-8 bytes (MySQL's `_utf8mb4 X'…'`), read
//! back for the preview: the column each is for and the text it is. Only shown, never sent.

use datarig_core::sql::dialect::Dialect;
use datarig_core::sql::lexer::{Tok, Token, lex_in};

/// What a literal's bytes are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Decoded {
    Text(String),
    /// Not UTF-8: this many bytes.
    Bytes(usize),
}

/// The `_utf8mb4 X'…'` literals of `sql` (in dialect `d`) a column is set to or compared with,
/// in order, with that column's name: `col = lit` and `col <=> lit`, and an INSERT's values by
/// their place in its column list. A literal after anything else (the column converted and
/// compared again byte for byte) repeats the one before it and is left out.
pub(super) fn text_literals(sql: &str, d: Dialect) -> Vec<(String, Decoded)> {
    let toks: Vec<Token> = lex_in(sql, d).into_iter().filter(|t| !t.is_trivia()).collect();
    let word = |t: &Token, w: &str| t.is_word() && t.text(sql).eq_ignore_ascii_case(w);
    let insert = toks.first().is_some_and(|t| word(t, "INSERT"));
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
            Tok::QuotedIdent if insert && depth == 1 && groups == 1 && !values => columns.push(unquote(t.text(sql))),
            Tok::Str if i > 0 && word(&toks[i - 1], "_utf8mb4") => {
                let Some(bytes) = hex_bytes(t.text(sql)) else { continue };
                let column = compared(sql, &toks[..i - 1])
                    .or_else(|| (insert && values && depth == 1).then(|| columns.get(place).cloned()).flatten());
                if let Some(c) = column {
                    let v = String::from_utf8(bytes).map_or_else(|e| Decoded::Bytes(e.as_bytes().len()), Decoded::Text);
                    out.push((c, v));
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
