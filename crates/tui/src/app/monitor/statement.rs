//! The statement of the Monitor tab's details as it is shown: laid out for reading by the
//! editor's formatter when it can be (for the eyes only: copying and opening it in a console
//! take the server's text), with its tokens for the syntax colours. Worked out once per text,
//! never per frame.

use datarig_core::monitor::QueryText;
use datarig_core::sql::dialect::Dialect;
use datarig_core::sql::format::{self, KeywordCase, Options};
use datarig_core::sql::lexer::{Tok, lex_in};
use std::ops::Range;

/// A statement longer than this many bytes is shown as the server sent it: laying it out
/// would hold the screen up too long.
pub const FORMAT_MAX: usize = 64 * 1024;

/// How the statement shown is laid out, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Laid out by the formatter (only the whitespace between its tokens differs).
    Formatted,
    /// As the server sent it: asked for (the toggle).
    AsSent,
    /// As sent: only its start is known (the list's, or the server cut it), and a layout
    /// would hide where it stops.
    Part,
    /// As sent: the formatter would change more than the whitespace.
    Refused,
    /// As sent: longer than [`FORMAT_MAX`].
    TooLong,
}

/// The statement shown, and what it was worked out from.
#[derive(Clone, Debug)]
pub struct Shown {
    /// The server's text it comes from.
    source: String,
    part: bool,
    want: bool,
    dialect: Dialect,
    indent: u8,
    /// The text shown.
    pub text: String,
    pub layout: Layout,
    /// The tokens of `text`: where, which kind, and whether a name is called (`name(`).
    pub tokens: Vec<(Range<usize>, Tok, bool)>,
}

impl Shown {
    /// The server's text this was worked out from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Whether this was worked out from `q` with these settings.
    fn same(&self, q: &QueryText, want: bool, dialect: Dialect, indent: u8) -> bool {
        self.want == want
            && self.dialect == dialect
            && self.indent == indent
            && self.part == part(q)
            && self.source == q.text
    }
}

/// Only part of the statement is known.
fn part(q: &QueryText) -> bool {
    q.shortened || q.maybe_cut_by_server
}

/// `q` as it is shown: laid out when `want` and the formatter can lay it out in `dialect`
/// with `indent` spaces, else as sent. `prev` is kept when it was worked out from the same.
pub fn shown(prev: Option<Shown>, q: &QueryText, want: bool, dialect: Dialect, indent: u8) -> Shown {
    if let Some(prev) = prev.filter(|p| p.same(q, want, dialect, indent)) {
        return prev;
    }
    let (text, layout) = if !want {
        (q.text.clone(), Layout::AsSent)
    } else if part(q) {
        (q.text.clone(), Layout::Part)
    } else if q.text.len() > FORMAT_MAX {
        (q.text.clone(), Layout::TooLong)
    } else {
        // Keywords as written: only the whitespace changes.
        let opts = Options { case: KeywordCase::Preserve, indent };
        match format::format_in(&q.text, opts, "", dialect) {
            // From the first token to the last: the blanks around them are only blanks.
            Ok((span, out)) if !span.is_empty() => (out, Layout::Formatted),
            Ok(_) => (q.text.clone(), Layout::Formatted),
            Err(format::Refused::Changed { .. }) => (q.text.clone(), Layout::Refused),
        }
    };
    let toks = lex_in(&text, dialect);
    let tokens = toks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let call = t.kind == Tok::Ident && toks.get(i + 1).is_some_and(|n| n.kind == Tok::LParen);
            (t.start..t.end, t.kind, call)
        })
        .collect();
    Shown { source: q.text.clone(), part: part(q), want, dialect, indent, text, layout, tokens }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datarig_core::sql::dialect::MySqlMode;

    fn q(text: &str, shortened: bool, cut: bool) -> QueryText {
        QueryText { text: text.into(), shortened, maybe_cut_by_server: cut }
    }

    const SQL: &str = "select a, b from t where a = 1 order by b";

    #[test]
    fn a_whole_statement_is_laid_out_with_only_its_blanks_changed() {
        for d in [Dialect::Postgres, Dialect::MySql(MySqlMode::default())] {
            let s = shown(None, &q(SQL, false, false), true, d, 2);
            assert_eq!(s.layout, Layout::Formatted, "{d:?}");
            assert!(s.text.contains('\n'), "{d:?}: {}", s.text);
            let words = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ");
            assert_eq!(words(&s.text), words(SQL), "{d:?}: keywords as written");
            assert!(s.tokens.iter().all(|(r, ..)| r.end <= s.text.len()));
            assert_eq!(s.tokens.first().map(|t| t.1), Some(Tok::Keyword));
        }
    }

    #[test]
    fn what_is_known_only_in_part_refused_too_long_or_not_wanted_is_shown_as_sent() {
        let d = Dialect::Postgres;
        let cases = [
            (q(SQL, true, false), true, Layout::Part),
            (q(SQL, false, true), true, Layout::Part),
            (q("SELECT U&'d\\0061t' FROM t", false, false), true, Layout::Refused),
            (q(&format!("SELECT {}", "a, ".repeat(FORMAT_MAX / 3)), false, false), true, Layout::TooLong),
            (q(SQL, false, false), false, Layout::AsSent),
            (q(SQL, true, true), false, Layout::AsSent),
        ];
        for (query, want, layout) in cases {
            let s = shown(None, &query, want, d, 2);
            assert_eq!(s.layout, layout);
            assert_eq!(s.text, query.text, "{layout:?}: the server's text");
        }
    }

    #[test]
    fn the_same_text_and_settings_keep_what_was_worked_out() {
        let d = Dialect::Postgres;
        let first = shown(None, &q(SQL, false, false), true, d, 2);
        let at = first.text.as_ptr();
        let again = shown(Some(first), &q(SQL, false, false), true, d, 2);
        assert_eq!(again.text.as_ptr(), at, "kept");
        // Anything else that changes it: worked out again.
        for (query, want, indent, d2) in [
            (q("select 2", false, false), true, 2, d),
            (q(SQL, false, false), false, 2, d),
            (q(SQL, false, false), true, 4, d),
            (q(SQL, true, false), true, 2, d),
            (q(SQL, false, false), true, 2, Dialect::MySql(MySqlMode::default())),
        ] {
            let prev = shown(None, &q(SQL, false, false), true, d, 2);
            assert!(!prev.same(&query, want, d2, indent), "{query:?} {want} {indent} {d2:?}");
            let next = shown(Some(prev), &query, want, d2, indent);
            assert_eq!(next.source(), query.text);
        }
    }
}
