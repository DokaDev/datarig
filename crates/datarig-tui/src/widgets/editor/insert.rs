//! Insert mode: typing, Enter with the line's indent (autoindent), Tab, Backspace, Delete,
//! `Ctrl+W` (the word before the cursor) and `Ctrl+U` (the line before the cursor), arrows.

use super::buffer::{class, graphemes, indent_of};
use super::motion::{Motion, Pos};
use super::{EdEvent, Editor, Mode};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

impl Editor {
    /// Enter Insert mode with the cursor at `at` (`snapshot`: as a new undo step; a change
    /// that started it has made one already).
    pub(super) fn start_insert(&mut self, snapshot: bool, at: Pos) -> EdEvent {
        if snapshot {
            self.snapshot();
        }
        self.insert_snap = true;
        self.mode = Mode::Insert;
        (self.row, self.col) = at;
        self.want_x = None;
        self.clamp();
        self.ins_start = (self.row, self.col);
        self.ai_row = None;
        EdEvent::Moved
    }

    pub(super) fn leave_insert(&mut self) {
        self.ins_repeat = None;
        self.drop_autoindent();
        if self.insert_snap {
            self.drop_empty_step();
        }
        self.insert_snap = false;
        self.mode = Mode::Normal;
        self.col = self.col.saturating_sub(1);
        self.clamp();
    }

    /// The cursor moved without typing (arrows, a click) from line `from`: Insert starts again
    /// from there, as far as `Ctrl+W` and `Ctrl+U` are concerned, and a line left with only
    /// its autoindent loses it.
    pub(super) fn moved_in_insert(&mut self, from: usize) {
        if self.row != from {
            self.drop_autoindent();
        }
        self.ins_start = (self.row, self.col);
        self.ai_row = None;
        self.ins_repeat = None;
        self.rec.moved_in_insert();
    }

    /// A line that holds only the indent Enter, `o` or `O` put there loses that indent.
    pub(super) fn drop_autoindent(&mut self) {
        let Some(r) = self.ai_row.take() else { return };
        let line = &self.lines[r];
        if !line.is_empty() && indent_of(line).len() == line.len() {
            let (a, b) = self.line_bounds(r);
            self.splice(a, b, "");
            if self.row == r {
                self.col = 0;
            }
        }
    }

    /// Text before where Insert started was deleted: it starts at the cursor now.
    fn deleted_back(&mut self) {
        self.ins_start = self.ins_start.min((self.row, self.col));
    }

    pub(super) fn key_insert(&mut self, key: KeyEvent) -> EdEvent {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.leave_insert();
                return EdEvent::Moved;
            }
            KeyCode::Enter => {
                // The indent of the line up to the cursor goes to the new line, in place of the
                // blanks the rest of the line starts with; a line that holds only such an
                // indent keeps none.
                let line = &self.lines[self.row];
                let upto = self.byte_at(self.row, self.col);
                let indent = indent_of(&line[..upto]).to_string();
                let blanks = indent_of(&line[upto..]).len();
                self.drop_autoindent();
                if blanks > 0 {
                    let at = self.offset();
                    self.splice(at, at + blanks, "");
                }
                self.insert_at_cursor(&format!("\n{indent}"));
                self.ai_row = (!indent.is_empty()).then_some(self.row);
                return EdEvent::Changed { typed: None };
            }
            code => {
                if let Some(m) = Motion::of_key(code) {
                    let from = self.row;
                    self.insert_motion(m);
                    self.moved_in_insert(from);
                    return EdEvent::Moved;
                }
            }
        }
        self.ai_row = None;
        match key.code {
            KeyCode::Char('w') if ctrl => self.delete_before(true),
            KeyCode::Char('u') if ctrl => self.delete_before(false),
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                let mut buf = [0u8; 4];
                self.insert_at_cursor(c.encode_utf8(&mut buf));
                EdEvent::Changed { typed: Some(c) }
            }
            KeyCode::Tab => {
                self.insert_at_cursor("    ");
                EdEvent::Changed { typed: None }
            }
            KeyCode::Backspace => {
                if self.col > 0 {
                    let a = self.offset_of(self.row, self.col - 1);
                    let b = self.offset();
                    self.delete_range(a, b);
                } else if self.row > 0 {
                    let b = self.offset();
                    self.delete_range(b - 1, b);
                } else {
                    return EdEvent::None;
                }
                self.deleted_back();
                EdEvent::Changed { typed: None }
            }
            KeyCode::Delete => {
                let a = self.offset();
                let n = self.gcount(self.row);
                if self.col < n {
                    let b = self.offset_of(self.row, self.col + 1);
                    self.delete_range(a, b);
                } else if self.row + 1 < self.lines.len() {
                    self.delete_range(a, a + 1);
                } else {
                    return EdEvent::None;
                }
                EdEvent::Changed { typed: None }
            }
            _ => EdEvent::None,
        }
    }

    /// Arrows, Home and End in Insert mode (the cursor may go after the line's last grapheme).
    fn insert_motion(&mut self, m: Motion) {
        match m {
            Motion::Left => self.set_pos(self.row, self.col.saturating_sub(1)),
            Motion::Right => self.set_pos(self.row, self.col + 1),
            Motion::LineStart => self.set_pos(self.row, 0),
            Motion::LineEnd => self.set_pos(self.row, self.gcount(self.row)),
            Motion::Down => self.move_vert(1),
            Motion::Up => self.move_vert(-1),
            _ => {}
        }
    }

    /// `Ctrl+W` (`word`: the word before the cursor and the blanks after it) and `Ctrl+U` (the
    /// line before the cursor, its indent last). Text typed in this Insert session goes first:
    /// the deletion stops where the session started, the next one goes on from there. At the
    /// start of a line they join it to the line before.
    fn delete_before(&mut self, word: bool) -> EdEvent {
        let (r, c) = (self.row, self.col);
        if c == 0 {
            if r == 0 {
                return EdEvent::None;
            }
            let b = self.offset();
            self.delete_range(b - 1, b);
            self.deleted_back();
            return EdEvent::Changed { typed: None };
        }
        let typed_from = (self.ins_start.0 == r && self.ins_start.1 < c).then_some(self.ins_start.1);
        let mut to = if word {
            let gs = graphemes(&self.lines[r]);
            let mut i = c;
            while i > 0 && class(gs[i - 1]) == 0 {
                i -= 1;
            }
            if i > 0 {
                let cls = class(gs[i - 1]);
                while i > 0 && class(gs[i - 1]) == cls {
                    i -= 1;
                }
            }
            i
        } else {
            let indent = indent_of(&self.lines[r]).len();
            let indent = self.lines[r][..indent].graphemes(true).count();
            if c > indent { indent } else { 0 }
        };
        if let Some(s) = typed_from {
            to = to.max(s);
        }
        let a = self.offset_of(r, to);
        let b = self.offset();
        self.delete_range(a, b);
        self.deleted_back();
        EdEvent::Changed { typed: None }
    }
}

#[cfg(test)]
mod tests;
