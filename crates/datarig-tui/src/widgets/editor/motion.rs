//! Motions: where one leads with a count, and the text it covers for an operator. The word
//! motions follow Vim's rules, which walk every grapheme and the end of each line.

use super::Editor;
use super::buffer::{class, graphemes};
use ratatui::crossterm::event::KeyCode;

/// A text position: line and grapheme (the grapheme count of the line is its end).
pub(super) type Pos = (usize, usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordForward,
    WordBack,
    WordEnd,
    LineStart,
    FirstNonBlank,
    LineEnd,
    /// `gg`: the first line, or line `count`.
    Top,
    /// `G`: the last line, or line `count`.
    Bottom,
}

/// How an operator takes the text between the cursor and where a motion leads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RangeKind {
    /// Up to the end, without it (`w`, `b`, `h`, `l`, `0`, `^`).
    Exclusive,
    /// Up to the end and the grapheme there (`e`, `$`).
    Inclusive,
    /// Whole lines (`j`, `k`, `gg`, `G`, doubled operators).
    Linewise,
}

/// The text an operator takes: from `start` to `end` (`start <= end`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Range {
    pub start: Pos,
    pub end: Pos,
    pub kind: RangeKind,
}

impl Motion {
    pub(super) fn of_char(c: char) -> Option<Motion> {
        Some(match c {
            'h' => Motion::Left,
            'l' => Motion::Right,
            'k' => Motion::Up,
            'j' => Motion::Down,
            'w' => Motion::WordForward,
            'b' => Motion::WordBack,
            'e' => Motion::WordEnd,
            '0' => Motion::LineStart,
            '^' => Motion::FirstNonBlank,
            '$' => Motion::LineEnd,
            'G' => Motion::Bottom,
            _ => return None,
        })
    }

    pub(super) fn of_key(code: KeyCode) -> Option<Motion> {
        Some(match code {
            KeyCode::Left => Motion::Left,
            KeyCode::Right => Motion::Right,
            KeyCode::Up => Motion::Up,
            KeyCode::Down => Motion::Down,
            KeyCode::Home => Motion::LineStart,
            KeyCode::End => Motion::LineEnd,
            _ => return None,
        })
    }

    fn kind(self) -> RangeKind {
        match self {
            Motion::Left
            | Motion::Right
            | Motion::WordForward
            | Motion::WordBack
            | Motion::LineStart
            | Motion::FirstNonBlank => RangeKind::Exclusive,
            Motion::WordEnd | Motion::LineEnd => RangeKind::Inclusive,
            Motion::Up | Motion::Down | Motion::Top | Motion::Bottom => RangeKind::Linewise,
        }
    }
}

/// A walk over the positions Vim's word motions see: every grapheme and the end of each line
/// (an empty line is only its end).
struct Walk<'a> {
    lines: &'a [String],
    r: usize,
    c: usize,
    gs: Vec<&'a str>,
}

impl<'a> Walk<'a> {
    fn new(lines: &'a [String], (r, c): Pos) -> Self {
        let gs = graphemes(&lines[r]);
        Walk { lines, r, c: c.min(gs.len()), gs }
    }

    fn pos(&self) -> Pos {
        (self.r, self.c)
    }

    /// Word class at the position; the end of a line is blank.
    fn cls(&self) -> u8 {
        self.gs.get(self.c).map_or(0, |g| class(g))
    }

    fn on_empty_line(&self) -> bool {
        self.gs.is_empty()
    }

    fn load(&mut self, r: usize) {
        self.r = r;
        self.gs = graphemes(&self.lines[r]);
    }

    /// One position on: 0 within the line, 2 onto its end, 1 onto the next line, -1 at the end
    /// of the text (no move).
    fn inc(&mut self) -> i8 {
        if self.c < self.gs.len() {
            self.c += 1;
            return if self.c < self.gs.len() { 0 } else { 2 };
        }
        if self.r + 1 < self.lines.len() {
            self.load(self.r + 1);
            self.c = 0;
            return 1;
        }
        -1
    }

