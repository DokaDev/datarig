//! Marks, as Vim keeps them: `m{a-z}` and the jumps to them (`'a` to the line's first
//! non-blank, `` `a `` to the place), the previous context mark (`''`, ``` `` ```: where the
//! last jump left from), the last Visual selection (`'<`, `'>`) and the last change (`'.`).
//!
//! Marks stay on their text as lines go in and out above them. A mark whose line is deleted
//! goes with it (the selection's ends go to the line after instead), and a line joined to the
//! one before takes its marks along. An undo puts the marks a change deleted back.

use super::motion::Pos;
use super::{EdEvent, Editor};
use unicode_segmentation::UnicodeSegmentation;

/// Why a jump to a mark did not happen, for the app to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkNotice {
    /// The mark has no position (never set, or its line was deleted).
    NotSet(char),
    /// Not a mark the editor keeps (`A`–`Z`, `0`–`9`, `[`, …).
    Unknown(char),
}

/// A Visual selection as its marks keep it: where it started, where the cursor was, and
/// whether it took whole lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Selection {
    pub start: Pos,
    pub end: Pos,
    pub lines: bool,
}

impl Selection {
    /// `'<` and `'>`: the ends in text order (by line, the first line's start and the last
    /// one's end).
    fn ends(&self) -> (Pos, Pos) {
        let (a, b) = if self.start <= self.end { (self.start, self.end) } else { (self.end, self.start) };
        if self.lines { ((a.0, 0), (b.0, usize::MAX)) } else { (a, b) }
    }
}

/// What the undo of a change gives back: the marks `a`–`z` and the selection as they were.
#[derive(Clone, Debug)]
pub(super) struct Saved {
    named: [Option<Pos>; 26],
    visual: Option<Selection>,
}

/// How the lines of a splice map, when its bytes alone do not say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Hint {
    /// Lines `first..=last` go away whole (`keep_first`: the first stays, emptied, as `cc`
    /// leaves it).
    Lines { first: usize, last: usize, keep_first: bool },
    /// Lines `first..=last` are joined into the first: for each line after it, the byte of
    /// the joined line where its text starts and the bytes it lost before that (its indent).
    Join { first: usize, at: Vec<(usize, usize)> },
    /// A line break typed into a line: its marks stay on the first part, even at its start
    /// (where lines put above it would take them down).
    Split,
}

/// A splice as the marks see it: bytes `ba` of line `ra` to `bb` of line `rb` replaced with
/// text of `ins` line breaks (`ends_line`: the text ends with one).
pub(super) struct Edit {
    pub ra: usize,
    pub ba: usize,
    pub rb: usize,
    pub bb: usize,
    pub ins: usize,
    pub ends_line: bool,
    pub hint: Option<Hint>,
}

/// Where a mark goes.
enum To {
    Stay,
    Gone,
    Row(usize),
    /// To line `row`, byte `base` plus its old byte less `cut` (at least `base`).
    Bytes {
        row: usize,
        base: usize,
        cut: usize,
    },
}

impl Edit {
    /// Where a mark on line `r` of the old text goes. `keep`: the mark stays on the next line
    /// when its own goes (the selection's ends).
    fn map(&self, r: usize, keep: bool) -> To {
        let (ra, rb, ins) = (self.ra, self.rb, self.ins);
        let rem = rb - ra;
        let shifted = |r: usize| (r + ins).saturating_sub(rem);
        if r < ra {
            return To::Stay;
        }
        match &self.hint {
            Some(Hint::Lines { first, last, keep_first }) => {
                let (first, last) = (*first, *last);
                if r < first || (*keep_first && r == first) {
                    To::Stay
                } else if r <= last {
                    if keep { To::Row(first + usize::from(*keep_first)) } else { To::Gone }
                } else {
                    To::Row(shifted(r))
                }
            }
            Some(Hint::Join { first, at }) => match r.checked_sub(*first) {
                Some(0) => To::Stay,
                Some(k) if k <= at.len() => {
                    let (base, cut) = at[k - 1];
                    To::Bytes { row: *first, base, cut }
                }
                _ if r > rb => To::Row(shifted(r)),
                _ => To::Stay,
            },
            _ if r > rb => To::Row(shifted(r)),
            // Lines put above a line (`O`, `P`, the undo of a delete) take its marks down.
            None if rem == 0 && self.ba == 0 && self.ends_line => To::Row(r + ins),
            _ if rem == ins || rem == 0 || (ins > 0 && rem < ins) => To::Stay,
            // Whole lines out: their marks go, the line after takes their place.
            _ if ins == 0 && self.ba == 0 && self.bb == 0 => match r < rb {
                true if keep => To::Row(ra),
                true => To::Gone,
                false => To::Row(ra),
            },
            // Lines joined by a delete: the last one's marks move to the first one by its
            // length up to the delete, as Vim moves them; the ones between go.
            _ if r == ra => To::Stay,
            _ if r < rb && !keep => To::Gone,
            _ if ins == 0 => To::Bytes { row: ra, base: self.ba, cut: 0 },
            _ => To::Row(ra + ins),
        }
    }
}

