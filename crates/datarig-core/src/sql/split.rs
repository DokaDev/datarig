//! Statement splitter. Splits on `;` outside strings, quoted identifiers,
//! dollar bodies and comments (as the text's dialect lexes them). Comment/whitespace-only pieces
//! are not statements.

use super::dialect::Dialect;
use super::lexer::{Tok, lex_in};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Statement {
    /// First non-trivia byte.
    pub start: usize,
    /// End of the last non-trivia token before the terminating `;` (exclusive).
    pub body_end: usize,
    /// End including the `;` when present (exclusive).
    pub end: usize,
}

impl Statement {
    pub fn body<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.body_end]
    }
}

/// The statements of PostgreSQL text `src` ([`split_in`]).
pub fn split(src: &str) -> Vec<Statement> {
    split_in(src, Dialect::Postgres)
}

/// The statements of `src`, text in dialect `d`.
pub fn split_in(src: &str, d: Dialect) -> Vec<Statement> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut body_end = 0;
    for t in lex_in(src, d) {
        if t.kind == Tok::Semi {
            if let Some(s) = start.take() {
                out.push(Statement { start: s, body_end, end: t.end });
            }
        } else if !t.is_trivia() {
            start.get_or_insert(t.start);
            body_end = t.end;
        }
    }
    if let Some(s) = start {
        out.push(Statement { start: s, body_end, end: body_end });
    }
    out
}

/// Statement under the cursor: the one whose `[start, end]` contains it (so "right after `;`"
/// still counts), otherwise the closest statement before the cursor (cursor on blank lines /
/// whitespace after a `;`).
pub fn statement_at(stmts: &[Statement], cursor: usize) -> Option<usize> {
    stmts
        .iter()
        .position(|s| cursor >= s.start && cursor <= s.end)
        .or_else(|| stmts.iter().rposition(|s| s.end <= cursor))
}

/// Byte range of the `;`-delimited segment containing `cursor` (used by completion so that
/// incomplete trailing text still forms its own context). PostgreSQL ([`segment_at_in`]).
pub fn segment_at(src: &str, cursor: usize) -> (usize, usize) {
    segment_at_in(src, cursor, Dialect::Postgres)
}

/// [`segment_at`] in text of dialect `d`.
pub fn segment_at_in(src: &str, cursor: usize, d: Dialect) -> (usize, usize) {
    let mut seg_start = 0;
    for t in lex_in(src, d) {
        if t.kind == Tok::Semi {
            if cursor <= t.start {
                return (seg_start, t.start);
            }
            seg_start = t.end;
        }
    }
    (seg_start, src.len())
}

#[cfg(test)]
mod tests;
