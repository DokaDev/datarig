//! Visual mode: by character (`v`) or by whole lines (`V`), the selection as text, text
//! objects that set or grow it, and the operators on it.

use super::buffer::graphemes;
use super::edit::Case;
use super::motion::RangeKind;
use super::textobj::Object;
use super::vim::{Op, Target, Token};
use super::{EdEvent, Editor, Mode, vim};
use ratatui::crossterm::event::{KeyCode, KeyEvent};

impl Editor {
    /// Start Visual mode (by line with `lines`) with the other end at `anchor`.
    pub(super) fn enter_visual(&mut self, lines: bool, anchor: (usize, usize)) -> EdEvent {
        if self.mode == Mode::Insert {
            self.leave_insert();
        }
        self.mode = Mode::Visual;
        self.visual_lines = lines;
        self.anchor = anchor;
        EdEvent::Moved
    }

    pub fn exit_visual(&mut self) {
        if self.mode == Mode::Visual {
            self.mode = Mode::Normal;
            self.clamp();
        }
    }

    /// The selection's ends in text order.
    fn visual_ends(&self) -> ((usize, usize), (usize, usize)) {
        let cursor = (self.row, self.col);
        if self.anchor <= cursor { (self.anchor, cursor) } else { (cursor, self.anchor) }
    }

    /// Where the selection starts (the start of its first line by line).
    fn visual_start(&self) -> (usize, usize) {
        let (a, _) = self.visual_ends();
        if self.visual_lines { (a.0, 0) } else { a }
    }

    /// Byte range of the selection: by character both ends included (a selected line end is
    /// its line break, and after `$` the cursor's line break is selected), by line whole lines
    /// with the line break after the last one.
    pub(super) fn visual_bounds(&self) -> (usize, usize) {
        let (a, b) = self.visual_ends();
        let more = |r: usize| usize::from(r + 1 < self.lines.len());
        if self.visual_lines {
            let (start, _) = self.line_bounds(a.0);
            let (_, end) = self.line_bounds(b.0);
            return (start, end + more(b.0));
        }
        let lo = self.offset_of(a.0, a.1);
        if b == (self.row, self.col) && self.want_x == Some(usize::MAX) {
            let (_, end) = self.line_bounds(b.0);
            return (lo, end + more(b.0));
        }
        let hi = self.offset_of(b.0, b.1);
        let extra = match graphemes(&self.lines[b.0]).get(b.1) {
            Some(g) => g.len(),
            None => more(b.0),
        };
        (lo, hi + extra)
    }

    /// What an operator on the selection takes (whole lines with `V`, or with `lines`: `D`,
    /// `Y`, `C` in Visual mode).
    pub(super) fn visual_target(&self, lines: bool) -> Target {
        let (a, b) = self.visual_ends();
        if lines || self.visual_lines {
            Target::Lines { first: a.0, last: b.0 }
        } else {
            let (a, b) = self.visual_bounds();
            Target::Chars { a, b }
        }
    }

    /// Selected text in Visual mode (for Ctrl+E): whole lines without the last line break by
    /// line.
    pub fn selection(&self) -> Option<String> {
        (self.mode == Mode::Visual).then(|| match self.visual_target(false) {
            Target::Chars { a, b } => self.slice(self.pos_bytes(a), self.pos_bytes(b)),
            Target::Lines { first, last } => self.lines[first..=last].join("\n"),
        })
    }

    pub(super) fn key_visual(&mut self, key: KeyEvent) -> EdEvent {
        let token = self.token(key, true);
        let (n, explicit) = self.cmd.count();
        if token == Token::More {
            return EdEvent::None;
        }
        self.cmd = vim::Pending::default();
        match token {
            Token::Cancel if key.code == KeyCode::Esc => {
                self.exit_visual();
                EdEvent::Moved
            }
            Token::Motion(m) => {
                self.move_by(m, n, explicit);
                EdEvent::Moved
            }
            Token::Object(o, around) => self.visual_object(o, around, n),
            Token::Ctrl(c @ ('d' | 'u')) => self.scroll_half(c == 'd', n, explicit),
            Token::Ctrl(c @ ('f' | 'b')) => self.scroll_page(c == 'f', n),
            Token::Z(c) => self.scroll_cursor(c, n, explicit),
            Token::Replace('\n') => EdEvent::None,
            Token::Replace(ch) => {
                self.record_selection();
                let (to, span) = (self.visual_start(), self.visual_bounds());
                self.replace_span(ch, span, to)
            }
            Token::G(c @ ('u' | 'U' | '~')) => self.visual_case(c),
            Token::G('J') => self.visual_join(false),
            Token::Key(c) => self.visual_command(c, n),
            _ => EdEvent::None,
        }
    }

