//! Repeating: `.` replays the last change, recorded as the keys that made it (its count
//! apart, which `N.` replaces) and the Insert session it started; a count before `i a I A o O`
//! types the session's text that many times. A change that named a numbered register takes
//! the next one when repeated (`"1p` then `.` puts `"2`, as Vim does).

use super::motion::Motion;
use super::{EdEvent, Editor, Mode};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One input of a recorded change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Input {
    Key(KeyEvent),
    /// A paste from the terminal in Insert mode, or the text `Ctrl+R` put there.
    Paste(String),
    /// The selection a Visual mode operator took, as much text from the cursor again: `lines`
    /// lines (whole with `by_line`), and on one line `cols` graphemes, on more the last line
    /// up to column `cols`; `eol`: with the line break of the last line.
    Select {
        lines: usize,
        cols: usize,
        by_line: bool,
        eol: bool,
    },
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
        let at = usize::from(matches!(self.inputs.first(), Some(Input::Select { .. }))).min(self.inputs.len());
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
            } else if self.version != version {
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
                        Input::Select { .. } => {}
                    }
                }
            }
            self.rec.replaying = replaying;
            self.ins_text = typed;
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
        if let Some(n) = count {
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
                Input::Select { lines, cols, by_line, eol } => self.select_again(*lines, *cols, *by_line, *eol),
            }
        }
        if self.mode == Mode::Insert {
            self.dispatch_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        }
        self.rec.replaying = false;
        self.rec.last = Some(change);
        if self.version != version { EdEvent::Changed { typed: None } } else { EdEvent::Moved }
    }

    /// What a Visual mode operator records of its selection.
    pub(super) fn selection_input(&self) -> Input {
        let (a, b) = if self.anchor <= (self.row, self.col) {
            (self.anchor, (self.row, self.col))
        } else {
            ((self.row, self.col), self.anchor)
        };
        let lines = b.0 - a.0 + 1;
        let cols = if lines == 1 { b.1 - a.1 + 1 } else { b.1 };
        let eol = b == (self.row, self.col) && self.want_x == Some(usize::MAX);
        Input::Select { lines, cols, by_line: self.visual_lines, eol }
    }

    /// Visual mode over as much text from the cursor as [`Input::Select`] says.
    fn select_again(&mut self, lines: usize, cols: usize, by_line: bool, eol: bool) {
        let cmd = self.cmd;
        self.enter_visual(by_line, (self.row, self.col));
        self.cmd = cmd;
        let r = (self.row + lines - 1).min(self.lines.len() - 1);
        let len = self.gcount(r);
        let c = if lines == 1 { self.col + cols - 1 } else { cols };
        self.row = r;
        self.col = c.min(len.saturating_sub(1));
        self.want_x = (eol || c >= len).then_some(usize::MAX);
    }
}

#[cfg(test)]
mod tests;
