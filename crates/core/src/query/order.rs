//! The text of an `ORDER BY` field as sort keys ([`parse_order`]), and sort keys as that text
//! ([`order_text`]).
//!
//! The field holds a comma separated list of keys, each a column of the table, then optionally
//! `ASC` or `DESC`, then optionally `NULLS FIRST` or `NULLS LAST` (PostgreSQL only: MySQL has no
//! such syntax). Nothing else: no expression, no position number, no comment. The keys are what
//! runs, written again by [`super::compile`] with every name quoted, so the field is not sent as
//! typed.
//!
//! A bare name is the column it names exactly, else the column the server folds it to
//! ([`Dialect::fold`]); MySQL compares column names without regard to case, so there a bare or
//! quoted name that matches one column so is that column. A quoted name is the column of exactly
//! that name. Names are read and written through the dialect only.

use super::{Dir, KeyOrigin, Nulls, OrderKey, Target};
use crate::sql::dialect::Dialect;
use crate::sql::lexer::{Tok, Token, lex_in};
use std::ops::Range;

/// Why an `ORDER BY` text is refused: the problem and the byte range of the text it is at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderError {
    pub range: Range<usize>,
    pub kind: OrderProblem,
}

/// What is wrong in an `ORDER BY` text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrderProblem {
    /// A key with no column: two commas in a row, or one at either end.
    MissingColumn,
    /// Not a column name (an expression, a number, a qualified name, a string).
    NotAColumn,
    /// The table has no such column (the name as it would be read).
    UnknownColumn(String),
    /// Two names match columns that differ only in case (MySQL), so which one is meant is not
    /// known.
    AmbiguousColumn(String),
    /// The same column twice (its name).
    Duplicate(String),
    /// A quoted name that does not end.
    Unterminated,
    /// `NULLS FIRST` or `NULLS LAST` on a server that has no such syntax (MySQL).
    NullsUnsupported,
    /// `NULLS` not followed by `FIRST` or `LAST`.
    NullsIncomplete,
    /// Something after a key that is neither its direction nor where its NULLs go, nor a comma.
    Unexpected,
}

/// The keys of `text`, an `ORDER BY` list for table `t`, each of [`KeyOrigin::User`]. A text of
/// blanks only is no order.
pub fn parse_order(text: &str, t: &Target) -> Result<Vec<OrderKey>, OrderError> {
    let d = t.dialect;
    let tokens: Vec<Token> = lex_in(text, d).into_iter().filter(|tok| tok.kind != Tok::Whitespace).collect();
    let mut keys: Vec<OrderKey> = Vec::new();
    if tokens.is_empty() {
        return Ok(keys);
    }
    let err = |tok: &Token, kind| Err(OrderError { range: tok.start..tok.end, kind });
    let word = |tok: &Token, w: &str| tok.is_word() && tok.text(text).eq_ignore_ascii_case(w);
    let mut i = 0;
    loop {
        // A column.
        let Some(tok) = tokens.get(i) else {
            // A comma at the end.
            let end = text.trim_end().len();
            return Err(OrderError { range: end.saturating_sub(1)..end, kind: OrderProblem::MissingColumn });
        };
        if tok.kind == Tok::Comma {
            return err(tok, OrderProblem::MissingColumn);
        }
        // A qualified name or a call is not a column.
        if let Some(next) = tokens.get(i + 1).filter(|n| n.kind == Tok::Dot || n.kind == Tok::LParen) {
            return err(next, OrderProblem::NotAColumn);
        }
        let col = column(text, tok, t)?;
        if keys.iter().any(|k| k.col == col) {
            return err(tok, OrderProblem::Duplicate(col));
        }
        i += 1;
        let mut key = OrderKey::user(col, Dir::Asc);
        // Its direction.
        if let Some(tok) = tokens.get(i) {
            if word(tok, "ASC") {
                i += 1;
            } else if word(tok, "DESC") {
                key.dir = Dir::Desc;
                i += 1;
            }
        }
        // Where its NULLs go.
        if let Some(tok) = tokens.get(i).filter(|tok| word(tok, "NULLS")) {
            let place = tokens.get(i + 1).and_then(|n| {
                if word(n, "FIRST") {
                    Some(Nulls::First)
                } else if word(n, "LAST") {
                    Some(Nulls::Last)
                } else {
                    None
                }
            });
            let Some(place) = place else {
                return err(tok, OrderProblem::NullsIncomplete);
            };
            if let Dialect::MySql(_) = d {
                let end = tokens[i + 1].end;
                return Err(OrderError { range: tok.start..end, kind: OrderProblem::NullsUnsupported });
            }
            key.nulls = Some(place);
            i += 2;
        }
        keys.push(key);
        match tokens.get(i) {
            None => return Ok(keys),
            Some(tok) if tok.kind == Tok::Comma => i += 1,
            Some(tok) => return err(tok, OrderProblem::Unexpected),
        }
    }
}

/// The column of `t` that token `tok` of `text` names.
fn column(text: &str, tok: &Token, t: &Target) -> Result<String, OrderError> {
    let d = t.dialect;
    let at = tok.start..tok.end;
    let fail = |kind| Err(OrderError { range: at.clone(), kind });
    let raw = tok.text(text);
    let (name, quoted) = match tok.kind {
        Tok::Ident | Tok::Keyword => (raw.to_string(), false),
        Tok::QuotedIdent => match d.unquote(raw) {
            Some(name) => (name, true),
            None => return fail(OrderProblem::Unterminated),
        },
        _ => return fail(OrderProblem::NotAColumn),
    };
    if t.column(&name).is_some() {
        return Ok(name);
    }
    if !quoted {
        let folded = d.fold(&name);
        if t.column(&folded).is_some() {
            return Ok(folded);
        }
    }
    if let Dialect::MySql(_) = d {
        let same: Vec<&str> =
            t.columns.iter().map(|c| c.name.as_str()).filter(|c| c.eq_ignore_ascii_case(&name)).collect();
        match same.as_slice() {
            [one] => return Ok(one.to_string()),
            [] => {}
            _ => return fail(OrderProblem::AmbiguousColumn(name)),
        }
    }
    let shown = if quoted { name } else { d.fold(&name) };
    fail(OrderProblem::UnknownColumn(shown))
}

/// The user's keys of `keys` as `ORDER BY` text of dialect `d`, the way [`parse_order`] reads it
/// back: each name quoted only when it must be, its direction always written. Tie-breaker keys
/// ([`KeyOrigin::TieBreak`]) are left out: the app adds them.
pub fn order_text(keys: &[OrderKey], d: Dialect) -> String {
    let mut parts = Vec::new();
    for k in keys.iter().filter(|k| k.origin == KeyOrigin::User) {
        let dir = match k.dir {
            Dir::Asc => "ASC",
            Dir::Desc => "DESC",
        };
        let mut part = format!("{} {dir}", d.quote_ident(&k.col));
        match k.nulls {
            Some(Nulls::First) => part.push_str(" NULLS FIRST"),
            Some(Nulls::Last) => part.push_str(" NULLS LAST"),
            None => {}
        }
        parts.push(part);
    }
    parts.join(", ")
}
