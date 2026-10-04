//! Repeating: `.` replays the last change, recorded as the keys that made it (its count
//! apart, which `N.` replaces, except for a Visual mode operator: Vim keeps its count) and the
//! Insert session it started; a count before `i a I A o O`
//! types the session's text that many times. A change that named a numbered register takes
//! the next one when repeated (`"1p` then `.` puts `"2`, as Vim does).

use super::motion::Motion;
use super::pairs::Pair;
use super::{EdEvent, Editor, Mode, Sel};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One input of a recorded change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Input {
    Key(KeyEvent),
    /// A paste from the terminal in Insert mode, or the text `Ctrl+R` put there.
    Paste(String),
    /// What auto-pairs did for the key typed there.
    Pair(Pair),
    /// The selection a Visual mode operator took, as much text from the cursor again: `lines`
    /// lines (whole with `by_line`), and on one line `cols` graphemes, on more the last line
    /// up to column `cols`; `eol`: with the line break of the last line.
    Select {
        lines: usize,
        cols: usize,
        by_line: bool,
        eol: bool,
    },
    /// The block a Visual mode operator took, as large a block from the cursor again: `lines`
    /// lines, `width` screen columns (`usize::MAX`: to the end of each line, after `$`).
    Block {
        lines: usize,
        width: usize,
    },
}

impl Input {
    /// The selection a Visual mode operator took.
    fn is_selection(&self) -> bool {
        matches!(self, Input::Select { .. } | Input::Block { .. })
    }
}

/// The last change: its inputs and its count (0: none); `clipboard`: it puts the system
/// clipboard's text (`"+p`), which the app reads again for `.`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Change {
    inputs: Vec<Input>,
    count: usize,
    clipboard: bool,
}

impl Change {
    /// A numbered register the change names, `"1`–`"8`, becomes the next one.
    fn next_numbered(&mut self) {
        let key = |c: char| Input::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        let at = usize::from(self.inputs.first().is_some_and(Input::is_selection)).min(self.inputs.len());
        if let [q, Input::Key(k), ..] = &mut self.inputs[at..]
            && *q == key('"')
            && let KeyCode::Char(d @ '1'..='8') = k.code
        {
            k.code = KeyCode::Char((d as u8 + 1) as char);
        }
    }
}

/// What is being recorded for `.`.
#[derive(Debug, Default)]
pub(super) struct Recorder {
    /// The inputs of the command being typed (counts left out), then of its Insert session.
    inputs: Vec<Input>,
    count: usize,
    /// The command started an Insert session, recorded with it until it ends.
    inserting: bool,
    /// The command is not a change to repeat (`u`, `Ctrl+R`, `.`).
    skip: bool,
    /// The command is a change to repeat even though the text stayed as it was (`gc` over
    /// blank lines, as Neovim records it).
    unchanged: bool,
    /// A replay is running: nothing is recorded.
    replaying: bool,
    /// The command puts the system clipboard's text.
    clipboard: bool,
    /// Where the inputs of the `Ctrl+R` being typed start.
    reg_from: Option<usize>,
    last: Option<Change>,
}

impl Recorder {
    pub(super) fn skip(&mut self) {
        self.skip = true;
    }

    /// The command repeats with `.` even if it changed nothing.
    pub(super) fn repeat_unchanged(&mut self) {
        self.unchanged = true;
    }

    /// Record the selection a Visual mode operator takes, before its keys.
    pub(super) fn select(&mut self, sel: Input) {
        if !self.replaying {
            self.inputs.insert(0, sel);
        }
    }

    /// Record a paste from the terminal in the Insert session.
    pub(super) fn paste(&mut self, text: &str) {
        if self.inserting && !self.replaying {
            self.inputs.push(Input::Paste(text.to_string()));
        }
    }

    /// Text pasted into the search prompt of the command being typed (`d/`).
    pub(super) fn prompt_paste(&mut self, text: &str) {
        if !self.replaying {
            self.inputs.push(Input::Paste(text.to_string()));
        }
    }

    /// Auto-pairs did `p` for the key just recorded: `.` does `p` again in place of the key.
    pub(super) fn pair(&mut self, p: Pair) {
        if self.inserting
            && !self.replaying
            && let Some(last @ Input::Key(_)) = self.inputs.last_mut()
        {
            *last = Input::Pair(p);
        }
    }

