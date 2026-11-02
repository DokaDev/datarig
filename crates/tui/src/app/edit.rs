//! Editing a result's cells: the changes pending on top of the rows as read
//! ([`EditBatch`]), kept with the result itself, and what may change them.
//!
//! A result is resolved on its first edit key: the statement and the result's columns name the
//! table ([`edit::source`]), then the table's structure, read fresh for it, says what each
//! column allows and how a row is found again ([`EditSource::target`]). The press waits for the
//! structure and is done once it comes, if nothing moved meanwhile. Every press checks again
//! what may have changed since (the connection, its environment, the statement's table), and
//! refuses with the reason, once per press, changing nothing.
//!
//! A cell is edited in a line drawn over it (`Ctx::CellEdit`), an enum or boolean column's in a
//! list of its values. Cells selected in one column take the same value, as one change.

use super::picker::Picker;
use super::*;
use crate::widgets::grid::Shape;
use crate::widgets::list_picker::{Entry, ListPicker, Outcome};
use crate::widgets::text_input::InputResult;
use datarig_core::driver::structure::{ColumnFill, TableStructure};
use datarig_core::driver::{Cell, ColumnMeta, DbError, EditFormat, ValueKind};
use datarig_core::edit::{
    self, ColumnReason, EditBatch, EditSource, EditTarget, Identity, Invalid, ReadOnly, TableColumn, Value, Write,
};
use datarig_core::export;
use std::ops::Range;

/// What an edit key does to the cursor's cell (or the cells selected in one column).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditAction {
    /// Type a value (a list of the values for an enum or a boolean).
    Cell,
    SetNull,
    SetDefault,
    Undo,
    Redo,
    /// Drop the pending changes of the cell or of the selection.
    Revert,
}

/// A result's pending changes and what may change it.
#[derive(Debug, Default)]
pub struct EditState {
    pub batch: EditBatch,
    pub target: Target,
    /// What the target was resolved under: changes are taken only while it holds.
    bound: Option<Bound>,
}

/// What a result's rows are to its edits.
#[derive(Debug, Default)]
pub enum Target {
    /// Not resolved yet: the first edit key reads the table's structure.
    #[default]
    Unresolved,
    /// The table's structure was asked for.
    Loading(Box<EditWait>),
    Ready(Box<EditTarget>),
    /// Why its rows cannot be edited. `retry`: the next edit key resolves it again.
    ReadOnly {
        why: Why,
        retry: bool,
    },
}

/// Why a result's rows cannot be edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    Rows(ReadOnly),
    /// The table's structure could not be read: this error.
    Structure(String),
}

/// What a result's edits are bound to: its tab's binding, profile and query session, and the
/// database the tab works in (`None`: the profile's own).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Bound {
    binding: u64,
    profile: ProfileId,
    session: u64,
    database: Option<String>,
}

/// An edit key waiting for its table's structure: the tab it was pressed in, that tab's binding,
/// the profile's attempt, the database and the result; the table and the format it is resolved
/// in; the press, with the cursor and the selection's anchor then.
#[derive(Debug)]
pub struct EditWait {
    tab: TabId,
    binding: u64,
    profile: ProfileId,
    generation: u64,
    database: Option<String>,
    result: u64,
    source: EditSource,
    format: EditFormat,
    then: (EditAction, (usize, usize), Option<(usize, usize)>),
}

/// The cell editor: a line over cell `at` of result `result`, whose text goes into `cells` (the
/// cells selected in its column, else that one cell) on `Enter`.
pub struct CellEdit {
    pub result: u64,
    pub at: (usize, usize),
    pub cells: Vec<(usize, usize)>,
    pub input: TextInput,
    /// Why the text typed was not taken.
    pub error: Option<String>,
}

/// The row result tab `t` shows.
fn rows_of(t: &Tab) -> Option<&ResultSet> {
    match &t.results {
        Results::Rows(rs) if t.exec.view == tabs::ResultView::Rows => Some(rs),
        _ => None,
    }
}

fn rows_of_mut(t: &mut Tab) -> Option<&mut ResultSet> {
    shown_mut(&mut t.results, t.exec.view)
}

/// The row result `results` holds, while the pane shows rows (`view`).
fn shown_mut(results: &mut Results, view: tabs::ResultView) -> Option<&mut ResultSet> {
    match results {
        Results::Rows(rs) if view == tabs::ResultView::Rows => Some(rs),
        _ => None,
    }
}

