//! Insert mode: typing, Enter with the line's indent (autoindent), Tab, Backspace, Delete,
//! `Ctrl+W` (the word before the cursor), `Ctrl+U` (the line before the cursor), `Ctrl+R`
//! and a register's name (its text, as it is; `Ctrl+R Ctrl+R`, `Ctrl+O`, `Ctrl+P` the same),
//! arrows. What a session types becomes the `".` register when it ends.

use super::buffer::{class, graphemes, indent_of};
use super::marks::Hint;
use super::motion::{Motion, Pos};
use super::registers::{self, RegKind};
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
        self.ins_text.clear();
        EdEvent::Moved
    }

    pub(super) fn leave_insert(&mut self) {
        self.ins_repeat = None;
        self.ins_reg = false;
        self.drop_autoindent();
        if self.insert_snap {
            self.drop_empty_step();
        }
        self.insert_snap = false;
        let typed = std::mem::take(&mut self.ins_text);
        self.regs.set_inserted(typed);
        self.mode = Mode::Normal;
        self.col = self.col.saturating_sub(1);
        self.clamp();
        self.finish_block_insert();
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
        self.ins_reg = false;
        self.ins_text.clear();
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
        if std::mem::take(&mut self.ins_reg) {
            return match key.code {
                // `Ctrl+R Ctrl+R x` and the like put the text as it is, as `Ctrl+R x` does.
                KeyCode::Char('r' | 'o' | 'p') if ctrl => {
                    self.ins_reg = true;
                    EdEvent::None
                }
                KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => self.insert_register(c),
                _ => EdEvent::None,
            };
        }
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
                self.mark_hint = Some(Hint::Split);
                self.insert_at_cursor(&format!("\n{indent}"));
                self.ins_text.push('\n');
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
            KeyCode::Char('r') if ctrl => {
                self.ins_reg = true;
                self.rec.register_wait();
                EdEvent::None
            }
            KeyCode::Char('w') if ctrl => self.delete_before(true),
            KeyCode::Char('u') if ctrl => self.delete_before(false),
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                let mut buf = [0u8; 4];
                self.insert_at_cursor(c.encode_utf8(&mut buf));
                self.ins_text.push(c);
                EdEvent::Changed { typed: Some(c) }
            }
            KeyCode::Tab => {
                self.insert_at_cursor("    ");
                self.ins_text.push_str("    ");
                EdEvent::Changed { typed: None }
            }
            KeyCode::Backspace => {
                let removed = if self.col > 0 {
                    let a = self.offset_of(self.row, self.col - 1);
                    let b = self.offset();
                    self.delete_range(a, b)
                } else if self.row > 0 {
                    let b = self.offset();
                    self.delete_range(b - 1, b)
                } else {
                    return EdEvent::None;
                };
                self.untyped(&removed);
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

    /// `Ctrl+R` and register `c`: its text goes in at the cursor as it is (whole lines with a
    /// line break after the last one, a block's pieces on lines of their own). A name that is
    /// not a register, or an empty register, puts nothing.
    fn insert_register(&mut self, c: char) -> EdEvent {
        if !registers::is_name(c) {
            return EdEvent::None;
        }
        let Some(reg) = self.put_source(c) else { return EdEvent::None };
        let text = match reg.kind {
            RegKind::Linewise => format!("{}\n", reg.text),
            RegKind::Charwise | RegKind::Blockwise => reg.text.clone(),
        };
        if text.is_empty() {
            return EdEvent::None;
        }
        self.ai_row = None;
        self.insert_at_cursor(&text);
        self.ins_text.push_str(&text);
        self.rec.register_text(&text);
        EdEvent::Changed { typed: None }
    }

    /// Text this session typed was deleted again: `".` loses it too.
    fn untyped(&mut self, removed: &str) {
        if let Some(rest) = self.ins_text.strip_suffix(removed) {
            self.ins_text.truncate(rest.len());
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
            let removed = self.delete_range(b - 1, b);
            self.untyped(&removed);
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
        let removed = self.delete_range(a, b);
        self.untyped(&removed);
        self.deleted_back();
        EdEvent::Changed { typed: None }
    }
}

#[cfg(test)]
mod tests;
