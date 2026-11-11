//! The filter and sort dialog of a table tab (`f`, `Space r f`; `Space r o` on the order): the
//! conditions, order and cap of the table's rows, built as chips and keys and applied as one
//! query through the tab's run ([`App::table_run`]), every check and question included.
//!
//! [`draft`] holds what the dialog edits, as data: no drawing, no keys, no app state. This
//! module holds the dialog's state ([`FilterDialog`]): the table's columns, the draft and what
//! is applied, where the keys are, the operator picker, the column search, and what the last
//! apply was told. The keys are in [`keys`], applying, undoing and closing in [`apply`].
//!
//! The dialog is modal and covers the whole screen, the key bar included (it has its own hint
//! line). Under the selected column's conditions it lists the column's values ([`list`]):
//! counted in the rows loaded, remembered from an earlier load of the table ([`values`]), or
//! counted by the server on request ([`count`]); ticking one writes the column's condition.
//! Applying keeps it open until the tab's statement is sent: a refusal of the app (the
//! filter's check, the limit, a busy tab, a read-only profile) is said in its hint line and
//! the part refused underlined; a question (functions to confirm, reads that lock rows, edits
//! to leave) opens above it, and a no leaves it as it was. `Esc` closes it and drops the draft.

mod apply;
pub(crate) mod count;
pub(crate) mod draft;
mod keys;
pub(crate) mod list;
pub(crate) mod values;

pub use apply::DialogAction;
pub(crate) use keys::op_off_label;

use std::collections::{BTreeSet, HashMap};

use super::table_query::OrderKeysState;
use super::*;
use crate::widgets::text_input::TextInput;
use datarig_core::query::{ColName, Query, Target};
use draft::{ColumnInfo, Draft, Undo};

/// A text field of the dialog typing coalesces in ([`Undo`]): a value field of a condition
/// (term index, field) or the `LIMIT` field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FieldKey {
    Value(usize, usize),
    Limit,
}

/// Where the dialog's keys go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialogFocus {
    /// The column list (with the search field while it is open).
    Columns,
    /// The operator of the condition of term `.0`.
    Op(usize),
    /// Value field `.1` of the condition of term `.0`.
    Value(usize, usize),
    /// `+ add condition` under the selected column's conditions.
    Add,
    /// The selected column's sort line.
    Sort,
    /// The chip at this place among the chips of the footer's `Where` row.
    Chips(usize),
    /// The key at this place of the footer's `Order` row.
    Order(usize),
    /// The footer's `LIMIT` field.
    Limit,
    /// The footer's `SQL` toggle.
    Sql,
    /// The value at this place of the selected column's value list ([`list::ValuesView`]).
    Values(usize),
}

/// Where the dialog's columns come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ColumnSource {
    /// The table's catalogue, in its order.
    Catalogue,
    /// The catalogue is not read yet: the columns of the rows loaded.
    Result,
    /// The catalogue could not be read (`.0` says why): the columns of the rows loaded.
    ResultFailed(String),
}

/// The part of the dialog the last refusal is about, underlined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    /// The chip of this term.
    Term(usize),
    Limit,
}

/// Where the parts of the dialog were drawn last, for the pointer.
#[derive(Clone, Debug, Default)]
pub(crate) struct DialogHits {
    pub(crate) columns: Vec<(Rect, usize)>,
    pub(crate) ops: Vec<(Rect, usize)>,
    pub(crate) values: Vec<(Rect, usize, usize)>,
    /// The values of the list: the line of each, its place in the list.
    pub(crate) items: Vec<(Rect, usize)>,
    /// `# count exactly`.
    pub(crate) count: Rect,
    pub(crate) removes: Vec<(Rect, usize)>,
    pub(crate) add: Rect,
    pub(crate) sort: Vec<(Rect, Option<datarig_core::query::Dir>)>,
    pub(crate) picker: Vec<(Rect, usize)>,
    pub(crate) chips: Vec<(Rect, usize)>,
    pub(crate) chip_removes: Vec<(Rect, usize)>,
    pub(crate) keys: Vec<(Rect, usize)>,
    pub(crate) undo: Rect,
    pub(crate) clear_all: Rect,
    pub(crate) limit: Rect,
    pub(crate) sql: Rect,
    pub(crate) apply: Rect,
    pub(crate) cancel: Rect,
}

