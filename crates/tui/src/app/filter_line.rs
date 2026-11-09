//! The filter line of a table tab, its fields and keys: `f` (or `Space r f`) edits the
//! condition, `Space r o` the order, `Space r x` clears both and runs the tab again, and
//! `Space r v` opens the statement in a new console.
//!
//! In a field `Enter` (or the run key) applies what both fields hold: the order is read into
//! keys first (refused and said when it is not a list of the table's columns), then the query
//! runs as any run of the tab does ([`App::table_run`]); a refusal leaves the focus on the
//! field it is about, else the keys go back to the grid. `Esc` puts back what is applied and
//! leaves, `Tab` and `Shift+Tab` move between the fields, `Ctrl+L` empties the field (applied
//! with `Enter`), `Ctrl+W` deletes a word, `Ctrl+U` the text before the cursor.

use super::*;
use crate::widgets::filter_line::{Applied, FIELD_CAP};
use datarig_core::query::{self, Filter, KeyOrigin, Query};

/// What the table keys of the grid do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableAction {
    /// Focus the condition (`f`).
    EditFilter,
    /// Focus the order.
    EditOrder,
    /// No condition, no order, and a run.
    Clear,
    /// The statement of the rows shown (else of the draft) in a new console.
    OpenSql,
}

impl App {
    /// A field of the active table tab's filter line has the keys.
    pub fn filter_line_focused(&self) -> bool {
        self.focus == Focus::Results && !self.tabs.is_empty() && self.filter_line_field().is_some()
    }

    /// The field of the active table tab's filter line that has the focus, if one does.
    pub(crate) fn filter_line_field(&self) -> Option<table_query::Field> {
        let t = self.tab();
        t.is_table().then(|| t.doc.table_query.as_ref().and_then(|q| q.line.focus)).flatten()
    }

    /// What table tab `id` holds as applied, as its filter line shows it.
    pub(crate) fn filter_line_applied(&self, id: TabId) -> Applied {
        let dialect = self.tab_dialect(id);
        let Some(q) = self.table_query(id).map(|tq| tq.applied().clone()) else {
            return Applied { filter: String::new(), order: String::new(), keys: Vec::new(), tie: None, dialect };
        };
        let keys: Vec<_> = q.order.iter().filter(|k| k.origin == KeyOrigin::User).cloned().collect();
        let tie = (!keys.is_empty()).then(|| self.table_target(id)).flatten().map(|target| {
            let c = query::compile(&q, &target);
            let ties: Vec<&str> =
                c.order.iter().filter(|k| k.origin == KeyOrigin::TieBreak).map(|k| k.col.as_str()).collect();
            match (ties.is_empty(), c.total) {
                (false, _) => format!("+ {}", ties.join(", ")),
                (true, true) => String::new(),
                (true, false) => self.i18n.label(Label::TableOrderNotUnique).to_string(),
            }
        });
        Applied {
            filter: table_query::filter_text(&q).unwrap_or_default().to_string(),
            order: query::order_text(&keys, dialect),
            keys,
            tie: tie.filter(|t| !t.is_empty()),
            dialect,
        }
    }

    /// The fields of the active table tab follow what it holds as applied where they are not
    /// edited (before they are drawn or read).
    pub(crate) fn filter_line_follow(&mut self) {
        if self.tabs.is_empty() || !self.tab().is_table() {
            return;
        }
        self.filter_line_follow_tab(self.tab().id);
    }

    /// [`App::filter_line_follow`] for table tab `id` (before and after what is applied
    /// changes: a field that held the applied text follows it).
    pub(super) fn filter_line_follow_tab(&mut self, id: TabId) {
        if self.table_query(id).is_none() {
            return;
        }
        let a = self.filter_line_applied(id);
        if let Some(tq) = self.table_query_mut(id) {
            tq.line.follow((&a.filter, &a.order));
        }
    }

    /// The table keys of the grid.
    pub(super) fn table_action(&mut self, a: TableAction) {
        let id = self.tab().id;
        match a {
            TableAction::EditFilter => self.filter_line_focus(id, table_query::Field::Where),
            TableAction::EditOrder => self.filter_line_focus(id, table_query::Field::Order),
            TableAction::Clear => {
                let Some(name) = self.table_name(id) else { return };
                let Some(tq) = self.table_query_mut(id) else { return };
                tq.line.focus = None;
                tq.draft = Query::table(name);
                let q = tq.draft.clone();
                self.table_run(id, q);
            }
            TableAction::OpenSql => {
                let Some(target) = self.table_target(id) else { return };
                let Some(tq) = self.table_query_mut(id) else { return };
                let q = tq.shown.clone().unwrap_or_else(|| tq.draft.clone());
                let sql = query::compile(&q, &target).sql;
                let t = self.tab();
                let (profile, context) = (t.profile, t.context.clone());
                self.console_with(&sql, profile, context);
            }
        }
    }

    /// Give field `f` of table tab `id`'s filter line the keys, the cursor at its end.
    fn filter_line_focus(&mut self, id: TabId, f: table_query::Field) {
        self.close_search_prompt();
        self.filter_line_follow();
        self.focus = Focus::Results;
        if let Some(tq) = self.table_query_mut(id) {
            tq.line.focus = Some(f);
            let input = tq.line.field_mut(f);
            let text = input.text().to_string();
            input.set(&text);
        }
    }