    /// One position back: 0 within the line, 1 onto the end of the line before, -1 at the
    /// start of the text (no move).
    fn dec(&mut self) -> i8 {
        if self.c > 0 {
            self.c -= 1;
            return 0;
        }
        if self.r > 0 {
            self.load(self.r - 1);
            self.c = self.gs.len();
            return 1;
        }
        -1
    }
}

/// `w`: the start of the `count`th next word; stops on an empty line. `eol` (an operator): the
/// last word ends at the end of its line rather than at the next line's first word.
fn fwd_word(lines: &[String], from: Pos, count: usize, eol: bool) -> Pos {
    let mut w = Walk::new(lines, from);
    for left in (0..count).rev() {
        let start_class = w.cls();
        let last_line = w.r + 1 == lines.len();
        // Always at least one position on, unless at the end of the text.
        let i = w.inc();
        if i == -1 || (i >= 1 && last_line) || (i >= 1 && eol && left == 0) {
            return w.pos();
        }
        if start_class != 0 {
            while w.cls() == start_class {
                let i = w.inc();
                if i == -1 || (i >= 1 && eol && left == 0) {
                    return w.pos();
                }
            }
        }
        while w.cls() == 0 {
            if w.c == 0 && w.on_empty_line() {
                break;
            }
            let i = w.inc();
            if i == -1 || (i >= 1 && eol && left == 0) {
                return w.pos();
            }
        }
    }
    w.pos()
}

/// `b`: the start of the `count`th word before; stops on an empty line.
fn bck_word(lines: &[String], from: Pos, count: usize) -> Pos {
    let mut w = Walk::new(lines, from);
    'words: for _ in 0..count {
        if w.dec() == -1 {
            return w.pos();
        }
        while w.cls() == 0 {
            if w.c == 0 && w.on_empty_line() {
                continue 'words;
            }
            if w.dec() == -1 {
                return w.pos();
            }
        }
        let word = w.cls();
        while w.cls() == word {
            if w.dec() == -1 {
                return w.pos();
            }
        }
        w.inc();
    }
    w.pos()
}

/// `e`: the end of the `count`th word. `stop` (`cw` on a word): from the end of a word, stay
/// on it instead of going to the end of the next one.
fn end_word(lines: &[String], from: Pos, count: usize, mut stop: bool) -> Pos {
    let mut w = Walk::new(lines, from);
    for _ in 0..count {
        let start_class = w.cls();
        if w.inc() == -1 {
            return w.pos();
        }
        if w.cls() == start_class && start_class != 0 {
            while w.cls() == start_class {
                if w.inc() == -1 {
                    return w.pos();
                }
            }
        } else if !stop || start_class == 0 {
            while w.cls() == 0 {
                if w.inc() == -1 {
                    return w.pos();
                }
            }
            let word = w.cls();
            while w.cls() == word {
                if w.inc() == -1 {
                    return w.pos();
                }
            }
        }
        w.dec();
        stop = false;
    }
    w.pos()
}

