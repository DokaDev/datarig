//! Normal mode: the command parser (`["x] [count] operator [count] motion|text object`,
//! doubled operators `dd cc yy >> << guu gUU g~~`, prefixes `g` and `z`, the character
//! argument of `f t F T r`, the single-key commands) and the operators on a range of text.

use super::buffer::{UNDO_BYTES, class, gw, indent_of};
use super::edit::Case;
use super::marks::Hint;
use super::motion::{Motion, Range, RangeKind};
use super::registers::{self, RegKind, RegProblem, Register, Registers};
use super::repeat::InsertRepeat;
use super::textobj::Object;
use super::{EdEvent, Editor, Mode, Sel};
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
    /// `gc`: comment the lines out or back in.
    Comment,
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
            'c' => Some(Op::Comment),
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
            Op::Comment => 'c',
        }
    }
}

/// A command typed so far: counts (0: none), the register (`"x`), the operator, a prefix
/// waiting for its second key (`g`, `z`, or `i`/`a` of a text object), a command waiting for a
/// character (`f`, `t`, `F`, `T`, `r`, `m`, `'`, `` ` ``).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Pending {
    count: usize,
    /// The count typed before the register (`2"a3yy` takes 6 lines).
    reg_count: usize,
    reg: Option<char>,
    /// `"` was typed: the register's name comes next.
    reg_wait: bool,
    op: Option<Op>,
    op_count: usize,
    prefix: Option<char>,
    arg: Option<char>,
}

impl Pending {
    /// The command waits for a key the keymap should hand over (`d…`, `g…`, `f…`, `"a…`).
    pub(super) fn awaiting(&self) -> bool {
        self.op.is_some() || self.prefix.is_some() || self.arg.is_some() || self.reg_wait || self.reg.is_some()
    }

    /// The next key starts the command itself (no operator, prefix or argument waits) and
    /// `reg` holds for the register typed so far.
    pub(super) fn ready(&self, reg: impl Fn(Option<char>) -> bool) -> bool {
        self.op.is_none() && self.prefix.is_none() && self.arg.is_none() && !self.reg_wait && reg(self.reg)
    }

    /// The command waits for a character, taken as it is typed (`f`, `t`, `r`); a mark's
    /// name (`m`, `'`, `` ` ``) is a key, as a register's is.
    pub(super) fn awaiting_char(&self) -> bool {
        matches!(self.arg, Some('f' | 'F' | 't' | 'T' | 'r'))
    }

    /// Nothing typed yet: the next key starts a command.
    pub(super) fn is_empty(&self) -> bool {
        *self == Pending::default()
    }

    /// The count of the command (the counts before and after the operator multiply) and
    /// whether one was typed.
    pub(super) fn count(&self) -> (usize, bool) {
        let n = [self.reg_count, self.count, self.op_count].iter().fold(1usize, |n, c| n.saturating_mul((*c).max(1)));
        (n.min(MAX_COUNT), self.reg_count + self.count + self.op_count > 0)
    }

    /// The last key went to a count.
    pub(super) fn counted(&self, before: &Pending) -> bool {
        (self.count, self.op_count) != (before.count, before.op_count) && self.count + self.op_count > 0
    }

