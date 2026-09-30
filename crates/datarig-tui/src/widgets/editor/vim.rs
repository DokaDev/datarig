//! Normal mode: the command parser (`[count] operator [count] motion`, doubled operators
//! `dd cc yy`, the single-key commands) and the operators `d`, `c`, `y` on a range of text.

use super::buffer::{UNDO_BYTES, class, indent_of};
use super::motion::{Motion, Range, RangeKind};
use super::{EdEvent, Editor, Mode};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

/// The largest count taken (more digits are ignored).
const MAX_COUNT: usize = 99_999;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Op {
    Delete,
    Change,
    Yank,
}

impl Op {
    fn of(c: char) -> Option<Op> {
        match c {
            'd' => Some(Op::Delete),
            'c' => Some(Op::Change),
            'y' => Some(Op::Yank),
            _ => None,
        }
    }

    fn key(self) -> char {
        match self {
            Op::Delete => 'd',
            Op::Change => 'c',
            Op::Yank => 'y',
        }
    }
}

/// A command typed so far: counts (0: none), the operator, a `g` waiting for its second key.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Pending {
    count: usize,
    op: Option<Op>,
    op_count: usize,
    g: bool,
}

impl Pending {
    /// The command waits for a key the keymap should hand over (`d…`, `g…`).
    pub(super) fn awaiting(&self) -> bool {
        self.op.is_some() || self.g
    }

    /// The count of the command (the counts before and after the operator multiply) and
    /// whether one was typed.
    pub(super) fn count(&self) -> (usize, bool) {
        let n = self.count.max(1).saturating_mul(self.op_count.max(1)).min(MAX_COUNT);
        (n, self.count > 0 || self.op_count > 0)
    }

    pub(super) fn waits_g(&self) -> bool {
        self.g
    }

    pub(super) fn set_g(&mut self) {
        self.g = true;
    }

    /// Take digit `d` as part of a count; false when it is not one (`0` is a motion unless a
    /// count is being typed).
    pub(super) fn digit(&mut self, d: u32) -> bool {
        let n = if self.op.is_some() { &mut self.op_count } else { &mut self.count };
        if d == 0 && *n == 0 {
            return false;
        }
        *n = (*n * 10 + d as usize).min(MAX_COUNT);
        true
    }
}

/// The text an operator works on: bytes of the text, or whole lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Chars { a: usize, b: usize },
    Lines { first: usize, last: usize },
}

