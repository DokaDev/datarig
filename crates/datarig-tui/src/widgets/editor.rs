//! SQL editor with a vim subset. The cursor moves by grapheme cluster and the
//! screen column is the sum of preceding grapheme widths, so the real terminal
//! cursor can be placed exactly for IME preedit.
//!
//! Keys arrive through the keymap (`crate::keymap`): Hangul typed in Normal/Visual mode has
//! already been turned into QWERTY keys, and app keys (run, commands, leader, …) never get here.
//!
//! Large files: the text is a vector of lines and every edit is a splice of
//! the lines it touches ([`Editor::splice`]); undo keeps what each command changed, not copies
//! of the text. Highlighting lexes only the lines on screen, from the nearest point where the
//! lexer's state is known (the state at each line start is cached, and edits invalidate it from
//! their line on). The statement under the cursor and the completion context are found in a
//! region of lines around the cursor that holds the `;` around it, with the same result as
//! splitting the whole text.

use crate::text::grapheme_width;
use crate::theme;
use datarig_core::i18n::Label;
use datarig_core::sql::lexer::{Tok, Token, lex};
use datarig_core::sql::split::{Statement, split, statement_at};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
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

/// One edit of the text: at byte `at`, `removed` was replaced with `inserted`.
#[derive(Clone)]
struct Change {
    at: usize,
    removed: String,
    inserted: String,
}

/// What one command changed (an Insert session, `x`, `dd`, a put, …), with the cursor before
/// it and where it was when it was undone.
#[derive(Clone)]
struct Step {
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
const UNDO_BYTES: usize = 64 << 20;

/// The lexer's state at the start of a line: between tokens, or inside a token that spans
/// lines (a block comment, a string, a dollar body, a quoted identifier) that starts at `line`,
/// `byte`, where lexing can start again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineState {
    Normal,
    Inside { line: usize, byte: usize },
}

/// Lines around the cursor searched first for the `;` around it.
const REGION_LINES: usize = 64;

/// Text versions, unique across editors: the same number means the same text of the same
/// editor.
static VERSIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_version() -> u64 {
    VERSIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Clone)]
struct Register {
    text: String,
    linewise: bool,
}

const TAB_WIDTH: usize = 4;

pub struct Editor {
    pub lines: Vec<String>,
    pub row: usize,
    pub col: usize,
    want_x: Option<usize>,
    pub mode: Mode,
    anchor: (usize, usize),
    pending: Option<char>,
    register: Option<Register>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    insert_snap: bool,
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

fn graphemes(s: &str) -> Vec<&str> {
    s.graphemes(true).collect()
}

fn gw(g: &str) -> usize {
    if g == "\t" { TAB_WIDTH } else { grapheme_width(g) }
}

fn class(g: &str) -> u8 {
    let c = g.chars().next().unwrap_or(' ');
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' || !c.is_ascii() {
        1
    } else {
        2
    }
}