/// Rows `rows` of `rs` as read, from its spill file too.
fn read_rows(rs: &ResultSet, rows: Range<usize>) -> Option<Vec<Vec<Cell>>> {
    let mut out = Vec::with_capacity(rows.len());
    rs.rows.for_each_chunk(rows.clone(), 1024, |chunk| out.extend_from_slice(chunk)).ok()?;
    (out.len() == rows.len()).then_some(out)
}

/// Whether `c` can be set to its default: it has a default expression, or the database numbers
/// it.
fn has_default(c: &TableColumn) -> bool {
    c.column.default.is_some()
        || matches!(
            c.column.fill,
            ColumnFill::IdentityAlways | ColumnFill::IdentityByDefault | ColumnFill::AutoIncrement
        )
}

/// The first row of `rows` (as read, from row `first` on) whose value in an identity column of
/// `t` is not shown whole, and why.
fn not_whole(
    d: Dialect,
    rs: &ResultSet,
    t: &EditTarget,
    rows: &[Vec<Cell>],
    first: usize,
    server_zone: Option<&str>,
) -> Option<(usize, export::Refused)> {
    let ids = match &t.identity {
        Identity::Key { columns, .. } | Identity::Values { columns, .. } => columns,
    };
    let cols: Vec<export::SqlColumn> = ids
        .iter()
        .map(|&i| {
            let m = &rs.columns[i].meta;
            export::SqlColumn { name: &m.name, type_name: &m.type_name, kind: m.kind, auto_increment: false }
        })
        .collect();
    let cells = match export::sql_cells(d, &cols, &rs.read_in, server_zone) {
        Ok(c) => c,
        Err(r) => return Some((first, r)),
    };
    rows.iter().enumerate().find_map(|(k, row)| {
        let refused = ids.iter().enumerate().find_map(|(j, &i)| match row.get(i) {
            Some(Some(v)) => cells.cell(j, v).err(),
            _ => None,
        })?;
        Some((first + k, refused))
    })
}

impl App {
    /// The edits tab `tab`'s server takes: as a session of its profile said (`DbEvent::Edit`),
    /// else its driver's (`Capabilities::edit`). `None` without a profile, or when the profile or
    /// its driver is unknown.
    pub(crate) fn tab_edit_format(&self, tab: TabId) -> Option<EditFormat> {
        let profile = self.tabs.get(tab).and_then(|t| t.profile);
        let caps = self.profile_capabilities(profile)?;
        // Only a driver with a format has servers that say theirs.
        let said = profile.and_then(|p| self.conns.get(p)).and_then(|c| c.edit);
        caps.edit.and(said.unwrap_or(caps.edit))
    }

    /// The cell editor is open in the active tab.
    pub fn cell_edit_open(&self) -> bool {
        !self.tabs.is_empty() && self.tab().cell_edit.is_some()
    }

    pub(super) fn edit_action(&mut self, a: EditAction) {
        match a {
            EditAction::Undo | EditAction::Redo => self.edit_history(a == EditAction::Redo),
            EditAction::Revert => self.edit_revert(),
            EditAction::Cell | EditAction::SetNull | EditAction::SetDefault => self.edit_resolve(a),
        }
    }

    /// Undo (`redo`: redo) the last change of the shown result.
    fn edit_history(&mut self, redo: bool) {
        let Some(rs) = rows_of_mut(self.tabs.active_mut()) else { return };
        let done = if redo { rs.edit.batch.redo() } else { rs.edit.batch.undo() };
        if !done {
            let says = if redo { Label::EditNothingToRedo } else { Label::EditNothingToUndo };
            self.flash(Notice::new(says, Level::Info));
        }
    }

    /// Drop the pending changes of the cursor's cell, or of the selected cells.
    fn edit_revert(&mut self) {
        let t = self.tabs.active_mut();
        let (grid, Some(rs)) = (&t.grid, shown_mut(&mut t.results, t.exec.view)) else { return };
        let (total, ncols) = (rs.rows.len(), rs.columns.len());
        let (rows, cols) = grid.selection(total, ncols).unwrap_or((grid.row..grid.row + 1, grid.col..grid.col + 1));
        let cells: Vec<(usize, usize)> = rs
            .edit
            .batch
            .rows()
            .filter(|(r, _)| rows.contains(r))
            .flat_map(|(r, e)| e.set.keys().filter(|c| cols.contains(c)).map(move |c| (r, *c)))
            .collect();
        if cells.is_empty() || !rs.edit.batch.revert(&cells, &[]) {
            self.flash(Notice::new(Label::EditNothingToRevert, Level::Info));
        }
    }