impl Editor {
    pub(super) fn key_normal(&mut self, key: KeyEvent) -> EdEvent {
        let ch = match key.code {
            KeyCode::Char(c) => Some(c),
            _ => None,
        };
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            let (n, _) = self.cmd.count();
            self.cmd = Pending::default();
            return match ch {
                Some('r') => self.undo_redo(false, n),
                _ => EdEvent::None,
            };
        }
        if key.code == KeyCode::Esc {
            self.cmd = Pending::default();
            return EdEvent::None;
        }
        if let Some(d) = ch.and_then(|c| c.to_digit(10))
            && !self.cmd.g
            && self.cmd.digit(d)
        {
            return EdEvent::None;
        }
        if self.cmd.g {
            self.cmd.g = false;
            if ch == Some('g') {
                return self.motion_key(Motion::Top);
            }
            self.cmd = Pending::default();
            return EdEvent::None;
        }
        if ch == Some('g') {
            self.cmd.g = true;
            return EdEvent::None;
        }
        if let Some(m) = ch.map_or_else(|| Motion::of_key(key.code), Motion::of_char) {
            return self.motion_key(m);
        }
        let (n, _) = self.cmd.count();
        if let Some(op) = self.cmd.op {
            self.cmd = Pending::default();
            // `dd`, `cc`, `yy`: `count` lines; any other key cancels.
            return if ch == Some(op.key()) { self.apply_lines(op, n) } else { EdEvent::None };
        }
        let Some(c) = ch else {
            self.cmd = Pending::default();
            return EdEvent::None;
        };
        if let Some(op) = Op::of(c) {
            self.cmd.op = Some(op);
            return EdEvent::None;
        }
        self.cmd = Pending::default();
        match c {
            'x' => self.apply_motion(Op::Delete, Motion::Right, n, false),
            'X' => self.apply_motion(Op::Delete, Motion::Left, n, false),
            's' => self.apply_motion(Op::Change, Motion::Right, n, false),
            'D' => self.apply_motion(Op::Delete, Motion::LineEnd, n, false),
            'C' => self.apply_motion(Op::Change, Motion::LineEnd, n, false),
            'Y' => self.apply_lines(Op::Yank, n),
            'S' => self.apply_lines(Op::Change, n),
            'i' => self.start_insert(true, (self.row, self.col)),
            'a' => {
                let c = if self.gcount(self.row) > 0 { self.col + 1 } else { 0 };
                self.start_insert(true, (self.row, c))
            }
            'I' => self.start_insert(true, (self.row, self.first_nonblank(self.row))),
            'A' => self.start_insert(true, (self.row, self.gcount(self.row))),
            'o' | 'O' => self.open_line(c == 'o'),
            'p' | 'P' => self.put(c == 'p', n),
            'u' => self.undo_redo(true, n),
            'v' => self.enter_visual(false, (self.row, self.col)),
            'V' => self.enter_visual(true, (self.row, self.col)),
            _ => EdEvent::None,
        }
    }

    /// A motion key: moves the cursor, or ends the pending operator.
    fn motion_key(&mut self, m: Motion) -> EdEvent {
        let (n, explicit) = self.cmd.count();
        let op = self.cmd.op;
        self.cmd = Pending::default();
        match op {
            None => {
                self.move_by(m, n, explicit);
                EdEvent::Moved
            }
            Some(op) => self.apply_motion(op, m, n, explicit),
        }
    }

    /// Operator `op` over motion `m` repeated `n` times. A change whose `h` or `l` cannot move
    /// still starts Insert mode (Vim).
    fn apply_motion(&mut self, op: Op, m: Motion, n: usize, explicit: bool) -> EdEvent {
        let cw = if op == Op::Change && m == Motion::WordForward { self.cw_range(n) } else { None };
        let Some(mut r) = cw.or_else(|| self.op_range(m, n, explicit)) else {
            if op == Op::Change && matches!(m, Motion::Left | Motion::Right) {
                return self.start_insert(true, (self.row, self.col));
            }
            return EdEvent::None;
        };
        if op == Op::Delete && self.deletes_whole_lines(r) {
            r.kind = RangeKind::Linewise;
        }
        let t = self.target_of(r);
        self.apply(op, t, r.start)
    }

    /// A delete over more than one line that starts in the indent and leaves only blanks
    /// after it on its last line takes the lines whole (Vim).
    fn deletes_whole_lines(&self, r: Range) -> bool {
        if r.kind == RangeKind::Linewise || r.end.0 == r.start.0 {
            return false;
        }
        let after = r.end.1 + usize::from(r.kind == RangeKind::Inclusive);
        let blank = |row: usize, from: usize, to: usize| {
            self.lines[row].graphemes(true).skip(from).take(to.saturating_sub(from)).all(|g| class(g) == 0)
        };
        blank(r.start.0, 0, r.start.1) && blank(r.end.0, after, usize::MAX)
    }

    /// `dd`, `cc`, `yy`, `S`, `Y`: `n` lines from the cursor's (as many as there are).
    fn apply_lines(&mut self, op: Op, n: usize) -> EdEvent {
        let last = (self.row + n - 1).min(self.lines.len() - 1);
        self.apply(op, Target::Lines { first: self.row, last }, (self.row, self.col))
    }

    /// The bytes or lines of range `r`.
    fn target_of(&self, r: Range) -> Target {
        match r.kind {
            RangeKind::Linewise => Target::Lines { first: r.start.0, last: r.end.0 },
            kind => {
                let a = self.offset_of(r.start.0, r.start.1);
                let mut b = self.offset_of(r.end.0, r.end.1);
                if kind == RangeKind::Inclusive {
                    b += self.lines[r.end.0].graphemes(true).nth(r.end.1).map_or(0, str::len);
                }
                Target::Chars { a, b }
            }
        }
    }

    /// Apply operator `op` to `t` as one undo step: the register gets the text (whole lines
    /// with `Lines`). A yank puts the cursor at `yank_to`; a delete at the start (on the first
    /// non-blank of the line for lines); a change starts Insert mode there (lines keep the
    /// first one's indent). Nothing happens to an empty span, except that a change still
    /// starts Insert mode.
    pub(super) fn apply(&mut self, op: Op, t: Target, yank_to: (usize, usize)) -> EdEvent {
        if self.mode == Mode::Visual {
            self.mode = Mode::Normal;
        }
        match (op, t) {
            (Op::Delete | Op::Yank, Target::Chars { a, b }) if a == b => {
                self.clamp();
                EdEvent::Moved
            }
            (Op::Yank, Target::Chars { a, b }) => {
                let text = self.slice(self.pos_bytes(a), self.pos_bytes(b));
                self.set_register(text, false);
                self.set_pos(yank_to.0, yank_to.1);
                EdEvent::Moved
            }
            (Op::Yank, Target::Lines { first, last }) => {
                self.set_register(self.lines[first..=last].join("\n"), true);
                self.set_pos(yank_to.0, yank_to.1);
                EdEvent::Moved
            }
            (Op::Delete, Target::Chars { a, b }) => {
                self.snapshot();
                let text = self.delete_range(a, b);
                self.set_register(text, false);
                self.clamp();
                EdEvent::Changed { typed: None }
            }
            (Op::Delete, Target::Lines { first, last }) => {
                self.snapshot();
                self.set_register(self.lines[first..=last].join("\n"), true);
                let (a, b) = self.lines_span(first, last);
                self.splice(a, b, "");
                let r = first.min(self.lines.len() - 1);
                self.set_pos(r, self.first_nonblank(r));
                EdEvent::Changed { typed: None }
            }
            (Op::Change, Target::Chars { a, b }) => {
                self.snapshot();
                let text = if a < b {
                    self.delete_range(a, b)
                } else {
                    self.set_cursor_offset(a);
                    String::new()
                };
                self.set_register(text, false);
                let at = (self.row, self.col);
                self.start_insert(false, at);
                EdEvent::Changed { typed: None }
            }
            (Op::Change, Target::Lines { first, last }) => {
                self.snapshot();
                self.set_register(self.lines[first..=last].join("\n"), true);
                let indent = indent_of(&self.lines[first]).to_string();
                let a = self.line_start(first);
                let b = self.line_start(last) + self.lines[last].len();
                self.splice(a, b, &indent);
                self.start_insert(false, (first, indent.graphemes(true).count()));
                self.ai_row = (!indent.is_empty()).then_some(first);
                EdEvent::Changed { typed: None }
            }
        }
    }

    /// `o` / `O`: a new line below or above, with the current line's indent, in Insert mode.
    fn open_line(&mut self, below: bool) -> EdEvent {
        self.snapshot();
        let indent = indent_of(&self.lines[self.row]).to_string();
        let (start, end) = self.line_bounds(self.row);
        let at = if below { self.row + 1 } else { self.row };
        if below {
            self.splice(end, end, &format!("\n{indent}"));
        } else {
            self.splice(start, start, &format!("{indent}\n"));
        }
        self.start_insert(false, (at, indent.graphemes(true).count()));
        self.ai_row = (!indent.is_empty()).then_some(at);
        EdEvent::Changed { typed: None }
    }

    /// `p` / `P`: the register `n` times after or before the cursor (lines below or above
    /// its line).
    fn put(&mut self, after: bool, n: usize) -> EdEvent {
        let Some(reg) = self.register.clone() else { return EdEvent::None };
        if reg.text.len().saturating_mul(n) > UNDO_BYTES {
            return EdEvent::None;
        }
        self.snapshot();
        if reg.linewise {
            let text = vec![reg.text.as_str(); n].join("\n");
            let at = if after { self.row + 1 } else { self.row };
            let (start, end) = self.line_bounds(self.row);
            if after {
                self.splice(end, end, &format!("\n{text}"));
            } else {
                self.splice(start, start, &format!("{text}\n"));
            }
            self.set_pos(at, self.first_nonblank(at));
        } else {
            let text = reg.text.repeat(n);
            let mut off = self.offset();
            if after && self.gcount(self.row) > 0 {
                off = self.offset_of(self.row, self.col + 1);
            }
            self.splice(off, off, &text);
            let last_len = text.graphemes(true).next_back().map(str::len).unwrap_or(0);
            self.set_cursor_offset(off + text.len() - last_len);
            self.want_x = None;
            self.clamp();
        }
        EdEvent::Changed { typed: None }
    }

    /// `u` / `Ctrl+R`, `n` times.
    fn undo_redo(&mut self, undo: bool, n: usize) -> EdEvent {
        let mut any = false;
        for _ in 0..n {
            if !self.restore(undo) {
                break;
            }
            any = true;
        }
        if any { EdEvent::Changed { typed: None } } else { EdEvent::None }
    }
}

#[cfg(test)]
mod tests;
