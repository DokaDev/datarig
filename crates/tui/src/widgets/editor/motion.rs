//! Motions: where one leads with a count, and the text it covers for an operator. The word
//! motions follow Vim's rules, which walk every grapheme and the end of each line.

use super::buffer::{class, graphemes};
use super::{Editor, Mode};
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
    /// `W`, `B`, `E`: words are runs of non-blanks.
    BigWordForward,
    BigWordBack,
    BigWordEnd,
    /// `ge`, `gE`: the end of the word before.
    WordEndBack,
    BigWordEndBack,
    LineStart,
    FirstNonBlank,
    LineEnd,
    /// `gg`: the first line, or line `count`.
    Top,
    /// `G`: the last line, or line `count`.
    Bottom,
    /// `f` `F` `t` `T` (`till`: stop next to the character); `again`: repeated by `;` or `,`,
    /// when `t` does not stay on a character right next to the cursor.
    Find {
        ch: char,
        forward: bool,
        till: bool,
        again: bool,
    },
    /// `%`: the bracket matching the one under or after the cursor; with a count, that
    /// percentage of the text.
    Match,
    /// `}` and `{`: the next or previous empty line.
    ParaForward,
    ParaBack,
    /// `H`, `M`, `L`: the top, middle or bottom line on screen.
    ScreenTop,
    ScreenMiddle,
    ScreenBottom,
    /// `/`, `?`, `n`, `N`, `*`, `#`: the next match of the last search forward or back, from
    /// the cursor or from column `from` of its line (the start of the word of `*`).
    Search {
        forward: bool,
        from: Option<usize>,
    },
    /// `'a` (`line`: to the first non-blank of its line, taking whole lines) and `` `a ``: a
    /// mark, found where it is when the key is typed.
    Mark {
        to: Pos,
        line: bool,
    },
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
            'W' => Motion::BigWordForward,
            'B' => Motion::BigWordBack,
            'E' => Motion::BigWordEnd,
            '0' => Motion::LineStart,
            '^' => Motion::FirstNonBlank,
            '$' => Motion::LineEnd,
            'G' => Motion::Bottom,
            '%' => Motion::Match,
            '}' => Motion::ParaForward,
            '{' => Motion::ParaBack,
            'H' => Motion::ScreenTop,
            'M' => Motion::ScreenMiddle,
            'L' => Motion::ScreenBottom,
            _ => return None,
        })
    }

    /// The motions after `g` (`gg`, `ge`, `gE`).
    pub(super) fn of_g(c: char) -> Option<Motion> {
        Some(match c {
            'g' => Motion::Top,
            'e' => Motion::WordEndBack,
            'E' => Motion::BigWordEndBack,
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

    /// A jump: the cursor's place before it becomes the context mark (`''`).
    pub(super) fn is_jump(self) -> bool {
        matches!(
            self,
            Motion::Top
                | Motion::Bottom
                | Motion::Match
                | Motion::ParaForward
                | Motion::ParaBack
                | Motion::ScreenTop
                | Motion::ScreenMiddle
                | Motion::ScreenBottom
                | Motion::Search { .. }
                | Motion::Mark { .. }
        )
    }

    /// How an operator takes the text (`%` with a count takes lines).
    fn kind(self, explicit: bool) -> RangeKind {
        match self {
            Motion::Mark { line: true, .. } => RangeKind::Linewise,
            Motion::Mark { line: false, .. } => RangeKind::Exclusive,
            Motion::Left
            | Motion::Right
            | Motion::WordForward
            | Motion::WordBack
            | Motion::BigWordForward
            | Motion::BigWordBack
            | Motion::LineStart
            | Motion::FirstNonBlank
            | Motion::ParaForward
            | Motion::ParaBack
            | Motion::Search { .. }
            | Motion::Find { forward: false, .. } => RangeKind::Exclusive,
            Motion::Match if explicit => RangeKind::Linewise,
            Motion::WordEnd
            | Motion::BigWordEnd
            | Motion::WordEndBack
            | Motion::BigWordEndBack
            | Motion::LineEnd
            | Motion::Match
            | Motion::Find { forward: true, .. } => RangeKind::Inclusive,
            Motion::Up
            | Motion::Down
            | Motion::Top
            | Motion::Bottom
            | Motion::ScreenTop
            | Motion::ScreenMiddle
            | Motion::ScreenBottom => RangeKind::Linewise,
        }
    }
}

/// A walk over the positions Vim's word motions see: every grapheme and the end of each line
/// (an empty line is only its end). `big`: words are runs of non-blanks (`W`, `B`, `E`).
pub(super) struct Walk<'a> {
    lines: &'a [String],
    pub r: usize,
    pub c: usize,
    gs: Vec<&'a str>,
    big: bool,
}

impl<'a> Walk<'a> {
    pub(super) fn new(lines: &'a [String], (r, c): Pos, big: bool) -> Self {
        let gs = graphemes(&lines[r]);
        Walk { lines, r, c: c.min(gs.len()), gs, big }
    }

    pub(super) fn pos(&self) -> Pos {
        (self.r, self.c)
    }

    /// Word class at the position; the end of a line is blank.
    pub(super) fn cls(&self) -> u8 {
        self.gs.get(self.c).map_or(0, |g| if self.big { class(g).min(1) } else { class(g) })
    }

    pub(super) fn on_empty_line(&self) -> bool {
        self.gs.is_empty()
    }

    pub(super) fn line_len(&self) -> usize {
        self.gs.len()
    }

    fn load(&mut self, r: usize) {
        self.r = r;
        self.gs = graphemes(&self.lines[r]);
    }

    /// One position on: 0 within the line, 2 onto its end, 1 onto the next line, -1 at the end
    /// of the text (no move).
    pub(super) fn inc(&mut self) -> i8 {
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
    pub(super) fn dec(&mut self) -> i8 {
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

    /// Past the graphemes of class `cls`; false at the end (or start) of the text.
    fn skip(&mut self, cls: u8, forward: bool) -> bool {
        while self.cls() == cls {
            if (if forward { self.inc() } else { self.dec() }) == -1 {
                return false;
            }
        }
        true
    }

    /// `w`: to the start of the `count`th next word; stops on an empty line. `eol` (an
    /// operator): the last word ends at the end of its line rather than at the next line's
    /// first word. False when it started on the last grapheme of the text.
    pub(super) fn fwd_word(&mut self, count: usize, eol: bool) -> bool {
        for left in (0..count).rev() {
            let start_class = self.cls();
            let last_line = self.r + 1 == self.lines.len();
            // Always at least one position on, unless at the end of the text.
            let i = self.inc();
            if i == -1 || (i >= 1 && last_line) {
                return false;
            }
            if i >= 1 && eol && left == 0 {
                return true;
            }
            if start_class != 0 {
                while self.cls() == start_class {
                    let i = self.inc();
                    if i == -1 || (i >= 1 && eol && left == 0) {
                        return true;
                    }
                }
            }
            while self.cls() == 0 {
                if self.c == 0 && self.on_empty_line() {
                    break;
                }
                let i = self.inc();
                if i == -1 || (i >= 1 && eol && left == 0) {
                    return true;
                }
            }
        }
        true
    }

    /// `b`: to the start of the `count`th word before; stops on an empty line. `stop`: from
    /// the start of a word, stay on it. False when it started at the start of the text.
    pub(super) fn bck_word(&mut self, count: usize, mut stop: bool) -> bool {
        for _ in 0..count {
            let start_class = self.cls();
            if self.dec() == -1 {
                return false;
            }
            if !stop || start_class == self.cls() || start_class == 0 {
                let mut empty = false;
                while self.cls() == 0 {
                    if self.c == 0 && self.on_empty_line() {
                        empty = true;
                        break;
                    }
                    if self.dec() == -1 {
                        return true;
                    }
                }
                if !empty && !self.skip(self.cls(), false) {
                    return true;
                }
                if empty {
                    stop = false;
                    continue;
                }
            }
            self.inc();
            stop = false;
        }
        true
    }

    /// `e`: to the end of the `count`th word. `stop` (`cw` on a word): from the end of a word,
    /// stay on it instead of going to the end of the next one. `empty`: stop on an empty
    /// line. False when it ran into the end of the text.
    pub(super) fn end_word(&mut self, count: usize, mut stop: bool, empty: bool) -> bool {
        for _ in 0..count {
            let start_class = self.cls();
            if self.inc() == -1 {
                return false;
            }
            if self.cls() == start_class && start_class != 0 {
                if !self.skip(start_class, true) {
                    return false;
                }
            } else if !stop || start_class == 0 {
                let mut on_empty = false;
                while self.cls() == 0 {
                    if self.c == 0 && self.on_empty_line() && empty {
                        on_empty = true;
                        break;
                    }
                    if self.inc() == -1 {
                        return false;
                    }
                }
                if on_empty {
                    stop = false;
                    continue;
                }
                if !self.skip(self.cls(), true) {
                    return false;
                }
            }
            self.dec();
            stop = false;
        }
        true
    }

    /// `ge`: to the end of the `count`th word before. `eol`: stop at the end of the line
    /// before. False when it started at the start of the text.
    pub(super) fn bckend_word(&mut self, count: usize, eol: bool) -> bool {
        for _ in 0..count {
            let start_class = self.cls();
            let i = self.dec();
            if i == -1 {
                return false;
            }
            if eol && i == 1 {
                return true;
            }
            if start_class != 0 {
                while self.cls() == start_class {
                    let i = self.dec();
                    if i == -1 || (eol && i == 1) {
                        return true;
                    }
                }
            }
            while self.cls() == 0 {
                if self.c == 0 && self.on_empty_line() {
                    break;
                }
                let i = self.dec();
                if i == -1 || (eol && i == 1) {
                    return true;
                }
            }
        }
        true
    }
}

impl Editor {
    /// Where `m` repeated `count` times leads from the cursor; `None` when it cannot move at
    /// all (`h` in the first column, `j` on the last line). `explicit`: a count was typed
    /// (`G` and `gg` go to line `count`). `op`: for an operator, the end of a line is a place
    /// too (`l` reaches past the last grapheme, a word motion stops at the end of the line of
    /// its last word).
    pub(super) fn target(&mut self, m: Motion, count: usize, explicit: bool, op: bool) -> Option<Pos> {
        self.dest(m, count, explicit, op).map(|(p, _)| p)
    }

    /// [`Editor::target`] and whether an operator takes the grapheme there (`}` at the end of
    /// the text does).
    fn dest(&mut self, m: Motion, count: usize, explicit: bool, op: bool) -> Option<(Pos, bool)> {
        let (r, c) = (self.row, self.col);
        let last = self.lines.len() - 1;
        let walk = |big: bool| Walk::new(&self.lines, (r, c), big);
        let to = match m {
            Motion::Left if c == 0 => return None,
            Motion::Left => (r, c.saturating_sub(count)),
            Motion::Right => {
                let n = self.gcount(r);
                let max = if op || self.visual_block() { n } else { n.saturating_sub(1) };
                if c >= max {
                    return None;
                }
                (r, c.saturating_add(count).min(max))
            }
            Motion::Down if r == last => return None,
            Motion::Down => (r.saturating_add(count).min(last), c),
            Motion::Up if r == 0 => return None,
            Motion::Up => (r.saturating_sub(count), c),
            Motion::WordForward | Motion::BigWordForward => {
                let mut w = walk(m == Motion::BigWordForward);
                w.fwd_word(count, op);
                w.pos()
            }
            Motion::WordBack | Motion::BigWordBack => {
                let mut w = walk(m == Motion::BigWordBack);
                w.bck_word(count, false);
                w.pos()
            }
            Motion::WordEnd | Motion::BigWordEnd => {
                let mut w = walk(m == Motion::BigWordEnd);
                w.end_word(count, false, false);
                w.pos()
            }
            Motion::WordEndBack | Motion::BigWordEndBack => {
                let mut w = walk(m == Motion::BigWordEndBack);
                if !w.bckend_word(count, false) {
                    return None;
                }
                w.pos()
            }
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
            Motion::Find { ch, forward, till, again } => self.find_char(ch, forward, till, again, count)?,
            Motion::Match if explicit => {
                if count > 100 {
                    return None;
                }
                let r = ((count * self.lines.len()).div_ceil(100)).saturating_sub(1).min(last);
                (r, self.first_nonblank(r))
            }
            Motion::Match => self.match_pair()?,
            Motion::ParaForward | Motion::ParaBack => return self.paragraph(m == Motion::ParaForward, count),
            Motion::ScreenTop | Motion::ScreenMiddle | Motion::ScreenBottom => {
                let r = self.screen_line(m, count);
                (r, self.first_nonblank(r))
            }
            Motion::Mark { to, .. } => to,
            Motion::Search { forward, from } => {
                let (r, c) = self.search_target(forward, from, count)?;
                // A match at the end of a line: Visual mode takes the line break there, anything
                // else stops on the last grapheme (Vim keeps the cursor on the text).
                let c = if self.mode == Mode::Visual { c } else { c.min(self.gcount(r).saturating_sub(1)) };
                (r, c)
            }
        };
        Some((to, false))
    }

    /// `f` `F` `t` `T` on the cursor's line: the `count`th `ch` after or before the cursor
    /// (next to it with `till`). A repeated `t` (`again`, count 1) does not stay where it is
    /// when the character is right next to the cursor.
    fn find_char(&self, ch: char, forward: bool, till: bool, again: bool, count: usize) -> Option<Pos> {
        let gs = graphemes(&self.lines[self.row]);
        let mut col = self.col as isize;
        let mut stop = !(again && till && count == 1);
        let dir: isize = if forward { 1 } else { -1 };
        for _ in 0..count {
            loop {
                col += dir;
                if col < 0 || col as usize >= gs.len() {
                    return None;
                }
                if stop && gs[col as usize].starts_with(ch) {
                    break;
                }
                stop = true;
            }
        }
        if till {
            col -= dir;
        }
        Some((self.row, col as usize))
    }

    /// `}` / `{` `count` times: the next or previous empty line, or the end (start) of the
    /// text. Going forward to the end of the text lands on its last grapheme, which an
    /// operator then takes (Vim). `None` when there is not a paragraph for every count.
    fn paragraph(&self, forward: bool, count: usize) -> Option<(Pos, bool)> {
        let n = self.lines.len();
        let mut curr = self.row;
        for left in (0..count).rev() {
            let mut did_skip = false;
            let mut first = true;
            loop {
                if !self.lines[curr].is_empty() {
                    did_skip = true;
                }
                if !first && did_skip && self.lines[curr].is_empty() {
                    break;
                }
                let next = if forward { curr + 1 } else { curr.wrapping_sub(1) };
                if next >= n {
                    if left > 0 {
                        return None;
                    }
                    break;
                }
                curr = next;
                first = false;
            }
        }
        let len = self.gcount(curr);
        if forward && curr == n - 1 && len > 0 {
            return Some(((curr, len - 1), true));
        }
        Some(((curr, 0), false))
    }

    /// The line `H`, `M` or `L` goes to: `count` lines from the top or the bottom of the lines
    /// on screen, or the middle one.
    fn screen_line(&self, m: Motion, count: usize) -> usize {
        let top = self.top.min(self.lines.len() - 1);
        let shown = self.view_h.max(1).min(self.lines.len() - top);
        let bottom = top + shown - 1;
        match m {
            Motion::ScreenTop => (top + count - 1).min(bottom),
            Motion::ScreenBottom => bottom.saturating_sub(count - 1).max(top),
            _ => top + shown.div_ceil(2) - 1,
        }
    }

    /// Move the cursor `count` times by `m` (it stays when `m` cannot move).
    pub(super) fn move_by(&mut self, m: Motion, count: usize, explicit: bool) {
        let Some((r, c)) = self.target(m, count, explicit, false) else { return };
        match m {
            Motion::Up | Motion::Down => self.move_vert(r as isize - self.row as isize),
            Motion::LineEnd => {
                // A Visual block takes the line's end too.
                let c = if self.visual_block() { self.gcount(r) } else { c };
                self.set_pos(r, c);
                self.want_x = Some(usize::MAX);
            }
            Motion::Search { .. } if c > 0 && c >= self.gcount(r) => {
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

    /// The text an operator with motion `m` takes; `None` when the motion cannot move.
    pub(super) fn op_range(&mut self, m: Motion, count: usize, explicit: bool) -> Option<Range> {
        let (to, inclusive) = self.dest(m, count, explicit, true)?;
        let from = (self.row, self.col);
        let (start, end) = if to < from { (to, from) } else { (from, to) };
        let kind = if inclusive { RangeKind::Inclusive } else { m.kind(explicit) };
        Some(self.exclusive_adjusted(Range { start, end, kind }))
    }

    /// As in Vim, an exclusive range that ends at the start of a later line ends at the end
    /// of the line before instead, and takes whole lines when it started in the indent (so
    /// `dw` on an empty line deletes it).
    pub(super) fn exclusive_adjusted(&self, r: Range) -> Range {
        let Range { start, end, kind } = r;
        if kind != RangeKind::Exclusive || end.1 != 0 || end.0 == start.0 {
            return r;
        }
        let before = end.0 - 1;
        if graphemes(&self.lines[start.0]).iter().take(start.1).all(|g| class(g) == 0) {
            return Range { start, end: (before, 0), kind: RangeKind::Linewise };
        }
        match self.gcount(before) {
            0 => Range { start, end: (before, 0), kind: RangeKind::Exclusive },
            n => Range { start, end: (before, n - 1), kind: RangeKind::Inclusive },
        }
    }

    /// `cw` (`cW`) with the cursor on a word: to the end of the `count`th word, like `ce`,
    /// without the blanks after it (Vim). `None` elsewhere (then it is a plain `w`).
    pub(super) fn cw_range(&self, count: usize, big: bool) -> Option<Range> {
        let on_word = graphemes(&self.lines[self.row]).get(self.col).is_some_and(|g| class(g) != 0);
        on_word.then(|| {
            let mut w = Walk::new(&self.lines, (self.row, self.col), big);
            w.end_word(count, true, false);
            Range { start: (self.row, self.col), end: w.pos(), kind: RangeKind::Inclusive }
        })
    }
}

#[cfg(test)]
mod tests;
