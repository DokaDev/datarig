//! Single-line text input used by the command line and the profile form.
//!
//! Same rules as the editor: the cursor moves by grapheme cluster, the screen
//! column is the sum of preceding grapheme widths, and [`TextInput::render`] returns where the
//! real terminal cursor belongs so IME preedit shows up in place. It remembers where and how it
//! was drawn, so [`TextInput::click`] puts the cursor under the pointer. There is no selection.

use crate::text::grapheme_width;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputResult {
    Changed,
    Moved,
    Ignored,
}

#[derive(Clone, Debug, Default)]
pub struct TextInput {
    text: String,
    /// Cursor position in graphemes.
    cursor: usize,
    /// First visible display column.
    scroll: usize,
    /// Where the last render drew it, and what it masked (for the mouse).
    drawn: Rect,
    drawn_mask: bool,
    drawn_hide: Option<(usize, usize)>,
}

pub const MASK: &str = "•";

impl TextInput {
    pub fn new(text: &str) -> Self {
        let mut t = Self::default();
        t.set(text);
        t
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Replace the content; cursor goes to the end.
    pub fn set(&mut self, text: &str) {
        self.text = text.replace(['\n', '\r'], " ");
        self.cursor = self.len();
        self.scroll = 0;
    }

    fn len(&self) -> usize {
        self.text.graphemes(true).count()
    }

    fn byte_at(&self, g: usize) -> usize {
        self.text.graphemes(true).take(g).map(str::len).sum()
    }

    pub fn insert_str(&mut self, s: &str) {
        let s = s.replace("\r\n", " ").replace(['\n', '\r'], " ");
        if s.is_empty() {
            return;
        }
        let b = self.byte_at(self.cursor);
        self.text.insert_str(b, &s);
        self.cursor = self.text[..b + s.len()].graphemes(true).count();
    }

    /// Delete the word before the cursor (`Ctrl+W` where an input takes it): the blanks
    /// before it, then a run of letters, digits and `_`, or of other symbols.
    pub fn delete_word_back(&mut self) -> InputResult {
        let gs: Vec<&str> = self.text.graphemes(true).take(self.cursor).collect();
        let word = |g: &str| g.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_');
        let mut a = gs.len();
        while a > 0 && gs[a - 1].chars().all(char::is_whitespace) {
            a -= 1;
        }
        if a > 0 {
            let w = word(gs[a - 1]);
            while a > 0 && word(gs[a - 1]) == w && !gs[a - 1].chars().all(char::is_whitespace) {
                a -= 1;
            }
        }
        if a == gs.len() {
            return InputResult::Ignored;
        }
        let (from, to) = (self.byte_at(a), self.byte_at(self.cursor));
        self.text.drain(from..to);
        self.cursor = a;
        InputResult::Changed
    }

    pub fn handle_key(&mut self, k: &KeyEvent) -> InputResult {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Char('u') if ctrl => {
                let b = self.byte_at(self.cursor);
                if b == 0 {
                    return InputResult::Ignored;
                }
                self.text.drain(..b);
                self.cursor = 0;
                InputResult::Changed
            }
            KeyCode::Char('a') if ctrl => {
                self.cursor = 0;
                InputResult::Moved
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                let mut buf = [0u8; 4];
                self.insert_str(c.encode_utf8(&mut buf));
                InputResult::Changed
            }
            KeyCode::Backspace if self.cursor > 0 => {
                let (a, b) = (self.byte_at(self.cursor - 1), self.byte_at(self.cursor));
                self.text.drain(a..b);
                self.cursor -= 1;
                InputResult::Changed
            }
            KeyCode::Delete if self.cursor < self.len() => {
                let (a, b) = (self.byte_at(self.cursor), self.byte_at(self.cursor + 1));
                self.text.drain(a..b);
                InputResult::Changed
            }
            KeyCode::Left if self.cursor > 0 => {
                self.cursor -= 1;
                InputResult::Moved
            }
            KeyCode::Right if self.cursor < self.len() => {
                self.cursor += 1;
                InputResult::Moved
            }
            KeyCode::Home => {
                self.cursor = 0;
                InputResult::Moved
            }
            KeyCode::End => {
                self.cursor = self.len();
                InputResult::Moved
            }
            _ => InputResult::Ignored,
        }
    }

    /// A click at screen (x, y): on the input as last drawn, the cursor goes before the grapheme
    /// under the pointer (either half of a wide one), or to the end past the text. `false` when
    /// the click is not on the input.
    pub fn click(&mut self, x: u16, y: u16) -> bool {
        let a = self.drawn;
        if !a.contains(ratatui::layout::Position::new(x, y)) {
            return false;
        }
        let col = self.scroll + usize::from(x - a.x);
        let mut at = 0;
        let mut cursor = None;
        for (i, (_, w)) in cells(&self.text, self.drawn_mask, self.drawn_hide).into_iter().enumerate() {
            if col < at + w {
                cursor = Some(i);
                break;
            }
            at += w;
        }
        self.cursor = cursor.unwrap_or_else(|| self.len());
        true
    }

    /// Draw into a one-line `area` (`mask` replaces every grapheme with `•`, `hide` masks a
    /// byte range, e.g. a password inside a DSN). An unfocused input shows its beginning.
    /// Returns the screen column of the cursor.
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        style: Style,
        focused: bool,
        mask: bool,
        hide: Option<(usize, usize)>,
    ) -> u16 {
        let w = area.width as usize;
        (self.drawn, self.drawn_mask, self.drawn_hide) = (area, mask, hide);
        if w == 0 {
            return area.x;
        }
        buf.set_stringn(area.x, area.y, " ".repeat(w), w, style);
        let cells = cells(&self.text, mask, hide);
        let cursor_x: usize = cells.iter().take(self.cursor).map(|c| c.1).sum();
        // Keep one column free for the cursor at the end.
        if !focused {
            self.scroll = 0;
        } else if cursor_x < self.scroll {
            self.scroll = cursor_x;
        } else if cursor_x >= self.scroll + w {
            self.scroll = cursor_x + 1 - w;
        }
        let mut x = 0;
        for (g, gw) in cells {
            if x >= self.scroll + w {
                break;
            }
            if x >= self.scroll && x + gw <= self.scroll + w {
                buf.set_stringn(area.x + (x - self.scroll) as u16, area.y, g, gw.max(1), style);
            } else if x + gw > self.scroll {
                // Wide grapheme cut by the viewport edge: blank the visible half.
                for px in x.max(self.scroll)..(x + gw).min(self.scroll + w) {
                    buf.set_stringn(area.x + (px - self.scroll) as u16, area.y, " ", 1, style);
                }
            }
            x += gw;
        }
        area.x + (cursor_x.saturating_sub(self.scroll)).min(w - 1) as u16
    }
}

/// (shown text, width) of each grapheme of `text`: `mask` shows every one as `•`, `hide` the
/// ones in a byte range.
fn cells(text: &str, mask: bool, hide: Option<(usize, usize)>) -> Vec<(&str, usize)> {
    let mut b = 0;
    text.graphemes(true)
        .map(|g| {
            let hidden = mask || hide.is_some_and(|(s, e)| b >= s && b < e);
            b += g.len();
            if hidden { (MASK, 1) } else { (g, grapheme_width(g)) }
        })
        .collect()
}

#[cfg(test)]
mod tests;