/// The filter and sort dialog of table tab [`FilterDialog::tab`].
pub struct FilterDialog {
    pub(crate) tab: TabId,
    /// The table as the title names it (`schema.table`).
    pub(crate) table: String,
    pub(crate) dialect: Dialect,
    /// The table's columns, in the table's order.
    pub(crate) columns: Vec<ColumnInfo>,
    pub(crate) source: ColumnSource,
    /// Columns hidden from the grid (not offered yet: always empty).
    #[allow(dead_code)]
    pub(crate) hidden: BTreeSet<ColName>,
    /// How many rows the tab holds of its query; `None` when it holds none.
    pub(crate) loaded: Option<usize>,
    pub(crate) draft: Draft,
    /// What the tab holds as applied, as a draft: the title says `edited` while they differ.
    pub(crate) applied: Draft,
    /// The tab's query before its last applied change: an undo with no step left goes back
    /// to it.
    pub(crate) before_last: Option<Draft>,
    pub(crate) undo: Undo<FieldKey>,
    pub(crate) focus: DialogFocus,
    /// The selected column, among those the search leaves ([`FilterDialog::shown_columns`]).
    pub(crate) cursor: usize,
    /// The first column row shown.
    pub(crate) scroll: usize,
    /// The column search, while it is open.
    pub(crate) search: Option<TextInput>,
    /// The operator picker of the focused condition, open: the place of its selection among
    /// the operators offered.
    pub(crate) picker: Option<usize>,
    /// The value fields of the focused condition.
    pub(crate) inputs: Vec<TextInput>,
    pub(crate) limit: TextInput,
    /// What the dialog says in its hint line instead of the keys: a refusal, a failure, a note.
    pub(crate) message: Option<Notice>,
    pub(crate) mark: Option<Mark>,
    /// The statement shown under the footer (`Q`).
    pub(crate) sql_open: bool,
    /// The query applied, waiting for its statement to be sent (questions asked meanwhile).
    pub(crate) applying: Option<Query>,
    pub(crate) hits: DialogHits,
    /// The value field typed in narrows the value list: its term and the text it looks for.
    pub(crate) narrow: Option<(usize, String)>,
    /// The first value of the list shown.
    pub(crate) values_scroll: usize,
    /// How many values the list of the selected column shows now (kept as the list is made).
    pub(crate) listed: usize,
    /// The column the list was made for last: another one starts its list from the top.
    pub(crate) list_of: Option<String>,
    /// The selected column's values counted in the rows loaded, for those rows.
    pub(crate) tally: Option<(list::TallyOf, values::Tally)>,
    /// The count of a column's values sent and not answered.
    pub(crate) counting: Option<count::Counting>,
    /// The counts of the server, by column.
    pub(crate) exact: HashMap<String, count::Exact>,
}

impl FilterDialog {
    /// The columns the search leaves, as indices of [`FilterDialog::columns`].
    pub(crate) fn shown_columns(&self) -> Vec<usize> {
        let needle = self.search.as_ref().map(|s| s.text().to_lowercase()).unwrap_or_default();
        (0..self.columns.len())
            .filter(|&i| needle.is_empty() || self.columns[i].name.to_lowercase().contains(&needle))
            .collect()
    }

    /// The selected column, when there is one.
    pub(crate) fn column(&self) -> Option<&ColumnInfo> {
        self.shown_columns().get(self.cursor).map(|&i| &self.columns[i])
    }

    /// The terms of the selected column's conditions.
    pub(crate) fn rows(&self) -> Vec<usize> {
        self.column().map(|c| self.draft.where_.rows_of(&c.name)).unwrap_or_default()
    }

    /// The draft sends something else than what is applied.
    pub(crate) fn edited(&self) -> bool {
        !self.draft.same_as(&self.applied)
    }

    /// A text field has the keys: what is typed goes into it.
    pub(crate) fn typing(&self) -> bool {
        self.picker.is_none()
            && (self.search.is_some() && self.focus == DialogFocus::Columns
                || matches!(self.focus, DialogFocus::Value(..) | DialogFocus::Limit))
    }