    /// The command keys of Visual mode.
    fn visual_command(&mut self, c: char, n: usize) -> EdEvent {
        match c {
            'o' => {
                let cursor = (self.row, self.col);
                (self.row, self.col) = self.anchor;
                self.anchor = cursor;
                self.want_x = None;
                EdEvent::Moved
            }
            'v' | 'V' if self.visual_lines == (c == 'V') => {
                self.exit_visual();
                EdEvent::Moved
            }
            'v' | 'V' => {
                self.visual_lines = c == 'V';
                EdEvent::Moved
            }
            'y' | 'Y' => {
                let lines = c == 'Y' || self.visual_lines;
                let (a, _) = self.visual_ends();
                // By line the cursor keeps its column when it is above the other end.
                let to = match lines {
                    false => a,
                    true if self.row < self.anchor.0 => (a.0, self.col),
                    true => (a.0, 0),
                };
                let t = self.visual_target(lines);
                self.apply(Op::Yank, t, to)
            }
            'd' | 'x' | 'D' | 'X' => {
                self.record_selection();
                let t = self.visual_target(c.is_ascii_uppercase());
                self.apply(Op::Delete, t, (self.row, self.col))
            }
            'c' | 's' | 'C' | 'S' => {
                self.record_selection();
                let t = self.visual_target(c.is_ascii_uppercase());
                self.apply(Op::Change, t, (self.row, self.col))
            }
            'u' | 'U' | '~' => self.visual_case(c),
            'J' => self.visual_join(true),
            '>' | '<' => {
                self.record_selection();
                let (a, b) = self.visual_ends();
                self.mode = Mode::Normal;
                self.shift_lines(a.0, b.0, n, c == '>')
            }
            _ => EdEvent::None,
        }
    }

    /// `.` repeats a Visual mode operator on as much text as it took.
    fn record_selection(&mut self) {
        let sel = self.selection_input();
        self.rec.select(sel);
    }

    /// `u` `U` `~` (and `gu` `gU` `g~`) on the selection.
    fn visual_case(&mut self, c: char) -> EdEvent {
        self.record_selection();
        let case = match c {
            'u' => Case::Lower,
            'U' => Case::Upper,
            _ => Case::Toggle,
        };
        let (to, span) = (self.visual_start(), self.visual_bounds());
        self.mode = Mode::Normal;
        self.recase_span(case, span, to)
    }

    /// `J` (`spaces`) and `gJ`: join the selected lines (at least two).
    fn visual_join(&mut self, spaces: bool) -> EdEvent {
        self.record_selection();
        let (a, b) = self.visual_ends();
        self.mode = Mode::Normal;
        self.join_lines(a.0, b.0 - a.0 + 1, spaces)
    }

    /// A text object in Visual mode: it becomes the selection (a word, a string or a block by
    /// character, paragraphs by line), or grows it when it is more than the cursor.
    fn visual_object(&mut self, o: Object, around: bool, count: usize) -> EdEvent {
        let cursor = (self.row, self.col);
        let grow = (self.anchor != cursor).then_some(self.anchor);
        let Some(f) = self.object(o, count, around, grow) else { return EdEvent::None };
        self.anchor = f.start;
        let end = match f.kind {
            RangeKind::Exclusive if f.end.1 > 0 => (f.end.0, f.end.1 - 1),
            RangeKind::Exclusive if f.end.0 > 0 => (f.end.0 - 1, self.gcount(f.end.0 - 1).saturating_sub(1)),
            _ => f.end,
        };
        (self.row, self.col) = end;
        self.want_x = f.eol.then_some(usize::MAX);
        if !f.keep {
            self.visual_lines = f.kind == RangeKind::Linewise;
        }
        EdEvent::Moved
    }
}

#[cfg(test)]
mod tests;
