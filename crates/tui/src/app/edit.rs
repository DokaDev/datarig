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
//! A cell is edited in a line drawn over it (`Ctx::CellEdit`), a value of several lines or a
//! JSON column's in the query editor's widget in a box (`Ctx::CellEditor`, [`lines`]), an enum or
//! boolean column's in a list of its values. Cells selected in one column take the same value,
//! as one change.
//!
//! New rows are added (every value the column's default, or a copy of the cursor's row) and
//! shown after the rows read, in the order they were added; rows read are marked to be deleted,
//! new rows removed. A range pasted into the grid sets many cells and adds rows at once
//! ([`paste`]).

use super::menu::Off;
use super::picker::Picker;
use super::*;
use crate::widgets::grid::Shape;
use crate::widgets::list_picker::{Entry, ListPicker, Outcome};
use crate::widgets::text_input::InputResult;
use datarig_core::driver::structure::{ColumnFill, TableStructure};
use datarig_core::driver::{ArrayElement, Cell, ColumnMeta, DbError, EditFormat, ValueKind};
use datarig_core::edit::{
    self, ColumnReason, EditBatch, EditSource, EditTarget, Expect, Identity, Invalid, ReadOnly, TableColumn, Value,
    Write,
};
use datarig_core::export;
use ratatui::style::{Color, Style};
use std::ops::Range;

/// Why the action menu does not offer an edit as one that can run.
pub(super) enum NotOffered {
    /// It has nothing to do there: left out.
    Hidden,
    /// The whole result takes no edit now, for this reason.
    Result(Box<Off>),
    /// It does not act on this cell or selection, for this reason.
    Here(Box<Off>),
}

mod applied;
mod ask;
mod changes;
mod lines;
mod paste;
mod preview;
mod tab;

pub use applied::{Applied, ApplyFailed, Applying};
pub use ask::{ANSWER_GUARD, ApplyAsk, ApplyWait, AskAction, StatusAsk};
pub use changes::{ChangesModel, Checks, ListEntry, ListLine, Mark, ModelCache, Pointer, Problem, pointer_text};
pub use lines::Lines;
pub use paste::{
    ASK_ROWS, CLIPBOARD_TIMEOUT, ClipAnswer, ClipReads, CsvAsk, GridCopy, InvalidCache, Invalids, LastPaste, PasteAsk,
    PasteIn,
};
pub(crate) use preview::PreviewBody;
pub use preview::{EditPreview, PreviewLine, PreviewView};
pub(crate) use tab::Seen;
pub use tab::{COUNT_SETTLE, ChangesAction, ChangesCursor};

/// What an edit key does to the cursor's cell (or the cells selected in one column).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditAction {
    /// Type a value (a list of the values for an enum or a boolean).
    Cell,
    SetNull,
    SetDefault,
    Undo,
    Redo,
    /// Drop the pending changes of the cell or of the selection (a new row in it goes whole).
    Revert,
    /// A new row below the cursor's.
    AddRow,
    /// A new row with the values of the cursor's.
    CloneRow,
    /// Mark the cursor's row (the selected rows) to be deleted, or unmark it; a new row is removed.
    DeleteRow,
    /// The pending changes in the Changes tab.
    Preview,
    /// Apply them, once asked (the preview first when it is needed).
    Apply,
    /// Drop every pending change, once the user says so.
    Discard,
    /// Paste the system clipboard's text at the cursor (`space d v`).
    Paste,
    /// Read the last paste again as the next format, in its place (`space d V`).
    PasteAs,
    /// Paste the cells `y` or `Y` last copied in a grid at the cursor (`g p`).
    PasteRegister,
    /// Add the cells `y` or `Y` last copied in a grid as new rows after the last one (`p`).
    PasteRows,
    /// Paste the text waiting in [`App::paste_in`] (a terminal's paste, the clipboard's text).
    Pasted,
}

/// What would drop a tab's pending changes, asked about first ([`App::ask_leave`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leave {
    /// A run in the tab (its statements wait in `App::pending_leave`): its results are replaced.
    Run(TabId),
    Close(TabId),
    Quit,
}

/// A row the grid shows: a row read (by its index in the result) or a new row (by its id).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RowAt {
    Read(usize),
    New(u64),
}

/// The row the grid shows at `row` of `rs`: the rows read, then the new rows an Apply added
/// (none of these: they are no row to edit), then the new rows.
pub fn row_at(rs: &ResultSet, row: usize) -> Option<RowAt> {
    let len = rs.rows.len();
    if row < len {
        return Some(RowAt::Read(row));
    }
    let len = len + rs.edit.applied_new();
    rs.edit.batch.drafts().get(row.checked_sub(len)?).map(|d| RowAt::New(d.id))
}

/// Where the grid shows row `at` of `rs`.
pub fn shown_row(rs: &ResultSet, at: RowAt) -> Option<usize> {
    let len = rs.rows.len();
    match at {
        RowAt::Read(r) => (r < len).then_some(r),
        RowAt::New(id) => {
            let len = len + rs.edit.applied_new();
            rs.edit.batch.drafts().iter().position(|d| d.id == id).map(|i| len + i)
        }
    }
}

/// A result's pending changes and what may change it.
#[derive(Debug, Default)]
pub struct EditState {
    pub batch: EditBatch,
    pub target: Target,
    /// What the rows were read under: changes are taken only while it holds.
    bound: Option<Bound>,
    /// The Apply of the changes sent and not answered yet.
    pub applying: Option<Box<Applying>>,
    /// What an Apply changed, shown over the rows as read.
    pub applied: Option<Box<Applied>>,
    /// An Apply's result is not known (its connection ended before the server said): the
    /// changes are applied again only once the user says so.
    pub apply_unknown: bool,
    /// What was counted for the changes, and the user's choices.
    pub checks: Checks,
    /// The changes as shown, as last made.
    pub changes: ModelCache,
    /// Where the Changes tab of the result is.
    pub cursor: ChangesCursor,
    /// Why the last Apply applied nothing, for the changes as they were then.
    pub failed: Option<ApplyFailed>,
    /// What an Apply did went with its transaction, said with this until the query runs
    /// again.
    pub gone: Option<Label>,
    /// The cells of the changes whose value their column does not take, as last found.
    pub invalid: InvalidCache,
}

impl EditState {
    /// The rows were read under `bound` (`None`: under no profile, so never edited).
    pub(super) fn bind(&mut self, bound: Option<Bound>) {
        self.bound = bound;
    }

    /// How many new rows an Apply added (shown after the rows read, before the new rows).
    pub fn applied_new(&self) -> usize {
        self.applied.as_ref().map_or(0, |a| a.new.len())
    }

    /// How many rows the grid shows after the rows read: those an Apply added, the new rows.
    pub fn new_shown(&self) -> usize {
        self.applied_new() + self.batch.drafts().len()
    }

    /// The user's transaction `epoch` ended (`end`: how): what an Apply did in it is committed,
    /// or gone, said with the label returned (rolled back, or ended the app cannot tell how).
    pub fn tx_ended(&mut self, epoch: u64, end: tabs::TxEnd) -> Option<Label> {
        let a = self.applied.as_mut().filter(|a| a.tx == Some(epoch))?;
        let label = match end {
            tabs::TxEnd::Committed => {
                a.tx = None;
                return None;
            }
            tabs::TxEnd::RolledBack => Label::EditAppliedGone,
            tabs::TxEnd::Unknown => Label::EditAppliedUnknown,
        };
        self.applied = None;
        self.gone = Some(label);
        Some(label)
    }

    /// What an Apply did in a transaction that has not ended yet goes (`true`), said with
    /// `label` until the query runs again.
    pub fn drop_uncommitted(&mut self, label: Label) -> bool {
        let gone = self.applied.as_ref().is_some_and(|a| a.tx.is_some());
        if gone {
            self.applied = None;
            self.gone = Some(label);
        }
        gone
    }

    /// What the Changes tab of the result is about: its pending changes (being applied too),
    /// what an Apply changed, or nothing but what it says (an Apply whose result is not known,
    /// applied changes gone); `None`: there is no Changes tab.
    pub fn tab_shows(&self) -> Option<TabShows> {
        if !self.batch.is_empty() {
            return Some(TabShows::Pending(self.batch.counts().total() as u64));
        }
        if self.applied.is_some() {
            return Some(TabShows::Applied);
        }
        (self.applying.is_some() || self.apply_unknown || self.gone.is_some()).then_some(TabShows::Said)
    }
}

/// What a result's Changes tab is about ([`EditState::tab_shows`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabShows {
    /// This many pending changes.
    Pending(u64),
    /// What an Apply changed.
    Applied,
    /// Only what it says.
    Said,
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
pub(super) struct Bound {
    binding: u64,
    profile: ProfileId,
    session: u64,
    database: Option<String>,
}

