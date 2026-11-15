//! The keys of the filter dialog's two list popups: the add popup (the column a new condition
//! is on, `/` to search the list) and the WHERE list (the conditions of the `WHERE` text, one a
//! line). Neither sends anything: picking a column turns the add popup into that column's
//! condition popup with a new condition waiting for its value; opening a condition of the list
//! turns it into its column's condition popup, an expression into the whole dialog on its chip.
//! Every change of view is [`FilterDialog::set_view`]: the draft and its undo stay.

use super::view::{PopupKind, View};
use super::*;

impl App {
    /// A key of the add popup or the WHERE list that no action took.
    pub(super) fn filter_list_key(&mut self, key: KeyEvent) {
        let Some(d) = self.filter_dialog() else { return };
        match d.popup_kind() {
            Some(PopupKind::AddPick) => self.filter_add_pick_key(key),
            Some(PopupKind::WhereList) => self.filter_where_list_key(key),
            _ => {}
        }
    }

    /// The add popup's keys: `↑`/`↓` (and `j`/`k`, `g`/`G` outside the search) through the
    /// columns, `→`, `l` and `Space` pick the column, `/` opens the search; while it is open
    /// what is typed goes into it.
    fn filter_add_pick_key(&mut self, key: KeyEvent) {
        let Some(d) = self.filter_dialog_mut() else { return };
        let typing = d.typing();
        let last = d.shown_columns().len().saturating_sub(1);
        let at = d.cursor;
        match key.code {
            KeyCode::Up => self.filter_dialog_select_column(at.saturating_sub(1)),
            KeyCode::Down => self.filter_dialog_select_column((at + 1).min(last)),
            _ if typing => self.filter_dialog_text_key(key),
            KeyCode::Char('k') => self.filter_dialog_select_column(at.saturating_sub(1)),
            KeyCode::Char('j') => self.filter_dialog_select_column((at + 1).min(last)),
            KeyCode::Home | KeyCode::Char('g') => self.filter_dialog_select_column(0),
            KeyCode::End | KeyCode::Char('G') => self.filter_dialog_select_column(last),
            KeyCode::Right | KeyCode::Char('l' | ' ') => self.filter_add_pick(),
            KeyCode::Char('/') => d.search = Some(TextInput::new("")),
            _ => {}
        }
    }

    /// The add popup's selected column picked (`Enter`, a click): the popup becomes that
    /// column's condition popup, by the same anchor, with a new condition of the column focused
    /// (one undo step). Nothing is sent.
    pub(super) fn filter_add_pick(&mut self) {
        let Some(d) = self.filter_dialog_mut() else { return };
        let View::Popup { kind: PopupKind::AddPick, anchor } = d.view().clone() else { return };
        let Some(col) = d.column().map(|c| c.name.clone()) else { return };
        d.set_view(View::Popup { kind: PopupKind::Condition { col: col.clone() }, anchor });
        d.select_column(&col);
        d.narrow = None;
        d.values_scroll = 0;
        self.filter_dialog_add();
    }

    /// The WHERE list's keys: `↑`/`↓` (`k`/`j`, `g`/`G`) through its conditions and its `+ add
    /// condition`, `→`, `l` and `Space` open the line (as `Enter`), `x` and `Delete` take the
    /// condition out of the draft, `+` and `a` turn the list into the add popup.
    fn filter_where_list_key(&mut self, key: KeyEvent) {
        let Some(d) = self.filter_dialog_mut() else { return };
        let chips: Vec<usize> = d.draft.where_.chips().iter().map(|c| c.index).collect();
        let n = chips.len();
        // The lines: the conditions, then `+ add condition` (at `n`).
        let at = match d.focus {
            DialogFocus::Chips(i) => i.min(n),
            _ => n,
        };
        let line = |i: usize| if i < n { DialogFocus::Chips(i) } else { DialogFocus::Add };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => d.focus_on(line(at.saturating_sub(1))),
            KeyCode::Down | KeyCode::Char('j') => d.focus_on(line((at + 1).min(n))),
            KeyCode::Home | KeyCode::Char('g') => d.focus_on(line(0)),
            KeyCode::End | KeyCode::Char('G') => d.focus_on(line(n)),
            KeyCode::Right | KeyCode::Char('l' | ' ') => self.filter_where_list_open(),
            KeyCode::Char('x') | KeyCode::Delete if at < n => {
                let t = chips[at];
                self.filter_dialog_edit(None, |d, _| {
                    d.draft.where_.remove(t);
                });
            }
            KeyCode::Char('+' | 'a') => self.filter_where_list_add(),
            _ => {}
        }
    }

    /// The WHERE list's focused line opened (`Enter`, a click): a column's condition becomes
    /// that column's condition popup on it, by the same anchor; an expression the whole dialog
    /// on its chip; `+ add condition` the add popup. Nothing is sent.
    pub(super) fn filter_where_list_open(&mut self) {
        let Some(d) = self.filter_dialog_mut() else { return };
        let View::Popup { kind: PopupKind::WhereList, anchor } = d.view().clone() else { return };
        let i = match d.focus {
            DialogFocus::Chips(i) => i,
            DialogFocus::Add => return self.filter_where_list_add(),
            _ => return,
        };
        let Some(t) = d.draft.where_.chips().get(i).map(|c| c.index) else { return };
        match d.draft.where_.terms()[t].row.as_ref().map(|r| r.col.clone()) {
            Some(col) => {
                d.set_view(View::Popup { kind: PopupKind::Condition { col: col.clone() }, anchor });
                d.select_column(&col);
                d.focus_on(DialogFocus::Op(t));
            }
            None => {
                d.set_view(View::Full);
                d.focus_on(DialogFocus::Chips(i));
            }
        }
        d.settle();
    }

    /// `+` in the WHERE list (or its `+ add condition`): the list becomes the add popup, by the
    /// same anchor, on the first column.
    fn filter_where_list_add(&mut self) {
        let Some(d) = self.filter_dialog_mut() else { return };
        let View::Popup { anchor, .. } = d.view().clone() else { return };
        d.set_view(View::Popup { kind: PopupKind::AddPick, anchor });
        d.cursor = 0;
        d.scroll = 0;
        d.focus_on(DialogFocus::Columns);
    }
}
