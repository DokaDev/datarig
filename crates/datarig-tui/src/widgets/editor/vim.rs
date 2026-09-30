//! Normal mode: the command parser (`[count] operator [count] motion|text object`, doubled
//! operators `dd cc yy >> << guu gUU g~~`, prefixes `g` and `z`, the character argument of
//! `f t F T r`, the single-key commands) and the operators on a range of text.

use super::buffer::{UNDO_BYTES, class, indent_of};
use super::edit::Case;
use super::motion::{Motion, Range, RangeKind};
use super::repeat::InsertRepeat;
use super::textobj::Object;
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
    /// `gu`, `gU`, `g~`.
    Case(Case),
    /// `>` (`right`) and `<`.
    Shift {
        right: bool,
    },
}

impl Op {
    fn of(c: char) -> Option<Op> {
        match c {
            'd' => Some(Op::Delete),
            'c' => Some(Op::Change),
            'y' => Some(Op::Yank),
            '>' => Some(Op::Shift { right: true }),
            '<' => Some(Op::Shift { right: false }),
            _ => None,
        }
    }

    /// The operators after `g`.
    fn of_g(c: char) -> Option<Op> {
        match c {
            'u' => Some(Op::Case(Case::Lower)),
            'U' => Some(Op::Case(Case::Upper)),
            '~' => Some(Op::Case(Case::Toggle)),
            _ => None,
        }
    }

    /// The key that doubles the operator for whole lines (`dd`, `>>`, `guu` or `gugu`).
    fn last_key(self) -> char {
        match self {
            Op::Delete => 'd',
            Op::Change => 'c',
            Op::Yank => 'y',
            Op::Shift { right: true } => '>',
            Op::Shift { right: false } => '<',
            Op::Case(Case::Lower) => 'u',
            Op::Case(Case::Upper) => 'U',
            Op::Case(Case::Toggle) => '~',
        }
    }
}

/// A command typed so far: counts (0: none), the operator, a prefix waiting for its second
/// key (`g`, `z`, or `i`/`a` of a text object), a command waiting for a character (`f`, `t`,
/// `F`, `T`, `r`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Pending {
    count: usize,
    op: Option<Op>,
    op_count: usize,
    prefix: Option<char>,
    arg: Option<char>,
}

impl Pending {
    /// The command waits for a key the keymap should hand over (`d…`, `g…`, `f…`).
    pub(super) fn awaiting(&self) -> bool {
        self.op.is_some() || self.prefix.is_some() || self.arg.is_some()
    }

    /// The command waits for a character, taken as it is typed (`f`, `t`, `r`).
    pub(super) fn awaiting_char(&self) -> bool {
        self.arg.is_some()
    }

    /// Nothing typed yet: the next key starts a command.
    pub(super) fn is_empty(&self) -> bool {
        *self == Pending::default()
    }

    /// The count of the command (the counts before and after the operator multiply) and
    /// whether one was typed.
    pub(super) fn count(&self) -> (usize, bool) {
        let n = self.count.max(1).saturating_mul(self.op_count.max(1)).min(MAX_COUNT);
        (n, self.count > 0 || self.op_count > 0)
    }

    /// The last key went to a count.
    pub(super) fn counted(&self, before: &Pending) -> bool {
        (self.count, self.op_count) != (before.count, before.op_count) && self.count + self.op_count > 0
    }

    /// Take digit `d` as part of a count; false when it is not one (`0` is a motion unless a
    /// count is being typed).
    fn digit(&mut self, d: u32) -> bool {
        let n = if self.op.is_some() { &mut self.op_count } else { &mut self.count };
        if d == 0 && *n == 0 {
            return false;
        }
        *n = (*n * 10 + d as usize).min(MAX_COUNT);
        true
    }
}

/// One step of a command, as the parser reads the keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Token {
    /// The command needs more keys.
    More,
    /// Not a command: what was typed is dropped.
    Cancel,
    Motion(Motion),
    /// A text object, `a…` (`true`) or `i…`.
    Object(Object, bool),
    /// A command key, after `g` or `z`, or with `Ctrl`.
    Key(char),
    G(char),
    Z(char),
    Ctrl(char),
    /// `r` and its character.
    Replace(char),
}

/// The text an operator works on: bytes of the text, or whole lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Chars { a: usize, b: usize },
    Lines { first: usize, last: usize },
}

