//! Scrolling by keys: `Ctrl+D` / `Ctrl+U` (half a screen), `Ctrl+F` / `Ctrl+B` (a page) and
//! `zz` `zt` `zb` (the cursor's line to the middle, top or bottom), as Vim scrolls, the cursor
//! on the first non-blank of its line.

use super::{EdEvent, Editor};

impl Editor {
    /// Lines on screen (as drawn last).
    fn screen_lines(&self) -> usize {
        self.view_h.max(1)
    }

    /// The cursor to line `r`, its first non-blank.
    fn cursor_to_line(&mut self, r: usize) {
        let r = r.min(self.lines.len() - 1);
        self.set_pos(r, self.first_nonblank(r));
    }

    /// `Ctrl+D` (`down`) / `Ctrl+U`: the text and the cursor move by half a screen, or by the
    /// count typed (which is kept for the next ones, as Vim's `scroll` option). At the end of
    /// the text the cursor moves on alone.
    pub(super) fn scroll_half(&mut self, down: bool, count: usize, explicit: bool) -> EdEvent {
        self.keep_on_screen();
        let h = self.screen_lines();
        if explicit {
            self.scroll_lines = count.min(h);
        }
        let n = if self.scroll_lines > 0 { self.scroll_lines } else { (h / 2).max(1) };
        let last = self.lines.len() - 1;
        if (down && self.row == last) || (!down && self.row == 0) {
            return EdEvent::None;
        }
        if down {
            let room = self.lines.len().saturating_sub(self.top + h);
            self.top += n.min(room);
            self.cursor_to_line(self.row.saturating_add(n));
        } else {
            self.top = self.top.saturating_sub(n);
            self.cursor_to_line(self.row.saturating_sub(n));
        }
        self.keep_on_screen();
        EdEvent::Moved
    }

    /// `Ctrl+F` (`forward`) / `Ctrl+B`: `count` pages (a screen less two lines), the cursor on
    /// the top line after `Ctrl+F` and on the bottom one after `Ctrl+B`. With the last line on
    /// screen, `Ctrl+F` scrolls it to the top.
    pub(super) fn scroll_page(&mut self, forward: bool, count: usize) -> EdEvent {
        self.keep_on_screen();
        let h = self.screen_lines();
        let page = if h > 2 { h - 2 } else { h };
        let last = self.lines.len() - 1;
        let top = self.top;
        for _ in 0..count {
            if forward {
                if self.top >= last {
                    break;
                }
                self.top = if self.top + h >= self.lines.len() { last } else { self.top + page };
            } else {
                if self.top == 0 {
                    break;
                }
                self.top = self.top.saturating_sub(page);
            }
        }
        if self.top == top {
            return EdEvent::None;
        }
        let r = if forward { self.top } else { self.top + h - 1 };
        self.cursor_to_line(r);
        EdEvent::Moved
    }

    /// `zz`, `zt`, `zb` (and `z` Enter: `zt` on the first non-blank): the cursor's line (line
    /// `count` when one is typed) to the middle, the top or the bottom of the screen.
    pub(super) fn scroll_cursor(&mut self, c: char, count: usize, explicit: bool) -> EdEvent {
        if !matches!(c, 'z' | 't' | 'b' | '\n') {
            return EdEvent::None;
        }
        if explicit {
            let r = count.min(self.lines.len()) - 1;
            let col = self.col;
            self.set_pos(r, col);
        }
        if c == '\n' {
            self.set_pos(self.row, self.first_nonblank(self.row));
        }
        let h = self.screen_lines();
        self.top = match c {
            'z' => self.row.saturating_sub((h - 1) / 2),
            'b' => self.row.saturating_sub(h - 1),
            _ => self.row,
        };
        EdEvent::Moved
    }

    /// The top line so that the cursor is on screen (it is drawn so, but the cursor may have
    /// moved since).
    fn keep_on_screen(&mut self) {
        let h = self.screen_lines();
        if self.row < self.top {
            self.top = self.row;
        } else if self.row >= self.top + h {
            self.top = self.row + 1 - h;
        }
    }
}

#[cfg(test)]
mod tests;
