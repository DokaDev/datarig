//! The filter dialog's actions: apply, cancel, undo, clear all, the SQL toggle; a filter
//! picked from the history; and the dialog closing once the tab's statement is sent.

use super::draft::{Origin, Where};
use super::*;
use datarig_core::query::{self, OrderKey};
use std::ops::Range;

/// What the keys of the filter dialog do through the key map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogAction {
    /// Apply the draft and close once it is sent (`Enter`; a picker or the search: pick).
    Apply,
    /// Close without applying (`Esc`; a picker, the search or the SQL: close that first).
    Cancel,
    /// One step back (`Ctrl+Z`, `u`): with none left, the query before the last applied change.
    Undo,
    /// No condition and no order (the limit stays).
    ClearAll,
    /// Show or hide the statement (`Q`).
    Sql,
}

/// The term of `w` whose text holds byte `at.start` of the text sent ([`Where::text`]).
pub(super) fn term_at(w: &Where, at: &Range<usize>) -> Option<usize> {
    let text = w.text();
    let mut spans: Vec<(usize, Range<usize>)> = Vec::new();
    let mut from = 0;
    for (i, t) in w.terms().iter().enumerate() {
        match (&t.origin, &t.text) {
            (Origin::Original(r), _) if !w.rebuilt() => spans.push((i, r.clone())),
            (_, Some(tt)) if w.rebuilt() => {
                if let Some(p) = text.get(from..).and_then(|rest| rest.find(tt.as_str())) {
                    spans.push((i, from + p..from + p + tt.len()));
                    from += p + tt.len();
                }
            }
            _ => {}
        }
    }
    spans.into_iter().find(|(_, r)| r.start <= at.start && at.start < r.end.max(r.start + 1)).map(|(i, _)| i)
}

impl App {
    /// An action of the filter dialog's keys.
    pub(crate) fn filter_dialog_action(&mut self, a: DialogAction) {
        match a {
            DialogAction::Apply => self.filter_dialog_apply(),
            DialogAction::Cancel => self.filter_dialog_cancel(),
            DialogAction::Undo => self.filter_dialog_undo(),
            DialogAction::ClearAll => self.filter_dialog_edit(None, |d, _| {
                d.draft.where_.clear();
                d.draft.order.clear();
            }),
            DialogAction::Sql => self.filter_dialog_toggle_sql(),
        }
    }

    /// `Q`, or the `SQL` toggle: the statement shown under the footer, or not.
    pub(super) fn filter_dialog_toggle_sql(&mut self) {
        if let Some(d) = self.filter_dialog_mut() {
            d.sql_open = !d.sql_open;
        }
    }

    /// Notice `n` where the user reads it: in the open filter dialog's hint line (it covers the
    /// status line), else on the status line.
    pub(crate) fn say_over_filter_dialog(&mut self, n: Notice) {
        match self.filter_dialog_mut() {
            Some(d) => d.message = Some(n),
            None => self.flash(n),
        }
    }

    /// `Enter`: the open picker picks, the open search keeps its column; else the draft runs
    /// as any run of the tab ([`App::table_run`]). A refusal of the app keeps the dialog open,
    /// says why in its hint line and underlines the chip (or the limit) refused; a question
    /// opens above it; the dialog closes once the statement is sent
    /// ([`App::filter_dialog_sent`]). Conditions still waiting for a value are not sent.
    fn filter_dialog_apply(&mut self) {
        let Some(d) = self.filter_dialog_mut() else { return };
        if d.picker.is_some() {
            return self.filter_dialog_pick();
        }
        if d.search.is_some() && d.focus == DialogFocus::Columns {
            let name = d.column().map(|c| c.name.clone());
            d.search = None;
            if let Some(name) = name {
                d.select_column(&name);
            }
            return;
        }
        let tab = d.tab;
        if self.tab_busy(tab) {
            let key = self.key_for(Action::CancelQuery, Ctx::Nav);
            return self.say_over_filter_dialog(Notice::new(Msg::QueryBusy { key }, Level::Warning));
        }
        let (Some(target), Some(base)) = (self.table_target(tab), self.table_query(tab).map(|tq| tq.applied().clone()))
        else {
            return;
        };
        let Some(d) = self.filter_dialog_mut() else { return };
        let q = match d.draft.to_query(&base, &target) {
            Ok(q) => q,
            Err((_, problem)) => {
                let why = table_query::limit_why(&self.i18n, problem);
                let Some(d) = self.filter_dialog_mut() else { return };
                d.message = Some(Notice::new(Msg::TableLimitRefused { why }, Level::Warning));
                d.mark = Some(Mark::Limit);
                return;
            }
        };
        d.applying = Some(q.clone());
        d.message = None;
        d.mark = None;
        if let Some(tq) = self.table_query_mut(tab) {
            tq.draft = q.clone();
            tq.refusal = None;
        }
        // What the run says while the dialog covers the status line is said in the dialog.
        self.status = None;
        self.transient = None;
        self.table_run(tab, q);
        if self.filter_dialog().is_none_or(|d| d.tab != tab) {
            return;
        }
        let refusal = self.table_query(tab).and_then(|tq| tq.refusal.clone());
        let said = refusal
            .as_ref()
            .map(|r| r.notice.clone())
            .or_else(|| self.transient.as_ref().map(|(n, _)| n.clone()))
            .or_else(|| self.status.clone());
        let asked = self.overlays.top().is_some_and(|o| o.kind() != OverlayKind::FilterDialog);
        let Some(d) = self.filter_dialog_mut() else { return };
        if let Some(r) = &refusal {
            d.applying = None;
            d.mark = match (r.field, &r.at) {
                (table_query::Field::Where, Some(at)) => term_at(&d.draft.where_, at).map(Mark::Term),
                (table_query::Field::Limit, _) => Some(Mark::Limit),
                _ => None,
            };
        }
        if !asked {
            d.message = said;
        }
    }