impl Editor {
    /// Read `key` as part of a command. `objects`: `i` and `a` start a text object (after an
    /// operator, and in Visual mode).
    pub(super) fn token(&mut self, key: KeyEvent, objects: bool) -> Token {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = !ctrl && !key.modifiers.contains(KeyModifiers::ALT);
        let ch = match key.code {
            KeyCode::Char(c) => Some(c),
            _ => None,
        };
        if let Some(cmd) = self.cmd.arg.take() {
            let arg = match key.code {
                KeyCode::Char(c) if plain => c,
                KeyCode::Enter => '\n',
                KeyCode::Tab => '\t',
                _ => return Token::Cancel,
            };
            if cmd == 'r' {
                return Token::Replace(arg);
            }
            let (forward, till) = (cmd.is_ascii_lowercase(), cmd == 't' || cmd == 'T');
            self.last_find = Some((arg, forward, till));
            return Token::Motion(Motion::Find { ch: arg, forward, till, again: false });
        }
        if key.code == KeyCode::Esc {
            return Token::Cancel;
        }
        if let Some(p) = self.cmd.prefix.take() {
            let Some(c) = ch.filter(|_| plain) else {
                return if p == 'z' && key.code == KeyCode::Enter && !ctrl { Token::Z('\n') } else { Token::Cancel };
            };
            return match p {
                'g' => Motion::of_g(c).map_or(Token::G(c), Token::Motion),
                'z' => Token::Z(c),
                _ => Object::of_char(c).map_or(Token::Cancel, |o| Token::Object(o, p == 'a')),
            };
        }
        if ctrl {
            return ch.map_or(Token::Cancel, Token::Ctrl);
        }
        if let Some(d) = ch.and_then(|c| c.to_digit(10))
            && self.cmd.digit(d)
        {
            return Token::More;
        }
        let Some(c) = ch else {
            return Motion::of_key(key.code).map_or(Token::Cancel, Token::Motion);
        };
        match c {
            'g' => self.cmd.prefix = Some(c),
            'z' if self.cmd.op.is_none() => self.cmd.prefix = Some(c),
            'i' | 'a' if objects => self.cmd.prefix = Some(c),
            'f' | 'F' | 't' | 'T' => self.cmd.arg = Some(c),
            'r' if self.cmd.op.is_none() => self.cmd.arg = Some(c),
            ';' | ',' => {
                return match self.last_find {
                    Some((ch, forward, till)) => {
                        Token::Motion(Motion::Find { ch, forward: forward == (c == ';'), till, again: true })
                    }
                    None => Token::Cancel,
                };
            }
            c => return Motion::of_char(c).map_or(Token::Key(c), Token::Motion),
        }
        Token::More
    }

    pub(super) fn key_normal(&mut self, key: KeyEvent) -> EdEvent {
        let token = self.token(key, self.cmd.op.is_some());
        let (n, explicit) = self.cmd.count();
        let op = self.cmd.op;
        match token {
            Token::More => return EdEvent::None,
            Token::Motion(m) => return self.motion_key(m),
            // A second `g` prefix: `gugu`.
            Token::G(c) if op.is_some() && Op::of_g(c) == op => {}
            Token::G(c) if op.is_none() && Op::of_g(c).is_some() => {
                self.cmd.op = Op::of_g(c);
                return EdEvent::None;
            }
            Token::Key(c) if op.is_none() && Op::of(c).is_some() => {
                self.cmd.op = Op::of(c);
                return EdEvent::None;
            }
            _ => {}
        }
        self.cmd = Pending::default();
        match (token, op) {
            (Token::Object(o, around), Some(op)) => self.apply_object(op, o, around, n),
            (Token::Key(c) | Token::G(c), Some(op)) if c == op.last_key() => self.apply_lines(op, n),
            (_, Some(_)) => EdEvent::None,
            (Token::Ctrl('r'), None) => self.undo_redo(false, n),
            (Token::Ctrl(c @ ('d' | 'u')), None) => self.scroll_half(c == 'd', n, explicit),
            (Token::Ctrl(c @ ('f' | 'b')), None) => self.scroll_page(c == 'f', n),
            (Token::Z(c), None) => self.scroll_cursor(c, n, explicit),
            (Token::Replace(c), None) => self.replace_chars(c, n),
            (Token::G('J'), None) => self.join_lines(self.row, n, false),
            (Token::Key(c), None) => self.command(c, n, explicit),
            _ => EdEvent::None,
        }
    }

    /// The single-key commands of Normal mode.
    fn command(&mut self, c: char, n: usize, explicit: bool) -> EdEvent {
        if matches!(c, 'i' | 'a' | 'I' | 'A' | 'o' | 'O') && n > 1 {
            self.ins_repeat = Some(InsertRepeat::new(n, matches!(c, 'o' | 'O')));
        }
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
            'I' => {
                let indent = indent_of(&self.lines[self.row]).graphemes(true).count();
                self.start_insert(true, (self.row, indent))
            }
            'A' => self.start_insert(true, (self.row, self.gcount(self.row))),
            'o' | 'O' => self.open_line(c == 'o'),
            'p' | 'P' => self.put(c == 'p', n),
            'u' => self.undo_redo(true, n),
            'v' => self.enter_visual(false, (self.row, self.col)),
            'V' => self.enter_visual(true, (self.row, self.col)),
            'J' => self.join_lines(self.row, n, true),
            '~' => self.tilde(n),
            '.' => self.dot(explicit.then_some(n)),
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
        let cw = match (op, m) {
            (Op::Change, Motion::WordForward | Motion::BigWordForward) => self.cw_range(n, m == Motion::BigWordForward),
            _ => None,
        };
        let Some(r) = cw.or_else(|| self.op_range(m, n, explicit)) else {
            if op == Op::Change && matches!(m, Motion::Left | Motion::Right) {
                return self.start_insert(true, (self.row, self.col));
            }
            return EdEvent::None;
        };
        self.apply_range(op, r)
    }