    /// An edit key that writes: refused with the reason when the result cannot be edited (now);
    /// done when its target is resolved, else the table's structure is asked for and the press
    /// waits for it.
    fn edit_resolve(&mut self, a: EditAction) {
        let t = self.tab();
        let Some(rs) = rows_of(t) else { return };
        let Some(pid) = t.profile.filter(|p| self.conns.is_connected(*p)) else {
            return self.say_edit_read_only(Label::EditWhyNotConnected);
        };
        if let Err(e) = self.policy_of(pid) {
            let name = self.profile(pid).map(|p| p.name.clone()).unwrap_or_default();
            let n = self.unknown_environment_notice(&name, &e);
            return self.flash(Notice::new(n.msg, Level::Info));
        }
        // The environment is read-only, or was when the tab's session opened (that session is
        // read-only on the server).
        if self.read_only(pid) || (t.exec.session.is_some() && t.exec.read_only) {
            return self.say_edit_read_only(Label::EditWhyReadOnly);
        }
        let Some(format) = self.tab_edit_format(t.id) else {
            return self.say_why(&Why::Rows(ReadOnly::NoFormat));
        };
        let bound = Bound {
            binding: t.binding,
            profile: pid,
            session: t.exec.generation,
            database: self.other_database(t).map(str::to_string),
        };
        let empty = rs.edit.batch.is_empty();
        let moved = rs.edit.bound.as_ref().is_some_and(|b| *b != bound);
        if moved && !empty {
            return self.flash(Notice::new(Label::EditConnectionChanged, Level::Info));
        }
        // What the rows are is read again while nothing is pending: the keys may have come or
        // changed since.
        let source = empty.then(|| {
            let cols: Vec<ColumnMeta> = rs.columns.iter().map(|c| c.meta.clone()).collect();
            edit::source(self.tab_dialect(t.id), self.tab_keys(t).catalog(), &cols, t.shown_sql())
        });
        let generation = self.conns.get(pid).map_or(0, |c| c.generation);
        let (tab, then) = (t.id, (a, (t.grid.row, t.grid.col), t.grid.anchor));
        let Some(rs) = rows_of_mut(self.tabs.active_mut()) else { return };
        let result = rs.id;
        let st = &mut rs.edit;
        if moved {
            st.target = Target::Unresolved;
        }
        st.bound = Some(bound.clone());
        let source = match source {
            Some(Err(r)) => {
                let why = Why::Rows(r);
                st.target = Target::ReadOnly { why: why.clone(), retry: true };
                return self.say_why(&why);
            }
            Some(Ok(s)) => Some(s),
            None => None,
        };
        let stale = match &st.target {
            Target::ReadOnly { retry, .. } => *retry,
            Target::Ready(t) => source.as_ref().is_some_and(|s| s.table != t.table),
            // Its answer comes on an attempt or a binding that is gone: never.
            Target::Loading(w) => {
                w.binding != bound.binding
                    || w.generation != generation
                    || w.database != bound.database
                    || w.result != result
            }
            Target::Unresolved => false,
        };
        if stale {
            st.target = Target::Unresolved;
        }
        match &st.target {
            Target::Ready(t) => {
                let t = (**t).clone();
                self.edit_ready(a, &t);
            }
            Target::Loading(_) => self.flash(Notice::new(Label::EditLoading, Level::Info)),
            Target::ReadOnly { why, .. } => {
                let why = why.clone();
                self.say_why(&why);
            }
            Target::Unresolved => {
                // A change pending means the target was resolved under this binding.
                let Some(source) = source else { return };
                let cmd =
                    DbCommand::LoadStructure { schema: source.table.schema.clone(), table: source.table.name.clone() };
                let session = match &bound.database {
                    None => self.conns.get(pid).and_then(|c| c.meta.as_ref()),
                    Some(db) => self.conns.aux(pid, db).and_then(|a| a.session.as_ref()),
                };
                let Some(session) = session else { return self.say_edit_read_only(Label::EditWhyNotConnected) };
                session.send(cmd);
                self.touch_profile(pid);
                let wait = EditWait {
                    tab,
                    binding: bound.binding,
                    profile: pid,
                    generation,
                    database: bound.database,
                    result,
                    source,
                    format,
                    then,
                };
                if let Some(rs) = rows_of_mut(self.tabs.active_mut()) {
                    rs.edit.target = Target::Loading(Box::new(wait));
                }
                self.flash(Notice::new(Label::EditLoading, Level::Info));
            }
        }
    }

