//! The filter dialog's actions: apply, cancel, undo, clear all, the SQL section (opened,
//! read back, closed) and its `Open in editor`; a filter picked from the history; and the
//! dialog closing once the tab's statement is sent.

use super::draft::{Origin, Where};
use super::*;
use datarig_core::query::{self, OrderKey};
use datarig_core::sql::lexer::Tok;

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
    /// Open or close the SQL section (`Q`).
    Sql,
    /// The server counts the selected column's values (`#`).
    Count,
}

/// An order refused: what to say, and where in its text when that is known.
type OrderRefused = Box<(Notice, Option<Range<usize>>)>;

/// Whether text `sql` (dialect `d`), shown in a field of one line with a space for each line
/// break, would read back as other SQL: a line break inside a string, a quoted name or a
/// comment, or one that ends a line comment (the comment would run on over what follows).
pub(super) fn breaks_lost(sql: &str, d: Dialect) -> bool {
    let broken = |t: &str| t.contains(['\n', '\r']);
    datarig_core::sql::lexer::lex_in(sql, d).iter().any(|t| match t.kind {
        Tok::Whitespace => false,
        // A line comment runs to the end of its line: what follows it would join it.
        Tok::LineComment => !sql[t.end..].trim().is_empty(),
        _ => broken(t.text(sql)),
    })
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
            DialogAction::Count => self.filter_dialog_count(),
        }
    }

    /// `Q`, or the `SQL` toggle: the SQL section opens with the draft's texts in its fields
    /// (the keys in its `WHERE` field), or closes ([`App::filter_dialog_close_sql`]).
    pub(super) fn filter_dialog_toggle_sql(&mut self) {
        let Some(d) = self.filter_dialog_mut() else { return };
        if d.sql.is_some() {
            self.filter_dialog_close_sql();
            return;
        }
        d.sql = Some(SqlSection::of(&d.draft, d.dialect));
        d.focus_on(DialogFocus::SqlWhere);
    }

    /// The SQL section read back ([`App::filter_dialog_read_back`]) and closed; one refused
    /// stays open. `false` when it stays.
    pub(super) fn filter_dialog_close_sql(&mut self) -> bool {
        if !self.filter_dialog_read_back() {
            return false;
        }
        self.filter_dialog_popup_close();
        let Some(d) = self.filter_dialog_mut() else { return true };
        d.sql = None;
        d.settle();
        true
    }

    /// What is typed in the SQL section's fields, into the draft: a changed `ORDER BY` text is
    /// read into keys of the table's columns, a changed `WHERE` text replaces the conditions
    /// (as text given anew: its terms read again, sent as written); each one step of undo. A
    /// field not typed in changes nothing (its text is the draft's, byte for byte). An order
    /// that is not one is refused: said, underlined where it is wrong, its field focused, and
    /// nothing is read back (`false`). The fields then hold the draft's texts again. A field
    /// typed in whose text had a line break its one line would change ([`breaks_lost`]) is
    /// refused too, said, and keeps the keys.
    pub(super) fn filter_dialog_read_back(&mut self) -> bool {
        let Some(d) = self.filter_dialog() else { return true };
        let Some(sql) = d.sql.as_ref() else { return true };
        let tab = d.tab;
        let where_ = sql.where_.edited().then(|| sql.where_.input.text().to_string());
        let order = sql.order.edited().then(|| sql.order.input.text().to_string());
        // A field given a text its one line does not hold is not read back once typed in: the
        // spaces in place of its line breaks would change what it says.
        let lost = match (where_.is_some() && breaks_lost(sql.where_.from(), d.dialect), order.is_some()) {
            (true, _) => Some(DialogFocus::SqlWhere),
            (false, true) if breaks_lost(sql.order.from(), d.dialect) => Some(DialogFocus::SqlOrder),
            _ => None,
        };
        if let Some(field) = lost {
            self.filter_dialog_popup_close();
            let Some(d) = self.filter_dialog_mut() else { return false };
            d.message = Some(Notice::new(Label::TableDialogSqlLineBreak, Level::Warning));
            d.focus_on(field);
            return false;
        }
        let Some(target) = self.table_target(tab) else { return true };
        let keys = match order.map(|text| self.dialog_order_keys(&text, &target)) {
            None => None,
            Some(Ok(keys)) => Some(keys),
            Some(Err(refused)) => {
                let (n, at) = *refused;
                self.filter_dialog_popup_close();
                let Some(d) = self.filter_dialog_mut() else { return false };
                let len = d.sql.as_ref().map_or(0, |s| s.order.input.text().len());
                d.message = Some(n);
                d.mark = Some(Mark::Order(at.unwrap_or(0..len)));
                d.focus_on(DialogFocus::SqlOrder);
                return false;
            }
        };
        if let Some(text) = where_ {
            self.filter_dialog_edit(None, |d, target| d.draft.where_.set_text(&text, target));
        }
        if let Some(keys) = keys {
            self.filter_dialog_edit(None, |d, _| d.draft.order = keys);
        }
        if let Some(d) = self.filter_dialog_mut()
            && d.sql.is_some()
        {
            d.sql = Some(SqlSection::of(&d.draft, d.dialect));
        }
        true
    }

    /// `ORDER BY` text `text` as keys of `target`'s columns; refused: what to say and where
    /// (bytes of `text`) when that is known.
    fn dialog_order_keys(&self, text: &str, target: &Target) -> Result<Vec<OrderKey>, OrderRefused> {
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        if target.columns.is_empty() {
            return Err(Box::new((Notice::new(Label::TableOrderColumnsUnknown, Level::Warning), None)));
        }
        query::parse_order(text, target).map_err(|e| {
            let why = table_query::order_why(&self.i18n, &e);
            Box::new((Notice::new(Msg::TableOrderRefused { why }, Level::Warning), Some(e.range)))
        })
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
        self.filter_dialog_popup_close();
        if !self.filter_dialog_read_back() {
            return;
        }
        if self.tab_busy(tab) {
            let key = self.key_for(Action::CancelQuery, Ctx::Nav);
            return self.say_over_filter_dialog(Notice::new(Msg::QueryBusy { key }, Level::Warning));
        }
        let Some(q) = self.filter_dialog_query() else { return };
        let Some(d) = self.filter_dialog_mut() else { return };
        // Only the columns hidden changed: they are, and nothing runs (a view only).
        let hidden = (!d.draft.same_hidden(&d.applied)).then(|| d.draft.hidden.clone());
        if let Some(h) = &hidden
            && d.draft.same_as(&d.applied)
        {
            let h = h.clone();
            self.overlays.close(OverlayKind::FilterDialog);
            self.set_hidden_columns(tab, &h, true);
            return;
        }
        d.applying_hidden = hidden;
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
        self.filter_dialog_told(tab);
    }

    /// The draft as the query it sends, of the tab's table; a `LIMIT` field that is no cap is
    /// refused (said, underlined) and `None`.
    fn filter_dialog_query(&mut self) -> Option<Query> {
        let tab = self.filter_dialog()?.tab;
        let (target, base) = (self.table_target(tab)?, self.table_query(tab)?.applied().clone());
        let d = self.filter_dialog()?;
        match d.draft.to_query(&base, &target) {
            Ok(q) => Some(q),
            Err((at, problem)) => {
                let why = table_query::limit_why(&self.i18n, problem);
                let d = self.filter_dialog_mut()?;
                d.message = Some(Notice::new(Msg::TableLimitRefused { why }, Level::Warning));
                d.mark = Some(Mark::Limit(at));
                None
            }
        }
    }

    /// What table tab `tab` said to the draft it was given (a refusal, else a notice; nothing
    /// while a question is open above the dialog), in the dialog's hint line, the part refused
    /// underlined.
    fn filter_dialog_told(&mut self, tab: TabId) {
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
                (table_query::Field::Where, Some(at)) => {
                    Some(Mark::Where { term: term_at(&d.draft.where_, at), at: at.clone() })
                }
                (table_query::Field::Limit, at) => Some(Mark::Limit(at.clone().unwrap_or(0..d.draft.limit.len()))),
                _ => None,
            };
        }
        if !asked {
            d.message = said;
        }
    }

    /// `Open in editor`: the draft's statement in a new console, checked first as the tab's
    /// `Space r v` checks it (its functions asked about first, `y` opening it); not run. Opened,
    /// the dialog closes without applying the draft.
    pub(super) fn filter_dialog_open_sql(&mut self) {
        let Some(tab) = self.filter_dialog().map(|d| d.tab) else { return };
        self.filter_dialog_popup_close();
        if !self.filter_dialog_read_back() {
            return;
        }
        let Some(q) = self.filter_dialog_query() else { return };
        if let Some(d) = self.filter_dialog_mut() {
            d.message = None;
            d.mark = None;
        }
        if let Some(tq) = self.table_query_mut(tab) {
            tq.refusal = None;
        }
        self.status = None;
        self.transient = None;
        match self.table_query_sql_to_open(tab, q, table_query::OpenIn::Dialog) {
            Some(sql) => self.filter_dialog_opened(tab, &sql),
            None => self.filter_dialog_told(tab),
        }
    }

    /// The statement `sql` of table tab `tab`'s dialog's draft, checked: the dialog closes
    /// (its draft not applied) and a new console holds it.
    pub(crate) fn filter_dialog_opened(&mut self, tab: TabId, sql: &str) {
        if self.filter_dialog().is_some_and(|d| d.tab == tab) {
            self.filter_dialog_popup_close();
            self.overlays.close(OverlayKind::FilterDialog);
        }
        if let Some(tq) = self.table_query_mut(tab) {
            tq.draft = tq.applied().clone();
            tq.refusal = None;
        }
        let Some(t) = self.tabs.get(tab) else { return };
        let (profile, context) = (t.profile, t.context.clone());
        self.console_with(sql, profile, context);
    }

    /// `Esc`: the open picker closes, a count of the values running is cancelled, the search
    /// or the SQL section closes (read back first: an order refused keeps it open); else the
    /// dialog, its draft dropped (the tab's next run sends what is applied).
    fn filter_dialog_cancel(&mut self) {
        let Some(d) = self.filter_dialog_mut() else { return };
        if d.picker.take().is_some() {
            return;
        }
        if self.filter_dialog_cancel_count() {
            return;
        }
        let Some(d) = self.filter_dialog_mut() else { return };
        if d.search.is_some() && d.focus == DialogFocus::Columns {
            let name = d.column().map(|c| c.name.clone());
            d.search = None;
            if let Some(name) = name {
                d.select_column(&name);
            }
            return;
        }
        if d.sql.is_some() {
            self.filter_dialog_close_sql();
            return;
        }
        let (tab, applying) = (d.tab, d.applying.take());
        self.filter_dialog_popup_close();
        self.overlays.close(OverlayKind::FilterDialog);
        // The query applied and still waiting (for a connection, the session's state, the
        // table's keys) is cancelled with the dialog: it never runs.
        if let Some(q) = applying
            && self.table_cancel_waiting(tab, &q)
        {
            self.tab_status(tab, Notice::new(Label::QueryCancelled, Level::Warning));
        }
        if let Some(tq) = self.table_query_mut(tab) {
            tq.draft = tq.applied().clone();
            tq.refusal = None;
        }
    }

    /// `Ctrl+Z`, `u`: what is typed in the SQL section's field with the keys and not read back
    /// yet goes; else the draft one step back; with no step left, the tab's query before its
    /// last applied change (once).
    fn filter_dialog_undo(&mut self) {
        self.filter_dialog_popup_close();
        let Some(d) = self.filter_dialog_mut() else { return };
        if d.sql_field().is_some_and(SqlField::edited) {
            let (focus, dialect) = (d.focus, d.dialect);
            let fresh = SqlSection::of(&d.draft, dialect);
            if let Some(s) = d.sql.as_mut() {
                match focus {
                    DialogFocus::SqlWhere => s.where_ = fresh.where_,
                    _ => s.order = fresh.order,
                }
            }
            d.message = None;
            d.mark = None;
            return;
        }
        let Some(prev) = d.undo.undo().or_else(|| d.before_last.take()) else {
            d.message = Some(Notice::new(Label::TableDialogNothingToUndo, Level::Info));
            return;
        };
        let before = std::mem::replace(&mut d.draft, prev);
        d.picker = None;
        d.message = None;
        d.mark = None;
        d.follow_terms(&before.where_, false);
        d.settle();
    }

    /// Table tab `id` sent its query's statement: its filter dialog, waiting for the query it
    /// applied, closes.
    pub(crate) fn filter_dialog_sent(&mut self, id: TabId) {
        let requested = self.table_query(id).and_then(|tq| tq.requested.as_ref().map(|(_, q)| q.clone()));
        let Some(d) = self.filter_dialog() else { return };
        let waiting = d.applying.as_ref().zip(requested.as_ref()).is_some_and(|(a, r)| table_query::same_query(a, r));
        if d.tab == id && waiting {
            let hidden = d.applying_hidden.clone();
            self.overlays.close(OverlayKind::FilterDialog);
            // The columns hidden with it: the run sent kept what was hidden before it.
            if let Some(h) = hidden {
                self.set_hidden_columns(id, &h, false);
            }
        }
    }

    /// A filter picked from table tab `tab`'s history (`filter`, `order` as stored): into the
    /// open dialog's draft as one step, else into a dialog opened with it; nothing runs. An
    /// order that is not of the table's columns is said, and the draft keeps its order.
    pub(crate) fn filter_dialog_from_history(&mut self, tab: TabId, filter: &str, order: &str) {
        let Some(target) = self.table_target(tab) else { return };
        let keys = self.dialog_order_keys(order, &target).map_err(|r| r.0);
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
        // The pick replaces what the SQL section's fields hold too.
        self.filter_dialog_popup_close();
        self.filter_dialog_edit(None, |d, target| {
            d.draft.where_ = Where::from_text(&filter, target);
            if let Some(keys) = ok {
                d.draft.order = keys;
            }
        });
        let Some(d) = self.filter_dialog_mut() else { return };
        if d.sql.is_some() {
            d.sql = Some(SqlSection::of(&d.draft, d.dialect));
        }
        if let Err(n) = keys {
            d.message = Some(n);
        }
    }
}