    /// Operator `op` over `count` text objects `o`.
    fn apply_object(&mut self, op: Op, o: Object, around: bool, count: usize) -> EdEvent {
        match self.object_range(o, count, around) {
            Some(r) => self.apply_range(op, r),
            None => EdEvent::None,
        }
    }

    /// Operator `op` over range `r`. An empty range (`di(` in `()`) takes nothing, not even
    /// the register: the cursor goes there, and a change starts Insert mode there.
    fn apply_range(&mut self, op: Op, mut r: Range) -> EdEvent {
        if let Op::Shift { right } = op {
            return self.shift_lines(r.start.0, r.end.0, 1, right);
        }
        if r.kind == RangeKind::Exclusive && r.start == r.end {
            if op == Op::Change {
                self.snapshot();
                return self.start_insert(false, r.start);
            }
            self.set_pos(r.start.0, r.start.1);
            return EdEvent::Moved;
        }
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

    /// `dd`, `cc`, `yy`, `>>`, `guu`, `S`, `Y`: `n` lines from the cursor's (as many as there
    /// are; nothing when more than one is asked for on the last line, as Vim). A case
    /// operator leaves the cursor where the range starts, as a line motion puts it.
    fn apply_lines(&mut self, op: Op, n: usize) -> EdEvent {
        let end = self.lines.len() - 1;
        if n > 1 && self.row == end {
            return EdEvent::None;
        }
        let last = (self.row + n - 1).min(end);
        let to = match op {
            Op::Case(_) => (self.row, self.col).min((last, self.first_nonblank(last))),
            _ => (self.row, self.col),
        };
        self.apply(op, Target::Lines { first: self.row, last }, to)
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
    /// with `Lines`). A yank or a case change puts the cursor at `to`; a delete at the start
    /// (on the first non-blank of the line for lines); a change starts Insert mode there
    /// (lines keep the first one's indent); a shift on the first line's first non-blank.
    /// Nothing happens to an empty span, except that a change still starts Insert mode.
    pub(super) fn apply(&mut self, op: Op, t: Target, to: (usize, usize)) -> EdEvent {
        if self.mode == Mode::Visual {
            self.mode = Mode::Normal;
        }
        match (op, t) {
            (Op::Shift { right }, Target::Lines { first, last }) => self.shift_lines(first, last, 1, right),
            (Op::Shift { right }, Target::Chars { a, b }) => {
                let (first, last) = (self.pos_bytes(a).0, self.pos_bytes(b).0);
                self.shift_lines(first, last, 1, right)
            }
            (Op::Case(case), Target::Chars { a, b }) => self.recase_span(case, (a, b), to),
            (Op::Case(case), Target::Lines { first, last }) => {
                let span = (self.line_start(first), self.line_start(last) + self.lines[last].len());
                self.recase_span(case, span, to)
            }
            (Op::Delete | Op::Yank, Target::Chars { a, b }) if a == b => {
                self.clamp();
                EdEvent::Moved
            }
            (Op::Yank, Target::Chars { a, b }) => {
                let text = self.slice(self.pos_bytes(a), self.pos_bytes(b));
                self.set_register(text, false);
                self.set_pos(to.0, to.1);
                EdEvent::Moved
            }
            (Op::Yank, Target::Lines { first, last }) => {
                self.set_register(self.lines[first..=last].join("\n"), true);
                self.set_pos(to.0, to.1);
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
        let at = self.open_line_at(below);
        self.start_insert(false, at);
        self.ai_row = (at.1 > 0).then_some(at.0);
        EdEvent::Changed { typed: None }
    }

    /// Insert a line below or above the cursor's with its indent; where the cursor goes.
    pub(super) fn open_line_at(&mut self, below: bool) -> (usize, usize) {
        let indent = indent_of(&self.lines[self.row]).to_string();
        let (start, end) = self.line_bounds(self.row);
        let at = if below { self.row + 1 } else { self.row };
        if below {
            self.splice(end, end, &format!("\n{indent}"));
        } else {
            self.splice(start, start, &format!("{indent}\n"));
        }
        (at, indent.graphemes(true).count())
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
        self.rec.skip();
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