    /// An edit key on a resolved result: the cursor's cell, or the cells selected in its
    /// column; each row must be found again by values shown whole.
    fn edit_ready(&mut self, a: EditAction, target: &EditTarget) {
        let t = self.tab();
        let Some(rs) = rows_of(t) else { return };
        let (total, ncols) = (rs.rows.len(), rs.columns.len());
        let (row, col) = (t.grid.row, t.grid.col);
        if row >= total || col >= ncols {
            return;
        }
        let rows = match t.grid.selection(total, ncols) {
            None => row..row + 1,
            Some((rows, cols)) if t.grid.shape == Shape::Cells && cols.len() == 1 => rows,
            Some(_) => return self.flash(Notice::new(Label::EditSeveralColumns, Level::Info)),
        };
        let name = rs.columns[col].meta.name.clone();
        let c = match target.allows(col, Write::Update) {
            Ok(c) => c.clone(),
            Err(r) => return self.say_refused(column_reason(r, name)),
        };
        let Some(read) = read_rows(rs, rows.clone()) else {
            return self.say_refused(Msg::EditWhyNotRead { row: (rows.start + 1).to_string() });
        };
        let d = self.tab_dialect(t.id);
        let zone = self.tab_keys(t).catalog().and_then(|k| k.server_zone());
        if let Some((r, refused)) = not_whole(d, rs, target, &read, rows.start, zone) {
            let reason = self.not_whole_reason(&refused.why);
            return self.say_refused(Msg::EditWhyNotWhole { row: (r + 1).to_string(), reason });
        }
        let cells: Vec<(usize, usize)> = rows.clone().map(|r| (r, col)).collect();
        // The cursor's value: its pending one, else as read.
        let pending = rs.edit.batch.value(row, col).cloned();
        let stored = read.get(row - rows.start).and_then(|r| r.get(col)).cloned().flatten();
        match a {
            EditAction::SetNull if c.column.not_null => self.say_refused(Msg::EditWhyNotNull { column: name }),
            EditAction::SetDefault if !has_default(&c) => self.say_refused(Msg::EditWhyNoDefault { column: name }),
            EditAction::SetNull => self.edit_write_cells(&cells, Value::Null),
            EditAction::SetDefault => self.edit_write_cells(&cells, Value::Default),
            EditAction::Cell if !c.column.enum_labels.is_empty() || c.kind == ValueKind::Bool => {
                let current = pending.or_else(|| Some(stored.map_or(Value::Null, Value::Text)));
                let result = rs.id;
                self.open_cell_picker(&c, cells, result, current);
            }
            EditAction::Cell => {
                let text = match pending {
                    Some(Value::Text(s)) => s,
                    Some(Value::Null | Value::Default) => String::new(),
                    None => stored.unwrap_or_default(),
                };
                if text.contains(['\n', '\r']) {
                    return self.flash(Notice::new(Label::EditSeveralLines, Level::Info));
                }
                let result = rs.id;
                self.tabs.active_mut().cell_edit =
                    Some(CellEdit { result, at: (row, col), cells, input: TextInput::new(&text), error: None });
            }
            EditAction::Undo | EditAction::Redo | EditAction::Revert => {}
        }
    }

    /// The list of the values of an enum or boolean column `c`, then NULL and DEFAULT when it
    /// takes them; a pick goes into `cells` of result `result`.
    fn open_cell_picker(&mut self, c: &TableColumn, cells: Vec<(usize, usize)>, result: u64, current: Option<Value>) {
        let i18n = &self.i18n;
        let labels: Vec<String> = match c.column.enum_labels.is_empty() {
            false => c.column.enum_labels.clone(),
            // As the result shows a boolean.
            true => vec!["true".into(), "false".into()],
        };
        let mut entries: Vec<Entry<Value>> =
            labels.into_iter().map(|l| Entry::new(Value::Text(l.clone()), l, String::new())).collect();
        if !c.column.not_null {
            let null = i18n.label(Label::ResultsNull).to_string();
            entries.push(Entry::new(Value::Null, null, i18n.label(Label::EditPickNull).to_string()));
        }
        if has_default(c) {
            let hint = c
                .column
                .default
                .clone()
                .map(|default| i18n.msg(&Msg::EditPickDefault { default }).to_string())
                .unwrap_or_default();
            entries.push(Entry::new(Value::Default, "DEFAULT".into(), hint));
        }
        let list = ListPicker::new(entries, None, current.as_ref());
        let tab = self.tab().id;
        self.overlays.push(Overlay::Picker(Picker::CellValue { tab, result, cells, list }));
    }

