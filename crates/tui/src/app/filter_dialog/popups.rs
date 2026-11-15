//! The keys of the filter dialog's popups that are lists: the add popup (the column a new
//! condition is on, `/` to search the list), the WHERE list (the conditions of the `WHERE`
//! text, one a line), the order popup (the keys, then the other columns), the columns popup
//! (shown and hidden) and the rows popup (pages and counts). Picking a column turns the add
//! popup into that column's condition popup with a new condition waiting for its value;
//! opening a condition of the list turns it into its column's condition popup, an expression
//! into the whole dialog on its chip; `x` in the list takes the condition out and runs the
//! query as the bar's `×` does. The order and columns popups edit the draft (`Enter` applies
//! it); the rows popup's items do what the bar's arrows and `#` do, and it stays open. Every
//! change of view is [`FilterDialog::set_view`]: the draft and its undo stay.

use super::draft::Draft;
use super::view::{PopupKind, View};
use super::*;
use crate::widgets::qbar::BarHit;
use datarig_core::query::Dir;

impl App {
    /// A key of a popup that is a list that no action took.
    pub(super) fn filter_list_key(&mut self, key: KeyEvent) {
        let Some(d) = self.filter_dialog() else { return };
        match d.popup_kind() {
            Some(PopupKind::AddPick) => self.filter_add_pick_key(key),
            Some(PopupKind::WhereList) => self.filter_where_list_key(key),
            Some(PopupKind::Order) => self.filter_order_key(key),
            Some(PopupKind::Columns) => self.filter_columns_key(key),
            Some(PopupKind::Rows) => self.filter_rows_key(key),
            Some(PopupKind::Condition { .. }) | None => {}
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
    /// condition out and run the query ([`App::filter_where_list_remove`]), `+` and `a` turn
    /// the list into the add popup.
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
            KeyCode::Char('x') | KeyCode::Delete if at < n => self.filter_where_list_remove(chips[at]),
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
    /// `x` in the WHERE list on the condition of term `t`: as the bar's `×` on it, the query
    /// applied runs without it, the other conditions sent as they were written; the list
    /// waits for the statement to be sent and closes then, as an apply does (a refusal or a
    /// question keeps it, saying why). The condition leaves the list's draft too (one undo
    /// step). One the query applied does not have (its text is not among its conditions) only
    /// leaves the draft.
    fn filter_where_list_remove(&mut self, t: usize) {
        let Some(d) = self.filter_dialog() else { return };
        let tab = d.tab;
        let text = d.draft.where_.terms().get(t).and_then(|x| x.text.clone());
        // A busy tab refuses it (said) and the draft stays.
        if self.tab_busy(tab) {
            return self.flash_busy();
        }
        let applied = self.table_target(tab).zip(self.table_query(tab).map(|tq| tq.applied().clone()));
        let at = applied.and_then(|(target, q)| {
            let w = Draft::from_query(&q, &target).where_;
            w.terms().iter().position(|x| x.text.is_some() && x.text == text)
        });
        let Some(at) = at else {
            return self.filter_dialog_edit(None, |d, _| {
                d.draft.where_.remove(t);
            });
        };
        let Some((tab, q)) = self.bar_without(tab, at) else { return };
        self.filter_dialog_edit(None, |d, _| {
            d.draft.where_.remove(t);
        });
        let Some(d) = self.filter_dialog_mut() else { return };
        d.applying = Some(q.clone());
        d.applying_hidden = None;
        d.message = None;
        d.mark = None;
        if let Some(tq) = self.table_query_mut(tab) {
            tq.draft = q.clone();
            tq.refusal = None;
        }
        // What the run says while the list is open is said on its border.
        self.status = None;
        self.transient = None;
        self.table_run(tab, q);
        if self.filter_dialog().is_some_and(|d| d.tab == tab) {
            self.filter_dialog_told(tab);
        }
    }

    /// The order popup's keys: `↑`/`↓` (`k`/`j`, `g`/`G`) through its lines, `Space` turns a
    /// key's direction (a column not in the order becomes its last key, ascending), `J`/`K`
    /// move a key later or earlier, `x` (`Delete`) takes it out of the order. Each change is one
    /// undo step; the keys follow the line they were on.
    fn filter_order_key(&mut self, key: KeyEvent) {
        let Some(d) = self.filter_dialog_mut() else { return };
        let rows = d.sort_rows();
        let last = rows.len().saturating_sub(1);
        let at = match d.focus {
            DialogFocus::Sorted(i) => i.min(last),
            _ => 0,
        };
        let Some(col) = rows.get(at).cloned() else { return };
        let keyed = d.draft.sort_of(&col);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => d.focus_on(DialogFocus::Sorted(at.saturating_sub(1))),
            KeyCode::Down | KeyCode::Char('j') => d.focus_on(DialogFocus::Sorted((at + 1).min(last))),
            KeyCode::Home | KeyCode::Char('g') => d.focus_on(DialogFocus::Sorted(0)),
            KeyCode::End | KeyCode::Char('G') => d.focus_on(DialogFocus::Sorted(last)),
            KeyCode::Char(' ') => {
                let dir = match keyed {
                    Some((Dir::Asc, _)) => Dir::Desc,
                    Some((Dir::Desc, _)) | None => Dir::Asc,
                };
                self.filter_dialog_edit(None, |d, _| {
                    d.draft.set_sort(&col, Some(dir));
                    d.focus = DialogFocus::Sorted(d.draft.sort_of(&col).map_or(at, |(_, i)| i));
                });
            }
            KeyCode::Char('J' | 'K') if keyed.is_some() => {
                let delta = if key.code == KeyCode::Char('K') { -1 } else { 1 };
                self.filter_dialog_edit(None, |d, _| {
                    d.draft.move_key(&col, delta);
                    d.focus = DialogFocus::Sorted(d.draft.sort_of(&col).map_or(at, |(_, i)| i));
                });
            }
            KeyCode::Char('x') | KeyCode::Delete if keyed.is_some() => {
                self.filter_dialog_edit(None, |d, _| d.draft.set_sort(&col, None))
            }
            _ => {}
        }
    }

    /// The columns popup's keys: `↑`/`↓` (`k`/`j`, `g`/`G`) through the columns, `Space` (`C`)
    /// hides the column from the grid or shows it again, `a` shows every column. Each change is
    /// one undo step, applied with `Enter` (hidden columns alone: without a run).
    fn filter_columns_key(&mut self, key: KeyEvent) {
        let Some(d) = self.filter_dialog_mut() else { return };
        let last = d.shown_columns().len().saturating_sub(1);
        let at = d.cursor;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.filter_dialog_select_column(at.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.filter_dialog_select_column((at + 1).min(last)),
            KeyCode::Home | KeyCode::Char('g') => self.filter_dialog_select_column(0),
            KeyCode::End | KeyCode::Char('G') => self.filter_dialog_select_column(last),
            KeyCode::Char(' ' | 'C') => {
                let Some(col) = d.column().map(|c| c.name.clone()) else { return };
                self.filter_dialog_edit(None, |d, _| d.draft.toggle_hidden(&col));
            }
            KeyCode::Char('a') => self.filter_dialog_edit(None, |d, _| d.draft.hidden.clear()),
            _ => {}
        }
    }

    /// The rows popup's keys: `↑`/`↓` (`k`/`j`, `g`/`G`) through its items, `Space`, `→` and
    /// `l` do the item (as `Enter`).
    fn filter_rows_key(&mut self, key: KeyEvent) {
        let Some(d) = self.filter_dialog_mut() else { return };
        let all = RowsItem::ALL;
        let at = match d.focus {
            DialogFocus::RowsItem(item) => all.iter().position(|x| *x == item).unwrap_or(0),
            _ => 0,
        };
        let to = |i: usize| DialogFocus::RowsItem(all[i.min(all.len() - 1)]);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => d.focus_on(to(at.saturating_sub(1))),
            KeyCode::Down | KeyCode::Char('j') => d.focus_on(to(at + 1)),
            KeyCode::Home | KeyCode::Char('g') => d.focus_on(to(0)),
            KeyCode::End | KeyCode::Char('G') => d.focus_on(to(all.len() - 1)),
            KeyCode::Right | KeyCode::Char('l' | ' ') => self.filter_rows_run(),
            _ => {}
        }
    }

    /// The rows popup's focused item done (`Enter`, a click), on the rows shown, by the same
    /// paths as the bar: its arrows (the previous and the next page) and `#` (the planner's
    /// estimate, the exact count with its time limit; each checked as `#` checks it, a refusal
    /// said). The popup stays open and shows what changed. A popup of a tab that is not the
    /// active one does nothing.
    pub(super) fn filter_rows_run(&mut self) {
        let Some(d) = self.filter_dialog().filter(|d| d.tab == self.tab().id) else { return };
        let DialogFocus::RowsItem(item) = d.focus else { return };
        match item {
            RowsItem::Prev => self.bar_hit(BarHit::Prev),
            RowsItem::Next => self.bar_hit(BarHit::Next),
            // The planner estimates filtered rows only (the whole table's: its statistics).
            RowsItem::Estimate if self.table_query(self.tab().id).and_then(|tq| tq.shown_filter()).is_none() => {
                self.flash(Notice::new(Label::TableRowsUnfiltered, Level::Info))
            }
            RowsItem::Estimate => self.estimate_rows(),
            RowsItem::Exact => self.count_exact(),
        }
    }
}
