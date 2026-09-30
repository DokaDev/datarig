//! Edits other than deletes, changes and puts: `r` (replace characters), `J` and `gJ` (join
//! lines), `~` and the case operators `gu` `gU` `g~`, and the indent operators `>` `<`.

use super::buffer::indent_of;
use super::{EdEvent, Editor, Mode, TAB_WIDTH};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

/// How the case operators change a character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Case {
    Lower,
    Upper,
    Toggle,
}

/// The one character a case mapping gives, if it gives one (a letter whose other case is two
/// letters stays as it is).
fn single(mut it: impl Iterator<Item = char>) -> Option<char> {
    let c = it.next()?;
    it.next().is_none().then_some(c)
}

/// `s` with the case of its letters changed (`ß` upper-cased is `ẞ`, as in Vim).
pub(super) fn recase(s: &str, case: Case) -> String {
    s.chars()
        .map(|c| {
            let m = match case {
                Case::Lower => single(c.to_lowercase()),
                Case::Upper | Case::Toggle if c == 'ß' => Some('ẞ'),
                Case::Upper => single(c.to_uppercase()),
                Case::Toggle if c.is_lowercase() => single(c.to_uppercase()),
                Case::Toggle if c.is_uppercase() => single(c.to_lowercase()),
                Case::Toggle => None,
            };
            m.unwrap_or(c)
        })
        .collect()
}

/// Display width of an indent, tabs to the next multiple of [`TAB_WIDTH`].
fn indent_width(indent: &str) -> usize {
    indent.chars().fold(0, |w, c| if c == '\t' { w + TAB_WIDTH - w % TAB_WIDTH } else { w + 1 })
}

impl Editor {
    /// `r{ch}`: the `n` graphemes from the cursor become `ch`, the cursor on the last one; a
    /// line break replaces them all with one (and the new line gets the indent). Nothing
    /// when the line has fewer.
    pub(super) fn replace_chars(&mut self, ch: char, n: usize) -> EdEvent {
        let (r, c) = (self.row, self.col);
        if c + n > self.gcount(r) {
            return EdEvent::None;
        }
        self.snapshot();
        let a = self.offset();
        let b = self.offset_of(r, c + n);
        if ch == '\n' {
            self.delete_range(a, b);
            // As Enter typed in Insert mode, then Esc.
            self.mode = Mode::Insert;
            self.key_insert(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            self.ai_row = None;
            self.mode = Mode::Normal;
            self.col = self.col.saturating_sub(1);
            self.clamp();
        } else {
            // A tab goes in as Tab types it: spaces.
            let with = if ch == '\t' { "    ".repeat(n) } else { ch.to_string().repeat(n) };
            self.splice(a, b, &with);
            self.set_pos(r, c + with.graphemes(true).count() - 1);
        }
        EdEvent::Changed { typed: None }
    }

    /// Visual `r{ch}`: every grapheme of the selection (not its line breaks) becomes `ch`.
    pub(super) fn replace_span(&mut self, ch: char, (a, b): (usize, usize), to: (usize, usize)) -> EdEvent {
        let (ra, ba) = self.pos_bytes(a);
        let (rb, bb) = self.pos_bytes(b);
        let old = self.slice((ra, ba), (rb, bb));
        let new: String = old
            .split('\n')
            .map(|l| if ch == '\n' { l.to_string() } else { ch.to_string().repeat(l.graphemes(true).count()) })
            .collect::<Vec<_>>()
            .join("\n");
        self.mode = Mode::Normal;
        if new != old {
            self.snapshot();
            self.splice(a, b, &new);
        }
        self.set_pos(to.0, to.1);
        EdEvent::Changed { typed: None }
    }

    /// `J` (`spaces`) or `gJ`: join `n` lines from the cursor's (at least two; as many as
    /// there are). `J` drops the next lines' indent and puts one space between (none after a
    /// blank, before a `)` or next to an empty line). The cursor goes where the last line
    /// was joined.
    pub(super) fn join_lines(&mut self, first: usize, n: usize, spaces: bool) -> EdEvent {
        let last = (first + n.max(2) - 1).min(self.lines.len() - 1);
        if last == first {
            return EdEvent::None;
        }
        self.snapshot();
        let mut joined = self.lines[first].clone();
        let mut col = 0;
        for r in first + 1..=last {
            let next = &self.lines[r];
            let next = if spaces { next.trim_start_matches([' ', '\t']) } else { next.as_str() };
            let mut sep = "";
            if spaces && !next.is_empty() && !joined.is_empty() && !next.starts_with(')') {
                match joined.chars().next_back() {
                    Some(' ') | Some('\t') => {}
                    _ => sep = " ",
                }
            }
            col = joined.len();
            joined.push_str(sep);
            joined.push_str(next);
        }
        let (a, _) = self.line_bounds(first);
        let (_, b) = self.line_bounds(last);
        self.splice(a, b, &joined);
        let c = self.lines[first][..col].graphemes(true).count();
        self.set_pos(first, c);
        EdEvent::Changed { typed: None }
    }

    /// `~`: toggle the case of `n` graphemes from the cursor, which goes past them (at most
    /// to the last grapheme).
    pub(super) fn tilde(&mut self, n: usize) -> EdEvent {
        let (r, c) = (self.row, self.col);
        let len = self.gcount(r);
        if len == 0 {
            return EdEvent::None;
        }
        let end = (c + n).min(len);
        let (a, b) = (self.offset(), self.offset_of(r, end));
        let old = self.slice(self.pos_bytes(a), self.pos_bytes(b));
        let new = recase(&old, Case::Toggle);
        if new != old {
            self.snapshot();
            self.splice(a, b, &new);
        }
        self.set_pos(r, end);
        EdEvent::Changed { typed: None }
    }

    /// The case operators on bytes `a..b` of the text, the cursor then at `to`.
    pub(super) fn recase_span(&mut self, case: Case, (a, b): (usize, usize), to: (usize, usize)) -> EdEvent {
        let old = self.slice(self.pos_bytes(a), self.pos_bytes(b));
        let new = recase(&old, case);
        if new != old {
            self.snapshot();
            self.splice(a, b, &new);
        }
        self.set_pos(to.0, to.1);
        EdEvent::Changed { typed: None }
    }

    /// `>` / `<` (`right`) on lines `first..=last`, `times` times: each non-empty line's indent
    /// grows or shrinks by [`TAB_WIDTH`] columns and is written with spaces, as Tab types
    /// them. The cursor goes to the first line's first non-blank.
    pub(super) fn shift_lines(&mut self, first: usize, last: usize, times: usize, right: bool) -> EdEvent {
        let step = TAB_WIDTH.saturating_mul(times);
        let mut changed = false;
        for r in first..=last {
            let line = &self.lines[r];
            if line.is_empty() {
                continue;
            }
            let indent = indent_of(line).to_string();
            let w = indent_width(&indent);
            let w = if right { w.saturating_add(step) } else { w.saturating_sub(step) };
            let new = " ".repeat(w);
            if new != indent {
                if !changed {
                    self.snapshot();
                    changed = true;
                }
                let len = indent.len();
                let a = self.line_start(r);
                self.splice(a, a + len, &new);
            }
        }
        self.set_pos(first, self.first_nonblank(first));
        if changed { EdEvent::Changed { typed: None } } else { EdEvent::Moved }
    }
}

#[cfg(test)]
mod tests;