    /// The text field with the keys.
    pub(crate) fn input_mut(&mut self) -> Option<&mut TextInput> {
        if self.picker.is_some() {
            return None;
        }
        match self.focus {
            DialogFocus::Columns => self.search.as_mut(),
            DialogFocus::Value(_, k) => self.inputs.get_mut(k),
            DialogFocus::Limit => Some(&mut self.limit),
            _ => None,
        }
    }

    /// Column `name` selected (the search closed when it hides it).
    pub(crate) fn select_column(&mut self, name: &str) {
        if !self.shown_columns().iter().any(|&i| self.columns[i].name == name) {
            self.search = None;
        }
        if let Some(at) = self.shown_columns().iter().position(|&i| self.columns[i].name == name) {
            self.cursor = at;
        }
    }

    /// The value fields of term `t`'s condition, as its values are now.
    pub(crate) fn fill_inputs(&mut self, t: usize) {
        let d = self.dialect;
        self.inputs = match self.draft.where_.terms().get(t).and_then(|t| t.row.as_ref()) {
            Some(row) => match row.op.arity() {
                draft::Arity::Zero => Vec::new(),
                draft::Arity::Two => {
                    let v = row.value_texts(d);
                    (0..2).map(|i| TextInput::new(v.get(i).map_or("", String::as_str))).collect()
                }
                _ => vec![TextInput::new(&row.input_text(d))],
            },
            None => Vec::new(),
        };
    }

    /// The focus moved: typing starts a new undo step, and the value fields are those of the
    /// condition now focused.
    pub(crate) fn focus_on(&mut self, f: DialogFocus) {
        if f != self.focus {
            self.undo.leave_field();
        }
        // The search narrows the list while the list has the keys; leaving it keeps the column.
        if self.focus == DialogFocus::Columns && f != DialogFocus::Columns && self.search.is_some() {
            let name = self.column().map(|c| c.name.clone());
            self.search = None;
            if let Some(name) = name {
                self.select_column(&name);
            }
        }
        // The list narrows while its value field or the list itself has the keys.
        if let Some((t, _)) = self.narrow
            && !matches!(f, DialogFocus::Value(x, _) if x == t)
            && !matches!(f, DialogFocus::Values(_))
        {
            self.narrow = None;
        }
        let same_term = matches!((self.focus, f), (DialogFocus::Op(a) | DialogFocus::Value(a, _), DialogFocus::Op(b) | DialogFocus::Value(b, _)) if a == b);
        self.focus = f;
        if let DialogFocus::Op(t) | DialogFocus::Value(t, _) = f
            && !same_term
        {
            self.fill_inputs(t);
        }
        if let DialogFocus::Value(t, k) = f
            && self.inputs.len() <= k
        {
            self.focus = if k > 0 && !self.inputs.is_empty() { DialogFocus::Value(t, 0) } else { DialogFocus::Op(t) };
        }
    }

    /// The lines of the right pane the keys move through, top to bottom.
    pub(crate) fn pane_lines(&self) -> Vec<DialogFocus> {
        let mut lines: Vec<DialogFocus> = self.rows().into_iter().map(DialogFocus::Op).collect();
        if self.column().is_some() {
            lines.push(DialogFocus::Add);
            if self.listed > 0 {
                lines.push(DialogFocus::Values(0));
            }
            lines.push(DialogFocus::Sort);
        }
        lines
    }

    /// The focus kept where the draft still has it: a term gone, a chip or key past the last.
    pub(crate) fn settle(&mut self) {
        let terms = self.draft.where_.terms().len();
        let chips = self.draft.where_.chips().len();
        let keys = self.draft.order.len();
        self.focus = match self.focus {
            DialogFocus::Op(t) | DialogFocus::Value(t, _) if t >= terms || !self.rows().contains(&t) => {
                if self.column().is_some() { DialogFocus::Add } else { DialogFocus::Columns }
            }
            DialogFocus::Chips(_) if chips == 0 => DialogFocus::Columns,
            DialogFocus::Chips(i) => DialogFocus::Chips(i.min(chips - 1)),
            DialogFocus::Order(_) if keys == 0 => DialogFocus::Columns,
            DialogFocus::Order(i) => DialogFocus::Order(i.min(keys - 1)),
            DialogFocus::Values(_) if self.listed == 0 => DialogFocus::Add,
            DialogFocus::Values(i) => DialogFocus::Values(i.min(self.listed - 1)),
            f => f,
        };
        if let DialogFocus::Op(t) = self.focus {
            self.fill_inputs(t);
        }
        if let DialogFocus::Value(t, k) = self.focus
            && k >= self.inputs.len()
        {
            self.focus = DialogFocus::Op(t);
        }
        if self.limit.text() != self.draft.limit {
            self.limit.set(&self.draft.limit);
        }
        let n = self.shown_columns().len();
        self.cursor = self.cursor.min(n.saturating_sub(1));
    }
}

