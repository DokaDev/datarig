//! What the editor's search and the result grid's search share: how a pattern is compiled
//! (Rust regular expressions, case sensitive unless they start with `(?i)`), how a pattern
//! error is told on one line, and what a search has to say for the app to show.

use regex::{Regex, RegexBuilder};

/// The most memory a compiled pattern may take (the regex crate's own default, named so both
/// searches keep the same bound).
pub const SIZE_LIMIT: usize = 10 * (1 << 20);

/// What a search has to say, for the app to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchNotice {
    /// It went past the end of the text (`bottom`) or its start, and on from the other end.
    Wrapped {
        bottom: bool,
    },
    NotFound(String),
    /// The pattern is not a valid regular expression: what is wrong with it.
    Invalid(String),
    /// `n`, `N` or an empty pattern without a search before.
    NoPrevious,
    /// `*` or `#` with nothing but blanks from the cursor to the end of the line.
    NoWord,
}

/// A summary of a pattern error on one line: the regex crate's own message without the
/// pattern and the caret drawn under it.
pub fn summary(e: &regex::Error) -> String {
    let s = e.to_string();
    match s.lines().rev().find_map(|l| l.trim().strip_prefix("error: ")) {
        Some(m) => m.to_string(),
        None => s.split_whitespace().collect::<Vec<_>>().join(" "),
    }
}

/// `pattern` compiled within [`SIZE_LIMIT`], or what is wrong with it ([`summary`]).
pub fn compile(pattern: &str) -> Result<Regex, String> {
    RegexBuilder::new(pattern).size_limit(SIZE_LIMIT).build().map_err(|e| summary(&e))
}