/// An edit key waiting for its table's structure: the tab it was pressed in, that tab's binding,
/// the profile's attempt, the database (and its session) and the result; the table and the format
/// it is resolved in; the press, with the cursor and the selection's anchor then.
#[derive(Debug)]
pub struct EditWait {
    tab: TabId,
    binding: u64,
    profile: ProfileId,
    generation: u64,
    database: Option<String>,
    /// The id of the other database's session it was asked on (a session opened again has
    /// another, and never answers what the lost one was asked).
    aux: Option<u64>,
    result: u64,
    /// Which `LoadStructure` of its session it waits for the answer to (they come in order).
    ticket: u64,
    source: EditSource,
    format: EditFormat,
    then: (EditAction, (usize, usize), Option<(usize, usize)>),
}

/// What an edit key that sets cells acts on, every check passed: rows read `rows` (as read,
/// `read`), or new rows' `cells` (the cursor's new row `at`, id `id`); and the column.
enum CellsPlan {
    Read { rows: Range<usize>, c: TableColumn, read: Vec<Vec<Cell>> },
    New { at: RowAt, id: u64, cells: Vec<(RowAt, usize)>, c: TableColumn },
}

/// What `D` changes: rows read `marked` (as read in `orig`, from row `first`) and new rows `ids`.
struct DeletePlan {
    first: usize,
    marked: Vec<usize>,
    ids: Vec<u64>,
    orig: Vec<Vec<Cell>>,
}

/// The cell editor: a line over cell `at` of result `result`, whose text goes into `cells` (the
/// cells selected in its column, else that one cell) on `Enter`.
pub struct CellEdit {
    pub result: u64,
    pub at: (RowAt, usize),
    pub cells: Vec<(RowAt, usize)>,
    pub input: TextInput,
    /// The text in the editor of several lines, which takes the place of `input` once open.
    pub lines: Option<Box<Lines>>,
    /// Why the text typed was not taken; or (`invalid`) why the value it opened with is not
    /// one its column takes.
    pub error: Option<String>,
    pub invalid: bool,
}

impl CellEdit {
    /// The grid's row and column of its cell, while `rs` shows it.
    pub fn shown_at(&self, rs: &ResultSet) -> Option<(usize, usize)> {
        shown_row(rs, self.at.0).map(|r| (r, self.at.1))
    }

    /// Its cells are of a new row.
    fn new_row(&self) -> bool {
        matches!(self.at.0, RowAt::New(_))
    }

    /// The text typed so far.
    pub fn text(&self) -> String {
        match &self.lines {
            Some(l) => l.text(),
            None => self.input.text().to_string(),
        }
    }
}

/// Whether column `c`'s values are edited in several lines whatever they hold: JSON, an array
/// of JSON values.
fn in_lines(c: &TableColumn) -> bool {
    matches!(c.kind, ValueKind::Json | ValueKind::Array(ArrayElement::Json))
}

/// The row result tab `t` shows: in the grid, or its changes in the Changes tab.
fn rows_of(t: &Tab) -> Option<&ResultSet> {
    match &t.results {
        Results::Rows(rs) if edited_view(t.exec.view) => Some(rs),
        _ => None,
    }
}

/// The pane shows a row result's rows (`view`) or its changes.
fn edited_view(view: tabs::ResultView) -> bool {
    matches!(view, tabs::ResultView::Rows | tabs::ResultView::Changes)
}

fn rows_of_mut(t: &mut Tab) -> Option<&mut ResultSet> {
    shown_mut(&mut t.results, t.exec.view)
}

