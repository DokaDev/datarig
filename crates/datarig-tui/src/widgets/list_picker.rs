//! A list to pick one entry from, with a typed filter: its entries (each a label, a hint after
//! it and the value it stands for) and, last, an entry that makes a new one ("New SSH
//! tunnel…"). The dialog that shows it acts on what [`ListPicker::key`] and
//! [`ListPicker::mouse`] return.
//!
//! The keys are those of the tab list and the action menu: typing filters the entries, ranked
//! as they rank theirs (a label that starts with the text, then a word of it, then its letters
//! in order; a match in the hint ranks below one in the label); `↓`/`↑`, `Ctrl+N`/`Ctrl+P` and
//! `Tab`/`Shift+Tab` move around the ends; `Enter` picks the selected entry; `Esc` cancels. The
//! new entry is listed whatever the filter, so it is always there to pick. With the mouse, as
//! the other lists: a press arms a row and the release over it picks it (not right after the
//! list came on top), the pointer only highlights the row under it, the wheel moves the
//! selection and a click outside the list does nothing.

use crate::app::overlay::Press;
use crate::text::{Align, fit};
use crate::theme;
use crate::widgets::dialog::{centered, modal};
use crate::widgets::put;
use crate::widgets::text_input::{InputResult, TextInput};
use datarig_core::i18n::Localized;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use std::time::Instant;

/// An entry of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry<T> {
    pub value: T,
    pub label: String,
    /// Shown dimmed after the label (empty: nothing); the filter looks at it too.
    pub hint: String,
}

/// A row of the list as shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// Entry `i` of the entries given.
    Entry(usize),
    /// The entry that makes a new one.
    New,
}

/// What a key or the mouse did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome<T> {
    /// Still open.
    Pending,
    /// This entry's value was picked: the list closes.
    Picked(T),
    /// The new entry was picked: the list closes.
    New,
    /// Closed without picking anything.
    Cancelled,
}

pub struct ListPicker<T> {
    entries: Vec<Entry<T>>,
    /// The new entry's label (`None`: there is none).
    new_label: Option<String>,
    pub filter: TextInput,
    /// The rows shown: the entries the filter lets through, best first, then the new entry.
    rows: Vec<Row>,
    /// Index into `rows`.
    selected: usize,
    /// First row shown, and where the rows were drawn (mouse), kept by the renderer.
    scroll: usize,
    list: Rect,
    /// The row under the pointer (highlighted; the selection does not move to it).
    hover: Option<usize>,
    /// The row a press armed (it is picked on the release).
    press: Press<usize>,
}

impl<T: Clone + PartialEq> ListPicker<T> {
    /// A list of `entries`, then the new entry labelled `new_label` (if any), the entry whose
    /// value is `current` selected (the first one when none is).
    pub fn new(entries: Vec<Entry<T>>, new_label: Option<String>, current: Option<&T>) -> Self {
        let mut p = ListPicker {
            entries,
            new_label,
            filter: TextInput::default(),
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            list: Rect::default(),
            hover: None,
            press: Press::default(),
        };
        p.refilter();
        if let Some(i) = current.and_then(|c| p.entries.iter().position(|e| e.value == *c)) {
            p.selected = i;
        }
        p
    }

    pub fn entries(&self) -> &[Entry<T>] {
        &self.entries
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The selected row (`None`: nothing is listed).
    pub fn selected(&self) -> Option<Row> {
        self.rows.get(self.selected).copied()
    }

    /// The row under the pointer.
    pub fn hover(&self) -> Option<usize> {
        self.hover
    }

    /// Where the rows were last drawn.
    pub fn list_area(&self) -> Rect {
        self.list
    }

    /// The label a row shows.
    pub fn label(&self, row: Row) -> &str {
        match row {
            Row::Entry(i) => &self.entries[i].label,
            Row::New => self.new_label.as_deref().unwrap_or_default(),
        }
    }

    /// Work the rows out again from the filter; the best match is selected.
    fn refilter(&mut self) {
        let query = self.filter.text().trim().to_string();
        let mut hits: Vec<(u8, usize)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| crate::app::menu::rank(&query, &[&e.label, &e.hint]).map(|r| (r, i)))
            .collect();
        hits.sort();
        self.rows = hits.into_iter().map(|(_, i)| Row::Entry(i)).collect();
        if self.new_label.is_some() {
            self.rows.push(Row::New);
        }
        self.selected = 0;
        self.hover = None;
    }

    /// Select the next (`d = 1`) or previous row, around the ends.
    fn step(&mut self, d: isize) {
        let n = self.rows.len() as isize;
        if n > 0 {
            self.selected = (self.selected as isize + d).rem_euclid(n) as usize;
        }
    }

    /// What picking the selected row does.
    fn pick(&self) -> Outcome<T> {
        match self.selected() {
            Some(Row::Entry(i)) => Outcome::Picked(self.entries[i].value.clone()),
            Some(Row::New) => Outcome::New,
            None => Outcome::Pending,
        }
    }

    /// A key (`repeat`: held down): typing filters, the arrows, `Ctrl+N`/`Ctrl+P` and `Tab`
    /// move, `Enter` picks, `Esc` cancels.
    pub fn key(&mut self, key: &KeyEvent, repeat: bool) -> Outcome<T> {
        let (plain, ctrl) = (key.modifiers.is_empty(), key.modifiers == KeyModifiers::CONTROL);
        // The rows or the scroll may change: the highlight under the pointer goes.
        self.hover = None;
        match key.code {
            KeyCode::Down | KeyCode::Tab if plain => self.step(1),
            KeyCode::Char('n') if ctrl => self.step(1),
            KeyCode::Up if plain => self.step(-1),
            KeyCode::BackTab => self.step(-1),
            KeyCode::Char('p') if ctrl => self.step(-1),
            KeyCode::Esc | KeyCode::Enter if repeat => {}
            KeyCode::Esc if plain => return Outcome::Cancelled,
            KeyCode::Enter if plain => return self.pick(),
            _ => {
                if self.filter.handle_key(key) == InputResult::Changed {
                    self.refilter();
                }
            }
        }
        Outcome::Pending
    }