    /// A key of the list of a cell's values.
    pub(super) fn cell_value_picked(&mut self, out: Outcome<Value>) {
        if out == Outcome::Pending {
            return;
        }
        let Some(Picker::CellValue { tab, result, cells, .. }) = self.overlays.picker() else { return };
        let (tab, result, cells) = (*tab, *result, cells.clone());
        self.overlays.close(OverlayKind::Picker);
        let Outcome::Picked(v) = out else { return };
        if !self.tabs.is_empty() && self.tab().id == tab && rows_of(self.tab()).is_some_and(|rs| rs.id == result) {
            self.edit_write_cells(&cells, v);
        }
    }

    /// Set `cells` (rows of one column, in order) of the active tab's shown result to `v`, as one
    /// change.
    fn edit_write_cells(&mut self, cells: &[(usize, usize)], v: Value) {
        let (Some(first), Some(last)) = (cells.first(), cells.last()) else { return };
        let first = first.0;
        let Some(rs) = rows_of_mut(self.tabs.active_mut()) else { return };
        let Some(read) = read_rows(rs, first..last.0 + 1) else {
            return self.say_refused(Msg::EditWhyNotRead { row: (first + 1).to_string() });
        };
        rs.edit.batch.set(cells, &v, |r| read.get(r - first).cloned().unwrap_or_default());
    }

    /// The open cell editor's cells and their table column, while its result is resolved.
    fn cell_edit_column(&self) -> Option<(Vec<(usize, usize)>, TableColumn)> {
        let t = self.tab();
        let e = t.cell_edit.as_ref()?;
        let rs = rows_of(t).filter(|rs| rs.id == e.result)?;
        let Target::Ready(target) = &rs.edit.target else { return None };
        let c = target.allows(e.at.1, Write::Update).ok()?;
        Some((e.cells.clone(), c.clone()))
    }

