//! The values a table's CHECK constraints allow a column to hold, as completion offers them
//! inside a string literal ([`check_values`]). Only conditions that list the values are read;
//! any other condition gives none.

use super::dialect::Dialect;
use super::lexer::{Tok, Token, lex_in};
use crate::driver::structure::CheckConstraint;

/// The values a table's checks allow one of its columns to hold, in the order they are written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckValues {
    pub column: String,
    pub values: Vec<String>,
}

/// The values the enforced `checks` of a table (read from a server of dialect `d`) allow its
/// columns to hold, for conditions of these forms on one of the check's columns, as the server
/// writes them (casts, parentheses and a MySQL charset introducer around the parts):
///
/// * `col IN ('a', 'b')`, `col = ANY (ARRAY['a', 'b'])` (PostgreSQL writes an `IN` so);
/// * `col = 'a'`;
/// * an `OR` of those on the same column.
///
/// A `NULL` in a list is left out; a list with something else than strings and `NULL`, and
/// every other condition, gives nothing. Two checks listing values of one column allow only the
/// values both list.
pub fn check_values(d: Dialect, checks: &[CheckConstraint]) -> Vec<CheckValues> {
    let d = d.printed();
    let mut out: Vec<CheckValues> = Vec::new();
    for c in checks.iter().filter(|c| c.enforced()) {
        let Some((column, values)) = condition_values(d, &c.expression) else { continue };
        // The check's own name for the column (the condition may write it in another case).
        let Some(column) = c.columns.iter().find(|n| same_name(d, n, &column)) else { continue };
        match out.iter_mut().find(|v| &v.column == column) {
            Some(v) => v.values.retain(|x| values.contains(x)),
            None => out.push(CheckValues { column: column.clone(), values }),
        }
    }
    out
}

/// A name the server reads `written` as is `name` (PostgreSQL: as written, a quoted one, or
/// folded; MySQL: whatever its case).
fn same_name(d: Dialect, name: &str, written: &str) -> bool {
    match d {
        Dialect::Postgres => name == written,
        Dialect::MySql(_) => name.eq_ignore_ascii_case(written),
    }
}

/// The column and the values of a condition of the forms [`check_values`] reads.
fn condition_values(d: Dialect, expression: &str) -> Option<(String, Vec<String>)> {
    let all = lex_in(expression, d);
    let toks: Vec<&Token> = all.iter().filter(|t| !t.is_trivia()).collect();
    let (mut column, mut values) = (None, Vec::new());
    or_terms(d, &toks, expression, &mut column, &mut values)?;
    Some((column?, values))
}

/// The terms of an `OR` (each in parentheses or not, nested or not) into `column` and
/// `values`; `None` when one of them is not of the forms read or names another column.
fn or_terms(
    d: Dialect,
    toks: &[&Token],
    src: &str,
    column: &mut Option<String>,
    values: &mut Vec<String>,
) -> Option<()> {
    let terms = split_top(unwrap_parens(toks), src, |t| is_word(t, src, "OR"));
    if terms.len() > 1 {
        return terms.into_iter().try_for_each(|t| or_terms(d, t, src, column, values));
    }
    let (col, vals) = term_values(d, unwrap_parens(terms[0]), src)?;
    if column.as_ref().is_some_and(|c| *c != col) {
        return None;
    }
    *column = Some(col);
    for v in vals {
        if !values.contains(&v) {
            values.push(v);
        }
    }
    Some(())
}