impl Editor {
    pub fn new(text: &str) -> Self {
        let mut e = Self {
            lines: Vec::new(),
            row: 0,
            col: 0,
            want_x: None,
            mode: Mode::Normal,
            anchor: (0, 0),
            pending: None,
            register: None,
            undo: Vec::new(),
            redo: Vec::new(),
            insert_snap: false,
            top: 0,
            left: 0,
            view_h: 1,
            gutter: 0,
            bytes: text.len(),
            version: next_version(),
            states: Vec::new(),
            valid: 0,
            region: REGION_LINES,
        };
        e.lines = text.split('\n').map(str::to_string).collect();
        e
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

    fn gcount(&self, r: usize) -> usize {
        self.lines[r].graphemes(true).count()
    }

    fn byte_at(&self, r: usize, c: usize) -> usize {
        self.lines[r].graphemes(true).take(c).map(str::len).sum()
    }

    fn line_start(&self, r: usize) -> usize {
        self.lines[..r].iter().map(|l| l.len() + 1).sum()
    }

    fn offset_of(&self, r: usize, c: usize) -> usize {
        self.line_start(r) + self.byte_at(r, c)
    }

    /// Byte offset of the cursor in [`Editor::text`].
    pub fn offset(&self) -> usize {
        self.offset_of(self.row, self.col)
    }

    fn pos_of(&self, mut off: usize) -> (usize, usize) {
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
    fn pos_bytes(&self, mut off: usize) -> (usize, usize) {
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
    fn slice(&self, a: (usize, usize), b: (usize, usize)) -> String {
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
    fn splice(&mut self, a: usize, b: usize, s: &str) -> String {
        let removed = self.splice_raw(a, b, s);
        if self.undo.is_empty() {
            self.undo.push(Step { changes: Vec::new(), before: (self.row, self.col), after: (self.row, self.col) });
        }
        if let Some(step) = self.undo.last_mut() {
            step.changes.push(Change { at: a, removed: removed.clone(), inserted: s.to_string() });
        }
        removed
    }

    fn splice_raw(&mut self, a: usize, b: usize, s: &str) -> String {
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

    fn set_cursor_offset(&mut self, off: usize) {
        let (r, c) = self.pos_of(off.min(self.bytes));
        self.row = r;
        self.col = c;
    }

    fn display_x(&self, r: usize, c: usize) -> usize {
        self.lines[r].graphemes(true).take(c).map(gw).sum()
    }

    fn col_for_x(&self, r: usize, x: usize) -> usize {
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

    fn first_nonblank(&self, r: usize) -> usize {
        graphemes(&self.lines[r]).iter().position(|g| class(g) != 0).unwrap_or(0)
    }

    fn clamp(&mut self) {
        self.row = self.row.min(self.lines.len() - 1);
        let n = self.gcount(self.row);
        let max = if self.mode == Mode::Insert { n } else { n.saturating_sub(1) };
        self.col = self.col.min(max);
    }

    /// Start an undo step: the changes until the next one are undone together.
    fn snapshot(&mut self) {
        self.undo.push(Step { changes: Vec::new(), before: (self.row, self.col), after: (self.row, self.col) });
        let mut bytes: usize = self.undo.iter().map(Step::bytes).sum();
        while self.undo.len() > UNDO_STEPS || (bytes > UNDO_BYTES && self.undo.len() > 1) {
            bytes -= self.undo.remove(0).bytes();
        }
        self.redo.clear();
    }

    fn restore(&mut self, from_undo: bool) -> bool {
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

    fn enter_insert(&mut self, snapshot: bool) {
        if snapshot {
            self.snapshot();
        }
        self.insert_snap = true;
        self.mode = Mode::Insert;
        self.want_x = None;
    }

    fn leave_insert(&mut self) {
        if self.insert_snap && self.undo.last().is_some_and(|s| s.changes.is_empty()) {
            self.undo.pop();
        }
        self.insert_snap = false;
        self.mode = Mode::Normal;
        self.col = self.col.saturating_sub(1);
        self.clamp();
    }

    fn move_vert(&mut self, delta: isize) {
        let want = self.want_x.unwrap_or_else(|| self.display_x(self.row, self.col));
        self.want_x = Some(want);
        let max = self.lines.len() as isize - 1;
        self.row = (self.row as isize + delta).clamp(0, max) as usize;
        self.col = if want == usize::MAX { self.gcount(self.row) } else { self.col_for_x(self.row, want) };
        self.clamp();
    }

    fn set_pos(&mut self, r: usize, c: usize) {
        self.row = r;
        self.col = c;
        self.want_x = None;
        self.clamp();
    }

    fn word_forward(&mut self) {
        let (mut r, mut c) = (self.row, self.col);
        let gs = graphemes(&self.lines[r]);
        if c < gs.len() {
            let cls = class(gs[c]);
            if cls != 0 {
                while c < gs.len() && class(gs[c]) == cls {
                    c += 1;
                }
            }
        }
        loop {
            let gs = graphemes(&self.lines[r]);
            while c < gs.len() && class(gs[c]) == 0 {
                c += 1;
            }
            if c < gs.len() {
                break;
            }
            if r + 1 >= self.lines.len() {
                c = gs.len().saturating_sub(1);
                break;
            }
            r += 1;
            c = 0;
            if self.lines[r].is_empty() {
                break;
            }
        }
        self.set_pos(r, c);
    }

    fn word_back(&mut self) {
        let (mut r, mut c) = (self.row, self.col);
        loop {
            if c == 0 {
                if r == 0 {
                    self.set_pos(0, 0);
                    return;
                }
                r -= 1;
                c = self.gcount(r);
                if c == 0 {
                    self.set_pos(r, 0);
                    return;
                }
            }
            c -= 1;
            if class(graphemes(&self.lines[r])[c]) != 0 {
                break;
            }
        }
        let gs = graphemes(&self.lines[r]);
        let cls = class(gs[c]);
        while c > 0 && class(gs[c - 1]) == cls {
            c -= 1;
        }
        self.set_pos(r, c);
    }

    fn word_end(&mut self) {
        let (mut r, mut c) = (self.row, self.col + 1);
        loop {
            let n = self.gcount(r);
            if c < n && class(graphemes(&self.lines[r])[c]) != 0 {
                break;
            }
            if c < n {
                c += 1;
                continue;
            }
            if r + 1 >= self.lines.len() {
                self.set_pos(r, n.saturating_sub(1));
                return;
            }
            r += 1;
            c = 0;
        }
        let gs = graphemes(&self.lines[r]);
        let cls = class(gs[c]);
        while c + 1 < gs.len() && class(gs[c + 1]) == cls {
            c += 1;
        }
        self.set_pos(r, c);
    }

    /// Shared Normal/Visual motions. Returns true when `c` was a motion.
    fn motion(&mut self, c: char) -> bool {
        match c {
            'h' => self.set_pos(self.row, self.col.saturating_sub(1)),
            'l' => {
                let n = self.gcount(self.row);
                if self.col + 1 < n {
                    self.set_pos(self.row, self.col + 1);
                }
            }
            'j' => self.move_vert(1),
            'k' => self.move_vert(-1),
            'w' => self.word_forward(),
            'b' => self.word_back(),
            'e' => self.word_end(),
            '0' => self.set_pos(self.row, 0),
            '^' => self.set_pos(self.row, self.first_nonblank(self.row)),
            '$' => {
                self.set_pos(self.row, self.gcount(self.row));
                self.want_x = Some(usize::MAX);
            }
            'G' => {
                let r = self.lines.len() - 1;
                self.set_pos(r, self.first_nonblank(r));
            }
            'g' => self.pending = Some('g'),
            _ => return false,
        }
        true
    }

    fn arrow_motion(&mut self, code: KeyCode) -> bool {
        let c = match code {
            KeyCode::Left => 'h',
            KeyCode::Right => 'l',
            KeyCode::Up => 'k',
            KeyCode::Down => 'j',
            KeyCode::Home => '0',
            KeyCode::End => '$',
            _ => return false,
        };
        if self.mode == Mode::Insert {
            match c {
                'h' => self.set_pos(self.row, self.col.saturating_sub(1)),
                'l' => self.set_pos(self.row, self.col + 1),
                '0' => self.set_pos(self.row, 0),
                '$' => self.set_pos(self.row, self.gcount(self.row)),
                _ => self.move_vert(if c == 'j' { 1 } else { -1 }),
            }
            true
        } else {
            self.motion(c)
        }
    }

    fn visual_bounds(&self) -> (usize, usize) {
        let (a, b) = if self.anchor <= (self.row, self.col) {
            (self.anchor, (self.row, self.col))
        } else {
            ((self.row, self.col), self.anchor)
        };
        let lo = self.offset_of(a.0, a.1);
        let gs = graphemes(&self.lines[b.0]);
        let hi_start = self.offset_of(b.0, b.1);
        let extra = match gs.get(b.1) {
            Some(g) => g.len(),
            None if b.0 + 1 < self.lines.len() => 1,
            None => 0,
        };
        (lo, hi_start + extra)
    }

    /// Selected text in Visual mode (for Ctrl+E).
    pub fn selection(&self) -> Option<String> {
        (self.mode == Mode::Visual).then(|| {
            let (a, b) = self.visual_bounds();
            self.slice(self.pos_bytes(a), self.pos_bytes(b))
        })
    }

    pub fn exit_visual(&mut self) {
        if self.mode == Mode::Visual {
            self.mode = Mode::Normal;
            self.clamp();
        }
    }

    fn insert_at_cursor(&mut self, s: &str) {
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

    fn delete_range(&mut self, a: usize, b: usize) -> String {
        let removed = self.splice(a, b, "");
        self.set_cursor_offset(a);
        removed
    }

    /// Byte offset of the start of line `r` and of its end (before its line break).
    fn line_bounds(&self, r: usize) -> (usize, usize) {
        let start = self.line_start(r);
        (start, start + self.lines[r].len())
    }

    /// Bracketed paste: inserted verbatim in Insert mode.
    pub fn paste(&mut self, text: &str) -> EdEvent {
        if self.mode != Mode::Insert {
            return EdEvent::None;
        }
        let norm = text.replace("\r\n", "\n").replace('\r', "\n");
        self.insert_at_cursor(&norm);
        EdEvent::Changed { typed: None }
    }

    /// A command waits for its next key (`d…`, `y…`, `g…`).
    pub fn awaiting_key(&self) -> bool {
        self.pending.is_some()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> EdEvent {
        match self.mode {
            Mode::Insert => self.key_insert(key),
            Mode::Normal => self.key_normal(key),
            Mode::Visual => self.key_visual(key),
        }
    }

    fn key_insert(&mut self, key: KeyEvent) -> EdEvent {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.leave_insert();
                EdEvent::Moved
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                let mut buf = [0u8; 4];
                self.insert_at_cursor(c.encode_utf8(&mut buf));
                EdEvent::Changed { typed: Some(c) }
            }
            KeyCode::Enter => {
                self.insert_at_cursor("\n");
                EdEvent::Changed { typed: None }
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
            code if self.arrow_motion(code) => EdEvent::Moved,
            _ => EdEvent::None,
        }
    }

    fn key_normal(&mut self, key: KeyEvent) -> EdEvent {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl {
            self.pending = None;
            return match key.code {
                KeyCode::Char('r') => {
                    if self.restore(false) {
                        EdEvent::Changed { typed: None }
                    } else {
                        EdEvent::None
                    }
                }
                _ => EdEvent::None,
            };
        }
        let ch = match key.code {
            KeyCode::Char(c) => Some(c),
            _ => None,
        };
        if let Some(p) = self.pending.take() {
            return match (p, ch) {
                ('g', Some('g')) => {
                    self.set_pos(0, self.first_nonblank(0));
                    EdEvent::Moved
                }
                ('d', Some('d')) => {
                    self.snapshot();
                    let (start, end) = self.line_bounds(self.row);
                    let removed = self.lines[self.row].clone();
                    if self.row + 1 < self.lines.len() {
                        self.splice(start, end + 1, "");
                    } else if self.row > 0 {
                        self.splice(start - 1, end, "");
                    } else {
                        self.splice(start, end, "");
                    }
                    self.register = Some(Register { text: removed, linewise: true });
                    self.row = self.row.min(self.lines.len() - 1);
                    self.set_pos(self.row, self.first_nonblank(self.row));
                    EdEvent::Changed { typed: None }
                }
                ('y', Some('y')) => {
                    self.register = Some(Register { text: self.lines[self.row].clone(), linewise: true });
                    EdEvent::None
                }
                _ => EdEvent::None,
            };
        }
        if let Some(c) = ch {
            if self.motion(c) {
                return EdEvent::Moved;
            }
            return match c {
                'i' => {
                    self.enter_insert(true);
                    EdEvent::Moved
                }
                'a' => {
                    self.enter_insert(true);
                    if self.gcount(self.row) > 0 {
                        self.col += 1;
                    }
                    EdEvent::Moved
                }
                'I' => {
                    self.enter_insert(true);
                    self.col = self.first_nonblank(self.row);
                    EdEvent::Moved
                }
                'A' => {
                    self.enter_insert(true);
                    self.col = self.gcount(self.row);
                    EdEvent::Moved
                }
                'o' | 'O' => {
                    self.snapshot();
                    let (start, end) = self.line_bounds(self.row);
                    let at = if c == 'o' { self.row + 1 } else { self.row };
                    self.splice(if c == 'o' { end } else { start }, if c == 'o' { end } else { start }, "\n");
                    self.row = at;
                    self.col = 0;
                    self.enter_insert(false);
                    EdEvent::Changed { typed: None }
                }
                'x' => {
                    if self.gcount(self.row) == 0 {
                        return EdEvent::None;
                    }
                    self.snapshot();
                    let a = self.offset();
                    let b = self.offset_of(self.row, self.col + 1);
                    let removed = self.delete_range(a, b);
                    self.register = Some(Register { text: removed, linewise: false });
                    self.clamp();
                    EdEvent::Changed { typed: None }
                }
                'd' | 'y' => {
                    self.pending = Some(c);
                    EdEvent::None
                }
                'p' | 'P' => self.put(c == 'p'),
                'u' => {
                    if self.restore(true) {
                        EdEvent::Changed { typed: None }
                    } else {
                        EdEvent::None
                    }
                }
                'v' => {
                    self.mode = Mode::Visual;
                    self.anchor = (self.row, self.col);
                    EdEvent::Moved
                }
                _ => EdEvent::None,
            };
        }
        if self.arrow_motion(key.code) {
            return EdEvent::Moved;
        }
        EdEvent::None
    }

    fn put(&mut self, after: bool) -> EdEvent {
        let Some(reg) = self.register.clone() else { return EdEvent::None };
        self.snapshot();
        if reg.linewise {
            let at = if after { self.row + 1 } else { self.row };
            let (start, end) = self.line_bounds(self.row);
            if after {
                self.splice(end, end, &format!("\n{}", reg.text));
            } else {
                self.splice(start, start, &format!("{}\n", reg.text));
            }
            self.set_pos(at, self.first_nonblank(at));
        } else {
            let mut off = self.offset();
            if after && self.gcount(self.row) > 0 {
                off = self.offset_of(self.row, self.col + 1);
            }
            self.splice(off, off, &reg.text);
            let last_len = reg.text.graphemes(true).next_back().map(str::len).unwrap_or(0);
            self.set_cursor_offset(off + reg.text.len() - last_len);
            self.want_x = None;
            self.clamp();
        }
        EdEvent::Changed { typed: None }
    }

    fn key_visual(&mut self, key: KeyEvent) -> EdEvent {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return EdEvent::None;
        }
        if key.code == KeyCode::Esc {
            self.pending = None;
            self.exit_visual();
            return EdEvent::Moved;
        }
        let ch = match key.code {
            KeyCode::Char(c) => Some(c),
            _ => None,
        };
        if let Some('g') = self.pending.take() {
            if ch == Some('g') {
                self.set_pos(0, self.first_nonblank(0));
            }
            return EdEvent::Moved;
        }
        match ch {
            Some('y') => {
                let (a, b) = self.visual_bounds();
                self.register =
                    Some(Register { text: self.slice(self.pos_bytes(a), self.pos_bytes(b)), linewise: false });
                let lo = self.anchor.min((self.row, self.col));
                self.mode = Mode::Normal;
                self.set_pos(lo.0, lo.1);
                EdEvent::Moved
            }
            Some('d') | Some('x') => {
                let (a, b) = self.visual_bounds();
                self.mode = Mode::Normal;
                self.snapshot();
                let removed = self.delete_range(a, b);
                self.register = Some(Register { text: removed, linewise: false });
                self.clamp();
                EdEvent::Changed { typed: None }
            }
            Some('v') => {
                self.exit_visual();
                EdEvent::Moved
            }
            Some(c) if self.motion(c) => EdEvent::Moved,
            _ if self.arrow_motion(key.code) => EdEvent::Moved,
            _ => EdEvent::None,
        }
    }

    /// Where lexing can start for line `r`: its start, or the start of the token that spans
    /// into it (from the cached line states; [`Editor::ensure_states`] first).
    fn restart_of(&self, r: usize) -> (usize, usize) {
        match self.states.get(r) {
            Some(LineState::Inside { line, byte }) => (*line, *byte),
            _ => (r, 0),
        }
    }

    /// Make the lexer states of lines `..=upto` current, lexing from the last current one.
    fn ensure_states(&mut self, upto: usize) {
        let upto = upto.min(self.lines.len() - 1);
        if self.valid == 0 {
            self.states.clear();
            self.states.push(LineState::Normal);
            self.valid = 1;
        }
        if self.valid > upto {
            return;
        }
        let from = self.valid - 1;
        let (base_line, base_byte) = self.restart_of(from);
        let mut region = String::from(&self.lines[base_line][base_byte..]);
        // Where each line after `base_line` starts in `region`.
        let mut starts = Vec::with_capacity(upto - base_line);
        // One line more than asked for (when there is one): a token that spans into line `upto`
        // must not end where the region ends.
        let end = (upto + 1).min(self.lines.len() - 1);
        for r in base_line + 1..=end {
            region.push('\n');
            starts.push((r, region.len()));
            region.push_str(&self.lines[r]);
        }
        let toks = lex(&region);
        // The region's line starts back to (line, byte).
        let line_of = |off: usize| -> (usize, usize) {
            match starts.binary_search_by(|(_, s)| s.cmp(&off)) {
                Ok(i) => (starts[i].0, 0),
                Err(0) => (base_line, base_byte + off),
                Err(i) => (starts[i - 1].0, off - starts[i - 1].1),
            }
        };
        self.states.truncate(from + 1);
        let mut ti = 0;
        for &(r, at) in &starts {
            if r <= from || r > upto {
                continue;
            }
            while ti < toks.len() && toks[ti].end <= at {
                ti += 1;
            }
            let state = match toks.get(ti) {
                Some(t)
                    if t.start < at
                        && matches!(t.kind, Tok::BlockComment | Tok::Str | Tok::Dollar | Tok::QuotedIdent) =>
                {
                    let (line, byte) = line_of(t.start);
                    LineState::Inside { line, byte }
                }
                _ => LineState::Normal,
            };
            self.states.push(state);
        }
        self.valid = upto + 1;
    }

    /// The text of lines `first..last` from where lexing can start for `first`, and the byte
    /// offset in the whole text where it starts.
    fn region_text(&mut self, first: usize, last: usize) -> (usize, String) {
        let first = first.min(self.lines.len() - 1);
        let last = last.clamp(first + 1, self.lines.len());
        self.ensure_states(last - 1);
        let (line, byte) = self.restart_of(first);
        let base = self.line_start(line) + byte;
        let mut region = String::from(&self.lines[line][byte..]);
        for l in &self.lines[line + 1..last] {
            region.push('\n');
            region.push_str(l);
        }
        (base, region)
    }

    /// Lines `row - k ..= row + k` from where lexing can start: where they start in the text,
    /// the text, its tokens, the cursor in it and whether they reach the end of the text.
    fn region_around(&mut self, k: usize) -> (usize, String, Vec<Token>, usize, bool) {
        let cursor = self.offset();
        let n = self.lines.len();
        let first = self.row.saturating_sub(k);
        let last = (self.row + k + 1).min(n);
        let (base, region) = self.region_text(first, last);
        let toks = lex(&region);
        (base, region, toks, cursor - base, last == n)
    }

    /// The statement under the cursor as [`statement_at`] finds it in the split whole text: its
    /// byte range in the text (to the end of its `;`) and its body. Found in lines around the
    /// cursor, taking more lines until the answer cannot depend on text outside them.
    pub fn current_statement(&mut self) -> Option<(usize, usize, String)> {
        let mut k = self.region.max(1);
        loop {
            let (base, region, toks, c, to_end) = self.region_around(k);
            // The statement the cursor is in must end in the region.
            if to_end || toks.iter().any(|t| t.kind == Tok::Semi && t.start >= c) {
                let stmts: Vec<Statement> = split(&region);
                // A statement before the region's first `;` may have started before the region.
                let whole_from =
                    if base == 0 { 0 } else { toks.iter().find(|t| t.kind == Tok::Semi).map_or(usize::MAX, |t| t.end) };
                match statement_at(&stmts, c) {
                    Some(i) if stmts[i].start >= whole_from => {
                        let st = stmts[i];
                        return Some((base + st.start, base + st.end, st.body(&region).to_string()));
                    }
                    None if base == 0 => return None,
                    _ => {}
                }
            }
            k = k.saturating_mul(4);
        }
    }

    /// Text around the cursor for completion: where it starts in the whole text, the text and
    /// the cursor in it. It holds the `;`-delimited segment around the cursor whole (a `;`
    /// before the cursor or the start of the text, one after it or the end of the text).
    pub fn completion_context(&mut self) -> (usize, String, usize) {
        let mut k = self.region.max(1);
        loop {
            let (base, region, toks, c, to_end) = self.region_around(k);
            let before = base == 0 || toks.iter().any(|t| t.kind == Tok::Semi && t.start < c);
            let after = to_end || toks.iter().any(|t| t.kind == Tok::Semi && t.start >= c);
            if before && after {
                return (base, region, c);
            }
            k = k.saturating_mul(4);
        }
    }

    /// The identifier characters right before the cursor (on its line).
    pub fn ident_before_cursor(&self) -> &str {
        let line = &self.lines[self.row];
        let b = self.byte_at(self.row, self.col);
        let start = line[..b].char_indices().rev().take_while(|(_, c)| c.is_alphanumeric() || *c == '_').last();
        start.map_or("", |(i, _)| &line[i..b])
    }

    /// Mouse click at a position relative to the editor's inner area.
    pub fn click(&mut self, x: u16, y: u16) {
        let r = (self.top + y as usize).min(self.lines.len() - 1);
        let tx = (x as usize).saturating_sub(self.gutter);
        let c = self.col_for_x(r, self.left + tx);
        self.set_pos(r, c);
        if self.mode == Mode::Visual {
            self.mode = Mode::Normal;
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
        self.pending = None;
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
        if self.mode == Mode::Insert {
            self.leave_insert();
        }
        self.mode = Mode::Visual;
        self.anchor = anchor;
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
        self.pending = None;
        if self.mode == Mode::Insert {
            self.leave_insert();
        }
        self.mode = Mode::Visual;
        self.anchor = (r, a);
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

    /// Screen position of the cursor inside `area` if it were rendered now. `stmt` is the byte
    /// range of the statement a run would take: its lines get a faint tint and a bar in the
    /// gutter. With a Visual selection the bar marks the selection's lines instead (a run takes
    /// the selection), and `stmt` is not used.
    pub fn render(&mut self, area: Rect, buf: &mut Buffer, stmt: Option<(usize, usize)>) -> (u16, u16) {
        let h = area.height as usize;
        let numw = self.lines.len().to_string().len().max(3);
        self.gutter = numw + 1;
        self.view_h = h;
        let text_w = (area.width as usize).saturating_sub(self.gutter).max(1);
        if self.row < self.top {
            self.top = self.row;
        } else if self.row >= self.top + h {
            self.top = self.row + 1 - h;
        }
        let cx = self.display_x(self.row, self.col);
        if cx < self.left {
            self.left = cx;
        } else if cx >= self.left + text_w {
            self.left = cx + 1 - text_w;
        }

        // Tokens of the lines on screen, lexed from where the lexer's state is known.
        let last = (self.top + h).min(self.lines.len());
        let (base, region) = self.region_text(self.top, last);
        let toks = lex(&region);
        let is_fn: Vec<bool> = toks
            .iter()
            .enumerate()
            .map(|(i, t)| t.kind == Tok::Ident && toks.get(i + 1).is_some_and(|n| n.kind == Tok::LParen))
            .collect();
        let sel = (self.mode == Mode::Visual).then(|| self.visual_bounds());
        let stmt = if sel.is_some() { None } else { stmt };
        // What a run takes: the selection, else the statement under the cursor.
        let run = sel.or(stmt);

        let mut ti = 0;
        let mut line_start = self.line_start(self.top.min(self.lines.len()));
        // Where line `top` starts in `region`.
        let mut rstart = line_start - base;
        for vy in 0..h {
            let r = self.top + vy;
            let y = area.y + vy as u16;
            if r >= self.lines.len() {
                buf.set_style(Rect::new(area.x, y, area.width, 1), theme::base());
                continue;
            }
            let line = &self.lines[r];
            let line_end = line_start + line.len();
            let in_stmt = stmt.is_some_and(|(a, b)| line_start < b && a <= line_end);
            let bg = if r == self.row && self.mode != Mode::Visual {
                theme::CURSOR_LINE_BG
            } else if in_stmt {
                theme::CURRENT_STMT_BG
            } else {
                theme::BG
            };
            buf.set_style(Rect::new(area.x, y, area.width, 1), Style::new().bg(bg).fg(theme::FG));
            let num_fg = if r == self.row { theme::FG } else { theme::FG_MUTED };
            let num = format!("{:>numw$} ", r + 1);
            buf.set_stringn(area.x, y, &num, self.gutter, Style::new().fg(num_fg).bg(bg));
            if run.is_some_and(|(a, b)| line_start < b && a <= line_end) {
                // The bar sits in the blank between the line number and the text.
                let style = Style::new().fg(theme::CURRENT_STMT_BAR).bg(bg);
                buf.set_stringn(area.x + numw as u16, y, "▎", 1, style);
            }

            let tx0 = area.x + self.gutter as u16;
            let mut x = 0usize;
            let mut b = line_start;
            let mut rb = rstart;
            for g in line.graphemes(true) {
                let w = gw(g);
                while ti < toks.len() && toks[ti].end <= rb {
                    ti += 1;
                }
                let mut style = match toks.get(ti) {
                    Some(t) if t.start <= rb => theme::syntax(t.kind, is_fn[ti]),
                    _ => Style::new().fg(theme::FG),
                }
                .bg(bg);
                if sel.is_some_and(|(a, z)| b >= a && b < z) {
                    style = style.bg(theme::SELECTION_BG);
                }
                if x + w > self.left && x < self.left + text_w {
                    if x < self.left || x + w > self.left + text_w {
                        // partially visible wide grapheme: blank the visible part
                        let from = x.max(self.left);
                        let to = (x + w).min(self.left + text_w);
                        for px in from..to {
                            buf.set_stringn(tx0 + (px - self.left) as u16, y, " ", 1, style);
                        }
                    } else if g == "\t" {
                        buf.set_stringn(tx0 + (x - self.left) as u16, y, "    ", w, style);
                    } else {
                        buf.set_stringn(tx0 + (x - self.left) as u16, y, g, w, style);
                    }
                }
                x += w;
                b += g.len();
                rb += g.len();
            }
            if let Some((a, z)) = sel {
                // show selected line breaks as a one-cell highlight
                if line_end >= a && line_end < z && x >= self.left && x < self.left + text_w {
                    buf.set_stringn(tx0 + (x - self.left) as u16, y, " ", 1, Style::new().bg(theme::SELECTION_BG));
                }
            }
            line_start = line_end + 1;
            rstart += line.len() + 1;
        }
        let cy = area.y + (self.row - self.top) as u16;
        let cxs = area.x + self.gutter as u16 + (cx - self.left) as u16;
        (cxs.min(area.x + area.width.saturating_sub(1)), cy)
    }
}

#[cfg(test)]
mod tests;
