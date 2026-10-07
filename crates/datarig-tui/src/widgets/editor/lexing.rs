//! Lexing only what is needed: the lexer's state cached at each line start, the text of a run
//! of lines from where lexing can start, and the statement or completion segment around the
//! cursor found in a region of lines around it.

use super::Editor;
use datarig_core::sql::lexer::{Tok, Token, lex_in};
use datarig_core::sql::split::{Statement, split_in, statement_at};

/// The lexer's state at the start of a line: between tokens, or inside a token that spans
/// lines (a block comment, a string, a dollar body, a quoted identifier) that starts at `line`,
/// `byte`, where lexing can start again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LineState {
    Normal,
    Inside { line: usize, byte: usize },
}

/// Lines around the cursor searched first for the `;` around it.
pub(super) const REGION_LINES: usize = 64;

impl Editor {
    /// Where lexing can start for line `r`: its start, or the start of the token that spans
    /// into it (from the cached line states; [`Editor::ensure_states`] first).
    pub(super) fn restart_of(&self, r: usize) -> (usize, usize) {
        match self.states.get(r) {
            Some(LineState::Inside { line, byte }) => (*line, *byte),
            _ => (r, 0),
        }
    }

    /// Make the lexer states of lines `..=upto` current, lexing from the last current one.
    pub(super) fn ensure_states(&mut self, upto: usize) {
        let upto = upto.min(self.lines.len() - 1);
        if self.valid == 0 {
            self.states.clear();
            self.states.push(LineState::Normal);
            self.valid = 1;
        }
        if self.valid > upto {
            return;
        }
        let from = self.valid - 1;
        let (base_line, base_byte) = self.restart_of(from);
        let mut region = String::from(&self.lines[base_line][base_byte..]);
        // Where each line after `base_line` starts in `region`.
        let mut starts = Vec::with_capacity(upto - base_line);
        // One line more than asked for (when there is one): a token that spans into line `upto`
        // must not end where the region ends.
        let end = (upto + 1).min(self.lines.len() - 1);
        for r in base_line + 1..=end {
            region.push('\n');
            starts.push((r, region.len()));
            region.push_str(&self.lines[r]);
        }
        let toks = lex_in(&region, self.lang.dialect());
        // The region's line starts back to (line, byte).
        let line_of = |off: usize| -> (usize, usize) {
            match starts.binary_search_by(|(_, s)| s.cmp(&off)) {
                Ok(i) => (starts[i].0, 0),
                Err(0) => (base_line, base_byte + off),
                Err(i) => (starts[i - 1].0, off - starts[i - 1].1),
            }
        };
        self.states.truncate(from + 1);
        let mut ti = 0;
        for &(r, at) in &starts {
            if r <= from || r > upto {
                continue;
            }
            while ti < toks.len() && toks[ti].end <= at {
                ti += 1;
            }
            let state = match toks.get(ti) {
                Some(t)
                    if t.start < at
                        && matches!(t.kind, Tok::BlockComment | Tok::Str | Tok::Dollar | Tok::QuotedIdent) =>
                {
                    let (line, byte) = line_of(t.start);
                    LineState::Inside { line, byte }
                }
                _ => LineState::Normal,
            };
            self.states.push(state);
        }
        self.valid = upto + 1;
    }

    /// The text of lines `first..last` from where lexing can start for `first`, and the byte
    /// offset in the whole text where it starts.
    pub(super) fn region_text(&mut self, first: usize, last: usize) -> (usize, String) {
        let first = first.min(self.lines.len() - 1);
        let last = last.clamp(first + 1, self.lines.len());
        self.ensure_states(last - 1);
        let (line, byte) = self.restart_of(first);
        let base = self.line_start(line) + byte;
        let mut region = String::from(&self.lines[line][byte..]);
        for l in &self.lines[line + 1..last] {
            region.push('\n');
            region.push_str(l);
        }
        (base, region)
    }

    /// Lines `row - k ..= row + k` from where lexing can start: where they start in the text,
    /// the text, its tokens, the cursor in it and whether they reach the end of the text.
    fn region_around(&mut self, k: usize) -> (usize, String, Vec<Token>, usize, bool) {
        let cursor = self.offset();
        let n = self.lines.len();
        let first = self.row.saturating_sub(k);
        let last = (self.row + k + 1).min(n);
        let (base, region) = self.region_text(first, last);
        let toks = lex_in(&region, self.lang.dialect());
        (base, region, toks, cursor - base, last == n)
    }

    /// The statement under the cursor as [`statement_at`] finds it in the split whole text: its
    /// byte range in the text (to the end of its `;`) and its body. Found in lines around the
    /// cursor, taking more lines until the answer cannot depend on text outside them.
    pub fn current_statement(&mut self) -> Option<(usize, usize, String)> {
        let mut k = self.region.max(1);
        loop {
            let (base, region, toks, c, to_end) = self.region_around(k);
            // The statement the cursor is in must end in the region.
            if to_end || toks.iter().any(|t| t.kind == Tok::Semi && t.start >= c) {
                let stmts: Vec<Statement> = split_in(&region, self.lang.dialect());
                // A statement before the region's first `;` may have started before the region.
                let whole_from =
                    if base == 0 { 0 } else { toks.iter().find(|t| t.kind == Tok::Semi).map_or(usize::MAX, |t| t.end) };
                match statement_at(&stmts, c) {
                    Some(i) if stmts[i].start >= whole_from => {
                        let st = stmts[i];
                        return Some((base + st.start, base + st.end, st.body(&region).to_string()));
                    }
                    None if base == 0 => return None,
                    _ => {}
                }
            }
            k = k.saturating_mul(4);
        }
    }

    /// Text around the cursor for completion: where it starts in the whole text, the text and
    /// the cursor in it. It holds the `;`-delimited segment around the cursor whole (a `;`
    /// before the cursor or the start of the text, one after it or the end of the text).
    pub fn completion_context(&mut self) -> (usize, String, usize) {
        let mut k = self.region.max(1);
        loop {
            let (base, region, toks, c, to_end) = self.region_around(k);
            let before = base == 0 || toks.iter().any(|t| t.kind == Tok::Semi && t.start < c);
            let after = to_end || toks.iter().any(|t| t.kind == Tok::Semi && t.start >= c);
            if before && after {
                return (base, region, c);
            }
            k = k.saturating_mul(4);
        }
    }

    /// The identifier characters right before the cursor (on its line).
    pub fn ident_before_cursor(&self) -> &str {
        let line = &self.lines[self.row];
        let b = self.byte_at(self.row, self.col);
        let start = line[..b].char_indices().rev().take_while(|(_, c)| c.is_alphanumeric() || *c == '_').last();
        start.map_or("", |(i, _)| &line[i..b])
    }
}

#[cfg(test)]
mod tests;