    /// `Ctrl+R` and the register's name put `text` in the Insert session: `.` types the text
    /// again, not what the register holds then (Vim).
    pub(super) fn register_text(&mut self, text: &str) {
        if let Some(n) = self.reg_from.take()
            && self.inserting
            && !self.replaying
        {
            self.inputs.truncate(n);
            self.inputs.push(Input::Paste(text.to_string()));
        }
    }

    /// `Ctrl+R` was typed (its key is the last input): the keys up to the register's name
    /// give way to the text it puts.
    pub(super) fn register_wait(&mut self) {
        self.reg_from = (self.inserting && !self.replaying).then(|| self.inputs.len().saturating_sub(1));
    }

    /// A replay of `.` is running.
    pub(super) fn replaying(&self) -> bool {
        self.replaying
    }

    /// The command puts the system clipboard's text.
    pub(super) fn put_clipboard(&mut self) {
        if !self.replaying {
            self.clipboard = true;
        }
    }

    /// `.` would put the system clipboard's text.
    pub(super) fn puts_clipboard(&self) -> bool {
        self.last.as_ref().is_some_and(|c| c.clipboard)
    }

    /// The cursor moved in Insert mode without typing: what is typed from here on is what
    /// `.` inserts (Vim).
    pub(super) fn moved_in_insert(&mut self) {
        if self.inserting && !self.replaying {
            self.inputs = vec![Input::Key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE))];
            self.count = 0;
        }
    }
}

/// A count before `i a I A o O`: what the session types goes in `count` times, on new lines
/// for `o` and `O`.
#[derive(Debug)]
pub(super) struct InsertRepeat {
    count: usize,
    lines: bool,
    inputs: Vec<Input>,
}

impl InsertRepeat {
    pub(super) fn new(count: usize, lines: bool) -> Self {
        InsertRepeat { count, lines, inputs: Vec::new() }
    }
}

impl Editor {
    /// A key, recorded for `.` when it is part of a change.
    pub(super) fn recorded_key(&mut self, key: KeyEvent) -> EdEvent {
        if self.rec.replaying {
            return self.dispatch_key(key);
        }
        if self.mode == Mode::Insert {
            if self.rec.inserting {
                if Motion::of_key(key.code).is_some() {
                    self.rec.moved_in_insert();
                } else {
                    self.rec.inputs.push(Input::Key(key));
                }
            }
            let ev = self.dispatch_key(key);
            if self.mode != Mode::Insert && self.rec.inserting {
                self.rec.inserting = false;
                self.save_change();
            }
            return ev;
        }
        if self.cmd.is_empty() {
            self.rec.inputs.clear();
            self.rec.clipboard = false;
            self.rec.skip = false;
            self.rec.unchanged = false;
            // An Insert session left without a key of its own (a mouse drag) is over.
            self.rec.inserting = false;
        }
        let before = self.cmd;
        let (n, explicit) = self.cmd.count();
        let version = self.version;
        let ev = self.dispatch_key(key);
        if !self.cmd.counted(&before) {
            self.rec.inputs.push(Input::Key(key));
        }
        if self.cmd.is_empty() && !self.rec.skip {
            self.rec.count = if explicit { n } else { 0 };
            if self.mode == Mode::Insert {
                self.rec.inserting = true;
            } else if self.version != version || self.rec.unchanged {
                self.save_change();
            }
        }
        ev
    }

    fn save_change(&mut self) {
        let inputs = std::mem::take(&mut self.rec.inputs);
        self.rec.last = Some(Change { inputs, count: self.rec.count, clipboard: self.rec.clipboard });
    }

    /// A key in Insert mode for the count of the command that started it: kept, and at `Esc`
    /// what was typed goes in again.
    pub(super) fn insert_repeat_key(&mut self, key: KeyEvent) {
        let Some(rep) = self.ins_repeat.as_mut() else { return };
        if Motion::of_key(key.code).is_some() {
            self.ins_repeat = None;
        } else if key.code != KeyCode::Esc || self.ins_reg {
            rep.inputs.push(Input::Key(key));
        } else if let Some(rep) = self.ins_repeat.take() {
            // Typed again, not recorded again (for `.` or `".`).
            let (replaying, typed) = (self.rec.replaying, self.ins_text.clone());
            self.rec.replaying = true;
            for _ in 1..rep.count {
                if rep.lines {
                    // On a new line below, the one just typed losing its indent if that is all
                    // there is on it.
                    self.drop_autoindent();
                    let at = self.open_line_at(true);
                    (self.row, self.col) = at;
                    self.ai_row = (at.1 > 0).then_some(at.0);
                }
                for i in &rep.inputs {
                    match i {
                        Input::Key(k) => {
                            self.key_insert(*k);
                        }
                        Input::Paste(s) => {
                            self.insert_at_cursor(s);
                        }
                        Input::Pair(p) => self.apply_pair(*p),
                        Input::Select { .. } | Input::Block { .. } => {}
                    }
                }
            }
            self.rec.replaying = replaying;
            self.ins_text = typed;
        }
    }