/// The marks of an editor.
#[derive(Clone, Debug)]
pub(super) struct Marks {
    named: [Option<Pos>; 26],
    /// The last Visual selection (`'<`, `'>`).
    visual: Option<Selection>,
    /// The Visual mode under way: its selection becomes `visual` when it ends.
    pending: Option<Selection>,
    /// The previous context mark, and the one before it while the jump that set it ends.
    pc: Option<Pos>,
    prev_pc: Option<Pos>,
    /// Where the last change was (`'.`).
    change: Option<Pos>,
}

impl Marks {
    /// The marks of a new text: its context mark is where the cursor starts, as in Vim.
    pub(super) fn new(start: Pos) -> Self {
        Marks { named: [None; 26], visual: None, pending: None, pc: Some(start), prev_pc: None, change: None }
    }

    /// The position of mark `c`. A Visual mark by line has the column `usize::MAX` at the
    /// line's end.
    pub(super) fn get(&self, c: char) -> Result<Pos, MarkNotice> {
        let p = match c {
            'a'..='z' => self.named[c as usize - 'a' as usize],
            '\'' | '`' => self.pc,
            '<' => self.visual.map(|s| s.ends().0),
            '>' => self.visual.map(|s| s.ends().1),
            '.' => self.change,
            _ => return Err(MarkNotice::Unknown(c)),
        };
        p.ok_or(MarkNotice::NotSet(c))
    }

    /// `m{c}`: mark `c` at `at` (`m'` the context mark, `m<` / `m>` an end of the selection).
    pub(super) fn set(&mut self, c: char, at: Pos) -> Result<(), MarkNotice> {
        match c {
            'a'..='z' => self.named[c as usize - 'a' as usize] = Some(at),
            '\'' | '`' => {
                self.set_pc(at);
                // Kept even when the cursor does not move.
                self.prev_pc = self.pc;
            }
            '<' | '>' => {
                let s = self.visual.get_or_insert(Selection { start: at, end: at, lines: false });
                let (a, b) = if s.start <= s.end { (s.start, s.end) } else { (s.end, s.start) };
                (s.start, s.end) = if c == '<' { (at, b) } else { (a, at) };
            }
            _ => return Err(MarkNotice::Unknown(c)),
        }
        Ok(())
    }

    /// A jump leaves from `at`: it becomes the context mark.
    pub(super) fn set_pc(&mut self, at: Pos) {
        self.prev_pc = self.pc;
        self.pc = Some(at);
    }

    /// A command is over with the cursor at `at`: when it jumped and the cursor did not move
    /// (or the jump's mark was deleted), the context mark is what it was before (Vim).
    pub(super) fn check_pc(&mut self, at: Pos) {
        if let Some(prev) = self.prev_pc.take()
            && (self.pc == Some(at) || self.pc.is_none())
        {
            self.pc = Some(prev);
        }
    }

    /// Visual mode now selects from `start` to `end`.
    pub(super) fn selecting(&mut self, sel: Option<Selection>) {
        self.pending = sel;
    }

