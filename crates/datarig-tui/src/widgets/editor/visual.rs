//! Visual mode: by character (`v`) or by whole lines (`V`), the selection as text, and the
//! operators on it.

use super::buffer::graphemes;
use super::motion::Motion;
use super::vim::{Op, Target};
use super::{EdEvent, Editor, Mode, vim};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            self.cmd = vim::Pending::default();
            return EdEvent::None;
        }
        if key.code == KeyCode::Esc {
            self.cmd = vim::Pending::default();
            self.exit_visual();
            return EdEvent::Moved;
        }
        let ch = match key.code {
            KeyCode::Char(c) => Some(c),
            _ => None,
        };
        if let Some(d) = ch.and_then(|c| c.to_digit(10))
            && !self.cmd.waits_g()
            && self.cmd.digit(d)
        {
            return EdEvent::None;
        }
        let (n, explicit) = self.cmd.count();
        if self.cmd.waits_g() {
            self.cmd = vim::Pending::default();
            if ch == Some('g') {
                self.move_by(Motion::Top, n, explicit);
            }
            return EdEvent::Moved;
        }
        if ch == Some('g') {
            self.cmd.set_g();
            return EdEvent::None;
        }
        self.cmd = vim::Pending::default();
        if let Some(m) = ch.map_or_else(|| Motion::of_key(key.code), Motion::of_char) {
            self.move_by(m, n, explicit);
            return EdEvent::Moved;
        }
        let Some(c) = ch else { return EdEvent::None };
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
                let t = self.visual_target(c.is_ascii_uppercase());
                self.apply(Op::Delete, t, (self.row, self.col))
            }
            'c' | 's' | 'C' | 'S' => {
                let t = self.visual_target(c.is_ascii_uppercase());
                self.apply(Op::Change, t, (self.row, self.col))
            }
            _ => EdEvent::None,
        }
    }
}

#[cfg(test)]
mod tests;