    /// The register `"` names: the count typed so far stays apart from the one that may follow.
    fn set_reg(&mut self, c: char) {
        self.reg = Some(c);
        if self.count > 0 {
            self.reg_count = self.reg_count.max(1).saturating_mul(self.count).min(MAX_COUNT);
            self.count = 0;
        }
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
    /// `m` and the mark's name.
    Mark(char),
}

/// Whether Normal-mode command `token` (after operator `op`, if one is pending) changes the
/// text: an operator other than a yank, a put, an undo or redo, a repeat, Insert mode, or one of
/// the commands that edit (`x`, `r`, `J`, `~`, `&`, …). Yanks, motions, Visual mode, marks,
/// search and scrolling do not.
fn changes(token: Token, op: Option<Op>) -> bool {
    match (token, op) {
        (_, Some(op)) => op != Op::Yank,
        (Token::Key(c), None) => Op::of(c).is_some_and(|o| o != Op::Yank) || "xXsDCSiaIAoOpPuJ~.&".contains(c),
        (Token::G(c), None) => Op::of_g(c).is_some() || matches!(c, 'J' | '&'),
        (Token::Replace(_) | Token::Ctrl('r'), None) => true,
        _ => false,
    }
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
        if self.cmd.reg_wait {
            self.cmd.reg_wait = false;
            return match ch {
                Some(c) if plain && registers::is_name(c) => {
                    self.cmd.set_reg(c);
                    Token::More
                }
                _ => Token::Cancel,
            };
        }
        if let Some(cmd) = self.cmd.arg.take() {
            let arg = match key.code {
                KeyCode::Char(c) if plain => c,
                KeyCode::Enter => '\n',
                KeyCode::Tab => '\t',
                _ => return Token::Cancel,
            };
            match cmd {
                'r' => return Token::Replace(arg),
                'm' => return Token::Mark(arg),
                '\'' | '`' => {
                    let line = cmd == '\'';
                    return self
                        .mark_target(arg, line)
                        .map_or(Token::Cancel, |to| Token::Motion(Motion::Mark { to, line }));
                }
                _ => {}
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
                'g' if matches!(c, '*' | '#') => self.search_word(c == '*', false).map_or(Token::Cancel, Token::Motion),
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
            'r' | 'm' if self.cmd.op.is_none() => self.cmd.arg = Some(c),
            '\'' | '`' => self.cmd.arg = Some(c),
            '"' if self.cmd.op.is_none() => self.cmd.reg_wait = true,
            '/' | '?' => self.open_prompt(c == '/'),
            'n' | 'N' => return self.search_again(c == 'n').map_or(Token::Cancel, Token::Motion),
            '*' | '#' => return self.search_word(c == '*', true).map_or(Token::Cancel, Token::Motion),
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

    /// The command typed so far is complete: it runs with the register it names.
    pub(super) fn end_command(&mut self) {
        self.reg = std::mem::take(&mut self.cmd).reg;
    }

    pub(super) fn key_normal(&mut self, key: KeyEvent) -> EdEvent {
        let token = self.token(key, self.cmd.op.is_some());
        let (n, explicit) = self.cmd.count();
        let op = self.cmd.op;
        if self.read_only && changes(token, op) {
            return self.refuse();
        }
        match token {
            Token::More => return EdEvent::None,
            Token::Motion(m) => return self.motion_key(m),
            // A second `g` prefix: `gugu` (not `gcgc`: Neovim's is a text object).
            Token::G(c) if op.is_some() && Op::of_g(c) == op && op != Some(Op::Comment) => {}
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
        self.end_command();
        match (token, op) {
            (Token::Object(o, around), Some(op)) => self.apply_object(op, o, around, n),
            (Token::Key(c), Some(op)) if c == op.last_key() => self.apply_lines(op, n),
            (Token::G(c), Some(op)) if c == op.last_key() && op != Op::Comment => self.apply_lines(op, n),
            (_, Some(_)) => EdEvent::None,
            (Token::Ctrl('r'), None) => self.undo_redo(false, n),
            (Token::Ctrl('v'), None) => self.enter_visual(Sel::Block, (self.row, self.col)),
            (Token::Ctrl(c @ ('d' | 'u')), None) => self.scroll_half(c == 'd', n, explicit),
            (Token::Ctrl(c @ ('f' | 'b')), None) => self.scroll_page(c == 'f', n),
            (Token::Z(c), None) => self.scroll_cursor(c, n, explicit),
            (Token::Replace(c), None) => self.replace_chars(c, n),
            (Token::Mark(c), None) => self.set_mark(c),
            (Token::G('J'), None) => self.join_lines(self.row, n, false),
            (Token::G('&'), None) => self.sub_again(true, 1),
            (Token::Key('&'), None) => self.sub_again(false, n),
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
            'v' => self.enter_visual(Sel::Chars, (self.row, self.col)),
            'V' => self.enter_visual(Sel::Lines, (self.row, self.col)),
            'J' => self.join_lines(self.row, n, true),
            '~' => self.tilde(n),
            '.' => self.dot(explicit.then_some(n)),
            _ => EdEvent::None,
        }
    }

    /// A motion key: moves the cursor, or ends the pending operator.
    pub(super) fn motion_key(&mut self, m: Motion) -> EdEvent {
        let (n, explicit) = self.cmd.count();
        let op = self.cmd.op;
        self.end_command();
        if m.is_jump() {
            self.marks.set_pc((self.row, self.col));
        }
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
        // Vim's rule: a delete over these motions goes to `"1` even within a line.
        self.reg_one = matches!(m, Motion::Match | Motion::ParaForward | Motion::ParaBack | Motion::Search { .. });
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
        match op {
            Op::Shift { right } => return self.shift_lines(r.start.0, r.end.0, 1, right),
            Op::Comment => return self.comment_lines(r.start.0, r.end.0, r.start),
            _ => {}
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
            Op::Case(_) | Op::Comment => (self.row, self.col).min((last, self.first_nonblank(last))),
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
        // A register Vim fills itself (`".`, `"%`…) takes no yank: the command fails.
        if matches!(op, Op::Delete | Op::Change | Op::Yank) && !Registers::can_write(self.reg) {
            self.problem = self.reg.map(RegProblem::ReadOnly);
            self.clamp();
            return EdEvent::Moved;
        }
        match (op, t) {
            (Op::Comment, Target::Lines { first, last }) => self.comment_lines(first, last, to),
            (Op::Comment, Target::Chars { a, b }) => {
                let (first, last) = (self.pos_bytes(a).0, self.pos_bytes(b).0);
                self.comment_lines(first, last, to)
            }
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
                self.yank_to_register(text, RegKind::Charwise);
                self.set_pos(to.0, to.1);
                EdEvent::Moved
            }
            (Op::Yank, Target::Lines { first, last }) => {
                self.yank_to_register(self.lines[first..=last].join("\n"), RegKind::Linewise);
                self.set_pos(to.0, to.1);
                EdEvent::Moved
            }
            (Op::Delete, Target::Chars { a, b }) => {
                self.snapshot();
                let text = self.delete_range(a, b);
                self.delete_to_register(text, RegKind::Charwise);
                self.clamp();
                EdEvent::Changed { typed: None }
            }
            (Op::Delete, Target::Lines { first, last }) => {
                self.snapshot();
                self.delete_to_register(self.lines[first..=last].join("\n"), RegKind::Linewise);
                let (a, b) = self.lines_span(first, last);
                self.mark_hint = Some(Hint::Lines { first, last, keep_first: false });
                self.splice(a, b, "");
                self.marks.set_change((first, 0));
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
                self.delete_to_register(text, RegKind::Charwise);
                let at = (self.row, self.col);
                self.start_insert(false, at);
                EdEvent::Changed { typed: None }
            }
            (Op::Change, Target::Lines { first, last }) => {
                self.snapshot();
                self.delete_to_register(self.lines[first..=last].join("\n"), RegKind::Linewise);
                let indent = indent_of(&self.lines[first]).to_string();
                let a = self.line_start(first);
                let b = self.line_start(last) + self.lines[last].len();
                self.mark_hint = Some(Hint::Lines { first, last, keep_first: true });
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
            self.mark_hint = Some(Hint::Above);
            self.splice(start, start, &format!("{indent}\n"));
        }
        (at, indent.graphemes(true).count())
    }

    /// `p` / `P`: the register the command names (the unnamed one) `n` times after or before
    /// the cursor (lines below or above its line, a block from the next or the cursor's
    /// column down).
    fn put(&mut self, after: bool, n: usize) -> EdEvent {
        let name = self.reg.unwrap_or('"');
        if matches!(name, '+' | '*') {
            self.rec.put_clipboard();
        }
        let Some(reg) = self.put_source(name) else { return EdEvent::None };
        if (reg.kind == RegKind::Charwise && reg.text.is_empty()) || self.put_size(&reg, n) > UNDO_BYTES {
            return EdEvent::None;
        }
        self.snapshot();
        if name == '.' {
            // Vim puts `".` by typing it again: the register then holds what was put.
            self.regs.set_inserted(reg.text.repeat(n));
        }
        if reg.kind == RegKind::Blockwise {
            self.put_block(&reg.text, after, n);
        } else if reg.linewise() {
            let text = vec![reg.text.as_str(); n].join("\n");
            let at = if after { self.row + 1 } else { self.row };
            let (start, end) = self.line_bounds(self.row);
            if after {
                self.splice(end, end, &format!("\n{text}"));
            } else {
                self.mark_hint = Some(Hint::Above);
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

    /// The bytes putting `reg` `n` times may add (a block's pieces with the blanks that fill
    /// them, and the blanks before them, counted at most).
    pub(super) fn put_size(&self, reg: &Register, n: usize) -> usize {
        if reg.kind != RegKind::Blockwise {
            return reg.text.len().saturating_mul(n);
        }
        let pieces = reg.text.split('\n').count();
        let width = reg.text.split('\n').map(graphemes_width).max().unwrap_or(0);
        let x = self.display_x(self.row, self.col) + 1;
        n.saturating_mul(reg.text.len().saturating_add(pieces.saturating_mul(width)))
            .saturating_add(pieces.saturating_mul(x))
    }

    /// Put block `text` (one piece per line) `n` times side by side, from the column after the
    /// cursor (`after`) or the cursor's, on its line and the ones below (added when the text
    /// ends first); lines too short get blanks up to that column, and each piece is filled
    /// with blanks to the block's width when text follows it. The cursor goes to the block's
    /// first character (Vim). One splice over the lines it changes.
    pub(super) fn put_block(&mut self, text: &str, after: bool, n: usize) {
        let pieces: Vec<&str> = text.split('\n').collect();
        let width = pieces.iter().map(|p| graphemes_width(p)).max().unwrap_or(0);
        let (row, col) = (self.row, self.col);
        let col = if after && self.gcount(row) > 0 { col + 1 } else { col };
        let x = self.display_x(row, col);
        let last = (row + pieces.len() - 1).min(self.lines.len() - 1);
        let mut out = String::new();
        for (i, piece) in pieces.iter().enumerate() {
            let line = self.lines.get(row + i).map_or("", String::as_str);
            // The column on this line: past its end, blanks lead up to it; inside a wide
            // character, the block goes before it after blanks up to the column (Vim).
            let (mut at, mut at_x) = (line.len(), 0);
            for (b, g) in line.grapheme_indices(true) {
                if at_x + gw(g) > x {
                    at = b;
                    break;
                }
                at_x += gw(g);
            }
            let tail = at < line.len();
            let fill = " ".repeat(width - graphemes_width(piece));
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&line[..at]);
            out.push_str(&" ".repeat(x.saturating_sub(at_x)));
            for k in 0..n {
                out.push_str(piece);
                if tail || k + 1 < n {
                    out.push_str(&fill);
                }
            }
            out.push_str(&line[at..]);
        }
        self.block_work += out.len();
        let (a, _) = self.line_bounds(row);
        let (_, b) = self.line_bounds(last);
        self.splice(a, b, &out);
        self.set_pos(row, col);
    }

    /// `u` / `Ctrl+R`, `n` times. Where the cursor was becomes the context mark, as for a
    /// jump (Vim).
    fn undo_redo(&mut self, undo: bool, n: usize) -> EdEvent {
        self.rec.skip();
        self.marks.set_pc((self.row, self.col));
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

/// Display width of `s`.
fn graphemes_width(s: &str) -> usize {
    s.graphemes(true).map(gw).sum()
}

#[cfg(test)]
mod tests;