impl App {
    /// The open filter dialog.
    pub(crate) fn filter_dialog(&self) -> Option<&FilterDialog> {
        self.overlays.iter().find_map(|o| if let Overlay::FilterDialog(d) = o { Some(&**d) } else { None })
    }

    /// The open filter dialog's draft as it would be sent: its `WHERE` text and its order as
    /// the app writes it.
    pub fn filter_dialog_draft(&self) -> Option<(String, String)> {
        let d = self.filter_dialog()?;
        Some((d.draft.where_.text(), datarig_core::query::order_text(&d.draft.order, d.dialect)))
    }

    pub(crate) fn filter_dialog_mut(&mut self) -> Option<&mut FilterDialog> {
        self.overlays.filter_dialog_mut()
    }

    /// `f`, `Space r f` (`order`: `Space r o`): the filter dialog of the active table tab, on
    /// the grid's column, with the query its next run sends as its draft (what is applied, or
    /// what was asked for last and refused, declined or failed; a run the server refused: what
    /// the server said); `order`: the keys on the footer's
    /// `Order` row (the column's sort line while there is none).
    pub(super) fn open_filter_dialog(&mut self, order: bool) {
        let id = self.tab().id;
        let Some(q) = self.table_query_mut(id).map(|tq| tq.draft.clone()) else { return };
        let column = match &self.tab().results {
            Results::Rows(rs) => rs.columns.get(self.tab().grid.col).map(|c| c.meta.name.clone()),
            _ => None,
        };
        self.open_filter_dialog_with(id, &q, column.as_deref(), order);
    }

    /// The filter dialog of table tab `id` with `q` as its draft, on column `column`.
    pub(super) fn open_filter_dialog_with(&mut self, id: TabId, q: &Query, column: Option<&str>, order: bool) {
        self.close_search_prompt();
        let (Some(target), Some(name)) = (self.table_target(id), self.table_name(id)) else { return };
        let Some(tq) = self.table_query(id) else { return };
        let applied = Draft::from_query(tq.applied(), &target);
        let before_last = tq.before_last.as_ref().map(|q| Draft::from_query(q, &target));
        let failure = tq.failure().map(|f| (f.message.clone(), f.at.clone()));
        let draft = Draft::from_query(q, &target);
        let (columns, source) = self.filter_dialog_columns(id, &target);
        let loaded = self.filter_dialog_loaded(id);
        // Where the server said the failed filter is wrong: that chip underlined.
        let mark = failure
            .as_ref()
            .and_then(|(_, at)| at.as_ref())
            .and_then(|at| apply::term_at(&draft.where_, at))
            .map(Mark::Term);
        let message = failure.map(|(said, _)| match said {
            Some(text) => Notice::new(Msg::TableDialogFailed { said: text }, Level::Error),
            None => Notice::new(Label::TableDialogFailedPlain, Level::Error),
        });
        let mut d = FilterDialog {
            tab: id,
            table: format!("{}.{}", name.schema, name.name),
            dialect: target.dialect,
            columns,
            source,
            hidden: BTreeSet::new(),
            loaded,
            limit: TextInput::new(&draft.limit),
            draft,
            applied,
            before_last,
            undo: Undo::default(),
            focus: DialogFocus::Columns,
            cursor: 0,
            scroll: 0,
            search: None,
            picker: None,
            inputs: Vec::new(),
            message,
            mark,
            sql_open: false,
            applying: None,
            hits: DialogHits::default(),
            narrow: None,
            values_scroll: 0,
            listed: 0,
            list_of: None,
            tally: None,
            counting: None,
            exact: HashMap::new(),
        };
        if let Some(c) = column {
            d.select_column(c);
        }
        if order {
            d.focus = if d.draft.order.is_empty() { DialogFocus::Sort } else { DialogFocus::Order(0) };
        }
        self.overlays.push(Overlay::FilterDialog(Box::new(d)));
    }

