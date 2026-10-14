//! A text's string literals written for another reader: the same text, every string as the
//! target dialect reads it as the same value. MySQL's `NO_BACKSLASH_ESCAPES` changes how a
//! string reads (`'a\\b'` is `a\b` without it, `a\\b` with it), so a text written in one mode (a
//! table's DDL as the server prints it, with backslash escapes) runs as written only in a session
//! of that mode; requoted, it runs the same in a session of the other.
//!
//! Only plain strings (`'…'`, and `"…"` where it is a string) are rewritten, each from its value:
//! hex (`X'…'`), bit (`B'…'`) and dollar-quoted strings have no escapes and stay; a national
//! string (`N'…'`) keeps its `N`. Names, comments, client commands and everything else stay as
//! they are.

use super::dialect::Dialect;
use super::lexer::{Tok, lex_in};

/// Why a text cannot be requoted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequoteError {
    /// The two dialects read `"…"` differently (one has `ANSI_QUOTES`, the other not) and the
    /// text has such a token: a name to one, a string to the other.
    DoubleQuotes,
    /// A string is not closed.
    Unclosed,
    /// Not MySQL text: nothing else is requoted.
    NotMySql,
    /// A value holds a control character (a carriage return, NUL, backspace, Ctrl+Z) that only a
    /// backslash escape writes visibly: without escapes it would go into the text raw.
    ControlChar,
}

/// `text`, written in `from`, with every string written as `to` reads the same value. The text as
/// it is when the two read strings alike.
pub fn requote_strings(text: &str, from: Dialect, to: Dialect) -> Result<String, RequoteError> {
    let (Dialect::MySql(f), Dialect::MySql(t)) = (from, to) else { return Err(RequoteError::NotMySql) };
    let tokens = lex_in(text, from);
    let double_quoted = tokens.iter().any(|k| k.text(text).starts_with('"'));
    if f.ansi_quotes != t.ansi_quotes && double_quoted {
        return Err(RequoteError::DoubleQuotes);
    }
    if f.no_backslash_escapes == t.no_backslash_escapes {
        return Ok(text.to_string());
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for k in tokens.iter().filter(|k| k.kind == Tok::Str) {
        let s = k.text(text);
        let (prefix, body) = match s.as_bytes().first() {
            Some(b'\'' | b'"') => ("", s),
            Some(b'n' | b'N') if s[1..].starts_with('\'') => (&s[..1], &s[1..]),
            // Hex, bit and dollar-quoted strings have no escapes.
            _ => continue,
        };
        let q = body.chars().next().unwrap_or('\'');
        // Closed only where the reader's rules close it: the token ends with its closing quote.
        if closing_quote(body, q, !f.no_backslash_escapes) != Some(body.len() - q.len_utf8()) {
            return Err(RequoteError::Unclosed);
        }
        let value = unescape(&body[1..body.len() - 1], q, !f.no_backslash_escapes);
        out.push_str(&text[at..k.start]);
        out.push_str(prefix);
        out.push_str(&written(&value, to, t.no_backslash_escapes)?);
        at = k.end;
    }
    out.push_str(&text[at..]);
    Ok(out)
}

/// Where string `body` (from its opening quote `q`) closes, as a reader with or without
/// `backslash` escapes reads it: the byte index of its closing quote, `None` when it does not.
fn closing_quote(body: &str, q: char, backslash: bool) -> Option<usize> {
    let mut chars = body.char_indices().skip(1).peekable();
    while let Some((i, c)) = chars.next() {
        if c == '\\' && backslash {
            chars.next();
        } else if c == q {
            if chars.peek().is_some_and(|(_, n)| *n == q) {
                chars.next();
            } else {
                return Some(i);
            }
        }
    }
    None
}

/// `value` as a string `to` reads it. Without backslash escapes a control character other than
/// a tab or a line feed would go into the text raw: [`RequoteError::ControlChar`]. With them, a
/// carriage return, a backspace and Ctrl+Z are written escaped (NUL is, by `quote_literal`).
fn written(value: &str, to: Dialect, no_backslash_escapes: bool) -> Result<String, RequoteError> {
    let raw = |c: char| c.is_control() && !matches!(c, '\t' | '\n');
    if no_backslash_escapes {
        if value.chars().any(raw) {
            return Err(RequoteError::ControlChar);
        }
        return Ok(to.quote_literal(value));
    }
    Ok(to.quote_literal(value).replace('\r', "\\r").replace('\u{8}', "\\b").replace('\u{1A}', "\\Z"))
}

/// The value of a string's inside `inner` between quotes `q`: a doubled quote is one; with
/// `backslash`, a backslash escapes the next character as MySQL reads it (`\0`, `\b`, `\n`, `\r`,
/// `\t`, `\Z`; `\%` and `\_` stay as written, for `LIKE`; any other is that character).
fn unescape(inner: &str, q: char, backslash: bool) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c == q && chars.peek() == Some(&q) {
            chars.next();
            out.push(q);
        } else if c == '\\' && backslash {
            match chars.next() {
                Some('0') => out.push('\0'),
                Some('b') => out.push('\u{8}'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('Z') => out.push('\u{1A}'),
                Some(e @ ('%' | '_')) => {
                    out.push('\\');
                    out.push(e);
                }
                Some(e) => out.push(e),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests;