    /// A key while the cell editor is open: `Enter` keeps the text (an empty one is the empty
    /// string), `Esc` cancels, `Ctrl+N` sets NULL and `Ctrl+D` the default; any other key edits
    /// the text.
    pub(super) fn cell_edit_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.tabs.active_mut().cell_edit = None,
            KeyCode::Enter => self.cell_edit_enter(),
            KeyCode::Char('n') if ctrl => self.cell_edit_value(Value::Null),
            KeyCode::Char('d') if ctrl => self.cell_edit_value(Value::Default),
            _ => {
                if let Some(e) = self.tabs.active_mut().cell_edit.as_mut()
                    && e.input.handle_key(&key) == InputResult::Changed
                {
                    e.error = None;
                }
            }
        }
    }

    /// Text pasted into the cell editor: one line only.
    pub(super) fn cell_edit_paste(&mut self, text: &str) {
        if text.contains(['\n', '\r']) {
            return self.flash(Notice::new(Label::EditPasteLines, Level::Info));
        }
        if let Some(e) = self.tabs.active_mut().cell_edit.as_mut() {
            e.input.insert_str(text);
            e.error = None;
        }
    }

    /// `Enter` in the cell editor: the text, when it is a value of the column's type.
    fn cell_edit_enter(&mut self) {
        let Some((cells, c)) = self.cell_edit_column() else {
            self.tabs.active_mut().cell_edit = None;
            return;
        };
        let text = self.tab().cell_edit.as_ref().map(|e| e.input.text().to_string()).unwrap_or_default();
        let d = self.tab_dialect(self.tab().id);
        if let Err(reason) = self.typed_ok(d, &c, &text) {
            if let Some(e) = self.tabs.active_mut().cell_edit.as_mut() {
                e.error = Some(reason.clone());
            }
            return self.flash(Notice::new(Msg::EditRefused { reason }, Level::Info));
        }
        self.tabs.active_mut().cell_edit = None;
        self.edit_write_cells(&cells, Value::Text(text));
    }

    /// `Ctrl+N` or `Ctrl+D` in the cell editor: NULL or the default, when the column takes it.
    fn cell_edit_value(&mut self, v: Value) {
        let Some((cells, c)) = self.cell_edit_column() else {
            self.tabs.active_mut().cell_edit = None;
            return;
        };
        let column = c.column.name.clone();
        match v {
            Value::Null if c.column.not_null => return self.say_refused(Msg::EditWhyNotNull { column }),
            Value::Default if !has_default(&c) => return self.say_refused(Msg::EditWhyNoDefault { column }),
            _ => {}
        }
        self.tabs.active_mut().cell_edit = None;
        self.edit_write_cells(&cells, v);
    }

    /// Whether `text` is a value of column `c`'s type in dialect `d`; why not, in words.
    fn typed_ok(&self, d: Dialect, c: &TableColumn, text: &str) -> Result<(), String> {
        use Invalid as I;
        let Err(e) = edit::check_input(d, c, text) else { return Ok(()) };
        let msg = match e {
            I::NotLabel => Label::EditInvalidNotLabel.into(),
            I::NotWholeNumber => Label::EditInvalidNotWholeNumber.into(),
            I::OutOfRange { type_name } => Msg::EditInvalidOutOfRange { type_name },
            I::NotNumber => Label::EditInvalidNotNumber.into(),
            I::TooManyDigits { precision, scale } => {
                Msg::EditInvalidTooManyDigits { precision: precision.to_string(), scale: scale.to_string() }
            }
            I::NotBool => Label::EditInvalidNotBool.into(),
            I::NotDate => Label::EditInvalidNotDate.into(),
            I::NotTime => Label::EditInvalidNotTime.into(),
            I::NotTimestamp => Label::EditInvalidNotTimestamp.into(),
            I::TooPrecise { digits } => Msg::EditInvalidTooPrecise { digits: digits.to_string() },
            I::NotUuid => Label::EditInvalidNotUuid.into(),
            I::NotBytes { mysql: false } => Label::EditInvalidNotBytes.into(),
            I::NotBytes { mysql: true } => Label::EditInvalidNotBytesMysql.into(),
            I::NotBits { mysql: false } => Label::EditInvalidNotBits.into(),
            I::NotBits { mysql: true } => Label::EditInvalidNotBitsMysql.into(),
            I::BitCount { count, exact: true } => Msg::EditInvalidBitCountExact { bits: count.to_string() },
            I::BitCount { count, exact: false } => Msg::EditInvalidBitCountAtMost { bits: count.to_string() },
            I::TooLong { max } => Msg::EditInvalidTooLong { max: max.to_string() },
        };
        Err(self.i18n.msg(&msg).to_string())
    }

    /// After every event: the cell editor closes (changing nothing) when its result is no
    /// longer the one shown, its tab not the active one, or the grid lost the focus; the list of
    /// a cell's values too.
    pub(super) fn sweep_cell_edit(&mut self) {
        let active = (!self.tabs.is_empty()).then(|| self.tab().id);
        let focused = self.focus == Focus::Results;
        for t in self.tabs.iter_mut() {
            let Some(e) = &t.cell_edit else { continue };
            if !focused || Some(t.id) != active || rows_of(t).is_none_or(|rs| rs.id != e.result) {
                t.cell_edit = None;
            }
        }
        if let Some(Picker::CellValue { tab, result, .. }) = self.overlays.picker() {
            let holds = active == Some(*tab) && rows_of(self.tab()).is_some_and(|rs| rs.id == *result);
            if !holds {
                self.overlays.close(OverlayKind::Picker);
            }
        }
    }

    /// The structure of `schema.table` in `profile`'s `database` came (`error`: why it could not
    /// be read): every result waiting for it whose tab is bound as it was is resolved; the
    /// others wait no more (their next edit key asks again). A press waiting in the active tab's
    /// shown result is done, if the cursor and the selection are where they were.
    pub(super) fn edit_structure_came(
        &mut self,
        profile: ProfileId,
        database: Option<&str>,
        schema: &str,
        table: &str,
        result: Result<&TableStructure, &DbError>,
        error: &str,
    ) {
        let generation = self.conns.get(profile).map(|c| c.generation);
        let active = (!self.tabs.is_empty()).then(|| self.tab().id);
        let others: Vec<(TabId, Option<String>)> =
            self.tabs.iter().map(|t| (t.id, self.other_database(t).map(str::to_string))).collect();
        let mut came = None;
        for (id, other) in others {
            let Some(t) = self.tabs.get_mut(id) else { continue };
            let (binding, bound_to, view) = (t.binding, t.profile, t.exec.view);
            let shown = match &mut t.results {
                Results::Rows(rs) => Some(rs),
                _ => None,
            };
            let shown_id = shown.as_ref().map(|rs| rs.id);
            for rs in shown.into_iter().chain(t.exec.steps.values_mut().map(|p| &mut p.rs)) {
                let Target::Loading(w) = &rs.edit.target else { continue };
                if w.profile != profile
                    || w.database.as_deref() != database
                    || w.source.table.schema != schema
                    || w.source.table.name != table
                {
                    continue;
                }
                let Target::Loading(w) = std::mem::take(&mut rs.edit.target) else { continue };
                let current = w.tab == id
                    && w.binding == binding
                    && bound_to == Some(profile)
                    && generation == Some(w.generation)
                    && other == w.database
                    && w.result == rs.id;
                if !current {
                    continue;
                }
                rs.edit.target = match result {
                    Err(_) => Target::ReadOnly { why: Why::Structure(error.to_string()), retry: true },
                    Ok(s) => match w.source.target(s, &*edit::format(w.format)) {
                        Ok(t) => Target::Ready(Box::new(t)),
                        Err(r) => Target::ReadOnly { why: Why::Rows(r), retry: false },
                    },
                };
                if Some(id) == active && Some(rs.id) == shown_id && view == tabs::ResultView::Rows {
                    let why = match &rs.edit.target {
                        Target::ReadOnly { why, .. } => Some(why.clone()),
                        _ => None,
                    };
                    came = Some((w.then, why));
                }
            }
        }
        let Some(((a, cursor, anchor), why)) = came else { return };
        let t = self.tab();
        let still = (t.grid.row, t.grid.col) == cursor && t.grid.anchor == anchor;
        if !still || self.focus != Focus::Results || self.modal_open() {
            return;
        }
        match why {
            Some(why) => self.say_why(&why),
            None => self.edit_action(a),
        }
    }

    /// Why the result's rows cannot be edited, as the status line says it.
    fn say_why(&mut self, why: &Why) {
        let label = |l| self.i18n.label(l).to_string();
        let reason = match why {
            Why::Rows(ReadOnly::Source(n)) => self.why(n),
            Why::Rows(ReadOnly::NoFormat) => label(Label::EditWhyNoFormat),
            Why::Rows(ReadOnly::NoTableColumn) => label(Label::EditWhyNoTableColumn),
            Why::Rows(ReadOnly::NotATable(_)) => label(Label::EditWhyNotATable),
            Why::Rows(ReadOnly::TableChanged) => label(Label::EditWhyTableChanged),
            Why::Rows(ReadOnly::NoIdentity) => label(Label::EditWhyNoIdentity),
            Why::Rows(ReadOnly::NoExactColumn) => label(Label::EditWhyNoExactColumn),
            Why::Structure(error) => self.i18n.msg(&Msg::EditWhyStructureFailed { error: error.clone() }).to_string(),
        };
        self.flash(Notice::new(Msg::EditReadOnly { reason }, Level::Info));
    }

    /// The result is read-only now, for `why`.
    fn say_edit_read_only(&mut self, why: Label) {
        let reason = self.i18n.label(why).to_string();
        self.flash(Notice::new(Msg::EditReadOnly { reason }, Level::Info));
    }

    /// This change cannot be made, for `why`.
    fn say_refused(&mut self, why: Msg) {
        let reason = self.i18n.msg(&why).to_string();
        self.flash(Notice::new(Msg::EditRefused { reason }, Level::Info));
    }
}

/// Why column `column` cannot be set.
fn column_reason(r: ColumnReason, column: String) -> Msg {
    match r {
        ColumnReason::Computed => Msg::EditWhyComputed { column },
        ColumnReason::Generated => Msg::EditWhyGenerated { column },
        ColumnReason::IdentityAlways => Msg::EditWhyIdentityAlways { column },
        ColumnReason::Repeated => Msg::EditWhyRepeated { column },
    }
}
