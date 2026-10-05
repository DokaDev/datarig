//! Auto-pairs (`[editor] auto_pairs = "on"`): in Insert mode `(`, `[`, `{`, `'`, `"` and `` ` ``
//! also put their closing character after the cursor. Typing that closing character right
//! before it steps over it, and Backspace between the two deletes both; both only for a pair
//! this Insert session put in (a closing character the user typed is never skipped or
//! deleted along). Nothing pairs inside a string, a quoted name, a comment or a dollar-quoted
//! body, before a character that is not a blank, a closing bracket, `,` or `;`, and a quote
//! does not pair after a letter, digit or the same quote (`E'`, `it''s`). A paste is never
//! paired.
//!
//! `.` and a count repeat what happened (the pair, the step over, the deletion), not the key
//! again, so a repeat does the same in another place or with the setting changed.

use super::{Editor, Mode};
use datarig_core::sql::lexer::{Tok, lex};

/// What auto-pairs did for one key in Insert mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pair {
    /// Both characters went in, the cursor between them.
    Open(char, char),
    /// The cursor stepped over the closing character.
    Over,
    /// Backspace deleted the opening and the closing character.
    Unpair,
}

/// The closing character of `c`, when it opens a pair.
fn closer(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '\'' | '"' | '`' => Some(c),
        _ => None,
    }
}

/// A pair may open before `next` (the character after the cursor, if any).
fn opens_before(next: Option<char>) -> bool {
    next.is_none_or(|n| n.is_whitespace() || matches!(n, ')' | ']' | '}' | ',' | ';'))
}

impl Editor {
    /// What auto-pairs does with character `c` typed in Insert mode, if anything.
    pub(super) fn pair_for(&mut self, c: char) -> Option<Pair> {
        if !self.auto_pairs || self.mode != Mode::Insert || self.rec.replaying() {
            return None;
        }
        let line = &self.lines[self.row];
        let at = self.byte_at(self.row, self.col);
        let next = line[at..].chars().next();
        if next == Some(c) && self.pair_closer() == Some(c) {
            return Some(Pair::Over);
        }
        let close = closer(c)?;
        let prev = line[..at].chars().next_back();
        let quote = close == c;
        if !opens_before(next) || (quote && prev.is_some_and(|p| p == c || p.is_alphanumeric() || p == '_')) {
            return None;
        }
        if self.in_literal() {
            return None;
        }
        Some(Pair::Open(c, close))
    }

    /// Backspace deletes a pair this session put in, the cursor between its characters.
    pub(super) fn pair_backspace(&self) -> Option<Pair> {
        if !self.auto_pairs || self.rec.replaying() {
            return None;
        }
        let close = self.pair_closer()?;
        let at = self.byte_at(self.row, self.col);
        let prev = self.lines[self.row][..at].chars().next_back()?;
        (closer(prev) == Some(close)).then_some(Pair::Unpair)
    }

    /// Do what `p` says (also when `.` or a count repeats it).
    pub(super) fn apply_pair(&mut self, p: Pair) {
        let at = self.offset();
        match p {
            Pair::Open(open, close) => {
                let mut s = String::with_capacity(8);
                s.push(open);
                s.push(close);
                self.splice(at, at, &s);
                self.set_cursor_offset(at + open.len_utf8());
                self.want_x = None;
                let line = &self.lines[self.row];
                let from_end = line.len() - self.byte_at(self.row, self.col);
                self.pairs.push((self.row, from_end, close));
                self.ins_text.push(open);
            }
            Pair::Over => {
                let close = self.lines[self.row][self.byte_at(self.row, self.col)..].chars().next();
                self.pairs.pop();
                self.set_pos(self.row, self.col + 1);
                if let Some(c) = close {
                    self.ins_text.push(c);
                }
            }
            Pair::Unpair => {
                let (r, c) = (self.row, self.col);
                if c == 0 {
                    return;
                }
                let a = self.offset_of(r, c - 1);
                let b = self.offset_of(r, (c + 1).min(self.gcount(r)));
                let removed = self.delete_range(a, b);
                self.pairs.pop();
                if let Some(open) = removed.chars().next() {
                    let mut buf = [0u8; 4];
                    self.untyped(open.encode_utf8(&mut buf));
                }
                self.ins_start = self.ins_start.min((self.row, self.col));
            }
        }
    }

    /// The closing character of the last pair this session put in, when it is right after the
    /// cursor (typing before it on its line keeps it there).
    fn pair_closer(&self) -> Option<char> {
        let &(row, from_end, close) = self.pairs.last()?;
        let line = &self.lines[self.row];
        let at = self.byte_at(self.row, self.col);
        (row == self.row && line.len() - at == from_end && line[at..].starts_with(close)).then_some(close)
    }

    /// The cursor is inside a string, a quoted name, a comment or a dollar-quoted body.
    fn in_literal(&mut self) -> bool {
        let (base, region) = self.region_text(self.row, self.row + 1);
        let cur = self.offset() - base;
        let literal =
            |k: Tok| matches!(k, Tok::Str | Tok::QuotedIdent | Tok::BlockComment | Tok::Dollar | Tok::LineComment);
        let toks = lex(&region);
        let Some(t) = toks.iter().find(|t| literal(t.kind) && t.start < cur && cur <= t.end) else { return false };
        if cur < t.end || t.kind == Tok::LineComment {
            return true;
        }
        // Right after its end: inside only when it is not closed, which a line break after it
        // shows (an open one takes it in).
        let more = format!("{region}\n");
        lex(&more).iter().any(|u| u.start == t.start && u.end > t.end)
    }

    /// The Insert session ended or the cursor moved: no pair is pending any more.
    pub(super) fn forget_pairs(&mut self) {
        self.pairs.clear();
    }

    /// `n` line breaks went in at the cursor on line `row` (or, negative, the break before
    /// `row` went): the pending closing characters, all after the cursor, move with the text.
    pub(super) fn pairs_shift(&mut self, row: usize, n: isize) {
        for p in self.pairs.iter_mut().filter(|p| p.0 >= row) {
            p.0 = p.0.saturating_add_signed(n);
        }
    }

    /// Step over the pending closing characters right after the cursor (before a count types
    /// the session's text again after it).
    pub(super) fn step_over_pairs(&mut self) {
        while self.pair_closer().is_some() {
            self.pairs.pop();
            self.col += 1;
        }
    }
}

#[cfg(test)]
mod tests;