    /// `Esc`: the open picker, the search or the statement closes; else the dialog, its draft
    /// dropped (the tab's next run sends what is applied).
    fn filter_dialog_cancel(&mut self) {
        let Some(d) = self.filter_dialog_mut() else { return };
        if d.picker.take().is_some() {
            return;
        }
        if d.search.is_some() && d.focus == DialogFocus::Columns {
            let name = d.column().map(|c| c.name.clone());
            d.search = None;
            if let Some(name) = name {
                d.select_column(&name);
            }
            return;
        }
        if std::mem::take(&mut d.sql_open) {
            return;
        }
        let tab = d.tab;
        self.overlays.close(OverlayKind::FilterDialog);
        if let Some(tq) = self.table_query_mut(tab) {
            tq.draft = tq.applied().clone();
            tq.refusal = None;
        }
    }

    /// `Ctrl+Z`, `u`: the draft one step back; with no step left, the tab's query before its
    /// last applied change (once).
    fn filter_dialog_undo(&mut self) {
        let Some(d) = self.filter_dialog_mut() else { return };
        let Some(prev) = d.undo.undo().or_else(|| d.before_last.take()) else {
            d.message = Some(Notice::new(Label::TableDialogNothingToUndo, Level::Info));
            return;
        };
        d.draft = prev;
        d.picker = None;
        d.message = None;
        d.mark = None;
        d.settle();
        if let DialogFocus::Value(t, _) = d.focus {
            d.fill_inputs(t);
        }
    }

    /// Table tab `id` sent its query's statement: its filter dialog, waiting for the query it
    /// applied, closes.
    pub(crate) fn filter_dialog_sent(&mut self, id: TabId) {
        let requested = self.table_query(id).and_then(|tq| tq.requested.as_ref().map(|(_, q)| q.clone()));
        let Some(d) = self.filter_dialog() else { return };
        let waiting = d.applying.as_ref().zip(requested.as_ref()).is_some_and(|(a, r)| table_query::same_query(a, r));
        if d.tab == id && waiting {
            self.overlays.close(OverlayKind::FilterDialog);
        }
    }

    /// A filter picked from table tab `tab`'s history (`filter`, `order` as stored): into the
    /// open dialog's draft as one step, else into a dialog opened with it; nothing runs. An
    /// order that is not of the table's columns is said, and the draft keeps its order.
    pub(crate) fn filter_dialog_from_history(&mut self, tab: TabId, filter: &str, order: &str) {
        let Some(target) = self.table_target(tab) else { return };
        let keys: Result<Vec<OrderKey>, Notice> = if order.trim().is_empty() {
            Ok(Vec::new())
        } else if target.columns.is_empty() {
            Err(Notice::new(Label::TableOrderColumnsUnknown, Level::Warning))
        } else {
            query::parse_order(order, &target).map_err(|e| {
                let why = table_query::order_why(&self.i18n, &e);
                Notice::new(Msg::TableOrderRefused { why }, Level::Warning)
            })
        };
        if self.filter_dialog().is_none_or(|d| d.tab != tab) {
            let Some(mut q) = self.table_query(tab).map(|tq| tq.applied().clone()) else { return };
            q.filter = (!filter.trim().is_empty())
                .then(|| query::Filter { recognised: query::recognise(filter, &target), text: filter.to_string() });
            if let Ok(keys) = &keys {
                q.order = keys.clone();
            }
            self.open_filter_dialog_with(tab, &q, None, false);
            if let Some(d) = self.filter_dialog_mut() {
                // Picked, not applied: the title says so.
                d.message = keys.err();
            }
            return;
        }
        let filter = filter.to_string();
        let ok = keys.as_ref().ok().cloned();
        self.filter_dialog_edit(None, |d, target| {
            d.draft.where_ = Where::from_text(&filter, target);
            if let Some(keys) = ok {
                d.draft.order = keys;
            }
        });
        if let (Err(n), Some(d)) = (keys, self.filter_dialog_mut()) {
            d.message = Some(n);
        }
    }
}
