//! The mouse on the filter dialog: a click does what the key for the part clicked does there.
//! A column, a value, an operator and the picker's entries, a sort choice, a chip, a key of
//! the order, the `×` marks and the buttons (`undo`, `clear all`, `SQL`, `Open in editor`,
//! `# count exactly`, `Enter apply`, `Esc cancel`) act as a dialog's buttons do: a press arms
//! the part and the release over it acts, presses right after the dialog came on top are
//! ignored ([`crate::app::overlay::ARM_DELAY`]). A text field (a value, `Limit`, the SQL
//! section's `WHERE` and `ORDER BY`) takes the focus and the cursor on the press. Leaving a
//! field of the SQL section reads it back first, as `Tab` does. While the operator picker is
//! open a press elsewhere closes it. The wheel moves through the column list, the value list
//! and the picker. The pointer lights the part under it; the focus stays where it is.

use super::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

/// A key press as the part clicked gives it.
fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

impl App {
    /// The mouse while the filter dialog is on top.
    pub(crate) fn filter_dialog_mouse(&mut self, m: MouseEvent) {
        let now = self.now();
        let (x, y) = (m.column, m.row);
        match m.kind {
            MouseEventKind::ScrollDown => return self.filter_dialog_wheel(x, y, 1),
            MouseEventKind::ScrollUp => return self.filter_dialog_wheel(x, y, -1),
            _ => {}
        }
        let Some(d) = self.filter_dialog_mut() else { return };
        let hit = d.hits.at(x, y);
        let act = match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // The picker covers the pane: a press elsewhere closes it, and does nothing else.
                if d.picker.is_some() && hit.is_none() {
                    d.picker = None;
                    d.press.disarm();
                    return;
                }
                if hit.is_some_and(DialogHit::field) {
                    d.press.disarm();
                    hit
                } else {
                    d.press.press(m.kind, hit, now);
                    None
                }
            }
            MouseEventKind::Up(MouseButton::Left) => d.press.press(m.kind, hit, now),
            _ => None,
        };
        if let Some(h) = act {
            self.filter_dialog_hit(h, x, y);
        }
    }

    /// The pointer moved over the dialog (no button held): `true` when another part (or none)
    /// is under it now.
    pub(crate) fn filter_dialog_hover(&mut self, m: MouseEvent) -> bool {
        let Some(d) = self.filter_dialog_mut() else { return false };
        // A move has no button held: a press it armed lost its release.
        d.press.disarm();
        let h = d.hits.at(m.column, m.row).filter(|h| !h.field());
        // Over the selected column or the focused value the highlight would not show.
        let h = h.filter(|h| match *h {
            DialogHit::Column(i) => i != d.cursor,
            DialogHit::Item(i) => d.focus != DialogFocus::Values(i),
            _ => true,
        });
        std::mem::replace(&mut d.hover, h) != h
    }

    /// The part `h` clicked at (x, y): what its key does.
    fn filter_dialog_hit(&mut self, h: DialogHit, x: u16, y: u16) {
        let Some(d) = self.filter_dialog() else { return };
        // The field of the SQL section with the keys is read back as it is left (an order
        // refused keeps the keys in it); the actions that close or undo it see to it themselves.
        let stays = match h {
            DialogHit::SqlWhere => d.focus == DialogFocus::SqlWhere,
            DialogHit::SqlOrder => d.focus == DialogFocus::SqlOrder,
            DialogHit::Undo | DialogHit::Apply | DialogHit::Cancel | DialogHit::Sql | DialogHit::OpenSql => true,
            _ => false,
        };
        if d.sql_field().is_some() && !stays {
            self.filter_dialog_popup_close();
            if !self.filter_dialog_read_back() {
                return;
            }
        }
        let Some(d) = self.filter_dialog_mut() else { return };
        match h {
            DialogHit::Column(i) => {
                d.focus_on(DialogFocus::Columns);
                d.cursor = i;
            }
            DialogHit::Op(t) => {
                d.focus_on(DialogFocus::Op(t));
                self.filter_dialog_key(press(KeyCode::Char(' ')));
            }
            DialogHit::Pick(i) => {
                d.picker = Some(i);
                self.filter_dialog_pick();
            }
            DialogHit::Value(t, k) => {
                let was = d.focus == DialogFocus::Value(t, k);
                let area = d.hits.rect(h).map(inner);
                d.focus_on(DialogFocus::Value(t, k));
                if let (DialogFocus::Value(t2, k2), Some(area)) = (d.focus, area)
                    && (t2, k2) == (t, k)
                    && let Some(input) = d.inputs.get_mut(k)
                {
                    input.click_at(area, was, x, y);
                }
            }
            DialogHit::Remove(t) => self.filter_dialog_edit(None, |d, _| {
                d.draft.where_.remove(t);
            }),
            DialogHit::Add => {
                d.focus_on(DialogFocus::Add);
                self.filter_dialog_key(press(KeyCode::Char(' ')));
            }
            DialogHit::Item(i) => {
                d.focus_on(DialogFocus::Values(i));
                self.filter_dialog_tick(i);
            }
            DialogHit::Count => self.filter_dialog_action(DialogAction::Count),
            DialogHit::Sort(dir) => {
                d.focus_on(DialogFocus::Sort);
                let Some(col) = d.column().map(|c| c.name.clone()) else { return };
                self.filter_dialog_edit(None, |d, _| d.draft.set_sort(&col, dir));
            }
            DialogHit::Chip(i) => {
                d.focus_on(DialogFocus::Chips(i));
                self.filter_dialog_key(press(KeyCode::Char(' ')));
            }
            DialogHit::ChipRemove(i) => {
                d.focus_on(DialogFocus::Chips(i));
                self.filter_dialog_key(press(KeyCode::Char('x')));
            }
            DialogHit::Key(i) => {
                d.focus_on(DialogFocus::Order(i));
                self.filter_dialog_key(press(KeyCode::Char(' ')));
            }
            DialogHit::Undo => self.filter_dialog_action(DialogAction::Undo),
            DialogHit::ClearAll => self.filter_dialog_action(DialogAction::ClearAll),
            DialogHit::Sql => self.filter_dialog_action(DialogAction::Sql),
            DialogHit::OpenSql => self.filter_dialog_open_sql(),
            DialogHit::Apply => self.filter_dialog_action(DialogAction::Apply),
            DialogHit::Cancel => self.filter_dialog_action(DialogAction::Cancel),
            DialogHit::Limit => {
                let was = d.focus == DialogFocus::Limit;
                d.focus_on(DialogFocus::Limit);
                if let Some(area) = d.hits.rect(h).map(inner) {
                    d.limit.click_at(area, was, x, y);
                }
            }
            DialogHit::SqlWhere => {
                let was = d.focus == DialogFocus::SqlWhere;
                let area = d.hits.rect(h).map(inner);
                d.focus_on(DialogFocus::SqlWhere);
                if let (Some(s), Some(area)) = (d.sql.as_mut(), area)
                    && let Some(at) = crate::widgets::filter_dialog::wrapped_byte_at(&s.where_.input, area, was, x, y)
                {
                    s.where_.input.set_cursor_byte(at);
                }
            }
            DialogHit::SqlOrder => {
                let was = d.focus == DialogFocus::SqlOrder;
                let area = d.hits.rect(h).map(inner);
                d.focus_on(DialogFocus::SqlOrder);
                if let (Some(s), Some(area)) = (d.sql.as_mut(), area) {
                    s.order.input.click_at(area, was, x, y);
                }
            }
        }
    }

    /// A notch of the wheel at (x, y) (`delta`: down 1, up -1): the picker's selection, the
    /// selected column, or the value list (its focused value, else the list scrolled).
    fn filter_dialog_wheel(&mut self, x: u16, y: u16, delta: isize) {
        let Some(d) = self.filter_dialog_mut() else { return };
        let at = Position::new(x, y);
        let code = if delta > 0 { KeyCode::Down } else { KeyCode::Up };
        if d.picker.is_some() {
            return self.filter_dialog_key(press(code));
        }
        d.hover = None;
        if DialogHits::span(&d.hits.columns).contains(at) {
            let last = d.shown_columns().len().saturating_sub(1);
            d.cursor = d.cursor.saturating_add_signed(delta).min(last);
        } else if DialogHits::span(&d.hits.items).contains(at) {
            match d.focus {
                DialogFocus::Values(i) => {
                    let last = d.listed.saturating_sub(1);
                    d.focus_on(DialogFocus::Values(i.saturating_add_signed(delta).min(last)));
                }
                _ => d.values_scroll = d.values_scroll.saturating_add_signed(delta),
            }
        }
    }
}

/// The text of a field drawn in `r`: a blank kept on both sides.
fn inner(r: Rect) -> Rect {
    Rect { x: r.x + 1, width: r.width.saturating_sub(2), ..r }
}
