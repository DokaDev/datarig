//! The text: lines, positions and byte offsets, edits as splices, and undo.

use super::{Editor, Mode, TAB_WIDTH};
use crate::text::grapheme_width;
use unicode_segmentation::UnicodeSegmentation;

/// One edit of the text: at byte `at`, `removed` was replaced with `inserted`.
#[derive(Clone)]
pub(super) struct Change {
    at: usize,
    removed: String,
    inserted: String,
}

/// What one command changed (an Insert session, `x`, `dd`, a put, …), with the cursor before
/// it and where it was when it was undone.
#[derive(Clone)]
pub(super) struct Step {
    changes: Vec<Change>,
    before: (usize, usize),
    after: (usize, usize),
}

impl Step {
    fn bytes(&self) -> usize {
        self.changes.iter().map(|c| c.removed.len() + c.inserted.len()).sum()
    }
}

/// Undo steps kept, and the bytes they may hold together.
const UNDO_STEPS: usize = 500;
pub(super) const UNDO_BYTES: usize = 64 << 20;

/// Text versions, unique across editors: the same number means the same text of the same
/// editor.
static VERSIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub(super) fn next_version() -> u64 {
    VERSIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub(super) fn graphemes(s: &str) -> Vec<&str> {
    s.graphemes(true).collect()
}

/// Display width of a grapheme (a tab takes [`TAB_WIDTH`]).
pub(super) fn gw(g: &str) -> usize {
    if g == "\t" { TAB_WIDTH } else { grapheme_width(g) }
}

/// Word class of a grapheme: 0 blank, 1 a word character (letters, digits, `_`, non-ASCII),
/// 2 other symbols.
pub(super) fn class(g: &str) -> u8 {
    let c = g.chars().next().unwrap_or(' ');
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' || !c.is_ascii() {
        1
    } else {
        2
    }
}

