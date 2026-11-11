//! Filter text as the conditions it is made of: the terms of a conjunction at its top level
//! ([`split_terms`]), terms joined back into filter text ([`join_terms`]), and what a pattern
//! written by [`like_contains`](super::like_contains) or
//! [`like_starts_with`](super::like_starts_with) looks for ([`like_kind`]).
//!
//! Text is split only where it is proven to be a conjunction at its top level: read with the
//! recogniser's reader (parentheses, square brackets and `CASE … END` hold their own `AND`s, and
//! a `BETWEEN` its own), it is cut at each `AND` of its top level, and only when nothing there
//! binds more loosely than `AND` or may be read so: no `OR`, no `XOR`, none of MySQL's `||`,
//! `&&`, `|`, `&`, `!` or `:=`, and no `NOT` but one over a parenthesised group that starts a
//! term, or one of `IS NOT`, `NOT IN`, `NOT LIKE`, `NOT ILIKE`, `NOT BETWEEN`, `NOT SIMILAR`,
//! `NOT REGEXP` and `NOT RLIKE`. Anything else, and text the filter's lexical check refuses, is
//! one term as written: `None`.
//!
//! The terms are byte ranges of the text, so what lies between them (the `AND`s and the blanks
//! around them, as typed) is kept: text whose terms are not changed is the text as it was, byte
//! for byte. Only when terms are added, changed or taken out is the text written again, from the
//! terms as they are written, joined with ` AND ` ([`join_terms`]).

use std::ops::Range;

use super::check::lexical;
use super::recognise::{group, is_word, level, split};
use crate::sql::dialect::Dialect;
use crate::sql::lexer::{Tok, Token, lex_in};

/// The byte ranges of the terms of filter `text` when it is a conjunction at its top level (see
/// the module docs), in order, each without the blanks around it; one range for a text of one
/// term, none for a blank one. `None` when it is not proven to be one: the whole text is then one
/// term.
pub fn split_terms(text: &str, d: Dialect) -> Option<Vec<Range<usize>>> {
    let toks: Vec<Token> = lex_in(text, d).into_iter().filter(|t| !t.is_trivia()).collect();
    if toks.is_empty() && text.trim().is_empty() {
        return Some(Vec::new());
    }
    lexical(text, d).ok()?;
    if level(text, &toks).any(|(i, t)| looser_than_and(text, &toks, i, t, d)) || !cases_close(text, &toks) {
        return None;
    }
    let parts = split(text, &toks, "AND");
    let several = parts.len() > 1;
    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        let (first, last) = (part.first()?, part.last()?);
        // A term led by `NOT`: only over one parenthesised group, so where it ends is not a
        // matter of precedence.
        if several && is_word(text, Some(first), "NOT") && group(&part[1..]).is_none() {
            return None;
        }
        out.push(first.start..last.end);
    }
    Some(out)
}

/// Words a `NOT` may come before inside a comparison (`a NOT IN (…)`).
const NOT_BEFORE: [&str; 7] = ["IN", "LIKE", "ILIKE", "BETWEEN", "SIMILAR", "REGEXP", "RLIKE"];

/// Whether token `t` (at `i` of `toks`, at the top level of `text`) binds more loosely than `AND`
/// or may be read so (see the module docs).
fn looser_than_and(text: &str, toks: &[Token], i: usize, t: &Token, d: Dialect) -> bool {
    let s = t.text(text);
    let glued = |k: usize, c: &str| {
        toks.get(k).is_some_and(|n| n.kind == Tok::Op && n.start == toks[k - 1].end && n.text(text) == c)
    };
    if t.is_word() {
        if s.eq_ignore_ascii_case("OR") || s.eq_ignore_ascii_case("XOR") {
            return true;
        }
        if s.eq_ignore_ascii_case("NOT") {
            let after_is = i > 0 && is_word(text, toks.get(i - 1), "IS");
            let before_op = NOT_BEFORE.iter().any(|w| is_word(text, toks.get(i + 1), w));
            // A term's leading `NOT` is judged with its term ([`split_terms`]).
            let leads = i == 0 || is_word(text, toks.get(i - 1), "AND");
            return !(after_is || before_op || leads);
        }
        return false;
    }
    match d {
        Dialect::MySql(_) if t.kind == Tok::Op => match s {
            "|" | "&" => true,
            // `!=` compares; `!` alone is `NOT`.
            "!" => !glued(i + 1, "="),
            // `:=` assigns.
            ":" => glued(i + 1, "="),
            _ => false,
        },
        _ => false,
    }
}

/// Whether every `CASE` at the top level of `text` (its tokens `toks`) is closed by its `END`.
fn cases_close(text: &str, toks: &[Token]) -> bool {
    let mut depth = 0usize;
    let mut cases = 0usize;
    for t in toks {
        match t.kind {
            Tok::LParen => depth += 1,
            Tok::RParen => depth = depth.saturating_sub(1),
            Tok::Op if t.text(text) == "[" => depth += 1,
            Tok::Op if t.text(text) == "]" => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            _ if is_word(text, Some(t), "CASE") => cases += 1,
            _ if is_word(text, Some(t), "END") => cases = cases.saturating_sub(1),
            _ => {}
        }
    }
    cases == 0
}

/// Filter text of `terms`, each as written, joined with ` AND `: one term alone as it is; with
/// others, a term that is not proven to be a conjunction of its own ([`split_terms`], one with
/// an `OR` at its top level) in parentheses, so that the `AND`s around it never change what it
/// means.
pub fn join_terms(terms: &[&str], d: Dialect) -> String {
    if let [one] = terms {
        return (*one).to_string();
    }
    let written: Vec<String> = terms
        .iter()
        .map(|t| match split_terms(t, d) {
            Some(_) => (*t).to_string(),
            None => format!("({t})"),
        })
        .collect();
    written.join(" AND ")
}

/// What a pattern looks for ([`like_kind`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LikeKind {
    /// Values that contain this text ([`like_contains`](super::like_contains)).
    Contains(String),
    /// Values that start with this text ([`like_starts_with`](super::like_starts_with)).
    StartsWith(String),
}

/// What `pattern`, escaped with `escape`, looks for when it is one that
/// [`like_contains`](super::like_contains) or [`like_starts_with`](super::like_starts_with)
/// writes (with `escape` `!`): a `%`, text in which `%`, `_` and `escape` are escaped, and
/// another `%`; or that text and a `%`. `None` for any other pattern (a wildcard in the text,
/// an escape before another character or at the end).
pub fn like_kind(pattern: &str, escape: char) -> Option<LikeKind> {
    enum Unit {
        Char(char),
        Any,
    }
    let mut units = Vec::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        units.push(match c {
            c if c == escape => match chars.next()? {
                n if matches!(n, '%' | '_') || n == escape => Unit::Char(n),
                _ => return None,
            },
            '%' => Unit::Any,
            '_' => return None,
            c => Unit::Char(c),
        });
    }
    let text = |units: &[Unit]| -> Option<String> {
        units
            .iter()
            .map(|u| match u {
                Unit::Char(c) => Some(*c),
                Unit::Any => None,
            })
            .collect()
    };
    match units.as_slice() {
        [Unit::Any, inner @ .., Unit::Any] => text(inner).map(LikeKind::Contains),
        [inner @ .., Unit::Any] => text(inner).map(LikeKind::StartsWith),
        _ => None,
    }
}
