//! Single-line text input used by the command line and the profile form.
//!
//! Same rules as the editor: the cursor moves by grapheme cluster, the screen
//! column is the sum of preceding grapheme widths, and [`TextInput::render`] returns where the
//! real terminal cursor belongs so IME preedit shows up in place.

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
        if w == 0 {
            return area.x;
        }
        buf.set_stringn(area.x, area.y, " ".repeat(w), w, style);
        // (display text, width) per grapheme
        let mut cells: Vec<(&str, usize)> = Vec::new();
        let mut b = 0;
        for g in self.text.graphemes(true) {
            let hidden = mask || hide.is_some_and(|(s, e)| b >= s && b < e);
            b += g.len();
            cells.push(if hidden { (MASK, 1) } else { (g, grapheme_width(g)) });
        }
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

#[cfg(test)]
mod tests;
