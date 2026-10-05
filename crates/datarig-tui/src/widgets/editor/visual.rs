//! Visual mode: by character (`v`) or by whole lines (`V`), the selection as text, text
//! objects that set or grow it, the operators on it, and `p` / `P` (a register in its place).
//! A block (`Ctrl+V`) has its own operators (`block`).

use super::buffer::{UNDO_BYTES, graphemes};
use super::edit::Case;
use super::marks::{Hint, Selection};
use super::motion::RangeKind;
use super::registers::RegKind;
use super::textobj::Object;
use super::vim::{Op, Target, Token};
use super::{EdEvent, Editor, Mode, Sel};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use unicode_segmentation::UnicodeSegmentation;

impl Editor {
    /// Start Visual mode selecting `sel` with the other end at `anchor`.
    pub(super) fn enter_visual(&mut self, sel: Sel, anchor: (usize, usize)) -> EdEvent {
        if self.mode == Mode::Insert {
            self.leave_insert();
        }
        self.mode = Mode::Visual;
        self.sel = sel;
        self.anchor = anchor;
        EdEvent::Moved
    }

    /// `v`, `V`, `Ctrl+V` in Visual mode: the selection becomes `sel`, or Visual mode ends
    /// when it is that already.
    pub(super) fn switch_visual(&mut self, sel: Sel) -> EdEvent {
        if self.sel == sel {
            self.exit_visual();
        } else {
            self.sel = sel;
        }
        EdEvent::Moved
    }

    pub fn exit_visual(&mut self) {
        if self.mode == Mode::Visual {
            self.keep_selection();
            self.mode = Mode::Normal;
            self.clamp();
        }
    }

    /// The selection as the marks keep it, in Visual mode.
    pub(super) fn selection_marks(&self) -> Option<Selection> {
        (self.mode == Mode::Visual).then(|| Selection {
            start: self.anchor,
            end: (self.row, self.col),
            lines: self.sel == Sel::Lines,
        })
    }

    /// Visual mode ends here: `'<` and `'>` take its selection.
    pub(super) fn keep_selection(&mut self) {
        self.marks.selecting(self.selection_marks());
        self.marks.selected();
    }

    /// The selection's ends in text order.
    pub(super) fn visual_ends(&self) -> ((usize, usize), (usize, usize)) {
        let cursor = (self.row, self.col);
        if self.anchor <= cursor { (self.anchor, cursor) } else { (cursor, self.anchor) }
    }

    /// Where the selection starts (the start of its first line by line).
    fn visual_start(&self) -> (usize, usize) {
        let (a, _) = self.visual_ends();
        if self.sel == Sel::Lines { (a.0, 0) } else { a }
    }

