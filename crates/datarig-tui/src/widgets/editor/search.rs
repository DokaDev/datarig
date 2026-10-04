//! Search: `/` and `?` with a prompt on the editor's last line (the cursor previews the match
//! while the pattern is typed), `n` and `N`, `*` `#` (the word under the cursor, whole words)
//! and `g*` `g#` (also inside longer words), and the highlight of the matches on the lines on
//! screen until `:nohlsearch`.
//!
//! Patterns are Rust regular expressions (the `regex` crate's syntax, not Vim's): case
//! sensitive unless they start with `(?i)`, `\b` for a word boundary. A match lies within one
//! line. The work is bounded: a search searches each line at most once in a round of the
//! text, whatever the count (a miss searches the text once; a count that goes around is taken
//! modulo the matches of a round), and the highlight searches only the lines on screen.

use super::buffer::graphemes;
use super::motion::{Motion, Pos};
use super::{EdEvent, Editor, Mode, vim};
use crate::widgets::text_input::{InputResult, TextInput};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use regex::Regex;
use unicode_segmentation::UnicodeSegmentation;

/// The `/` or `?` prompt being typed.
pub(super) struct Prompt {
    pub(super) input: TextInput,
    pub(super) forward: bool,
    /// The cursor (with the column it wants) and the view when the prompt opened: `Esc` goes
    /// back there, and the preview searches from there.
    origin: (usize, usize, Option<usize>, usize, usize),
    /// The pattern typed so far, when it is a valid one (the preview highlights it).
    pub(super) preview: Option<Regex>,
}

/// The last search: its pattern and direction (`n` keeps it, `N` turns it around).
pub(super) struct Last {
    pub(super) re: Regex,
    forward: bool,
}

/// What a search has to say, for the app to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchNotice {
    /// It went past the end of the text (`bottom`) or its start, and on from the other end.
    Wrapped {
        bottom: bool,
    },
    NotFound(String),
    /// The pattern is not a valid regular expression: what is wrong with it.
    Invalid(String),
    /// `n`, `N` or an empty pattern without a search before.
    NoPrevious,
    /// `*` or `#` with nothing but blanks from the cursor to the end of the line.
    NoWord,
}

/// The work searches did since it was last taken: bytes a search searched (each line with its
/// line break), and lines a frame highlighted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchWork {
    pub bytes: usize,
    pub highlighted: usize,
}

/// A summary of a pattern error on one line: the regex crate's own message without the
/// pattern and the caret drawn under it.
pub(super) fn summary(e: &regex::Error) -> String {
    let s = e.to_string();
    match s.lines().rev().find_map(|l| l.trim().strip_prefix("error: ")) {
        Some(m) => m.to_string(),
        None => s.split_whitespace().collect::<Vec<_>>().join(" "),
    }
}

/// Bytes of the grapheme of `line` at byte `at` (1 at its end, as Vim counts it there).
fn glen(line: &str, at: usize) -> usize {
    line.get(at..).and_then(|t| t.graphemes(true).next()).map_or(1, str::len)
}

/// The matches of `line` that count, in order: the regex crate's matches from the start of the
/// line (each search after a match goes on from its end, as Vim with `cpo` `c` does), the ones
/// starting after the grapheme at `after` only (a match at the end of the line counts as on its
/// last grapheme, as in Vim), and each after the grapheme of the one before. Each match takes
/// one from `left`; the one that takes the last is returned. `taken`: how many counted; `any`:
/// whether the line has a match at all.
fn forward_line(
    line: &str,
    re: &Regex,
    after: Option<usize>,
    left: &mut usize,
    taken: &mut usize,
    any: &mut bool,
) -> Option<usize> {
    let mut after = after.map_or(-1, |a| a as isize);
    for m in re.find_iter(line) {
        *any = true;
        let s = m.start();
        if s as isize - isize::from(s == line.len()) < after {
            continue;
        }
        *taken += 1;
        *left -= 1;
        if *left == 0 {
            return Some(s);
        }
        after = (s + glen(line, s)) as isize;
    }
    None
}