    /// Auto-pairs did `p` for the key just typed: recorded in its place for `.` and the count.
    pub(super) fn record_pair(&mut self, p: Pair) {
        self.rec.pair(p);
        if let Some(rep) = self.ins_repeat.as_mut()
            && let Some(last @ Input::Key(_)) = rep.inputs.last_mut()
        {
            *last = Input::Pair(p);
        }
    }

    /// A paste in Insert mode, for the count of the command that started it.
    pub(super) fn insert_repeat_paste(&mut self, text: &str) {
        if let Some(rep) = self.ins_repeat.as_mut() {
            rep.inputs.push(Input::Paste(text.to_string()));
        }
    }

    /// `.`: the last change again, with `count` in place of its own when one is typed (and
    /// kept for the next `.`).
    pub(super) fn dot(&mut self, count: Option<usize>) -> EdEvent {
        self.rec.skip();
        let Some(mut change) = self.rec.last.clone() else { return EdEvent::None };
        if let Some(n) = count
            && !change.inputs.first().is_some_and(Input::is_selection)
        {
            change.count = n;
        }
        change.next_numbered();
        let version = self.version;
        self.rec.replaying = true;
        let key = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        if change.count > 0 {
            for c in change.count.to_string().chars() {
                self.dispatch_key(key(c));
            }
        }
        for i in &change.inputs {
            match i {
                Input::Key(k) => {
                    self.dispatch_key(*k);
                }
                Input::Paste(s) => {
                    self.paste(s);
                }
                Input::Pair(p) => self.apply_pair(*p),
                Input::Select { lines, cols, by_line, eol } => self.select_again(*lines, *cols, *by_line, *eol),
                Input::Block { lines, width } => self.select_block_again(*lines, *width),
            }
        }
        if self.mode == Mode::Insert {
            self.dispatch_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        }
        self.redo_width = None;
        self.rec.replaying = false;
        self.rec.last = Some(change);
        if self.version != version { EdEvent::Changed { typed: None } } else { EdEvent::Moved }
    }

    /// What a Visual mode operator records of its selection.
    pub(super) fn selection_input(&self) -> Input {
        if self.sel == Sel::Block {
            let b = self.block();
            let width = if b.max { usize::MAX } else { b.end - b.start + 1 };
            return Input::Block { lines: b.last - b.first + 1, width };
        }
        let (a, b) = if self.anchor <= (self.row, self.col) {
            (self.anchor, (self.row, self.col))
        } else {
            ((self.row, self.col), self.anchor)
        };
        let lines = b.0 - a.0 + 1;
        let cols = if lines == 1 { b.1 - a.1 + 1 } else { b.1 };
        let eol = b == (self.row, self.col) && self.want_x == Some(usize::MAX);
        Input::Select { lines, cols, by_line: self.sel == Sel::Lines, eol }
    }

    /// Visual mode over as much text from the cursor as [`Input::Select`] says.
    fn select_again(&mut self, lines: usize, cols: usize, by_line: bool, eol: bool) {
        let cmd = self.cmd;
        self.enter_visual(if by_line { Sel::Lines } else { Sel::Chars }, (self.row, self.col));
        self.cmd = cmd;
        let r = (self.row + lines - 1).min(self.lines.len() - 1);
        let len = self.gcount(r);
        let c = if lines == 1 { self.col + cols - 1 } else { cols };
        self.row = r;
        self.col = c.min(len.saturating_sub(1));
        self.want_x = (eol || c >= len).then_some(usize::MAX);
    }

    /// A block as [`Input::Block`] says, from the cursor: its left column is the cursor's.
    fn select_block_again(&mut self, lines: usize, width: usize) {
        let cmd = self.cmd;
        self.enter_visual(Sel::Block, (self.row, self.col));
        self.cmd = cmd;
        self.row = (self.row + lines - 1).min(self.lines.len() - 1);
        self.clamp();
        self.want_x = (width == usize::MAX).then_some(usize::MAX);
        self.redo_width = Some(width);
    }
}

#[cfg(test)]
mod tests;