/// The row result `results` holds, while the pane shows rows (`view`).
fn shown_mut(results: &mut Results, view: tabs::ResultView) -> Option<&mut ResultSet> {
    match results {
        Results::Rows(rs) if edited_view(view) => Some(rs),
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

/// Whether `c`'s default draws a new number (an identity, MySQL's `AUTO_INCREMENT`, a call of
/// a sequence): an existing row set to it would take another number (MySQL stores 0), not keep
/// its own.
fn new_number(c: &TableColumn) -> bool {
    matches!(c.column.fill, ColumnFill::IdentityAlways | ColumnFill::IdentityByDefault | ColumnFill::AutoIncrement)
        || c.column.default.as_deref().is_some_and(|d| d.to_ascii_lowercase().contains("nextval("))
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

/// Why a value typed for column `col` would not be stored as the text shown for it: the column's
/// (no row) or the first row's whose value in `rows` (as read, from row `first` on) was read in a
/// session that shows text or a time otherwise (its character set or time zone, not known or
/// changed). A column that refuses every value is refused whatever its rows hold (NULL too). A
/// value not in its kind's form does not matter here: the typed text replaces it, and the editor
/// checks that text against the type. A value shown rounded (a MySQL `FLOAT`) matters only for
/// `many` cells: a cell shown as the typed text may hold another value, and would be kept.
fn value_not_whole(
    d: Dialect,
    rs: &ResultSet,
    col: usize,
    rows: &[Vec<Cell>],
    first: usize,
    server_zone: Option<&str>,
    many: bool,
) -> Option<(Option<usize>, export::Refused)> {
    let m = &rs.columns[col].meta;
    let cols = [export::SqlColumn { name: &m.name, type_name: &m.type_name, kind: m.kind, auto_increment: false }];
    use export::NotWhole as N;
    let typed = |r: &export::Refused| {
        matches!(r.why, N::Charset(_) | N::TimeZone(_) | N::SessionUnknown | N::SessionChanged)
            || (many && r.why == N::Float)
    };
    let cells = match export::sql_cells(d, &cols, &rs.read_in, server_zone) {
        Ok(c) => c,
        Err(r) => return typed(&r).then_some((None, r)),
    };
    let row = rows.iter().enumerate().find_map(|(k, row)| match row.get(col) {
        Some(Some(v)) => cells.cell(0, v).err().filter(typed).map(|r| (Some(first + k), r)),
        _ => None,
    });
    row.or_else(|| cells.column_refused(0).filter(typed).map(|r| (None, r)))
}

/// Whether value `text` of column `col` (as read) is shown whole, so a copy of it is the value
/// itself: as an SQL copy judges it ([`export::sql_cells`]: a MySQL `FLOAT` shown rounded, bytes
/// not in their form, text or a time read in a session that shows it otherwise).
fn shown_whole(d: Dialect, rs: &ResultSet, col: usize, text: &str, server_zone: Option<&str>) -> bool {
    let m = &rs.columns[col].meta;
    let cols = [export::SqlColumn { name: &m.name, type_name: &m.type_name, kind: m.kind, auto_increment: false }];
    export::sql_cells(d, &cols, &rs.read_in, server_zone).is_ok_and(|c| c.cell(0, text).is_ok())
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
        if a == EditAction::Preview {
            return self.changes_open();
        }
        if matches!(
            a,
            EditAction::Paste
                | EditAction::PasteAs
                | EditAction::PasteRegister
                | EditAction::PasteRows
                | EditAction::Pasted
        ) && self.changes_shown()
        {
            return self.flash(Notice::new(Label::EditPasteGridOnly, Level::Info));
        }
        // Nothing changes the changes being applied.
        if !self.tabs.is_empty() && rows_of(self.tab()).is_some_and(|rs| rs.edit.applying.is_some()) {
            return self.say_refused(Label::EditWhyApplying.into());
        }
        // A key for the cursor's cell from the Changes tab: the grid again, at the selected
        // line's cell.
        if !matches!(a, EditAction::Undo | EditAction::Redo | EditAction::Apply | EditAction::Discard)
            && self.changes_shown()
            && !self.changes_to_grid()
        {
            return;
        }
        match a {
            EditAction::Undo | EditAction::Redo => self.edit_history(a == EditAction::Redo),
            EditAction::Revert => self.edit_revert(),
            EditAction::Apply => self.apply_request(),
            EditAction::Preview => self.changes_open(),
            EditAction::Discard => self.edit_discard_ask(),
            EditAction::Paste => self.paste_key(),
            EditAction::PasteAs => self.paste_again(),
            EditAction::PasteRegister => self.paste_register(false),
            EditAction::PasteRows => self.paste_register(true),
            EditAction::Cell
            | EditAction::SetNull
            | EditAction::SetDefault
            | EditAction::AddRow
            | EditAction::CloneRow
            | EditAction::DeleteRow
            | EditAction::Pasted => self.edit_resolve(a),
        }
    }

    /// Undo (`redo`: redo) the last change of the shown result.
    fn edit_history(&mut self, redo: bool) {
        let Some(rs) = rows_of_mut(self.tabs.active_mut()) else { return };
        let done = if redo { rs.edit.batch.redo() } else { rs.edit.batch.undo() };
        self.edit_grid_sync();
        if !done {
            let says = if redo { Label::EditNothingToRedo } else { Label::EditNothingToUndo };
            self.flash(Notice::new(says, Level::Info));
        }
    }

    /// Drop the pending changes of the cursor's cell, or of the selected cells: a row marked to
    /// be deleted among them is no longer marked, a new row's cell goes back to its default.
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
            .flat_map(|(r, e)| {
                let marked = e.delete.then_some((r, cols.start));
                e.set.keys().filter(|c| cols.contains(c)).map(move |c| (r, *c)).chain(marked)
            })
            .collect();
        let new: Vec<(u64, usize)> = rows
            .filter_map(|r| match row_at(rs, r) {
                Some(RowAt::New(id)) => Some(id),
                _ => None,
            })
            .flat_map(|id| cols.clone().map(move |c| (id, c)))
            .collect();
        let done = !(cells.is_empty() && new.is_empty()) && rs.edit.batch.revert(&cells, &new);
        self.edit_grid_sync();
        if !done {
            self.flash(Notice::new(Label::EditNothingToRevert, Level::Info));
        }
    }

    /// The active tab's grid after its shown result's new rows changed: it counts them, and its
    /// cursor and a selection's anchor stay on rows it shows.
    fn edit_grid_sync(&mut self) {
        let t = self.tabs.active_mut();
        let Some(rs) = shown_mut(&mut t.results, t.exec.view) else { return };
        let n = rs.edit.new_shown();
        let last = (rs.rows.len() + n).saturating_sub(1);
        let g = &mut t.grid;
        g.new_rows = n;
        g.row = g.row.min(last);
        if let Some(a) = g.anchor.as_mut() {
            a.0 = a.0.min(last);
        }
    }

    /// The cursor to row `at` of the shown result, column `col` (its page shown), the selection
    /// dropped.
    fn edit_cursor_to(&mut self, at: RowAt, col: usize) {
        self.edit_grid_sync();
        let t = self.tabs.active_mut();
        let Some(rs) = shown_mut(&mut t.results, t.exec.view) else { return };
        let (Some(row), len) = (shown_row(rs, at), rs.rows.len()) else { return };
        let g = &mut t.grid;
        if !g.window(len).contains(&row) && g.page_size > 0 {
            // A row read is on its own page, on screen; new rows are on the last page.
            match at {
                RowAt::Read(_) => (g.page, g.top) = (row / g.page_size, row),
                RowAt::New(_) => g.page = len.saturating_sub(1) / g.page_size,
            }
        }
        (g.row, g.col) = (row, col);
        g.anchor = None;
        g.detached = false;
        self.selecting_done();
    }

    /// What results tab `t` reads now are bound to; `None` without a profile.
    pub(super) fn edit_bound(&self, t: &Tab) -> Option<Bound> {
        Some(Bound {
            binding: t.binding,
            profile: t.profile?,
            session: t.exec.generation,
            database: self.other_database(t).map(str::to_string),
        })
    }

    /// Whether the active tab's shown result takes a change now: its profile connected, its
    /// environment known and not read-only, its server taking edits, and the tab bound as when
    /// the rows were read. Why not is said; else the profile, the format and that binding.
    fn edit_gate(&mut self) -> Option<(ProfileId, EditFormat, Bound)> {
        match self.edit_check() {
            Ok(ok) => Some(ok),
            Err(why) => {
                self.say_not(why.map(|o| o.notice));
                None
            }
        }
    }

    /// Say why an edit key did nothing, if there is something to say.
    fn say_not(&mut self, why: Option<Notice>) {
        if let Some(n) = why {
            self.flash(n);
        }
    }

    /// [`App::edit_gate`] without saying anything: else what says why not, and its few words
    /// for a menu (nothing without a row result).
    fn edit_check(&self) -> Result<(ProfileId, EditFormat, Bound), Option<Box<Off>>> {
        let t = self.tab();
        let rs = rows_of(t).ok_or(None)?;
        let off = |short: Label, n: Notice| Err(Some(Box::new(Off::new(short, n))));
        let Some(pid) = t.profile.filter(|p| self.conns.is_connected(*p)) else {
            return off(Label::MenuOffNotConnected, self.read_only_notice(Label::EditWhyNotConnected));
        };
        if let Err(e) = self.policy_of(pid) {
            let name = self.profile(pid).map(|p| p.name.clone()).unwrap_or_default();
            let n = self.unknown_environment_notice(&name, &e);
            return off(Label::MenuOffEnvironment, Notice::new(n.msg, Level::Info));
        }
        // The environment is read-only, or was when the tab's session opened (that session is
        // read-only on the server).
        if self.read_only(pid) || (t.exec.session.is_some() && t.exec.read_only) {
            return off(Label::MenuOffReadOnly, self.read_only_notice(Label::EditWhyReadOnly));
        }
        let Some(format) = self.tab_edit_format(t.id) else {
            return off(Label::MenuOffNotEditable, self.why_notice(&Why::Rows(ReadOnly::NoFormat)));
        };
        // Nothing changes the changes being applied (a cell editor or a list of values opened
        // before the Apply was sent).
        if rs.edit.applying.is_some() {
            return off(Label::MenuOffApplying, self.refused_notice(Label::EditWhyApplying.into()));
        }
        // What an Apply changed shows over the rows as read: they are read again first.
        if rs.edit.applied.is_some() {
            return off(Label::MenuOffRunAgain, self.refused_notice(Label::EditWhyApplied.into()));
        }
        // Read under another connection or database, the rows may not be the table's now.
        let bound = self.edit_bound(t).filter(|b| rs.edit.bound.as_ref() == Some(b));
        let Some(bound) = bound else {
            return off(Label::MenuOffRunAgain, Notice::new(Label::EditConnectionChanged, Level::Info));
        };
        Ok((pid, format, bound))
    }

    /// The table `rs` (tab `t`'s shown result) reads, as its statement and columns say, while
    /// nothing is pending (`None` then: the keys may have come or changed since it was resolved).
    fn edit_source(&self, t: &Tab, rs: &ResultSet) -> Option<Result<EditSource, ReadOnly>> {
        rs.edit.batch.is_empty().then(|| {
            let cols: Vec<ColumnMeta> = rs.columns.iter().map(|c| c.meta.clone()).collect();
            edit::source(self.tab_dialect(t.id), self.tab_keys(t).catalog(), &cols, t.shown_sql())
        })
    }

    /// Whether edit action `a` would act on the active tab's shown result, its cursor and its
    /// selection now, by the checks its key makes before it changes anything (the action menu
    /// offers those, and shows the others it has a reason for dimmed). Before the key first read
    /// the table's details, what its columns take is not known: only the result, the cursor and
    /// the selection are checked then.
    pub(super) fn edit_offer(&self, a: EditAction) -> Result<(), NotOffered> {
        use NotOffered as N;
        if self.tabs.is_empty() {
            return Err(N::Hidden);
        }
        let t = self.tab();
        let Some(rs) = rows_of(t) else { return Err(N::Hidden) };
        let gate = || self.edit_check().map(drop).map_err(|o| o.map_or(N::Hidden, N::Result));
        let has = |b: bool| if b { Ok(()) } else { Err(N::Hidden) };
        let batch = &rs.edit.batch;
        let writes = !matches!(
            a,
            EditAction::Undo | EditAction::Redo | EditAction::Revert | EditAction::Discard | EditAction::Preview
        );
        // While changes are applied nothing changes them: the writes say so.
        if rs.edit.applying.is_some() {
            return Err(if writes { gate().err().unwrap_or(N::Hidden) } else { N::Hidden });
        }
        match a {
            EditAction::Undo => return has(batch.can_undo()),
            EditAction::Redo => return has(batch.can_redo()),
            EditAction::Revert | EditAction::Discard | EditAction::Preview => return has(!batch.is_empty()),
            EditAction::Paste | EditAction::Pasted => return gate(),
            EditAction::PasteAs => return has(self.last_paste.is_some()).and_then(|()| gate()),
            EditAction::PasteRegister | EditAction::PasteRows => {
                return has(self.grid_copy.is_some()).and_then(|()| gate());
            }
            EditAction::Apply => {
                has(!batch.is_empty())?;
                gate()?;
                return has(matches!(rs.edit.target, Target::Ready(_)) && !self.tab_busy(t.id));
            }
            _ => {}
        }
        gate()?;
        // As the key resolves it: a statement or columns that are not one table's, a table
        // known to be read-only, or changes pending on a table no longer known.
        let not_editable = |why: &Why| N::Result(Box::new(Off::new(Label::MenuOffNotEditable, self.why_notice(why))));
        let target = match (&rs.edit.target, self.edit_source(t, rs)) {
            (_, Some(Err(r))) => return Err(not_editable(&Why::Rows(r))),
            (Target::ReadOnly { why, retry: false }, _) | (Target::ReadOnly { why, .. }, None) => {
                return Err(not_editable(why));
            }
            (Target::Unresolved, None) => {
                let n = self.read_only_notice(Label::EditWhyNotResolved);
                return Err(N::Result(Box::new(Off::new(Label::MenuOffRunAgain, n))));
            }
            (Target::Ready(r), None) => Some(r),
            (Target::Ready(r), Some(Ok(s))) if s.table == r.table => Some(r),
            _ => None,
        };
        // A new row is refused for the table, not for the cell: no column takes a value in one.
        let short = if a == EditAction::AddRow { Label::MenuOffNoColumn } else { Label::MenuOffHere };
        let here = |r: Result<(), Option<Notice>>| {
            r.map_err(|n| n.map_or(N::Hidden, |n| N::Here(Box::new(Off::new(short, n)))))
        };
        match target {
            // Every check the key makes before it changes anything.
            Some(target) => here(self.edit_acts(a, target)),
            // The table's details are read by the key first: what its columns take is not
            // known yet, only what the cursor and the selection allow.
            None => match a {
                EditAction::Cell | EditAction::SetNull | EditAction::SetDefault => here(self.cells_rows().map(drop)),
                EditAction::DeleteRow => here(self.delete_rows().map(drop)),
                _ => Ok(()),
            },
        }
    }

    /// An edit key that writes: refused with the reason when the result cannot be edited (now);
    /// done when its target is resolved, else the table's structure is asked for and the press
    /// waits for it.
    fn edit_resolve(&mut self, a: EditAction) {
        let Some((pid, format, bound)) = self.edit_gate() else { return };
        let t = self.tab();
        let Some(rs) = rows_of(t) else { return };
        // What the rows are is read again while nothing is pending: the keys may have come or
        // changed since.
        let source = self.edit_source(t, rs);
        let generation = self.conns.get(pid).map_or(0, |c| c.generation);
        let aux = self.open_aux(pid, bound.database.as_deref());
        let (tab, then) = (t.id, (a, (t.grid.row, t.grid.col), t.grid.anchor));
        let Some(rs) = rows_of_mut(self.tabs.active_mut()) else { return };
        let result = rs.id;
        let st = &mut rs.edit;
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
                    || w.aux != aux
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
                // Changes pending on a target no longer known: what they are changes is not.
                let Some(source) = source else { return self.say_edit_read_only(Label::EditWhyNotResolved) };
                let cmd =
                    DbCommand::LoadStructure { schema: source.table.schema.clone(), table: source.table.name.clone() };
                // Another database's session, lost or closed for being idle, opens again.
                let aux = match &bound.database {
                    Some(db) => {
                        self.ensure_aux(pid, db);
                        self.open_aux(pid, Some(db))
                    }
                    None => None,
                };
                let session = match &bound.database {
                    None => self.conns.get(pid).and_then(|c| c.meta.as_ref()),
                    Some(db) => self.conns.aux(pid, db).and_then(|a| a.session.as_ref()),
                };
                let Some(session) = session else { return self.say_edit_read_only(Label::EditWhyNotConnected) };
                session.send(cmd);
                let ticket = session.structures_asked();
                self.touch_profile(pid);
                let wait = EditWait {
                    tab,
                    binding: bound.binding,
                    profile: pid,
                    generation,
                    database: bound.database,
                    aux,
                    result,
                    ticket,
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
        match a {
            EditAction::AddRow => return self.edit_add_row(target),
            EditAction::CloneRow => return self.edit_clone_row(target),
            EditAction::DeleteRow => return self.edit_delete_rows(target),
            EditAction::Pasted => return self.edit_paste(target),
            _ => {}
        }
        let plan = match self.cells_plan(a, target) {
            Ok(p) => p,
            Err(why) => return self.say_not(why),
        };
        let t = self.tab();
        let Some(rs) = rows_of(t) else { return };
        let (row, col) = (t.grid.row, t.grid.col);
        let result = rs.id;
        match plan {
            CellsPlan::New { at, id, cells, c } => {
                let current =
                    rs.edit.batch.drafts().iter().find(|d| d.id == id).and_then(|d| d.cells.get(col)).cloned();
                match a {
                    EditAction::SetNull => self.edit_write_cells(&cells, Value::Null),
                    // A new row takes any column's default, a new number too.
                    EditAction::SetDefault => self.edit_write_cells(&cells, Value::Default),
                    EditAction::Cell if !c.column.enum_labels.is_empty() || c.kind == ValueKind::Bool => {
                        self.open_cell_picker(&c, cells, result, current, true);
                    }
                    EditAction::Cell => {
                        let text = match current {
                            Some(Value::Text(s)) => s,
                            _ => String::new(),
                        };
                        let lines = text.contains(['\n', '\r']) || in_lines(&c);
                        let input = TextInput::new(&text);
                        let error = self.invalid_reason(at, col);
                        let invalid = error.is_some();
                        // A cell that needs a value is not invalid: it says why it needs one.
                        let error = error.or_else(|| {
                            let column = c.column.name.clone();
                            self.value_needed(at, col)
                                .then(|| self.i18n.msg(&Msg::EditWhyNoDefault { column }).to_string())
                        });
                        self.open_cell_edit(
                            CellEdit { result, at: (at, col), cells, input, lines: None, error, invalid },
                            &text,
                            lines,
                        );
                    }
                    _ => {}
                }
            }
            CellsPlan::Read { rows, c, read } => {
                let cells: Vec<(RowAt, usize)> = rows.clone().map(|r| (RowAt::Read(r), col)).collect();
                // The cursor's value: its pending one, else as read.
                let pending = rs.edit.batch.value(row, col).cloned();
                let stored = read.get(row - rows.start).and_then(|r| r.get(col)).cloned().flatten();
                match a {
                    EditAction::SetNull => self.edit_write_cells(&cells, Value::Null),
                    EditAction::SetDefault => self.edit_write_cells(&cells, Value::Default),
                    EditAction::Cell if !c.column.enum_labels.is_empty() || c.kind == ValueKind::Bool => {
                        let current = pending.or_else(|| Some(stored.map_or(Value::Null, Value::Text)));
                        self.open_cell_picker(&c, cells, result, current, false);
                    }
                    EditAction::Cell => {
                        let text = match pending {
                            Some(Value::Text(s)) => s,
                            Some(Value::Null | Value::Default) => String::new(),
                            None => stored.unwrap_or_default(),
                        };
                        let at = (RowAt::Read(row), col);
                        let lines = text.contains(['\n', '\r']) || in_lines(&c);
                        let error = self.invalid_reason(at.0, col);
                        let invalid = error.is_some();
                        let input = TextInput::new(&text);
                        let e = CellEdit { result, at, cells, input, lines: None, error, invalid };
                        self.open_cell_edit(e, &text, lines);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Whether edit key `a` would act on a result resolved to `target`, by the checks it makes
    /// before it changes anything ([`App::edit_ready`]); else what says why not.
    fn edit_acts(&self, a: EditAction, target: &EditTarget) -> Result<(), Option<Notice>> {
        match a {
            EditAction::Cell | EditAction::SetNull | EditAction::SetDefault => self.cells_plan(a, target).map(drop),
            EditAction::AddRow => {
                let n = rows_of(self.tab()).ok_or(None)?.columns.len();
                self.insert_column(target, n).map(drop)
            }
            EditAction::CloneRow => self.clone_plan(target).map(drop),
            EditAction::DeleteRow => self.delete_plan(target).map(drop),
            _ => Ok(()),
        }
    }

    /// The rows an edit key that sets cells acts on, by the cursor and the selection alone
    /// (nothing about the table): the cursor's, or those selected in one column; new rows or
    /// rows read, not both; no row read marked to be deleted. Else what says why not (nothing
    /// when the cursor is on no cell).
    fn cells_rows(&self) -> Result<Range<usize>, Option<Notice>> {
        let t = self.tab();
        let rs = rows_of(t).ok_or(None)?;
        let (total, ncols) = (rs.rows.len(), rs.columns.len());
        let (row, col) = (t.grid.row, t.grid.col);
        let shown = total + rs.edit.batch.drafts().len();
        if row >= shown || col >= ncols {
            return Err(None);
        }
        let rows = match t.grid.selection(total, ncols) {
            None => row..row + 1,
            Some((rows, cols)) if t.grid.shape == Shape::Cells && cols.len() == 1 => rows,
            Some(_) => return Err(Some(Notice::new(Label::EditSeveralColumns, Level::Info))),
        };
        if rows.end > total {
            if rows.start < total {
                return Err(Some(Notice::new(Label::EditMixedRows, Level::Info)));
            }
            return Ok(rows);
        }
        if let Some(r) = rows.clone().find(|r| rs.edit.batch.deleted(*r)) {
            return Err(Some(self.refused_notice(Msg::EditWhyRowDeleted { row: (r + 1).to_string() })));
        }
        Ok(rows)
    }

    /// What an edit key that sets cells does on a result resolved to `target`, once every check
    /// passed; else what says why not.
    fn cells_plan(&self, a: EditAction, target: &EditTarget) -> Result<CellsPlan, Option<Notice>> {
        let rows = self.cells_rows()?;
        let t = self.tab();
        let rs = rows_of(t).ok_or(None)?;
        let (total, ncols) = (rs.rows.len(), rs.columns.len());
        let col = t.grid.col;
        if rows.end > total {
            return self.new_cells_plan(a, target, rows, col);
        }
        let refused = |why: Msg| Some(self.refused_notice(why));
        let name = crate::text::escape_controls(&rs.columns[col].meta.name);
        let c = match target.allows(col, Write::Update) {
            Ok(c) => c.clone(),
            Err(r) => return Err(refused(column_reason(r, name))),
        };
        let Some(read) = read_rows(rs, rows.clone()) else {
            return Err(refused(Msg::EditWhyNotRead { row: (rows.start + 1).to_string() }));
        };
        // A row with fewer values than the result has columns was not read whole.
        if let Some(k) = read.iter().position(|r| r.len() < ncols) {
            return Err(refused(Msg::EditWhyNotRead { row: (rows.start + k + 1).to_string() }));
        }
        let d = self.tab_dialect(t.id);
        let zone = self.tab_keys(t).catalog().and_then(|k| k.server_zone());
        if let Some((r, why)) = not_whole(d, rs, target, &read, rows.start, zone) {
            let reason = self.not_whole_reason(&why.why);
            return Err(refused(Msg::EditWhyNotWhole { row: (r + 1).to_string(), reason }));
        }
        // NULL and DEFAULT read no value: only a typed one must be stored as shown.
        let typed = a == EditAction::Cell;
        let many = rows.len() > 1;
        if let Some((r, why)) = value_not_whole(d, rs, col, &read, rows.start, zone, many).filter(|_| typed) {
            let (column, reason) = (crate::text::escape_controls(&why.column), self.not_whole_reason(&why.why));
            return Err(refused(match r {
                None if why.why == export::NotWhole::Float => Msg::EditWhyShownRounded { column },
                None => Msg::EditWhyColumnNotWhole { column, reason },
                Some(r) => Msg::EditWhyValueNotWhole { row: (r + 1).to_string(), column, reason },
            }));
        }
        if let Some(why) = column_refusal(a, &c, name) {
            return Err(refused(why));
        }
        Ok(CellsPlan::Read { rows, c, read })
    }

    /// An edit key on cells of new rows `rows` (as the grid shows them) in column `col`: the
    /// column must take a value in a new row; no row is found again, so nothing is read. Any
    /// column that takes one takes its default.
    fn new_cells_plan(
        &self,
        a: EditAction,
        target: &EditTarget,
        rows: Range<usize>,
        col: usize,
    ) -> Result<CellsPlan, Option<Notice>> {
        let t = self.tab();
        let rs = rows_of(t).ok_or(None)?;
        let ids: Vec<RowAt> = rows.clone().filter_map(|r| row_at(rs, r)).collect();
        let Some(at @ RowAt::New(id)) = row_at(rs, t.grid.row).filter(|_| ids.len() == rows.len()) else {
            return Err(None);
        };
        let refused = |why: Msg| Some(self.refused_notice(why));
        let name = crate::text::escape_controls(&rs.columns[col].meta.name);
        let c = match target.allows(col, Write::Insert) {
            Ok(c) => c.clone(),
            Err(r) => return Err(refused(column_reason(r, name))),
        };
        let d = self.tab_dialect(t.id);
        let zone = self.tab_keys(t).catalog().and_then(|k| k.server_zone());
        // A typed value is stored as shown only in a session that shows it so (a new row holds
        // no value shown rounded).
        let typed = a == EditAction::Cell;
        if let Some((_, why)) = value_not_whole(d, rs, col, &[], 0, zone, false).filter(|_| typed) {
            let (column, reason) = (crate::text::escape_controls(&why.column), self.not_whole_reason(&why.why));
            return Err(refused(Msg::EditWhyColumnNotWhole { column, reason }));
        }
        if a == EditAction::SetNull && c.column.not_null {
            return Err(refused(Msg::EditWhyNotNull { column: name }));
        }
        let cells: Vec<(RowAt, usize)> = ids.into_iter().map(|r| (r, col)).collect();
        Ok(CellsPlan::New { at, id, cells, c })
    }

    /// The first column of `target` that takes a value in a new row; else why none does.
    fn insert_column(&self, target: &EditTarget, ncols: usize) -> Result<usize, Option<Notice>> {
        (0..ncols)
            .find(|&c| target.allows(c, Write::Insert).is_ok())
            .ok_or_else(|| Some(self.refused_notice(Label::EditWhyNoInsertColumn.into())))
    }

    /// `o`: a new row, every value its column's default, below the cursor's row (right after
    /// the cursor's new row; else after the other new rows). The cursor goes to its first column
    /// that takes a value.
    fn edit_add_row(&mut self, target: &EditTarget) {
        let t = self.tab();
        let Some(rs) = rows_of(t) else { return };
        let ncols = rs.columns.len();
        let next_to = match row_at(rs, t.grid.row) {
            Some(RowAt::New(id)) => Some(id),
            _ => None,
        };
        let first = match self.insert_column(target, ncols) {
            Ok(c) => c,
            Err(why) => return self.say_not(why),
        };
        let Some(rs) = rows_of_mut(self.tabs.active_mut()) else { return };
        // Every new row is shown after the rows read: in the order of the batch.
        let id = rs.edit.batch.add(usize::MAX, next_to, vec![Value::Default; ncols]);
        self.edit_cursor_to(RowAt::New(id), first);
    }

    /// `O`: a new row with the values of the cursor's row (a row read: its pending values, else
    /// as read), but what the table gives a new row itself: the key's columns, a number drawn
    /// (an identity, `AUTO_INCREMENT`, a sequence), a generated or computed column take their
    /// default. A value as read must be shown whole, else the copy is refused.
    fn edit_clone_row(&mut self, target: &EditTarget) {
        let (cells, next_to) = match self.clone_plan(target) {
            Ok(p) => p,
            Err(why) => return self.say_not(why),
        };
        let col = self.tab().grid.col;
        let Some(rs) = rows_of_mut(self.tabs.active_mut()) else { return };
        let id = rs.edit.batch.add(usize::MAX, next_to, cells);
        self.edit_cursor_to(RowAt::New(id), col);
    }

    /// The values of the row `O` adds, and the new row it goes after; else why not.
    fn clone_plan(&self, target: &EditTarget) -> Result<(Vec<Value>, Option<u64>), Option<Notice>> {
        let t = self.tab();
        let rs = rows_of(t).ok_or(None)?;
        let ncols = rs.columns.len();
        let at = row_at(rs, t.grid.row).ok_or(None)?;
        self.insert_column(target, ncols)?;
        // The values, and the one as read of each cell not set.
        let (values, read): (Vec<Value>, Vec<Option<String>>) = match at {
            RowAt::New(id) => {
                let d = rs.edit.batch.drafts().iter().find(|d| d.id == id).ok_or(None)?;
                (d.cells.clone(), vec![None; ncols])
            }
            RowAt::Read(r) => {
                let row = read_rows(rs, r..r + 1).and_then(|mut v| v.pop()).filter(|row| row.len() >= ncols);
                let Some(row) = row else {
                    return Err(Some(self.refused_notice(Msg::EditWhyNotRead { row: (r + 1).to_string() })));
                };
                (0..ncols)
                    .map(|c| match rs.edit.batch.value(r, c) {
                        Some(v) => (v.clone(), None),
                        None => (row[c].clone().map_or(Value::Null, Value::Text), row[c].clone()),
                    })
                    .unzip()
            }
        };
        let keys: &[usize] = match &target.identity {
            Identity::Key { columns, .. } => columns,
            Identity::Values { .. } => &[],
        };
        let d = self.tab_dialect(t.id);
        let zone = self.tab_keys(t).catalog().and_then(|k| k.server_zone());
        let mut cells = Vec::with_capacity(ncols);
        for (c, (v, as_read)) in values.into_iter().zip(read).enumerate() {
            let own = keys.contains(&c) || target.allows(c, Write::Insert).map_or(true, new_number);
            if own {
                cells.push(Value::Default);
                continue;
            }
            if let Some(text) = as_read
                && !shown_whole(d, rs, c, &text, zone)
            {
                let column = rs.columns[c].meta.name.clone();
                return Err(Some(self.refused_notice(Msg::EditWhyCloneNotWhole { column })));
            }
            cells.push(v);
        }
        let next_to = match at {
            RowAt::New(id) => Some(id),
            RowAt::Read(_) => None,
        };
        Ok((cells, next_to))
    }

    /// The rows `D` acts on, by the cursor and the selection alone: the cursor's, or the
    /// selected rows (not whole columns). Else what says why not.
    fn delete_rows(&self) -> Result<Range<usize>, Option<Notice>> {
        let t = self.tab();
        let rs = rows_of(t).ok_or(None)?;
        let (len, ncols) = (rs.rows.len(), rs.columns.len());
        let shown = len + rs.edit.batch.drafts().len();
        let rows = match t.grid.selection(len, ncols) {
            None => t.grid.row..t.grid.row + 1,
            Some(_) if t.grid.shape == Shape::Cols => {
                return Err(Some(Notice::new(Label::EditDeleteColumns, Level::Info)));
            }
            Some((rows, _)) => rows,
        };
        let rows = rows.start..rows.end.min(shown);
        if rows.is_empty() { Err(None) } else { Ok(rows) }
    }

    /// What `D` changes on a result resolved to `target`: the rows read it marks (from the
    /// first, as read) and the new rows it removes; else why not. A row read must be found
    /// again by values shown whole.
    fn delete_plan(&self, target: &EditTarget) -> Result<DeletePlan, Option<Notice>> {
        let rows = self.delete_rows()?;
        let t = self.tab();
        let rs = rows_of(t).ok_or(None)?;
        let (len, ncols) = (rs.rows.len(), rs.columns.len());
        let read = rows.start.min(len)..rows.end.min(len);
        let ids: Vec<u64> = (rows.start.max(len)..rows.end)
            .filter_map(|r| match row_at(rs, r) {
                Some(RowAt::New(id)) => Some(id),
                _ => None,
            })
            .collect();
        let refused = |why: Msg| Some(self.refused_notice(why));
        let mut orig = Vec::new();
        if !read.is_empty() {
            let Some(got) = read_rows(rs, read.clone()) else {
                return Err(refused(Msg::EditWhyNotRead { row: (read.start + 1).to_string() }));
            };
            if let Some(k) = got.iter().position(|r| r.len() < ncols) {
                return Err(refused(Msg::EditWhyNotRead { row: (read.start + k + 1).to_string() }));
            }
            let d = self.tab_dialect(t.id);
            let zone = self.tab_keys(t).catalog().and_then(|k| k.server_zone());
            if let Some((r, why)) = not_whole(d, rs, target, &got, read.start, zone) {
                let reason = self.not_whole_reason(&why.why);
                return Err(refused(Msg::EditWhyNotWhole { row: (r + 1).to_string(), reason }));
            }
            orig = got;
        }
        Ok(DeletePlan { first: read.start, marked: read.collect(), ids, orig })
    }

    /// `D`: the cursor's row, or the rows of the selection (not whole columns): rows read are
    /// marked to be deleted (unmarked when every one is), new rows removed, as one change.
    fn edit_delete_rows(&mut self, target: &EditTarget) {
        let p = match self.delete_plan(target) {
            Ok(p) => p,
            Err(why) => return self.say_not(why),
        };
        let Some(rs) = rows_of_mut(self.tabs.active_mut()) else { return };
        let (first, orig) = (p.first, p.orig);
        rs.edit.batch.delete_with_new(&p.marked, &p.ids, |r| orig.get(r - first).cloned().unwrap_or_default());
        self.tab_mut().grid.anchor = None;
        self.selecting_done();
        self.edit_grid_sync();
    }

    /// The list of the values of an enum or boolean column `c`, then NULL and DEFAULT when it
    /// takes them (DEFAULT always in a new row, `new_row`); a pick goes into `cells` of result
    /// `result`.
    fn open_cell_picker(
        &mut self,
        c: &TableColumn,
        cells: Vec<(RowAt, usize)>,
        result: u64,
        current: Option<Value>,
        new_row: bool,
    ) {
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
        if new_row || (has_default(c) && !new_number(c)) {
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
        if !self.tabs.is_empty()
            && self.tab().id == tab
            && rows_of(self.tab()).is_some_and(|rs| rs.id == result)
            && self.edit_gate().is_some()
        {
            self.edit_write_cells(&cells, v);
        }
    }

    /// Set `cells` (rows of one column, in order: rows read, or new rows) of the active tab's
    /// shown result to `v`, as one change.
    fn edit_write_cells(&mut self, cells: &[(RowAt, usize)], v: Value) {
        let Some(rs) = rows_of_mut(self.tabs.active_mut()) else { return };
        let new: Vec<(u64, usize)> = cells
            .iter()
            .filter_map(|&(r, c)| match r {
                RowAt::New(id) => Some((id, c)),
                RowAt::Read(_) => None,
            })
            .collect();
        if !new.is_empty() {
            if new.len() == cells.len() {
                rs.edit.batch.set_new(&new, &v);
            }
            return;
        }
        let cells: Vec<(usize, usize)> = cells
            .iter()
            .filter_map(|&(r, c)| match r {
                RowAt::Read(r) => Some((r, c)),
                RowAt::New(_) => None,
            })
            .collect();
        let (Some(first), Some(last)) = (cells.first(), cells.last()) else { return };
        let first = first.0;
        let Some(read) = read_rows(rs, first..last.0 + 1) else {
            return self.say_refused(Msg::EditWhyNotRead { row: (first + 1).to_string() });
        };
        rs.edit.batch.set(&cells, &v, |r| read.get(r - first).cloned().unwrap_or_default());
    }

    /// The open cell editor's cells and their table column, while its result is resolved.
    fn cell_edit_column(&self) -> Option<(Vec<(RowAt, usize)>, TableColumn)> {
        let t = self.tab();
        let e = t.cell_edit.as_ref()?;
        let rs = rows_of(t).filter(|rs| rs.id == e.result)?;
        let Target::Ready(target) = &rs.edit.target else { return None };
        let w = if e.new_row() { Write::Insert } else { Write::Update };
        let c = target.allows(e.at.1, w).ok()?;
        Some((e.cells.clone(), c.clone()))
    }

    /// A key while the cell editor is open: `Enter` keeps the text (an empty one is the empty
    /// string), `Esc` cancels, `Ctrl+N` sets NULL and `Ctrl+D` the default, `Ctrl+L` takes the
    /// text to the editor of several lines; any other key edits the text.
    pub(super) fn cell_edit_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.tabs.active_mut().cell_edit = None,
            KeyCode::Enter => self.cell_edit_enter(),
            KeyCode::Char('n') if ctrl => self.cell_edit_value(Value::Null),
            KeyCode::Char('d') if ctrl => self.cell_edit_value(Value::Default),
            KeyCode::Char('l') if ctrl => self.cell_edit_to_lines(),
            _ => {
                if let Some(e) = self.tabs.active_mut().cell_edit.as_mut()
                    && e.input.handle_key(&key) == InputResult::Changed
                {
                    e.error = None;
                }
            }
        }
    }

    /// A press at (`x`, `y`) outside the open cell editor closes it, changing nothing (as `Esc`
    /// does); the press is then handled as anywhere else. A press inside is the editor's (a left
    /// one moves its text cursor) and goes nowhere else: `true`.
    pub(super) fn cell_edit_click(&mut self, x: u16, y: u16, left: bool) -> bool {
        if !self.cell_edit_open() {
            return false;
        }
        if self.cell_lines_open() {
            return self.cell_lines_click(x, y, left);
        }
        let t = self.tab();
        let over = match (&t.results, &t.cell_edit) {
            (Results::Rows(rs), Some(e)) => e.shown_at(rs).and_then(|(r, c)| t.grid.cell_rect(r, c, rs.rows.len())),
            _ => None,
        };
        let inside = over.is_some_and(|r| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height);
        let e = self.tabs.active_mut().cell_edit.as_mut();
        match e {
            Some(e) if inside => {
                if left {
                    e.input.click(x, y);
                }
            }
            _ => self.tabs.active_mut().cell_edit = None,
        }
        inside
    }

    /// Text pasted into the cell editor: one line only (the editor of several lines takes any).
    pub(super) fn cell_edit_paste(&mut self, text: &str) {
        if self.cell_lines_open() {
            return self.cell_lines_paste(text);
        }
        if text.contains(['\n', '\r']) {
            return self.flash(Notice::new(Label::EditPasteLines, Level::Info));
        }
        if let Some(e) = self.tabs.active_mut().cell_edit.as_mut() {
            e.input.insert_str(text);
            e.error = None;
        }
    }

    /// `Enter` in the cell editor (`Enter` in Normal mode or `:w` in the editor of several lines):
    /// the text, when it is a value of the column's type.
    fn cell_edit_enter(&mut self) {
        let Some((cells, c)) = self.cell_edit_column().filter(|_| self.edit_gate().is_some()) else {
            self.tabs.active_mut().cell_edit = None;
            return;
        };
        let text = self.tab().cell_edit.as_ref().map(CellEdit::text).unwrap_or_default();
        let d = self.tab_dialect(self.tab().id);
        if let Err(reason) = self.typed_whole(d, &text).and_then(|_| self.typed_ok(d, &c, &text)) {
            if let Some(e) = self.tabs.active_mut().cell_edit.as_mut() {
                e.error = Some(reason.clone());
                e.invalid = false;
            }
            return self.flash(Notice::new(Msg::EditRefused { reason }, Level::Info));
        }
        self.tabs.active_mut().cell_edit = None;
        self.edit_write_cells(&cells, Value::Text(text));
    }

    /// `Ctrl+N` or `Ctrl+D` in the cell editor: NULL or the default, when the column takes it.
    fn cell_edit_value(&mut self, v: Value) {
        let Some((cells, c)) = self.cell_edit_column().filter(|_| self.edit_gate().is_some()) else {
            self.tabs.active_mut().cell_edit = None;
            return;
        };
        let column = crate::text::escape_controls(&c.column.name);
        // A new row takes any column's default.
        let new_row = self.tab().cell_edit.as_ref().is_some_and(|e| e.new_row());
        let why = match v {
            Value::Null if c.column.not_null => Some(Msg::EditWhyNotNull { column }),
            Value::Default if new_row => None,
            Value::Default if !has_default(&c) => Some(Msg::EditWhyNoDefault { column }),
            Value::Default if new_number(&c) => Some(Msg::EditWhyNewNumber { column }),
            _ => None,
        };
        if let Some(why) = why {
            if let Some(e) = self.tabs.active_mut().cell_edit.as_mut() {
                e.error = Some(self.i18n.msg(&why).to_string());
                e.invalid = false;
            }
            return self.say_refused(why);
        }
        self.tabs.active_mut().cell_edit = None;
        self.edit_write_cells(&cells, v);
    }

    /// Whether `text`, typed in the open editor, is stored as typed: judged itself as the
    /// column's values are ([`value_not_whole`]); why not, in words.
    fn typed_whole(&self, d: Dialect, text: &str) -> Result<(), String> {
        let t = self.tab();
        let (Some(rs), Some(e)) = (rows_of(t), t.cell_edit.as_ref()) else { return Ok(()) };
        let col = e.at.1;
        let mut row: Vec<Cell> = vec![None; col + 1];
        row[col] = Some(text.to_string());
        let zone = self.tab_keys(t).catalog().and_then(|k| k.server_zone());
        let Some((_, refused)) = value_not_whole(d, rs, col, &[row], 0, zone, false) else { return Ok(()) };
        let (column, reason) = (crate::text::escape_controls(&refused.column), self.not_whole_reason(&refused.why));
        Err(self.i18n.msg(&Msg::EditWhyColumnNotWhole { column, reason }).to_string())
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
            // Named as the table's structure names the type (MySQL's `decimal(14,2)`).
            I::TooManyDigits { .. } => Msg::EditInvalidTooManyDigits { type_name: c.column.type_name.clone() },
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
            I::Rounded { type_name, stored } => Msg::EditInvalidRounded { type_name, stored },
            I::TrailingSpaces => Label::EditInvalidTrailingSpaces.into(),
        };
        Err(self.i18n.msg(&msg).to_string())
    }

    /// After every event: the cell editor closes (changing nothing) when its result is no
    /// longer the one shown in the grid, its tab not the active one, or the grid lost the focus;
    /// the list of a cell's values too, and the preview of the changes when it is no longer
    /// current.
    pub(super) fn sweep_cell_edit(&mut self) {
        let active = (!self.tabs.is_empty()).then(|| self.tab().id);
        let focused = self.focus == Focus::Results;
        for t in self.tabs.iter_mut() {
            let Some(e) = &t.cell_edit else { continue };
            let grid = t.exec.view == tabs::ResultView::Rows;
            if !focused || !grid || Some(t.id) != active || rows_of(t).is_none_or(|rs| rs.id != e.result) {
                t.cell_edit = None;
            }
        }
        if let Some(Picker::CellValue { tab, result, .. }) = self.overlays.picker() {
            let holds = active == Some(*tab)
                && self.tab().exec.view == tabs::ResultView::Rows
                && rows_of(self.tab()).is_some_and(|rs| rs.id == *result);
            if !holds {
                self.overlays.close(OverlayKind::Picker);
            }
        }
        // The filter history of a table goes with its tab, and when the tab's table changed.
        if let Some(Picker::FilterHistory { tab, key, .. }) = self.overlays.picker()
            && (active != Some(*tab) || self.filter_history_key(*tab).as_ref() != Some(key))
        {
            self.overlays.close(OverlayKind::Picker);
        }
        self.sweep_edit_preview();
        self.sweep_status_ask();
        self.sweep_changes();
        // What became of an Apply's changes, said after the event's own outcome.
        let said: Vec<(TabId, Notice)> =
            self.tabs.iter_mut().filter_map(|t| Some((t.id, t.edit_said.take()?))).collect();
        for (id, n) in said {
            if self.tab().id == id {
                self.edit_grid_sync();
            }
            self.tab_status(id, n);
        }
    }

    /// The structure of `schema.table` in `profile`'s `database` came (`error`: why it could not
    /// be read): every result waiting for this answer (not an earlier request's, which may be of
    /// the table before it changed) whose tab is bound as it was is resolved; the others wait no
    /// more (their next edit key asks again). A press waiting in the active tab's shown result
    /// is done, if the cursor and the selection are where they were.
    pub(super) fn edit_structure_came(
        &mut self,
        profile: ProfileId,
        database: Option<&str>,
        schema: &str,
        table: &str,
        result: Result<&TableStructure, &DbError>,
        error: &str,
    ) {
        let session = match database {
            None => self.conns.get(profile).and_then(|c| c.meta.as_ref()),
            Some(db) => self.conns.aux(profile, db).and_then(|a| a.session.as_ref()),
        };
        let answer = session.map_or(0, |s| s.structure_answered());
        let aux = self.open_aux(profile, database);
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
                    || w.ticket > answer
                {
                    continue;
                }
                let Target::Loading(w) = std::mem::take(&mut rs.edit.target) else { continue };
                let current = w.tab == id
                    && w.binding == binding
                    && bound_to == Some(profile)
                    && generation == Some(w.generation)
                    && other == w.database
                    && w.aux == aux
                    && w.result == rs.id
                    && w.ticket == answer;
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
        // The details came: the press is done or refused now, no longer loading.
        if self.transient.as_ref().is_some_and(|(n, _)| n.msg == Label::EditLoading.into()) {
            self.transient = None;
        }
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

    /// The id of `profile`'s open session in other database `database` (`None`: the profile's
    /// own, or no session open there).
    fn open_aux(&self, profile: ProfileId, database: Option<&str>) -> Option<u64> {
        let a = self.conns.aux(profile, database?)?;
        a.session.is_some().then_some(a.id)
    }

    /// Why the result's rows cannot be edited, as the status line says it.
    fn say_why(&mut self, why: &Why) {
        let n = self.why_notice(why);
        self.flash(n);
    }

    fn why_notice(&self, why: &Why) -> Notice {
        Notice::new(Msg::EditReadOnly { reason: self.why_text(why) }, Level::Info)
    }

    /// Why the result's rows cannot be edited, in words.
    fn why_text(&self, why: &Why) -> String {
        let label = |l| self.i18n.label(l).to_string();
        match why {
            Why::Rows(ReadOnly::Source(n)) => self.why(n),
            Why::Rows(ReadOnly::NoFormat) => label(Label::EditWhyNoFormat),
            Why::Rows(ReadOnly::NoTableColumn) => label(Label::EditWhyNoTableColumn),
            Why::Rows(ReadOnly::NotATable(_)) => label(Label::EditWhyNotATable),
            Why::Rows(ReadOnly::TableChanged) => label(Label::EditWhyTableChanged),
            Why::Rows(ReadOnly::NoIdentity) => label(Label::EditWhyNoIdentity),
            Why::Rows(ReadOnly::NoExactColumn) => label(Label::EditWhyNoExactColumn),
            Why::Structure(error) => self.i18n.msg(&Msg::EditWhyStructureFailed { error: error.clone() }).to_string(),
        }
    }

    /// How many changes are pending in tab `tab`'s results (shown or parked), or in every tab's
    /// (`None`); not those being applied.
    pub(super) fn pending_in(&self, tab: Option<TabId>) -> u64 {
        let pending = |t: &Tab| {
            let shown = match &t.results {
                Results::Rows(rs) => Some(rs),
                _ => None,
            };
            shown
                .into_iter()
                .chain(t.exec.steps.values().map(|p| &p.rs))
                .filter(|rs| rs.edit.applying.is_none())
                .map(|rs| rs.edit.batch.counts().total() as u64)
                .sum::<u64>()
        };
        self.tabs.iter().filter(|t| tab.is_none_or(|id| t.id == id)).map(pending).sum()
    }

    /// `leave` would drop pending changes: ask first (`Enter` keeps them, `y` drops them and
    /// goes on). Whether it asked.
    pub(super) fn ask_leave(&mut self, leave: Leave) -> bool {
        let tab = match leave {
            Leave::Run(t) | Leave::Close(t) => Some(t),
            Leave::Quit => None,
        };
        let count = self.pending_in(tab);
        if count == 0 {
            return false;
        }
        let text = match leave {
            Leave::Run(_) => Msg::EditLeaveRun { count },
            Leave::Close(_) => Msg::EditLeaveClose { count },
            Leave::Quit => Msg::EditLeaveQuit { count },
        };
        self.confirm(
            Label::EditDiscardTitle,
            Label::EditDiscardKeys,
            Label::EditDiscardKeys,
            ConfirmAction::LeaveEdits(leave),
        );
        if let Some(c) = self.overlays.confirm_mut() {
            c.text = text;
        }
        true
    }

    /// Yes to `leave`: the pending changes it would drop are dropped (each result's as one step
    /// `u` undoes), then it goes on.
    pub(super) fn leave_confirmed(&mut self, leave: Leave) {
        let tab = match leave {
            Leave::Run(t) | Leave::Close(t) => Some(t),
            Leave::Quit => None,
        };
        for t in self.tabs.iter_mut().filter(|t| tab.is_none_or(|id| t.id == id)) {
            for rs in t.row_results_mut().filter(|rs| rs.edit.applying.is_none()) {
                rs.edit.batch.discard();
            }
        }
        if !self.tabs.is_empty() {
            self.edit_grid_sync();
        }
        match leave {
            Leave::Run(t) => {
                if let Some((id, statements)) = self.pending_leave.take().filter(|p| p.0 == t) {
                    self.run_in(id, statements);
                }
            }
            Leave::Close(t) => self.request_close(t),
            Leave::Quit => self.request_quit(),
        }
    }

    /// How many changes of the active tab's shown result are pending.
    pub fn edits_pending(&self) -> usize {
        if self.tabs.is_empty() {
            return 0;
        }
        rows_of(self.tab()).map_or(0, |rs| rs.edit.batch.counts().total())
    }

    /// What result `rs` is to its edits, in at most `room` columns, and its style on background
    /// `bg`: its changes being applied, how many are pending, what an Apply changed, else whether its rows can be edited (the table and how
    /// a row is found, as room allows) or why not. Nothing before an edit key resolved it.
    /// `short`: what an Apply changed may say so in its short form.
    pub(crate) fn edit_badge(&self, rs: &ResultSet, room: usize, short: bool, bg: Color) -> Option<(String, Style)> {
        let th = crate::theme::cur();
        let width = crate::text::width;
        let fits = |text: String| (width(&text) <= room).then_some(text);
        if rs.edit.applying.is_some() {
            let text = self.i18n.label(Label::EditBadgeApplying).to_string();
            return fits(text).map(|t| (t, th.edit_pending.bg(bg)));
        }
        let pending = rs.edit.batch.counts().total();
        let invalid = self.invalid_count(rs) as u64;
        if pending > 0 && invalid > 0 {
            let p = datarig_i18n::fmt_count(pending as u64);
            let long = Msg::EditBadgePendingInvalid { pending: p.clone(), count: invalid };
            let text = self.i18n.msg(&long).to_string();
            let short = self.i18n.msg(&Msg::EditBadgePendingInvalidShort { pending: p, count: invalid }).to_string();
            let text = fits(text).or_else(|| fits(short));
            return text.map(|t| (t, th.edit_pending.bg(bg)));
        }
        if pending > 0 {
            let text = self.i18n.msg(&Msg::EditBadgePending { count: pending as u64 }).to_string();
            return fits(text).map(|t| (t, th.edit_pending.bg(bg)));
        }
        if let Some(a) = &rs.edit.applied {
            let label = match (a.tx, short) {
                (Some(_), false) => Label::EditBadgeNotCommitted,
                (Some(_), true) => Label::EditBadgeNotCommittedShort,
                (None, false) => Label::EditBadgeApplied,
                (None, true) => Label::EditBadgeAppliedShort,
            };
            return fits(self.i18n.label(label).to_string()).map(|t| (t, th.edit_applied.bg(bg)));
        }
        // The fullest text that fits; else `what` cut to the room left, at least a few columns
        // of it; else `plain`.
        let pick = |full: Vec<String>, what: &str, cut: &dyn Fn(&str) -> String, plain: Label| {
            full.into_iter()
                .find(|t| width(t) <= room)
                .or_else(|| {
                    let left = room.checked_sub(width(&cut("")))?;
                    (left >= 4).then(|| cut(&crate::text::clip(what, left)))
                })
                .or_else(|| fits(self.i18n.label(plain).to_string()))
        };
        match &rs.edit.target {
            Target::Ready(t) => {
                let table = crate::text::escape_controls(&format!("{}.{}", t.table.schema, t.table.name));
                let editable =
                    |table: &str| self.i18n.msg(&Msg::EditBadgeEditable { table: table.to_string() }).to_string();
                let found = match &t.identity {
                    Identity::Key { columns, .. } => {
                        let names: Vec<String> = (columns.iter().filter_map(|&c| rs.columns.get(c)))
                            .map(|c| crate::text::escape_controls(&c.meta.name))
                            .collect();
                        self.i18n.msg(&Msg::EditBadgeKey { columns: names.join(", ") }).to_string()
                    }
                    Identity::Values { .. } => self.i18n.label(Label::EditBadgeByValues).to_string(),
                };
                let full = vec![format!("{} · {found}", editable(&table)), editable(&table)];
                let text = pick(full, &table, &editable, Label::EditBadgeEditablePlain)?;
                Some((text, Style::new().fg(th.fg_muted).bg(bg)))
            }
            Target::ReadOnly { why, .. } => {
                let reason = self.why_text(why);
                let read_only =
                    |reason: &str| self.i18n.msg(&Msg::EditBadgeReadOnly { reason: reason.to_string() }).to_string();
                let text = pick(vec![read_only(&reason)], &reason, &read_only, Label::EditBadgeReadOnlyPlain)?;
                Some((text, Style::new().fg(th.fg_dim).bg(bg)))
            }
            Target::Unresolved | Target::Loading(_) => None,
        }
    }

    /// The result is read-only now, for `why`.
    fn say_edit_read_only(&mut self, why: Label) {
        let n = self.read_only_notice(why);
        self.flash(n);
    }

    fn read_only_notice(&self, why: Label) -> Notice {
        Notice::new(Msg::EditReadOnly { reason: self.i18n.label(why).to_string() }, Level::Info)
    }

    /// This change cannot be made, for `why`.
    fn say_refused(&mut self, why: Msg) {
        let n = self.refused_notice(why);
        self.flash(n);
    }

    fn refused_notice(&self, why: Msg) -> Notice {
        Notice::new(Msg::EditRefused { reason: self.i18n.msg(&why).to_string() }, Level::Info)
    }
}

/// Why `a` cannot set column `c` (named `column`) of a row read, whatever its rows are.
fn column_refusal(a: EditAction, c: &TableColumn, column: String) -> Option<Msg> {
    match a {
        EditAction::SetNull if c.column.not_null => Some(Msg::EditWhyNotNull { column }),
        EditAction::SetDefault if !has_default(c) => Some(Msg::EditWhyNoDefault { column }),
        EditAction::SetDefault if new_number(c) => Some(Msg::EditWhyNewNumber { column }),
        _ => None,
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