/// The starts of the matches of `line` (as [`forward_line`] finds them) before byte `before`;
/// `any`: whether the line has a match at all.
fn backward_line(line: &str, re: &Regex, before: Option<usize>, any: &mut bool) -> Vec<usize> {
    let mut starts = Vec::new();
    for m in re.find_iter(line) {
        *any = true;
        if before.is_some_and(|b| m.start() >= b) {
            break;
        }
        starts.push(m.start());
    }
    starts
}

/// The `count`th match from byte `at` of line `row` (line, byte), forward or back, going on to
/// the other lines and around the text, and whether it went around. Each line is searched once
/// in a round of the text, the matches of a line one after the other in that one search (the
/// cursor's line again at the end of the round only when it has a match). A
/// count that goes all the way around is taken modulo the matches of a round, so any count
/// searches the text fewer than three times. `work` counts the bytes searched (with each
/// line's line break).
pub(super) fn find(
    lines: &[String],
    re: &Regex,
    (row, at): (usize, usize),
    forward: bool,
    count: usize,
    work: &mut usize,
) -> Option<((usize, usize), bool)> {
    let n = lines.len();
    let mut left = count.max(1);
    // The other lines in the order a search meets them, the cursor's last: (line, around).
    let order = |k: usize| if forward { ((row + k) % n, row + k >= n) } else { ((row + n - k) % n, k > row) };
    // One line, from where the search goes on: the match that takes the last of the count.
    let mut any = false;
    let mut search = |r: usize, from: Option<usize>, left: &mut usize, taken: &mut usize, any: &mut bool| {
        let line = lines[r].as_str();
        *work += line.len() + 1;
        if forward {
            forward_line(line, re, from, left, taken, any)
        } else {
            let starts = backward_line(line, re, from, any);
            *taken += starts.len();
            if starts.len() >= *left {
                let s = starts[starts.len() - *left];
                *left = 0;
                Some(s)
            } else {
                *left -= starts.len();
                None
            }
        }
    };
    let from = if forward { at + glen(&lines[row], at) } else { at };
    if let Some(s) = search(row, Some(from), &mut left, &mut 0, &mut any) {
        return Some(((row, s), false));
    }
    // The cursor's line is searched again at the end of a round only when it has a match.
    let rounds = if any { n } else { n - 1 };
    let mut round = 0;
    for k in 1..=rounds {
        let (r, around) = order(k);
        if let Some(s) = search(r, None, &mut left, &mut round, &mut false) {
            return Some(((r, s), around));
        }
    }
    if round == 0 {
        return None;
    }
    // A whole round counted `round` matches: the rest of the count is in the next one.
    left = (left - 1) % round + 1;
    (1..=rounds).find_map(|k| {
        let (r, _) = order(k);
        search(r, None, &mut left, &mut 0, &mut false).map(|s| ((r, s), true))
    })
}

/// The grapheme of `line` that byte `b` falls in (its end past the last one).
fn col_of(line: &str, b: usize) -> usize {
    if b >= line.len() {
        return line.graphemes(true).count();
    }
    line.grapheme_indices(true).take_while(|(i, _)| *i <= b).count().saturating_sub(1)
}

/// A keyword character for `*` and `#`: a letter, a digit or `_` (Unicode ones too).
fn keyword(g: &str) -> bool {
    g.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_')
}

impl Editor {
    /// The `/` or `?` prompt is open.
    pub fn searching(&self) -> bool {
        self.prompt.is_some()
    }

    /// What the last search had to say, for the app to show; once.
    pub fn take_search_notice(&mut self) -> Option<SearchNotice> {
        self.search_notice.take()
    }

    /// The work searches did since the last call (the counts start again).
    pub fn take_search_work(&mut self) -> SearchWork {
        std::mem::take(&mut self.search_work)
    }

    /// `:nohlsearch`: no highlight until the next search.
    pub fn clear_highlight(&mut self) {
        self.hl = false;
    }

    /// The pattern of the last search (the `"/` register), if any.
    pub fn search_pattern(&self) -> Option<&str> {
        self.last_search.as_ref().map(|l| l.re.as_str())
    }

