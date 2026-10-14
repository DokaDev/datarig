//! What the app's actions on SQL work on: the Visual selection, else the statement under the
//! cursor; and putting their result in its place as one undo step.

use super::Sel;
use super::vim::Target;
use super::{EdEvent, Editor, Mode};
use datarig_core::sql::lexer::Tok;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl Editor {
    /// The text an action on SQL takes, as a byte range: in Visual mode the selection (by line
    /// or block, its whole lines), which ends; otherwise the statement under the cursor (to the
    /// end of its `;`). `None` without a statement.
    pub fn sql_target(&mut self) -> Option<(usize, usize)> {
        if self.mode == Mode::Visual {
            let target = self.visual_target(self.sel == Sel::Block);
            self.exit_visual();
            return Some(match target {
                Target::Chars { a, b } => (a, b),
                Target::Lines { first, last } => self.lines_target(first, last),
            });
        }
        let (a, b, _) = self.current_statement()?;
        Some((a, b))
    }

    /// The byte range of lines `first..=last` (from 0; `:'<,'>format`).
    pub fn lines_target(&self, first: usize, last: usize) -> (usize, usize) {
        (self.line_start(first), self.line_start(last) + self.lines[last].len())
    }

    /// What comes before byte `a` on its line, when that is only blanks (the column text
    /// starting at `a` lines up with).
    pub fn indent_before(&self, a: usize) -> String {
        let (row, byte) = self.pos_bytes(a);
        let before = &self.lines[row][..byte];
        if before.bytes().all(|c| c == b' ' || c == b'\t') { before.to_string() } else { String::new() }
    }

    /// Byte `off` lies inside a token of the text around it (a word, a string, a comment, a
    /// dollar body, also one that starts on an earlier line), not between two: text from there
    /// is not SQL of its own.
    pub fn splits_token(&mut self, off: usize) -> bool {
        let row = self.pos_bytes(off).0;
        // One line more: a token that goes on past `off`'s line ends after it.
        let (base, region, state) = self.region_text(row, row + 2);
        let at = off - base;
        self.lex(&region, state).iter().any(|t| t.kind != Tok::Whitespace && t.start < at && at < t.end)
    }

    /// The line (from 0) of byte `off`.
    pub fn line_of(&self, off: usize) -> usize {
        self.pos_bytes(off).0
    }

    /// Bytes `a..b` of the text.
    pub fn text_between(&self, a: usize, b: usize) -> String {
        self.slice(self.pos_bytes(a), self.pos_bytes(b))
    }

    /// Replace bytes `a..b` with `s` as one undo step, in Normal mode; the cursor goes to `a`
    /// (where an undo puts it back too).
    pub fn replace_with(&mut self, a: usize, b: usize, s: &str) {
        self.set_cursor_offset(a);
        self.snapshot();
        self.splice(a, b, s);
        self.set_cursor_offset(a);
        self.want_x = None;
        self.clamp();
    }

    /// Comment the lines out with `-- `, or back in: `gc` on the Visual selection, else `gcc`
    /// on the cursor's line, typed as those keys so that `.` repeats it. From Insert mode, as
    /// after `Esc`.
    pub fn comment_toggle(&mut self) -> EdEvent {
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        if self.prompt.is_some() {
            self.close_prompt();
        }
        if self.mode == Mode::Insert {
            self.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        }
        self.cmd = super::vim::Pending::default();
        let keys: &[char] = if self.mode == Mode::Visual { &['g', 'c'] } else { &['g', 'c', 'c'] };
        let mut ev = EdEvent::None;
        for &c in keys {
            ev = self.handle_key(key(c));
        }
        ev
    }
}

#[cfg(test)]
mod tests;