/// `col = lit`, `col = ANY (array)` or `col IN (list)`.
fn term_values(d: Dialect, term: &[&Token], src: &str) -> Option<(String, Vec<String>)> {
    let at = top_level(term, src).find(|&i| is_equals(term, i, src) || is_word(term[i], src, "IN"))?;
    let column = name_of(d, bare(&term[..at], src)?, src)?;
    let rhs = &term[at + 1..];
    let items = if is_word(term[at], src, "IN") {
        // Not `NOT IN`.
        if at > 0 && is_word(term[at - 1], src, "NOT") {
            return None;
        }
        list(rhs, src, Tok::LParen)?
    } else if rhs.first().is_some_and(|t| is_word(t, src, "ANY") || is_word(t, src, "SOME")) {
        let inner = unwrap_parens(rhs.get(1..)?);
        if inner.len() + 1 == rhs.len() {
            // `ANY` without parentheses after it.
            return None;
        }
        let array = bare(inner, src)?;
        if !array.first().is_some_and(|t| is_word(t, src, "ARRAY")) {
            return None;
        }
        list(&array[1..], src, Tok::Op)?
    } else {
        vec![bare(rhs, src)?]
    };
    let mut values = Vec::new();
    for item in items {
        match literal(d, item, src)? {
            Some(v) if !values.contains(&v) => values.push(v),
            _ => {}
        }
    }
    Some((column, values))
}

/// The items of `(a, b)` (`open`: `LParen`) or `[a, b]` (`open`: `Op`, a `[`), which must be
/// all of `toks`.
fn list<'t, 'a>(toks: &'t [&'a Token], src: &str, open: Tok) -> Option<Vec<&'t [&'a Token]>> {
    let (first, last) = (toks.first()?, toks.last()?);
    let ok = match open {
        Tok::LParen => first.kind == Tok::LParen && last.kind == Tok::RParen,
        _ => first.text(src) == "[" && last.text(src) == "]",
    };
    if !ok || toks.len() < 2 || matching(toks, 0, src) != Some(toks.len() - 1) {
        return None;
    }
    Some(split_top(&toks[1..toks.len() - 1], src, |t| t.kind == Tok::Comma))
}

/// What literal `item` holds: `Some(value)` for a string (a MySQL charset introducer before
/// it, `_utf8mb4'a'`), `None` for `NULL`; anything else is not a literal (`None` outside).
fn literal(d: Dialect, item: &[&Token], src: &str) -> Option<Option<String>> {
    match bare(item, src)? {
        [t] if is_word(t, src, "NULL") => Some(None),
        [t] if t.kind == Tok::Str => d.unquote_literal(t.text(src)).map(Some),
        [cs, t]
            if t.kind == Tok::Str
                && cs.kind == Tok::Ident
                && cs.end == t.start
                && cs.text(src).starts_with('_')
                && matches!(d, Dialect::MySql(_)) =>
        {
            d.unquote_literal(t.text(src)).map(Some)
        }
        _ => None,
    }
}

/// The column a reference names (`col`, `t.col`, a quoted name).
fn name_of(d: Dialect, toks: &[&Token], src: &str) -> Option<String> {
    let last = toks.last()?;
    let chain = toks.iter().enumerate().all(|(i, t)| {
        if i % 2 == 0 { matches!(t.kind, Tok::Ident | Tok::QuotedIdent | Tok::Keyword) } else { t.kind == Tok::Dot }
    });
    if !chain || toks.len().is_multiple_of(2) {
        return None;
    }
    let text = last.text(src);
    Some(if last.kind == Tok::QuotedIdent { d.unquote_lenient(text) } else { d.fold(text) })
}

/// `toks` without the casts after it (`::text`, `::character varying(10)[]`) and the
/// parentheses around it, as often as they are there.
fn bare<'t, 'a>(mut toks: &'t [&'a Token], src: &str) -> Option<&'t [&'a Token]> {
    loop {
        let before = toks.len();
        toks = unwrap_parens(toks);
        if let Some(i) = top_level(toks, src).find(|&i| is_cast(toks, i, src)) {
            // What follows is a type: names, a modifier in parentheses, `[]`.
            const OPERATORS: &[&str] = &["AND", "OR", "NOT", "IS", "IN", "LIKE", "ILIKE", "BETWEEN", "COLLATE"];
            let ty = &toks[i + 2..];
            let type_part = |t: &&Token| match t.kind {
                Tok::Ident | Tok::Keyword => !OPERATORS.iter().any(|w| is_word(t, src, w)),
                Tok::QuotedIdent | Tok::Dot | Tok::Number | Tok::LParen | Tok::RParen | Tok::Comma => true,
                _ => matches!(t.text(src), "[" | "]"),
            };
            if ty.is_empty() || !ty.iter().all(type_part) {
                return None;
            }
            toks = &toks[..i];
        }
        if toks.is_empty() {
            return None;
        }
        if toks.len() == before {
            return Some(toks);
        }
    }
}