    /// What the frame highlights: the pattern being typed, else the last search's while the
    /// highlight is on.
    pub(super) fn highlight_re(&self) -> Option<&Regex> {
        match &self.prompt {
            Some(p) if !p.input.text().is_empty() => p.preview.as_ref(),
            _ if self.hl => self.last_search.as_ref().map(|l| &l.re),
            _ => None,
        }
    }

    /// Open the prompt of `/` (`forward`) or `?`; the command typed so far (a count, an
    /// operator) waits for it.
    pub(super) fn open_prompt(&mut self, forward: bool) {
        let origin = (self.row, self.col, self.want_x, self.top, self.left);
        self.prompt = Some(Prompt { input: TextInput::default(), forward, origin, preview: None });
    }

    /// A key while the prompt is open: `Enter` searches, `Esc` (or `Backspace` on an empty
    /// pattern) gives up, `Ctrl+W` deletes the word before the cursor, any other key edits the
    /// pattern.
    pub(super) fn prompt_key(&mut self, key: KeyEvent) -> EdEvent {
        let Some(p) = self.prompt.as_mut() else { return EdEvent::None };
        match key.code {
            KeyCode::Esc => self.close_prompt(),
            KeyCode::Backspace if p.input.text().is_empty() => self.close_prompt(),
            KeyCode::Enter => self.prompt_enter(),
            _ => {
                let ctrl_w = key.code == KeyCode::Char('w') && key.modifiers.contains(KeyModifiers::CONTROL);
                let r = if ctrl_w { p.input.delete_word_back() } else { p.input.handle_key(&key) };
                if r == InputResult::Changed {
                    self.preview();
                }
                EdEvent::Moved
            }
        }
    }

    /// Text pasted into the prompt (part of the command `.` repeats).
    pub(super) fn prompt_paste(&mut self, text: &str) -> EdEvent {
        self.rec.prompt_paste(text);
        if let Some(p) = self.prompt.as_mut() {
            p.input.insert_str(text);
            self.preview();
        }
        EdEvent::Moved
    }

    /// Close the prompt without searching, as `Esc` does: the cursor and the view go back,
    /// the command that waited for it is dropped (the app does this before a key of its own
    /// and when the editor loses the keyboard).
    pub fn close_prompt(&mut self) -> EdEvent {
        if let Some(p) = self.prompt.take() {
            self.back_to(p.origin);
            self.cmd = vim::Pending::default();
        }
        EdEvent::Moved
    }

    fn back_to(&mut self, (row, col, want_x, top, left): (usize, usize, Option<usize>, usize, usize)) {
        (self.row, self.col, self.want_x, self.top, self.left) = (row, col, want_x, top, left);
    }

    /// The cursor on the match of the pattern typed so far (with the count typed before
    /// `/`), from where the prompt opened; back there when there is none. Nothing is said.
    fn preview(&mut self) {
        let Some(p) = self.prompt.as_mut() else { return };
        let origin = p.origin;
        p.preview = match p.input.text() {
            "" => None,
            t => Regex::new(t).ok(),
        };
        self.back_to(origin);
        if self.rec.replaying() {
            return;
        }
        let Some(p) = self.prompt.as_ref() else { return };
        let Some(re) = p.preview.as_ref() else { return };
        let (n, _) = self.cmd.count();
        let from = (origin.0, self.byte_at(origin.0, origin.1));
        if let Some(((r, b), _)) = find(&self.lines, re, from, p.forward, n, &mut self.search_work.bytes) {
            self.row = r;
            self.col = col_of(&self.lines[r], b);
            self.clamp();
        }
    }

