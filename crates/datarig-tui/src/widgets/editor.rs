//! SQL editor with vim keys. The cursor moves by grapheme cluster and the screen column is the
//! sum of preceding grapheme widths, so the real terminal cursor can be placed exactly for IME
//! preedit.
//!
//! Keys arrive through the keymap (`crate::keymap`): Hangul typed in Normal/Visual mode has
//! already been turned into QWERTY keys, and app keys (run, commands, leader, …) never get here.
//!
//! The parts:
//! * `buffer` holds the text as a vector of lines; every edit is a splice of the lines it
//!   touches ([`Editor::splice`]), and undo keeps what each command changed, not copies of the
//!   text.
//! * `lexing` lexes only what is needed: highlighting lexes the lines on screen, from the
//!   nearest point where the lexer's state is known (the state at each line start is cached,
//!   and edits invalidate it from their line on). The statement under the cursor and the
//!   completion context are found in a region of lines around the cursor that holds the `;`
//!   around it, with the same result as splitting the whole text.
//! * `motion` says where a motion leads, and the range it covers for an operator
//!   (exclusive, inclusive or whole lines, as Vim decides).
//! * `vim` parses Normal-mode commands (`[count] operator [count] motion`, doubled
//!   operators, the single-key commands) and applies the operators.
//! * `visual` is Visual mode by character (`v`) or by line (`V`).
//! * `insert` is Insert mode, with autoindent and `Ctrl+W` / `Ctrl+U`.
//! * `render` draws.
//!
//! One command is one undo step: an operator, a put, a paste, or an Insert session with the
//! change that started it.

mod buffer;
mod insert;
mod lexing;
mod motion;
mod render;
mod vim;
mod visual;

use buffer::{Step, class, graphemes, next_version};
use datarig_core::i18n::Label;
use lexing::{LineState, REGION_LINES};
use ratatui::crossterm::event::KeyEvent;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    /// By character, or by line ([`Editor::visual_lines`]).
    Visual,
}