    /// The grid takes the keys back; what the fields hold stays.
    pub(super) fn filter_line_blur(&mut self) {
        if self.tabs.is_empty() || !self.tab().is_table() {
            return;
        }
        let id = self.tab().id;
        if let Some(tq) = self.table_query_mut(id) {
            tq.line.focus = None;
        }
    }

    /// A key while a field of the filter line has the focus.
    pub(super) fn filter_line_key(&mut self, key: KeyEvent) {
        use table_query::Field;
        let id = self.tab().id;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => return self.filter_line_apply(),
            KeyCode::Esc => return self.filter_line_cancel(),
            _ => {}
        }
        let Some(tq) = self.table_query_mut(id) else { return };
        let Some(f) = tq.line.focus else { return };
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                let next = if f == Field::Where { Field::Order } else { Field::Where };
                tq.line.focus = Some(next);
            }
            KeyCode::Char('w') if ctrl => {
                tq.line.field_mut(f).delete_word_back();
            }
            KeyCode::Char('l') if ctrl => tq.line.field_mut(f).set(""),
            _ => {
                tq.line.field_mut(f).handle_key(&key);
            }
        }
    }

    /// Text pasted into the focused field: on one line, the field holding at most
    /// [`FIELD_CAP`] bytes (the rest is left out, and said).
    pub(super) fn filter_line_paste(&mut self, text: &str) {
        let id = self.tab().id;
        let Some(tq) = self.table_query_mut(id) else { return };
        let Some(f) = tq.line.focus else { return };
        let input = tq.line.field_mut(f);
        let room = FIELD_CAP.saturating_sub(input.text().len());
        let mut end = text.len().min(room);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        input.insert_str(&text[..end]);
        if end < text.len() {
            self.flash(Notice::new(Label::TableFilterPasteCut, Level::Warning));
        }
    }

    /// `Esc` in a field: both fields back to what is applied, the keys back to the grid.
    fn filter_line_cancel(&mut self) {
        let id = self.tab().id;
        let a = self.filter_line_applied(id);
        if let Some(tq) = self.table_query_mut(id) {
            tq.line.restore((&a.filter, &a.order));
            tq.line.focus = None;
            tq.refusal = None;
        }
    }

    /// `Enter` (or the run key) in a field: the fields' query runs. The order is read into
    /// keys first and written back as the app writes it; a refused order or filter keeps the
    /// focus on its field, else the grid takes the keys back.
    pub(super) fn filter_line_apply(&mut self) {
        use table_query::{Field, Refusal};
        let id = self.tab().id;
        let Some(target) = self.table_target(id) else { return };
        let d = self.tab_dialect(id);
        let Some(tq) = self.table_query_mut(id) else { return };
        let (filter, order) = (tq.line.where_.text().to_string(), tq.line.order.text().to_string());
        let keys = if order.trim().is_empty() {
            Ok(Vec::new())
        } else if target.columns.is_empty() {
            // Unknown, not absent: no column is said not to exist.
            Err((None, Notice::new(Label::TableOrderColumnsUnknown, Level::Warning)))
        } else {
            query::parse_order(&order, &target).map_err(|e| {
                let why = table_query::order_why(&self.i18n, &e);
                (Some(e.range), Notice::new(Msg::TableOrderRefused { why }, Level::Warning))
            })
        };
        let Some(tq) = self.table_query_mut(id) else { return };
        let keys = match keys {
            Ok(keys) => keys,
            Err((at, notice)) => {
                tq.refusal = Some(Refusal { field: Field::Order, at, notice: notice.clone() });
                tq.line.focus = Some(Field::Order);
                return self.tab_status(id, notice);
            }
        };
        tq.line.order.set(&query::order_text(&keys, d));
        let filter = (!filter.trim().is_empty())
            .then(|| Filter { recognised: query::recognise(&filter, &target), text: filter });
        tq.draft = Query { filter, order: keys, ..tq.draft.clone() };
        tq.refusal = None;
        tq.line.focus = None;
        let q = tq.draft.clone();
        self.table_run(id, q);
        if let Some(tq) = self.table_query_mut(id) {
            tq.line.focus = tq.refusal.as_ref().map(|r| r.field);
        }
    }

    /// A click on the filter line at (x, y): a field takes the focus with the cursor under the
    /// pointer; the refresh mark runs what is applied again.
    pub(super) fn filter_line_click(&mut self, x: u16, y: u16) {
        use table_query::Field;
        let l = self.layout;
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        let id = self.tab().id;
        if inside(l.filter_refresh) {
            self.filter_line_blur();
            let Some(q) = self.table_query_mut(id).map(|tq| tq.applied().clone()) else { return };
            return self.table_run(id, q);
        }
        let f = if inside(l.filter_order) {
            Field::Order
        } else if inside(l.filter_where) {
            Field::Where
        } else {
            return;
        };
        self.filter_line_focus(id, f);
        if let Some(tq) = self.table_query_mut(id) {
            tq.line.field_mut(f).click(x, y);
        }
    }
}