/// `toks` without the parentheses around all of it.
fn unwrap_parens<'t, 'a>(mut toks: &'t [&'a Token]) -> &'t [&'a Token] {
    while toks.len() >= 2
        && toks[0].kind == Tok::LParen
        && toks[toks.len() - 1].kind == Tok::RParen
        && matching(toks, 0, "") == Some(toks.len() - 1)
    {
        toks = &toks[1..toks.len() - 1];
    }
    toks
}

/// The index of the bracket closing the one at `open` (`(`, or `[` when `src` is given).
fn matching(toks: &[&Token], open: usize, src: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, t) in toks.iter().enumerate().skip(open) {
        depth += delta(t, src);
        if depth == 0 {
            return Some(i);
        }
    }
    None
}

/// How a token changes the bracket depth: `(` and `[` open, `)` and `]` close (brackets only
/// when `src` is given).
fn delta(t: &Token, src: &str) -> i32 {
    match t.kind {
        Tok::LParen => 1,
        Tok::RParen => -1,
        Tok::Op if !src.is_empty() && t.text(src) == "[" => 1,
        Tok::Op if !src.is_empty() && t.text(src) == "]" => -1,
        _ => 0,
    }
}

/// The indexes of the tokens of `toks` outside any bracket.
fn top_level<'a>(toks: &'a [&'a Token], src: &'a str) -> impl Iterator<Item = usize> + 'a {
    let mut depth = 0i32;
    toks.iter().enumerate().filter_map(move |(i, t)| {
        let d = delta(t, src);
        let top = depth == 0 && d == 0;
        depth += d;
        top.then_some(i)
    })
}

/// `toks` cut at the tokens outside brackets `sep` says separate it.
fn split_top<'t, 'a>(toks: &'t [&'a Token], src: &str, sep: impl Fn(&Token) -> bool) -> Vec<&'t [&'a Token]> {
    let mut out = Vec::new();
    let (mut depth, mut from) = (0i32, 0);
    for (i, t) in toks.iter().enumerate() {
        depth += delta(t, src);
        if depth == 0 && sep(t) {
            out.push(&toks[from..i]);
            from = i + 1;
        }
    }
    out.push(&toks[from..]);
    out
}

fn is_word(t: &Token, src: &str, word: &str) -> bool {
    t.is_word() && t.text(src).eq_ignore_ascii_case(word)
}

/// `toks[i]` is a `=` that compares (not part of `<=`, `>=`, `!=`, `<=>`, `=>`, `:=`).
fn is_equals(toks: &[&Token], i: usize, src: &str) -> bool {
    let t = toks[i];
    let glued_before = i > 0 && toks[i - 1].end == t.start && toks[i - 1].kind == Tok::Op;
    let glued_after = toks.get(i + 1).is_some_and(|n| n.start == t.end && n.kind == Tok::Op);
    t.kind == Tok::Op && t.text(src) == "=" && !glued_before && !glued_after
}

/// `toks[i]` and the next token are a cast's `::`.
fn is_cast(toks: &[&Token], i: usize, src: &str) -> bool {
    let colon = |t: &Token| t.kind == Tok::Op && t.text(src) == ":";
    colon(toks[i]) && toks.get(i + 1).is_some_and(|n| colon(n) && n.start == toks[i].end)
}

#[cfg(test)]
mod tests;