impl Mode {
    /// Catalog label of the mode (status bar).
    pub fn label(self) -> Label {
        match self {
            Mode::Normal => Label::StatusModeNormal,
            Mode::Insert => Label::StatusModeInsert,
            Mode::Visual => Label::StatusModeVisual,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EdEvent {
    None,
    /// Text changed; `typed` is the character inserted in Insert mode, if any.
    Changed {
        typed: Option<char>,
    },
    /// Cursor moved or mode changed without a text change.
    Moved,
}

/// What a yank or a delete put in the register.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Register {
    pub text: String,
    /// Whole lines (`yy`, `dj`, `V`…): put as lines of their own.
    pub linewise: bool,
}

const TAB_WIDTH: usize = 4;

pub struct Editor {
    pub lines: Vec<String>,
    pub row: usize,
    pub col: usize,
    want_x: Option<usize>,
    pub mode: Mode,
    /// Visual mode takes whole lines (`V`).
    visual_lines: bool,
    anchor: (usize, usize),
    /// The Normal/Visual command typed so far (count, operator, `g`).
    cmd: vim::Pending,
    register: Option<Register>,
    /// The last write to the register, until the app takes it ([`Editor::take_yank`]).
    yanked: Option<Register>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    insert_snap: bool,
    /// Where the Insert session started: `Ctrl+W` and `Ctrl+U` stop there once.
    ins_start: (usize, usize),
    /// The line whose only text is the indent Enter, `o` or `O` copied: the indent goes again
    /// when nothing is typed on it.
    ai_row: Option<usize>,
    pub top: usize,
    pub left: usize,
    view_h: usize,
    gutter: usize,
    /// Bytes of the text (the lines and their line breaks).
    bytes: usize,
    /// Bumped by every change of the text.
    version: u64,
    /// The lexer state at the start of each line; `states[..valid]` are current.
    states: Vec<LineState>,
    valid: usize,
    /// Lines searched first around the cursor (smaller in tests).
    region: usize,
}

impl Editor {
    pub fn new(text: &str) -> Self {
        Self {
            lines: text.split('\n').map(str::to_string).collect(),
            row: 0,
            col: 0,
            want_x: None,
            mode: Mode::Normal,
            visual_lines: false,
            anchor: (0, 0),
            cmd: vim::Pending::default(),
            register: None,
            yanked: None,
            undo: Vec::new(),
            redo: Vec::new(),
            insert_snap: false,
            ins_start: (0, 0),
            ai_row: None,
            top: 0,
            left: 0,
            view_h: 1,
            gutter: 0,
            bytes: text.len(),
            version: next_version(),
            states: Vec::new(),
            valid: 0,
            region: REGION_LINES,
        }
    }

    /// Bytes of the text.
    pub fn len_bytes(&self) -> usize {
        self.bytes
    }

    /// Changes whenever the text changes, and differs between editors: the same number means
    /// the same text.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Catalog label of the mode as the status bar shows it (Visual by line is `V-LINE`).
    pub fn mode_label(&self) -> Label {
        match self.mode {
            Mode::Visual if self.visual_lines => Label::StatusModeVisualLine,
            m => m.label(),
        }
    }

    /// Visual mode takes whole lines (`V`).
    pub fn visual_lines(&self) -> bool {
        self.mode == Mode::Visual && self.visual_lines
    }

    /// The register (the text of the last yank or delete).
    pub fn register(&self) -> Option<&Register> {
        self.register.as_ref()
    }

    /// The last write to the register since the previous call, for the app to pass on (the
    /// editor never touches the clipboard itself).
    pub fn take_yank(&mut self) -> Option<Register> {
        self.yanked.take()
    }

    fn set_register(&mut self, text: String, linewise: bool) {
        let r = Register { text, linewise };
        self.yanked = Some(r.clone());
        self.register = Some(r);
    }

    /// A command waits for its next key (`d…`, `y…`, `g…`).
    pub fn awaiting_key(&self) -> bool {
        self.cmd.awaiting()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> EdEvent {
        match self.mode {
            Mode::Insert => self.key_insert(key),
            Mode::Normal => self.key_normal(key),
            Mode::Visual => self.key_visual(key),
        }
    }

    /// Bracketed paste: typed as it is in Insert mode; in Normal mode inserted at the cursor
    /// (the cursor ends on the character that was under it, so pastes follow each other), in
    /// Visual mode in place of the selection. Outside Insert mode it is one undo step.
    pub fn paste(&mut self, text: &str) -> EdEvent {
        let norm = text.replace("\r\n", "\n").replace('\r', "\n");
        if norm.is_empty() {
            return EdEvent::None;
        }
        self.cmd = vim::Pending::default();
        let (a, b) = match self.mode {
            Mode::Insert => {
                self.ai_row = None;
                self.insert_at_cursor(&norm);
                return EdEvent::Changed { typed: None };
            }
            Mode::Normal => {
                let off = self.offset();
                (off, off)
            }
            Mode::Visual => {
                let span = match self.visual_target(false) {
                    vim::Target::Chars { a, b } => (a, b),
                    // The lines' text, their line breaks stay.
                    vim::Target::Lines { first, last } => {
                        (self.line_start(first), self.line_start(last) + self.lines[last].len())
                    }
                };
                self.mode = Mode::Normal;
                span
            }
        };
        self.snapshot();
        self.splice(a, b, &norm);
        self.set_cursor_offset(a + norm.len());
        self.want_x = None;
        self.clamp();
        EdEvent::Changed { typed: None }
    }

    /// Mouse click at a position relative to the editor's inner area.
    pub fn click(&mut self, x: u16, y: u16) {
        let r = (self.top + y as usize).min(self.lines.len() - 1);
        let tx = (x as usize).saturating_sub(self.gutter);
        let c = self.col_for_x(r, self.left + tx);
        self.cmd = vim::Pending::default();
        let from = self.row;
        self.set_pos(r, c);
        if self.mode == Mode::Visual {
            self.mode = Mode::Normal;
        }
        if self.mode == Mode::Insert {
            self.moved_in_insert(from);
        }
        self.clamp();
    }

    /// The text position (line, grapheme) at a point relative to the editor's inner area, as
    /// drawn last: the line on screen there (the first or last line past the text), the
    /// grapheme under the column (the second column of a wide one is that grapheme; past the
    /// line's end, its end). `y` may be above (negative) or below the area while dragging.
    pub fn pos_at(&self, x: u16, y: i32) -> (usize, usize) {
        let r = (self.top as i64 + i64::from(y)).clamp(0, self.lines.len() as i64 - 1) as usize;
        let tx = (x as usize).saturating_sub(self.gutter);
        (r, self.col_for_x(r, self.left + tx))
    }

    /// Mouse drag from `anchor` (where the button went down) to `to`: a Visual selection from
    /// one to the other (both ends included, grapheme by grapheme), the cursor at `to`. Back
    /// on the anchor it is no selection, just the cursor there.
    pub fn drag_select(&mut self, anchor: (usize, usize), to: (usize, usize)) {
        self.cmd = vim::Pending::default();
        let clamp_col = |e: &Self, (r, c): (usize, usize)| {
            let r = r.min(e.lines.len() - 1);
            (r, c.min(e.gcount(r).saturating_sub(1)))
        };
        let (anchor, to) = (clamp_col(self, anchor), clamp_col(self, to));
        if anchor == to {
            self.exit_visual();
            self.set_pos(to.0, to.1);
            return;
        }
        self.enter_visual(false, anchor);
        self.row = to.0;
        self.col = to.1;
        self.want_x = None;
    }

    /// Double click: select the word (a run of letters, digits and `_`, or of other symbols,
    /// or of blanks) at `at`.
    pub fn select_word(&mut self, at: (usize, usize)) {
        let gs = graphemes(&self.lines[at.0.min(self.lines.len() - 1)]);
        if gs.is_empty() {
            return self.drag_select(at, at);
        }
        let r = at.0.min(self.lines.len() - 1);
        let c = at.1.min(gs.len() - 1);
        let cls = class(gs[c]);
        let mut a = c;
        while a > 0 && class(gs[a - 1]) == cls {
            a -= 1;
        }
        let mut b = c;
        while b + 1 < gs.len() && class(gs[b + 1]) == cls {
            b += 1;
        }
        self.visual_span(r, a, b);
    }

    /// Visual from grapheme `a` to `b` (included) of line `r`, the cursor on `b`.
    fn visual_span(&mut self, r: usize, a: usize, b: usize) {
        self.cmd = vim::Pending::default();
        self.enter_visual(false, (r, a));
        self.row = r;
        self.col = b;
        self.want_x = None;
    }

    /// Triple click: select line `r` (its text, without the line break).
    pub fn select_line(&mut self, r: usize) {
        let r = r.min(self.lines.len() - 1);
        let n = self.gcount(r);
        if n == 0 {
            return self.drag_select((r, 0), (r, 0));
        }
        self.visual_span(r, 0, n - 1);
    }

    /// Lines shown by the last render.
    pub fn view_height(&self) -> usize {
        self.view_h
    }

    pub fn scroll(&mut self, delta: isize) {
        let max_top = self.lines.len().saturating_sub(1) as isize;
        self.top = (self.top as isize + delta).clamp(0, max_top) as usize;
        let h = self.view_h.max(1);
        if self.row < self.top {
            self.row = self.top;
        } else if self.row >= self.top + h {
            self.row = self.top + h - 1;
        }
        self.row = self.row.min(self.lines.len() - 1);
        self.clamp();
    }
}

#[cfg(test)]
mod tests;