impl Editor {
    /// Where `m` repeated `count` times leads from the cursor; `None` when it cannot move at
    /// all (`h` in the first column, `j` on the last line). `explicit`: a count was typed
    /// (`G` and `gg` go to line `count`). `op`: for an operator, the end of a line is a place
    /// too (`l` reaches past the last grapheme, a word motion stops at the end of the line of
    /// its last word).
    pub(super) fn target(&self, m: Motion, count: usize, explicit: bool, op: bool) -> Option<Pos> {
        let (r, c) = (self.row, self.col);
        let last = self.lines.len() - 1;
        Some(match m {
            Motion::Left if c == 0 => return None,
            Motion::Left => (r, c.saturating_sub(count)),
            Motion::Right => {
                let n = self.gcount(r);
                let max = if op { n } else { n.saturating_sub(1) };
                if c >= max {
                    return None;
                }
                (r, c.saturating_add(count).min(max))
            }
            Motion::Down if r == last => return None,
            Motion::Down => (r.saturating_add(count).min(last), c),
            Motion::Up if r == 0 => return None,
            Motion::Up => (r.saturating_sub(count), c),
            Motion::WordForward => fwd_word(&self.lines, (r, c), count, op),
            Motion::WordBack => bck_word(&self.lines, (r, c), count),
            Motion::WordEnd => end_word(&self.lines, (r, c), count, false),
            Motion::LineStart => (r, 0),
            Motion::FirstNonBlank => (r, self.first_nonblank(r)),
            Motion::LineEnd if count > 1 && r == last => return None,
            Motion::LineEnd => {
                let r = (r + count - 1).min(last);
                (r, self.gcount(r).saturating_sub(1))
            }
            Motion::Top | Motion::Bottom => {
                let r = match (explicit, m) {
                    (true, _) => count.saturating_sub(1).min(last),
                    (false, Motion::Top) => 0,
                    (false, _) => last,
                };
                (r, self.first_nonblank(r))
            }
        })
    }

    /// Move the cursor `count` times by `m` (it stays when `m` cannot move).
    pub(super) fn move_by(&mut self, m: Motion, count: usize, explicit: bool) {
        let Some((r, c)) = self.target(m, count, explicit, false) else { return };
        match m {
            Motion::Up | Motion::Down => self.move_vert(r as isize - self.row as isize),
            Motion::LineEnd => {
                self.set_pos(r, c);
                self.want_x = Some(usize::MAX);
            }
            _ => self.set_pos(r, c),
        }
    }

    /// Up or down `delta` lines, at the column the cursor wants (where a vertical move
    /// started, or the line's end after `$`).
    pub(super) fn move_vert(&mut self, delta: isize) {
        let want = self.want_x.unwrap_or_else(|| self.display_x(self.row, self.col));
        self.want_x = Some(want);
        let max = self.lines.len() as isize - 1;
        self.row = (self.row as isize + delta).clamp(0, max) as usize;
        self.col = if want == usize::MAX { self.gcount(self.row) } else { self.col_for_x(self.row, want) };
        self.clamp();
    }

    /// The text an operator with motion `m` takes; `None` when the motion cannot move. As in
    /// Vim, an exclusive motion that ends at the start of a later line ends at the end of the
    /// line before instead, and takes whole lines when it started in the indent (so `dw` on an
    /// empty line deletes it).
    pub(super) fn op_range(&self, m: Motion, count: usize, explicit: bool) -> Option<Range> {
        let to = self.target(m, count, explicit, true)?;
        let from = (self.row, self.col);
        let (start, end) = if to < from { (to, from) } else { (from, to) };
        let kind = m.kind();
        if kind != RangeKind::Exclusive || end.1 != 0 || end.0 == start.0 {
            return Some(Range { start, end, kind });
        }
        let before = end.0 - 1;
        if graphemes(&self.lines[start.0]).iter().take(start.1).all(|g| class(g) == 0) {
            return Some(Range { start, end: (before, 0), kind: RangeKind::Linewise });
        }
        Some(match self.gcount(before) {
            0 => Range { start, end: (before, 0), kind: RangeKind::Exclusive },
            n => Range { start, end: (before, n - 1), kind: RangeKind::Inclusive },
        })
    }

    /// `cw` with the cursor on a word: to the end of the `count`th word, like `ce`, without the
    /// blanks after it (Vim). `None` elsewhere (then it is a plain `w`).
    pub(super) fn cw_range(&self, count: usize) -> Option<Range> {
        let on_word = graphemes(&self.lines[self.row]).get(self.col).is_some_and(|g| class(g) != 0);
        on_word.then(|| Range {
            start: (self.row, self.col),
            end: end_word(&self.lines, (self.row, self.col), count, true),
            kind: RangeKind::Inclusive,
        })
    }
}

#[cfg(test)]
mod tests;
