//! What the filter dialog shows of its one editing session ([`View`]): the whole dialog, or a
//! small popup by the query bar or a column's header. Every view edits the same draft, with
//! the same undo, the same apply waiting to be sent and the same count of values: changing
//! the view ([`FilterDialog::set_view`]) keeps them, and drops only what was drawn.

use super::{DialogHits, FilterDialog};
use crate::widgets::qbar::BarHit;
use ratatui::layout::Rect;

/// What the filter dialog shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View {
    /// The whole dialog: columns, the selected column's pane, the footer.
    Full,
    /// A small popup of kind `kind`, drawn by `anchor`.
    Popup { kind: PopupKind, anchor: Anchor },
}

/// What a popup of the filter dialog is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PopupKind {
    /// The conditions, values and sort of column `col`.
    Condition { col: String },
    /// The column a new condition is on, to pick.
    AddPick,
    /// The order's keys.
    Order,
    /// The columns shown and hidden.
    Columns,
    /// The rows of the result shown, their pages and their count.
    Rows,
    /// The conditions of the `WHERE` text, as a list.
    WhereList,
}

/// A [`PopupKind`] without what it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupKindId {
    Condition,
    AddPick,
    Order,
    Columns,
    Rows,
    WhereList,
}

impl PopupKind {
    /// The kind, without what it is on.
    pub fn id(&self) -> PopupKindId {
        match self {
            PopupKind::Condition { .. } => PopupKindId::Condition,
            PopupKind::AddPick => PopupKindId::AddPick,
            PopupKind::Order => PopupKindId::Order,
            PopupKind::Columns => PopupKindId::Columns,
            PopupKind::Rows => PopupKindId::Rows,
            PopupKind::WhereList => PopupKindId::WhereList,
        }
    }
}

/// Where a popup is drawn from: the part of the query bar it opened on, if any, and the cells
/// it is placed by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub seg: Option<BarHit>,
    pub at: Rect,
}

impl FilterDialog {
    /// What the dialog shows.
    pub fn view(&self) -> &View {
        &self.view
    }

    /// How many times the view changed: a press made in an earlier view is not one of this.
    pub fn view_gen(&self) -> u64 {
        self.view_gen
    }

    /// The dialog is shown as a popup.
    pub fn is_popup(&self) -> bool {
        matches!(self.view, View::Popup { .. })
    }

    /// The kind of popup shown, when it is one.
    pub fn popup_kind(&self) -> Option<&PopupKind> {
        match &self.view {
            View::Popup { kind, .. } => Some(kind),
            View::Full => None,
        }
    }

    /// The keys stay in the lines the view draws: a popup has no column list and no footer to
    /// move to.
    pub fn confined(&self) -> bool {
        self.is_popup()
    }

    /// Show `view`: the only way the view, or the kind of popup, changes. The draft, its undo,
    /// an apply waiting to be sent (with the columns it hides), the count of values sent and
    /// those counted, and the query before the last change are kept; where things were drawn
    /// (and whether the values had room), what the pointer is over or pressed (the press waits
    /// again before it arms), the operator picker, the lines a column's popup had above it and
    /// the width it was drawn in are dropped, and so is the column search unless the popup picks a column. The whole
    /// dialog never turns into a popup.
    pub fn set_view(&mut self, view: View) {
        if !self.is_popup() && matches!(view, View::Popup { .. }) {
            debug_assert!(false, "the whole filter dialog does not turn into a popup");
            return;
        }
        let picks = matches!(view, View::Popup { kind: PopupKind::AddPick, .. });
        self.view = view;
        self.view_gen = self.view_gen.wrapping_add(1);
        self.hits = DialogHits::default();
        self.hover = None;
        self.press = Default::default();
        self.picker = None;
        self.column_lines = None;
        self.popup_w = 0;
        self.values_drawn = true;
        if !picks && self.search.is_some() {
            let name = self.column().map(|c| c.name.clone());
            self.search = None;
            if let Some(name) = name {
                self.select_column(&name);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