/// The leading blanks of `line`.
pub(super) fn indent_of(line: &str) -> &str {
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

impl Editor {
    pub(super) fn gcount(&self, r: usize) -> usize {
        self.lines[r].graphemes(true).count()
    }

    pub(super) fn byte_at(&self, r: usize, c: usize) -> usize {
        self.lines[r].graphemes(true).take(c).map(str::len).sum()
    }

    pub(super) fn line_start(&self, r: usize) -> usize {
        self.lines[..r].iter().map(|l| l.len() + 1).sum()
    }

    pub(super) fn offset_of(&self, r: usize, c: usize) -> usize {
        self.line_start(r) + self.byte_at(r, c)
    }

    /// Byte offset of the cursor in [`Editor::text`].
    pub fn offset(&self) -> usize {
        self.offset_of(self.row, self.col)
    }

    pub(super) fn pos_of(&self, mut off: usize) -> (usize, usize) {
        for (r, l) in self.lines.iter().enumerate() {
            if off <= l.len() {
                let mut b = off;
                while !l.is_char_boundary(b) {
                    b -= 1;
                }
                return (r, l[..b].graphemes(true).count());
            }
            off -= l.len() + 1;
        }
        let r = self.lines.len() - 1;
        (r, self.gcount(r))
    }

    /// Line and byte in it of byte offset `off` of the text.
    pub(super) fn pos_bytes(&self, mut off: usize) -> (usize, usize) {
        for (r, l) in self.lines.iter().enumerate() {
            if off <= l.len() {
                return (r, off);
            }
            off -= l.len() + 1;
        }
        let r = self.lines.len() - 1;
        (r, self.lines[r].len())
    }

    /// The text from `a` to `b` (line, byte).
    pub(super) fn slice(&self, a: (usize, usize), b: (usize, usize)) -> String {
        if a.0 == b.0 {
            return self.lines[a.0][a.1..b.1].to_string();
        }
        let mut s = String::from(&self.lines[a.0][a.1..]);
        for l in &self.lines[a.0 + 1..b.0] {
            s.push('\n');
            s.push_str(l);
        }
        s.push('\n');
        s.push_str(&self.lines[b.0][..b.1]);
        s
    }

    /// Replace bytes `a..b` of the text with `s`, touching only the lines in between; returns
    /// what was there. Recorded in the current undo step.
    pub(super) fn splice(&mut self, a: usize, b: usize, s: &str) -> String {
        let removed = self.splice_raw(a, b, s);
        if self.undo.is_empty() {
            self.undo.push(Step { changes: Vec::new(), before: (self.row, self.col), after: (self.row, self.col) });
        }
        if let Some(step) = self.undo.last_mut() {
            step.changes.push(Change { at: a, removed: removed.clone(), inserted: s.to_string() });
        }
        removed
    }

    pub(super) fn splice_raw(&mut self, a: usize, b: usize, s: &str) -> String {
        let (ra, ba) = self.pos_bytes(a);
        let (rb, bb) = self.pos_bytes(b.max(a));
        let removed = self.slice((ra, ba), (rb, bb));
        let mut joined = String::with_capacity(ba + s.len() + self.lines[rb].len() - bb);
        joined.push_str(&self.lines[ra][..ba]);
        joined.push_str(s);
        joined.push_str(&self.lines[rb][bb..]);
        let new: Vec<String> = joined.split('\n').map(str::to_string).collect();
        self.lines.splice(ra..=rb, new);
        self.bytes = self.bytes - removed.len() + s.len();
        self.version = next_version();
        // Every state from line `ra` on may have changed. (Line `ra`'s own: a token that ran to
        // the end of the text exactly at its start grows over it when text is added there.)
        self.valid = self.valid.min(ra.max(1));
        removed
    }

    pub(super) fn set_cursor_offset(&mut self, off: usize) {
        let (r, c) = self.pos_of(off.min(self.bytes));
        self.row = r;
        self.col = c;
    }

    pub(super) fn display_x(&self, r: usize, c: usize) -> usize {
        self.lines[r].graphemes(true).take(c).map(gw).sum()
    }

    pub(super) fn col_for_x(&self, r: usize, x: usize) -> usize {
        let mut acc = 0;
        for (i, g) in self.lines[r].graphemes(true).enumerate() {
            let w = gw(g);
            if acc + w > x {
                return i;
            }
            acc += w;
        }
        self.gcount(r)
    }

    pub(super) fn first_nonblank(&self, r: usize) -> usize {
        graphemes(&self.lines[r]).iter().position(|g| class(g) != 0).unwrap_or(0)
    }

    /// Keep the cursor on the text: Insert mode may sit after the line's last grapheme, the
    /// other modes on it.
    pub(super) fn clamp(&mut self) {
        self.row = self.row.min(self.lines.len() - 1);
        let n = self.gcount(self.row);
        let max = if self.mode == Mode::Insert { n } else { n.saturating_sub(1) };
        self.col = self.col.min(max);
    }

    pub(super) fn set_pos(&mut self, r: usize, c: usize) {
        self.row = r;
        self.col = c;
        self.want_x = None;
        self.clamp();
    }

    /// Start an undo step: the changes until the next one are undone together.
    pub(super) fn snapshot(&mut self) {
        self.undo.push(Step { changes: Vec::new(), before: (self.row, self.col), after: (self.row, self.col) });
        let mut bytes: usize = self.undo.iter().map(Step::bytes).sum();
        while self.undo.len() > UNDO_STEPS || (bytes > UNDO_BYTES && self.undo.len() > 1) {
            bytes -= self.undo.remove(0).bytes();
        }
        self.redo.clear();
    }

    /// Drop the current undo step if it changed nothing (an Insert session without typing).
    pub(super) fn drop_empty_step(&mut self) {
        if self.undo.last().is_some_and(|s| s.changes.is_empty()) {
            self.undo.pop();
        }
    }

    /// Undo (`from_undo`) or redo one step; false when there is none.
    pub(super) fn restore(&mut self, from_undo: bool) -> bool {
        let src = if from_undo { &mut self.undo } else { &mut self.redo };
        let Some(mut step) = src.pop() else { return false };
        if from_undo {
            step.after = (self.row, self.col);
            for c in step.changes.iter().rev() {
                self.splice_raw(c.at, c.at + c.inserted.len(), &c.removed);
            }
            (self.row, self.col) = step.before;
            self.redo.push(step);
        } else {
            for c in &step.changes {
                self.splice_raw(c.at, c.at + c.removed.len(), &c.inserted);
            }
            (self.row, self.col) = step.after;
            self.undo.push(step);
        }
        self.clamp();
        true
    }

    pub(super) fn insert_at_cursor(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        let off = self.offset();
        self.splice(off, off, s);
        self.set_cursor_offset(off + s.len());
        self.want_x = None;
    }

    /// Replace `[a, b)` (byte offsets in the full text) with `s`, cursor after it.
    pub fn replace_range(&mut self, a: usize, b: usize, s: &str) {
        self.splice(a, b, s);
        self.set_cursor_offset(a + s.len());
        self.want_x = None;
    }

    pub(super) fn delete_range(&mut self, a: usize, b: usize) -> String {
        let removed = self.splice(a, b, "");
        self.set_cursor_offset(a);
        self.want_x = None;
        removed
    }

    /// Byte offset of the start of line `r` and of its end (before its line break).
    pub(super) fn line_bounds(&self, r: usize) -> (usize, usize) {
        let start = self.line_start(r);
        (start, start + self.lines[r].len())
    }

    /// The bytes that go when lines `first..=last` are deleted: with the line break after
    /// them, or before them for the last lines of the text.
    pub(super) fn lines_span(&self, first: usize, last: usize) -> (usize, usize) {
        let a = self.line_start(first);
        let end = a + self.lines[first..=last].iter().map(|l| l.len() + 1).sum::<usize>() - 1;
        if last + 1 < self.lines.len() {
            (a, end + 1)
        } else if first > 0 {
            (a - 1, end)
        } else {
            (a, end)
        }
    }
}

#[cfg(test)]
mod tests;