    /// Text pasted while it has the keyboard: into its filter.
    pub fn paste(&mut self, text: &str) {
        self.filter.insert_str(text);
        self.refilter();
    }

    /// The row drawn at (x, y).
    fn row_at(&self, x: u16, y: u16) -> Option<usize> {
        self.list
            .contains(Position::new(x, y))
            .then(|| self.scroll + usize::from(y - self.list.y))
            .filter(|i| *i < self.rows.len())
    }

    /// The mouse at `now`: a row is picked on the release over the row pressed, a click on the
    /// filter line puts the cursor there, the wheel moves the selection; anything else (a click
    /// outside the list) does nothing.
    pub fn mouse(&mut self, m: &MouseEvent, now: Instant) -> Outcome<T> {
        match m.kind {
            MouseEventKind::ScrollDown => return self.key(&KeyEvent::from(KeyCode::Down), false),
            MouseEventKind::ScrollUp => return self.key(&KeyEvent::from(KeyCode::Up), false),
            _ => {}
        }
        let row = self.row_at(m.column, m.row);
        if let Some(i) = self.press.press(m.kind, row, now) {
            self.selected = i;
            return self.pick();
        }
        if m.kind == MouseEventKind::Down(MouseButton::Left) && row.is_none() {
            self.filter.click(m.column, m.row);
        }
        Outcome::Pending
    }

    /// The pointer moved to (x, y) with no button held: the row under it is highlighted (not
    /// the selected one, where it would not show), and a press it armed lost its release.
    /// `true` when the highlight changed.
    pub fn pointer(&mut self, x: u16, y: u16) -> bool {
        self.press.disarm();
        let h = self.row_at(x, y).filter(|i| *i != self.selected);
        std::mem::replace(&mut self.hover, h) != h
    }

    /// It came on top again: presses count only after [`crate::app::overlay::ARM_DELAY`].
    pub fn rearm(&mut self) {
        self.press = Press::default();
    }

    /// Draw it centered in `area` (`top`: it has the mouse); `none` is said when no entry
    /// matches the filter, `placeholder` while there is none. Returns the hardware cursor (the
    /// filter's).
    pub fn draw(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        (title, footer): (&Localized, &Localized),
        (placeholder, none): (&str, &str),
        top: bool,
        now: Instant,
    ) -> (u16, u16) {
        let th = theme::cur();
        self.press.drawn(top, now);
        // No entry matches: a line says so above the new entry.
        let note = usize::from(!self.rows.iter().any(|r| matches!(r, Row::Entry(_))));
        let w = area.width.saturating_sub(8).min(56);
        let rows = (self.rows.len() + note).clamp(1, (area.height as usize).saturating_sub(6).max(1));
        let rect = centered(area, w, rows as u16 + 4);
        let inner = modal(rect, title, footer, buf);
        let iw = inner.width as usize;
        let base = Style::new().bg(th.surface);
        put(buf, inner.x + 1, inner.y, "›", 1, base.fg(th.accent).add_modifier(Modifier::BOLD));
        if note == 1 {
            put(buf, inner.x + 1, inner.y + 2, none, iw.saturating_sub(2), base.fg(th.fg_dim));
        }
        // Keep the selection on screen; the rows stay put while it moves among them.
        let shown = rows - note.min(rows - 1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + shown {
            self.scroll = self.selected + 1 - shown;
        }
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(shown));
        let y0 = inner.y + 2 + note.min(rows - 1) as u16;
        self.list = Rect::new(inner.x, y0, inner.width, shown.min(self.rows.len()) as u16);
        for (i, row) in self.rows.iter().enumerate().skip(self.scroll).take(shown) {
            let y = y0 + (i - self.scroll) as u16;
            let bg = if i == self.selected {
                th.selection
            } else if self.hover == Some(i) {
                crate::widgets::hover_style()
            } else {
                base
            };
            buf.set_stringn(inner.x, y, fit("", iw, Align::Left), iw, bg);
            let room = |x: u16| (inner.x + inner.width).saturating_sub(x + 1) as usize;
            let x = inner.x + 1;
            match *row {
                Row::Entry(e) => {
                    let e = &self.entries[e];
                    let used = put(buf, x, y, &e.label, room(x), Style::new().fg(th.fg).patch(bg));
                    if !e.hint.is_empty() {
                        let hx = x + used + 2;
                        put(buf, hx, y, &e.hint, room(hx), Style::new().fg(th.fg_dim).patch(bg));
                    }
                }
                Row::New => {
                    let label = self.new_label.as_deref().unwrap_or_default();
                    put(buf, x, y, label, room(x), Style::new().fg(th.accent).patch(bg));
                }
            }
        }
        let input = Rect::new(inner.x + 3, inner.y, inner.width.saturating_sub(4), 1);
        let cx = self.filter.render(input, buf, base.fg(th.fg), true, false, None);
        if self.filter.text().is_empty() {
            put(buf, input.x, input.y, placeholder, input.width as usize, base.fg(th.fg_dim));
        }
        (cx, inner.y)
    }
}

#[cfg(test)]
mod tests;