    /// The Visual mode under way ended: its selection is the last one.
    pub(super) fn selected(&mut self) {
        if let Some(s) = self.pending.take() {
            self.visual = Some(s);
        }
    }

    /// The last Visual selection's first and last lines.
    pub(super) fn visual_lines(&self) -> Option<(usize, usize)> {
        self.visual.map(|s| {
            let (a, b) = s.ends();
            (a.0, b.0)
        })
    }

    pub(super) fn set_change(&mut self, at: Pos) {
        self.change = Some(at);
    }

    pub(super) fn save(&self) -> Saved {
        Saved { named: self.named, visual: self.visual }
    }

    /// An undo or redo of a change is over, `before` being the marks when it started: the
    /// marks the change saved come back (those that were set), and it keeps `before` for the
    /// way back (Vim).
    pub(super) fn swap(&mut self, saved: &mut Saved, before: Saved) {
        for (m, s) in self.named.iter_mut().zip(saved.named) {
            if s.is_some() {
                *m = s;
            }
        }
        if saved.visual.is_some() {
            self.visual = saved.visual;
            saved.visual = before.visual;
        }
        saved.named = before.named;
    }

    /// Move the marks for `e`: `old` is the text before it, `new_line` gives a line of the text
    /// after it (for a column given in bytes).
    pub(super) fn adjust(&mut self, e: &Edit, old: &[String], new_line: impl Fn(usize) -> Option<String>) {
        let place = |(r, c): Pos, keep: bool| -> Option<Pos> {
            match e.map(r, keep) {
                To::Stay => Some((r, c)),
                To::Gone => None,
                To::Row(row) => Some((row, c)),
                To::Bytes { row, base, cut } => {
                    let b: usize = old.get(r).map_or(c, |l| l.graphemes(true).take(c).map(str::len).sum());
                    let line = new_line(row).unwrap_or_default();
                    Some((row, byte_col(&line, base + b.saturating_sub(cut))))
                }
            }
        };
        for m in self.named.iter_mut().chain([&mut self.pc, &mut self.prev_pc, &mut self.change]) {
            *m = m.and_then(|p| place(p, false));
        }
        for s in [&mut self.visual, &mut self.pending].into_iter().flatten() {
            // The ends of a selection are never deleted: their line goes, they go to the next.
            s.start = place(s.start, true).unwrap_or(s.start);
            s.end = place(s.end, true).unwrap_or(s.end);
        }
    }
}

/// The grapheme of `line` at byte `b` (past its end, a column as far past it).
fn byte_col(line: &str, b: usize) -> usize {
    if b >= line.len() {
        return line.graphemes(true).count() + (b - line.len());
    }
    line.grapheme_indices(true).take_while(|(i, _)| *i <= b).count().saturating_sub(1)
}

impl Editor {
    /// `m{c}` at the cursor.
    pub(super) fn set_mark(&mut self, c: char) -> EdEvent {
        if let Err(n) = self.marks.set(c, (self.row, self.col)) {
            self.mark_notice = Some(n);
        }
        EdEvent::None
    }

    /// Where a jump to mark `c` goes (its line's first non-blank with `line`), on the text;
    /// `None` when it is not set (said).
    pub(super) fn mark_target(&mut self, c: char, line: bool) -> Option<Pos> {
        match self.marks.get(c) {
            Ok((r, col)) => {
                let r = r.min(self.lines.len() - 1);
                let col = if line { self.first_nonblank(r) } else { col.min(self.gcount(r).saturating_sub(1)) };
                Some((r, col))
            }
            Err(n) => {
                self.mark_notice = Some(n);
                None
            }
        }
    }

    /// Why the last jump to a mark did not happen, for the app to say; once.
    pub fn take_mark_notice(&mut self) -> Option<MarkNotice> {
        self.mark_notice.take()
    }

    /// The first and last lines of the last Visual selection (`'<,'>`), if there was one.
    pub fn visual_marks(&self) -> Option<(usize, usize)> {
        self.marks.visual_lines()
    }
}

#[cfg(test)]
mod tests;