    /// `Enter` in the prompt: the pattern (an empty one: the last search's) becomes the last
    /// search, and the cursor goes to its match as a motion (an operator waiting for it takes
    /// the text up to there).
    fn prompt_enter(&mut self) -> EdEvent {
        let Some(p) = self.prompt.take() else { return EdEvent::None };
        self.back_to(p.origin);
        let re = match p.input.text() {
            "" => self.last_search.as_ref().map(|l| l.re.clone()).ok_or(SearchNotice::NoPrevious),
            t => Regex::new(t).map_err(|e| SearchNotice::Invalid(summary(&e))),
        };
        let re = match re {
            Ok(re) => re,
            Err(n) => {
                self.search_notice = Some(n);
                self.cmd = vim::Pending::default();
                return EdEvent::Moved;
            }
        };
        self.set_search(re, p.forward);
        let m = Motion::Search { forward: p.forward, from: None };
        if self.mode == Mode::Visual {
            let (n, explicit) = self.cmd.count();
            self.end_command();
            self.marks.set_pc((self.row, self.col));
            self.move_by(m, n, explicit);
            return EdEvent::Moved;
        }
        self.motion_key(m)
    }

    /// Make `re` the last search (and the `"/` register); the highlight comes back on.
    fn set_search(&mut self, re: Regex, forward: bool) {
        self.regs.set_search(re.as_str().to_string());
        self.last_search = Some(Last { re, forward });
        self.hl = true;
    }

    /// `n` (`again`) or `N`: the last search in its direction or the other way.
    pub(super) fn search_again(&mut self, same: bool) -> Option<Motion> {
        match &self.last_search {
            Some(l) => Some(Motion::Search { forward: l.forward == same, from: None }),
            None => {
                self.search_notice = Some(SearchNotice::NoPrevious);
                None
            }
        }
    }

    /// `*` (`forward`) and `#`, `g*` and `g#` (not `whole`): the keyword under or after the
    /// cursor on its line (else the run of other non-blanks there) becomes the last search,
    /// as whole words with `whole` (`\b` around a keyword), and the search starts from its
    /// first character.
    pub(super) fn search_word(&mut self, forward: bool, whole: bool) -> Option<Motion> {
        let gs = graphemes(&self.lines[self.row]);
        let col = self.col.min(gs.len());
        let run = |pick: &dyn Fn(&str) -> bool| {
            let mut a = (col..gs.len()).find(|&i| pick(gs[i]))?;
            if a == col {
                while a > 0 && pick(gs[a - 1]) {
                    a -= 1;
                }
            }
            let b = (a..gs.len()).find(|&i| !pick(gs[i])).unwrap_or(gs.len());
            Some((a, b))
        };
        let (word, (a, b)) = match run(&keyword) {
            Some(r) => (true, r),
            None => match run(&|g: &str| !keyword(g) && !g.chars().all(char::is_whitespace)) {
                Some(r) => (false, r),
                None => {
                    self.search_notice = Some(SearchNotice::NoWord);
                    return None;
                }
            },
        };
        let text = regex::escape(&gs[a..b].concat());
        let pattern = if word && whole { format!(r"\b{text}\b") } else { text };
        // An escaped literal within `\b` always compiles; the size limit is the only way out.
        match Regex::new(&pattern) {
            Ok(re) => self.set_search(re, forward),
            Err(e) => {
                self.search_notice = Some(SearchNotice::Invalid(summary(&e)));
                return None;
            }
        }
        Some(Motion::Search { forward, from: Some(a) })
    }

    /// Where the last search leads from the cursor (or from column `from` of its line),
    /// `count` matches on; `None` (and a notice) when it matches nowhere. Going around the end
    /// of the text is said.
    pub(super) fn search_target(&mut self, forward: bool, from: Option<usize>, count: usize) -> Option<Pos> {
        let re = &self.last_search.as_ref()?.re;
        let at = (self.row, self.byte_at(self.row, from.unwrap_or(self.col)));
        self.hl = true;
        match find(&self.lines, re, at, forward, count, &mut self.search_work.bytes) {
            Some(((r, b), wrapped)) => {
                if wrapped {
                    self.search_notice = Some(SearchNotice::Wrapped { bottom: forward });
                }
                Some((r, col_of(&self.lines[r], b)))
            }
            None => {
                self.search_notice = Some(SearchNotice::NotFound(re.as_str().to_string()));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests;