    /// Byte range of the selection: by character both ends included (a selected line end is
    /// its line break, and after `$` the cursor's line break is selected), by line whole lines
    /// with the line break after the last one.
    pub(super) fn visual_bounds(&self) -> (usize, usize) {
        let (a, b) = self.visual_ends();
        let more = |r: usize| usize::from(r + 1 < self.lines.len());
        if self.sel == Sel::Lines {
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
        if lines || self.sel == Sel::Lines {
            Target::Lines { first: a.0, last: b.0 }
        } else {
            let (a, b) = self.visual_bounds();
            Target::Chars { a, b }
        }
    }

    /// Whether the text position `at` (line, grapheme) is in the Visual selection.
    pub fn in_selection(&self, at: (usize, usize)) -> bool {
        if self.mode != Mode::Visual {
            return false;
        }
        let (a, b) = self.visual_ends();
        match self.sel {
            Sel::Lines => (a.0..=b.0).contains(&at.0),
            // The block's columns are screen columns: a wide character takes two, and after `$`
            // it goes to every line's end.
            Sel::Block => {
                let bl = self.block();
                let x = self.display_x(at.0, at.1);
                (bl.first..=bl.last).contains(&at.0) && x >= bl.start && (bl.max || x <= bl.end)
            }
            Sel::Chars => (a..=b).contains(&at),
        }
    }

    /// Selected text in Visual mode (for Ctrl+E): whole lines without the last line break by
    /// line, a block's pieces as `y` takes them.
    pub fn selection(&self) -> Option<String> {
        if self.visual_block() {
            return Some(self.selected_block_text());
        }
        (self.mode == Mode::Visual).then(|| match self.visual_target(false) {
            Target::Chars { a, b } => self.slice(self.pos_bytes(a), self.pos_bytes(b)),
            Target::Lines { first, last } => self.lines[first..=last].join("\n"),
        })
    }

    pub(super) fn key_visual(&mut self, key: KeyEvent) -> EdEvent {
        let token = self.token(key, true);
        let (n, explicit) = self.cmd.count();
        if token == Token::More {
            return EdEvent::None;
        }
        self.end_command();
        match token {
            Token::Cancel if key.code == KeyCode::Esc => {
                self.exit_visual();
                EdEvent::Moved
            }
            Token::Motion(m) => {
                if m.is_jump() {
                    self.marks.set_pc((self.row, self.col));
                }
                self.move_by(m, n, explicit);
                EdEvent::Moved
            }
            Token::Mark(c) => self.set_mark(c),
            Token::G('c') => self.visual_comment(),
            Token::Object(o, around) => self.visual_object(o, around, n),
            Token::Ctrl(c @ ('d' | 'u')) => self.scroll_half(c == 'd', n, explicit),
            Token::Ctrl(c @ ('f' | 'b')) => self.scroll_page(c == 'f', n),
            Token::Ctrl('v') => self.switch_visual(Sel::Block),
            Token::Z(c) => self.scroll_cursor(c, n, explicit),
            Token::Replace(ch) if self.sel == Sel::Block => self.block_replace(ch),
            Token::G(c @ ('u' | 'U' | '~')) if self.sel == Sel::Block => self.block_case(c),
            Token::Key(c) if self.sel == Sel::Block => self.block_command(c, n),
            Token::Replace('\n') => EdEvent::None,
            Token::Replace(ch) => {
                self.record_selection();
                let (to, span) = (self.visual_start(), self.visual_bounds());
                self.replace_span(ch, span, to)
            }
            Token::G(c @ ('u' | 'U' | '~')) => self.visual_case(c),
            Token::G('J') => self.visual_join(false),
            Token::Key(c) => self.visual_command(c, n),
            _ => EdEvent::None,
        }
    }

    /// The command keys of Visual mode.
    pub(super) fn visual_command(&mut self, c: char, n: usize) -> EdEvent {
        match c {
            'o' => {
                let cursor = (self.row, self.col);
                (self.row, self.col) = self.anchor;
                self.anchor = cursor;
                self.want_x = None;
                EdEvent::Moved
            }
            'v' => self.switch_visual(Sel::Chars),
            'V' => self.switch_visual(Sel::Lines),
            'y' | 'Y' => {
                let lines = c == 'Y' || self.sel == Sel::Lines;
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
                self.record_selection();
                let t = self.visual_target(c.is_ascii_uppercase());
                self.apply(Op::Delete, t, (self.row, self.col))
            }
            'c' | 's' | 'C' | 'S' => {
                self.record_selection();
                let t = self.visual_target(c.is_ascii_uppercase());
                self.apply(Op::Change, t, (self.row, self.col))
            }
            'u' | 'U' | '~' => self.visual_case(c),
            'p' | 'P' => self.visual_put(c == 'P', n),
            'J' => self.visual_join(true),
            '>' | '<' => {
                self.record_selection();
                let (a, b) = self.visual_ends();
                self.mode = Mode::Normal;
                self.shift_lines(a.0, b.0, n, c == '>')
            }
            _ => EdEvent::None,
        }
    }

    /// `p` / `P`: the selection is replaced by the register the command names (else the
    /// unnamed one), `n` times. With `p` the text replaced goes to the registers as a delete
    /// without a register does (Neovim); with `P` (`keep`) nowhere. An empty register still
    /// takes the selection away (and `.` repeats only that, as Neovim does). By line, the lines' text is replaced; a register of lines
    /// into a selection by character goes on lines of its own between the two ends.
    fn visual_put(&mut self, keep: bool, n: usize) -> EdEvent {
        let name = self.reg.unwrap_or('"');
        if matches!(name, '+' | '*') {
            self.rec.put_clipboard();
        }
        // Neovim's `.` repeats it as a delete of as much text: nothing is put.
        let src = if self.rec.replaying() { None } else { self.put_source(name) };
        if src.as_ref().is_some_and(|r| self.put_size(r, n) > UNDO_BYTES) {
            return EdEvent::None;
        }
        self.record_selection();
        let t = self.visual_target(false);
        self.mode = Mode::Normal;
        self.snapshot();
        self.reg = None;
        match t {
            Target::Lines { first, last } => {
                let text = self.lines[first..=last].join("\n");
                match &src {
                    Some(r) => {
                        let put = match r.kind {
                            RegKind::Charwise => r.text.repeat(n),
                            RegKind::Linewise | RegKind::Blockwise => vec![r.text.as_str(); n].join("\n"),
                        };
                        let (a, b) = (self.line_start(first), self.line_start(last) + self.lines[last].len());
                        self.mark_hint = Some(Hint::Lines { first, last, keep_first: false });
                        self.splice(a, b, &put);
                    }
                    None => {
                        let (a, b) = self.lines_span(first, last);
                        self.mark_hint = Some(Hint::Lines { first, last, keep_first: false });
                        self.splice(a, b, "");
                    }
                }
                if !keep {
                    self.delete_to_register(text, RegKind::Linewise);
                }
                let r = first.min(self.lines.len() - 1);
                self.set_pos(r, self.first_nonblank(r));
            }
            Target::Chars { a, b } => {
                let text = self.delete_range(a, b);
                if !keep {
                    self.delete_to_register(text, RegKind::Charwise);
                }
                self.set_cursor_offset(a);
                match src {
                    Some(r) if r.kind == RegKind::Blockwise => self.put_block(&r.text, false, n),
                    Some(r) if r.kind == RegKind::Linewise => {
                        let lines = vec![r.text.as_str(); n].join("\n");
                        self.splice(a, a, &format!("\n{lines}\n"));
                        let row = self.pos_bytes(a).0 + 1;
                        self.set_pos(row, self.first_nonblank(row));
                    }
                    Some(r) if !r.text.is_empty() => {
                        let put = r.text.repeat(n);
                        self.splice(a, a, &put);
                        let last = put.graphemes(true).next_back().map_or(0, str::len);
                        self.set_cursor_offset(a + put.len() - last);
                        self.want_x = None;
                    }
                    _ => {}
                }
                self.clamp();
            }
        }
        EdEvent::Changed { typed: None }
    }

    /// `.` repeats a Visual mode operator on as much text as it took.
    fn record_selection(&mut self) {
        let sel = self.selection_input();
        self.rec.select(sel);
    }

    /// `u` `U` `~` (and `gu` `gU` `g~`) on the selection.
    fn visual_case(&mut self, c: char) -> EdEvent {
        self.record_selection();
        let case = match c {
            'u' => Case::Lower,
            'U' => Case::Upper,
            _ => Case::Toggle,
        };
        let (to, span) = (self.visual_start(), self.visual_bounds());
        self.mode = Mode::Normal;
        self.recase_span(case, span, to)
    }

    /// Where an operator leaves the cursor: the selection's start (by line, the first line's
    /// start, or the cursor's column when the cursor is on it).
    pub(super) fn selection_start(&self) -> (usize, usize) {
        let (a, _) = self.visual_ends();
        match self.sel {
            Sel::Lines if self.row < self.anchor.0 => (a.0, self.col),
            Sel::Lines => (a.0, 0),
            _ => a,
        }
    }

    /// `gc` on the selected lines; the cursor goes to the selection's start, as `y` puts it.
    fn visual_comment(&mut self) -> EdEvent {
        self.record_selection();
        let (a, b) = self.visual_ends();
        let to = self.selection_start();
        self.mode = Mode::Normal;
        self.comment_lines(a.0, b.0, to)
    }

    /// `J` (`spaces`) and `gJ`: join the selected lines (at least two).
    fn visual_join(&mut self, spaces: bool) -> EdEvent {
        self.record_selection();
        let (a, b) = self.visual_ends();
        self.mode = Mode::Normal;
        self.join_lines(a.0, b.0 - a.0 + 1, spaces)
    }

    /// A text object in Visual mode: it becomes the selection (a word, a string or a block by
    /// character, paragraphs by line), or grows it when it is more than the cursor.
    fn visual_object(&mut self, o: Object, around: bool, count: usize) -> EdEvent {
        let cursor = (self.row, self.col);
        let grow = (self.anchor != cursor).then_some(self.anchor);
        let Some(f) = self.object(o, count, around, grow) else { return EdEvent::None };
        self.anchor = f.start;
        let end = match f.kind {
            RangeKind::Exclusive if f.end.1 > 0 => (f.end.0, f.end.1 - 1),
            RangeKind::Exclusive if f.end.0 > 0 => (f.end.0 - 1, self.gcount(f.end.0 - 1).saturating_sub(1)),
            _ => f.end,
        };
        (self.row, self.col) = end;
        self.want_x = f.eol.then_some(usize::MAX);
        // A word keeps a block a block (Vim); the other objects choose characters or lines.
        let word_in_block = self.sel == Sel::Block && matches!(o, Object::Word { .. });
        if !f.keep && !word_in_block {
            self.sel = if f.kind == RangeKind::Linewise { Sel::Lines } else { Sel::Chars };
        }
        EdEvent::Moved
    }
}

#[cfg(test)]
mod tests;