    /// How many rows table tab `id` holds of its query; `None` when it holds none.
    fn filter_dialog_loaded(&self, id: TabId) -> Option<usize> {
        let shown = self.table_query(id).is_some_and(|tq| tq.shown.is_some());
        match self.tabs.get(id).map(|t| &t.results) {
            Some(Results::Rows(rs)) if shown => Some(rs.rows.len()),
            _ => None,
        }
    }

    /// Table tab `id` read its table's catalogue (with its keys) or rows of its query: its open
    /// dialog's columns become those it would open with now, the selected one kept by its name
    /// (the list's first when it is gone).
    pub(crate) fn filter_dialog_columns_came(&mut self, id: TabId) {
        if self.filter_dialog().is_none_or(|d| d.tab != id) {
            return;
        }
        let Some(target) = self.table_target(id) else { return };
        let (columns, source) = self.filter_dialog_columns(id, &target);
        let loaded = self.filter_dialog_loaded(id);
        let Some(d) = self.filter_dialog_mut() else { return };
        let selected = d.column().map(|c| c.name.clone());
        d.columns = columns;
        d.source = source;
        d.loaded = loaded;
        match selected {
            Some(name) if d.columns.iter().any(|c| c.name == name) => d.select_column(&name),
            _ => d.cursor = 0,
        }
        d.settle();
    }

    /// The columns of table tab `id`'s table: the catalogue's, in the table's order, when the
    /// key-only read on the tab's metadata session said them; else those of the rows loaded
    /// (with why when that read failed); else the names the app knows. Each with the rows'
    /// column of that name when rows are loaded: what it compares by is known only then.
    fn filter_dialog_columns(&self, id: TabId, target: &Target) -> (Vec<ColumnInfo>, ColumnSource) {
        let Some(t) = self.tabs.get(id) else { return (Vec::new(), ColumnSource::Result) };
        let d = target.dialect;
        let on = self.keys_on(t);
        let tq = t.doc.table_query.as_ref();
        let metas: Vec<&datarig_core::driver::ColumnMeta> = match &t.results {
            Results::Rows(rs) if tq.is_some_and(|q| q.shown.is_some()) => rs.columns.iter().map(|c| &c.meta).collect(),
            _ => Vec::new(),
        };
        let structure = self.table_structure_of(id);
        let enum_known = |name: &str| {
            structure.and_then(|s| s.columns.iter().find(|c| c.name == name)).is_some_and(|c| !c.enum_labels.is_empty())
        };
        let with_meta = |mut c: ColumnInfo| {
            if let Some(m) = metas.iter().find(|m| m.name == c.name) {
                c = c.with_meta(m);
            }
            c.enum_known = enum_known(&c.name);
            c
        };
        let (catalogue, failed) = match tq.map(|q| &q.order_keys) {
            Some(OrderKeysState::Known { on: o, keys }) if Some(o) == on.as_ref() => (keys.columns.as_ref(), None),
            Some(OrderKeysState::Failed { on: o, reason }) if Some(o) == on.as_ref() => (None, Some(reason.clone())),
            _ => (None, None),
        };
        if let Some(cols) = catalogue.filter(|c| !c.is_empty()) {
            return (
                cols.iter().map(|c| with_meta(ColumnInfo::from_catalogue(d, c))).collect(),
                ColumnSource::Catalogue,
            );
        }
        let source = failed.map_or(ColumnSource::Result, ColumnSource::ResultFailed);
        let columns = if metas.is_empty() {
            target
                .columns
                .iter()
                .map(|f| ColumnInfo {
                    name: f.name.clone(),
                    type_name: String::new(),
                    kind: f.kind,
                    meta: None,
                    enum_known: false,
                    primary_key: false,
                    foreign_key: false,
                    not_null: false,
                    invisible: false,
                })
                .map(with_meta)
                .collect()
        } else {
            metas.iter().map(|m| with_meta(ColumnInfo::from_meta(m))).collect()
        };
        (columns, source)
    }
}
