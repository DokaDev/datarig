//! Copying results: `y` copies the cell, `Y` the
//! row, `v` starts a rectangular selection that `y` copies as TSV. A copy
//! takes a scope (the selection, or every fetched row: [`CopyScope`]) and a format
//! ([`CopyFormat::MENU`]: TSV without or with headers, a comma list, CSV, JSON, indented JSON,
//! Markdown, HTML, XML, an SQL IN list, SQL INSERT or UPDATE statements; `core::export`), from
//! the grid's menu, `Space r y`/`Space r a` or `:copy <format> [selection|all]`. The text goes to the clipboard the `clipboard` setting picks
//! ([`crate::clipboard`]) and the status bar says how.
//!
//! The editor's registers reach the clipboard here too ([`App::editor_yanked`]): yanks and
//! deletes that name no register when `[editor] clipboard` is on, and `"+` / `"*` always; `"+p`
//! reads it ([`App::editor_clipboard_text`]), only then.
//!
//! Only rows already fetched are copied; when the server has more, the notice says so. More
//! than [`LARGE_COPY`] rows ask first. The rows are written a chunk at a time
//! ([`export::Writer`]), read from the result's spill file when they are not in memory, so a
//! large copy never holds every row twice.

use super::*;
use crate::clipboard::{self, Method, Plan};
use crate::widgets::editor::Yank;
use crate::widgets::grid::Shape;
use datarig_core::config::{ClipboardSetting, CopyHeader, EditorClipboard};
use datarig_core::driver::keys::{InsertPlan, NotInsertable, NotUpdatable, insert_into, insert_source, update_source};
use datarig_core::export::{self, Kind, Target};
use datarig_core::fault::{ErrorLog, Fault};
use datarig_core::sql::dialect::Dialect;
use std::ops::Range;

/// The reasons a register write did not reach the clipboard that were said already: each is
/// said once a session (else every `x` would say it again).
#[derive(Debug, Default)]
pub(super) struct YankNotices {
    too_long: bool,
    failed: bool,
}

/// Copies of more rows than this ask first.
pub const LARGE_COPY: usize = 10_000;

/// Rows read and written at a time.
const COPY_CHUNK: usize = 4_096;

/// The formats of a copy. [`CopyFormat::Tsv`] is `y`/`Y`'s TSV (its header row follows
/// `copy_header`); the menu, `Space r` and `:copy` offer [`CopyFormat::MENU`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyFormat {
    Tsv,
    TsvPlain,
    TsvHeader,
    List,
    Csv,
    Json,
    JsonPretty,
    Markdown,
    Html,
    Xml,
    SqlIn,
    SqlInsert,
    SqlUpdate,
}

impl CopyFormat {
    /// The formats a user picks, in menu order.
    pub const MENU: [CopyFormat; 12] = [
        CopyFormat::TsvPlain,
        CopyFormat::TsvHeader,
        CopyFormat::List,
        CopyFormat::Csv,
        CopyFormat::Json,
        CopyFormat::JsonPretty,
        CopyFormat::Markdown,
        CopyFormat::Html,
        CopyFormat::Xml,
        CopyFormat::SqlIn,
        CopyFormat::SqlInsert,
        CopyFormat::SqlUpdate,
    ];

    /// Its name as `:copy` takes it (and in action ids).
    pub fn name(self) -> &'static str {
        match self {
            CopyFormat::Tsv | CopyFormat::TsvPlain => "tsv",
            CopyFormat::TsvHeader => "tsv_header",
            CopyFormat::List => "list",
            CopyFormat::Csv => "csv",
            CopyFormat::Json => "json",
            CopyFormat::JsonPretty => "json_pretty",
            CopyFormat::Markdown => "markdown",
            CopyFormat::Html => "html",
            CopyFormat::Xml => "xml",
            CopyFormat::SqlIn => "in",
            CopyFormat::SqlInsert => "insert",
            CopyFormat::SqlUpdate => "update",
        }
    }

    /// The format `:copy` names `name` (`sql` is `insert`, the older name).
    pub fn parse(name: &str) -> Option<CopyFormat> {
        let name = name.to_ascii_lowercase().replace('-', "_");
        if name == "sql" {
            return Some(CopyFormat::SqlInsert);
        }
        CopyFormat::MENU.into_iter().find(|f| f.name() == name)
    }

    /// Its key after `Space r y` / `Space r a` (and in the menu).
    pub fn key(self) -> char {
        match self {
            CopyFormat::Tsv | CopyFormat::TsvPlain => 't',
            CopyFormat::TsvHeader => 'T',
            CopyFormat::List => 'l',
            CopyFormat::Csv => 'c',
            CopyFormat::Json => 'j',
            CopyFormat::JsonPretty => 'J',
            CopyFormat::Markdown => 'm',
            CopyFormat::Html => 'h',
            CopyFormat::Xml => 'x',
            CopyFormat::SqlIn => 'n',
            CopyFormat::SqlInsert => 'i',
            CopyFormat::SqlUpdate => 'u',
        }
    }

    /// Its name in messages ("Copied 3 rows as …").
    pub fn label(self) -> Label {
        match self {
            CopyFormat::Tsv | CopyFormat::TsvPlain => Label::CopyFormatTsv,
            CopyFormat::TsvHeader => Label::CopyFormatTsvHeader,
            CopyFormat::List => Label::CopyFormatList,
            CopyFormat::Csv => Label::CopyFormatCsv,
            CopyFormat::Json => Label::CopyFormatJson,
            CopyFormat::JsonPretty => Label::CopyFormatJsonPretty,
            CopyFormat::Markdown => Label::CopyFormatMarkdown,
            CopyFormat::Html => Label::CopyFormatHtml,
            CopyFormat::Xml => Label::CopyFormatXml,
            CopyFormat::SqlIn => Label::CopyFormatIn,
            CopyFormat::SqlInsert => Label::CopyFormatSql,
            CopyFormat::SqlUpdate => Label::CopyFormatUpdate,
        }
    }

    /// Its name in the menu.
    pub fn menu_label(self) -> Label {
        match self {
            CopyFormat::Tsv | CopyFormat::TsvPlain => Label::CopyMenuTsv,
            CopyFormat::TsvHeader => Label::CopyMenuTsvHeader,
            CopyFormat::List => Label::CopyMenuList,
            CopyFormat::Csv => Label::CopyMenuCsv,
            CopyFormat::Json => Label::CopyMenuJson,
            CopyFormat::JsonPretty => Label::CopyMenuJsonPretty,
            CopyFormat::Markdown => Label::CopyMenuMarkdown,
            CopyFormat::Html => Label::CopyMenuHtml,
            CopyFormat::Xml => Label::CopyMenuXml,
            CopyFormat::SqlIn => Label::CopyMenuIn,
            CopyFormat::SqlInsert => Label::CopyMenuInsert,
            CopyFormat::SqlUpdate => Label::CopyMenuUpdate,
        }
    }
}

/// What a copy of the menu, `Space r` or `:copy` takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyScope {
    /// The selected range, else the cell under the cursor.
    Selection,
    /// Every row fetched so far, every column.
    Fetched,
}

impl CopyScope {
    pub const ALL: [CopyScope; 2] = [CopyScope::Selection, CopyScope::Fetched];

    /// Its name as `:copy <format> <scope>` takes it (and in action ids).
    pub fn name(self) -> &'static str {
        match self {
            CopyScope::Selection => "selection",
            CopyScope::Fetched => "all",
        }
    }

    fn what(self) -> CopyWhat {
        match self {
            CopyScope::Selection => CopyWhat::Cell,
            CopyScope::Fetched => CopyWhat::Every,
        }
    }
}

/// What a copy takes from the result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyWhat {
    /// The selected range, else the cell under the cursor.
    Cell,
    /// The rows of the selected range, else the cursor's row; every column.
    Row,
    /// The selected range, else every fetched row (`:copy <format>` without a scope).
    All,
    /// Every fetched row, every column, whatever is selected.
    Every,
}

/// A copy waiting for "yes" (a large one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyRequest {
    pub what: CopyWhat,
    pub format: CopyFormat,
    /// `:copy insert <table>`: the table the user named for the SQL INSERT statements.
    pub into: Option<String>,
}

/// What a copy waiting for an answer or for rows belongs to: its tab, the tab's connection
/// and binding, its session and its result. It goes ahead only while all of them are the
/// same (a repro: one left from before a connection switch took the next
/// result), like a statement queued for a connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Intent {
    pub tab: TabId,
    profile: Option<ProfileId>,
    binding: u64,
    generation: u64,
    query: u64,
}

impl Intent {
    fn of(t: &Tab) -> Self {
        Self {
            tab: t.id,
            profile: t.profile,
            binding: t.binding,
            generation: t.exec.generation,
            query: t.exec.query_id,
        }
    }

    /// The tab it belongs to still shows the same result on the same session.
    fn holds(&self, app: &App) -> bool {
        app.tabs.get(self.tab).is_some_and(|t| Self::of(t) == *self)
    }
}

/// The rows and columns a copy takes, and whether it is one cell without a selection.
struct Block {
    rows: Range<usize>,
    cols: Vec<usize>,
    cell: bool,
    /// Every fetched row, and the server has more.
    partial: bool,
}

/// The rectangle between the selection's anchor and the cursor.
pub fn range(anchor: (usize, usize), cursor: (usize, usize)) -> (Range<usize>, Range<usize>) {
    let rows = anchor.0.min(cursor.0)..anchor.0.max(cursor.0) + 1;
    let cols = anchor.1.min(cursor.1)..anchor.1.max(cursor.1) + 1;
    (rows, cols)
}

impl App {
    /// `v` in the grid: start a range at the cursor, or drop the one there is; `V` (`rows`):
    /// the same with whole rows (vim's linewise Visual). Either one on a
    /// selection of the other shape turns it into its own shape, keeping its anchor.
    pub(super) fn toggle_selection(&mut self, rows: bool) {
        let t = self.tabs.active_mut();
        if !matches!(t.results, Results::Rows(_)) {
            return;
        }
        let g = &mut t.grid;
        let shape = if rows { Shape::Rows } else { Shape::Cells };
        g.anchor = match g.anchor {
            Some(_) if g.shape == shape => None,
            Some(a) => Some(a),
            None => Some((g.row, g.col)),
        };
        g.shape = shape;
        if g.anchor.is_some() {
            let (copy, cancel) = (
                self.key_for(Action::Grid(GridAction::CopyCell), Ctx::Grid),
                self.key_for(Action::PaneBack, Ctx::Grid),
            );
            self.flash(Notice::new(Msg::CopySelecting { copy, cancel }, Level::Info));
        } else {
            self.selecting_done();
        }
    }

    /// The selection was dropped: its "Selecting…" notice goes with it.
    pub(super) fn selecting_done(&mut self) {
        if self.transient.as_ref().is_some_and(|(m, _)| matches!(m.msg, Msg::CopySelecting { .. })) {
            self.transient = None;
        }
    }

    fn block(&self, what: CopyWhat) -> Option<Block> {
        let t = self.tab();
        let Results::Rows(rs) = &t.results else { return None };
        if rs.rows.is_empty() || rs.columns.is_empty() {
            return None;
        }
        let g = &t.grid;
        let cursor = (g.row.min(rs.rows.len() - 1), g.col.min(rs.columns.len() - 1));
        let all_cols: Vec<usize> = (0..rs.columns.len()).collect();
        let more = rs.more || t.exec.paging.closed() || t.exec.paging == Paging::Stopped;
        let sel = g.anchor.map(|a| g.shape.span(a, cursor, rs.rows.len(), rs.columns.len()));
        // Whole columns are every fetched row: the notice says when the server has more.
        let whole_cols = g.shape == Shape::Cols && more;
        Some(match (what, sel) {
            (CopyWhat::Cell | CopyWhat::All, Some((rows, cols))) => {
                Block { rows, cols: cols.collect(), cell: false, partial: whole_cols }
            }
            (CopyWhat::Row, Some((rows, _))) => Block { rows, cols: all_cols, cell: false, partial: false },
            (CopyWhat::Cell, None) => {
                Block { rows: cursor.0..cursor.0 + 1, cols: vec![cursor.1], cell: true, partial: false }
            }
            (CopyWhat::Row, None) => {
                Block { rows: cursor.0..cursor.0 + 1, cols: all_cols, cell: false, partial: false }
            }
            (CopyWhat::All, None) | (CopyWhat::Every, _) => {
                Block { rows: 0..rs.rows.len(), cols: all_cols, cell: false, partial: more }
            }
        })
    }

    /// Copy `what` of the active tab's result as `format`. A copy of more than
    /// [`LARGE_COPY`] rows asks first (unless `confirmed`).
    pub(super) fn copy(&mut self, what: CopyWhat, format: CopyFormat, confirmed: bool) {
        self.copy_request(CopyRequest { what, format, into: None }, confirmed);
    }

    /// Copy `scope` as `format` (the menu, `Space r`, `:copy <format> <scope>`).
    pub(super) fn copy_scoped(&mut self, scope: CopyScope, format: CopyFormat) {
        self.copy(scope.what(), format, false);
    }

    /// `:copy insert <table>`: copy the result (or the selected range) as SQL INSERT statements
    /// for table `into`, which the user names.
    pub(super) fn copy_into(&mut self, into: &str) {
        let r = CopyRequest { what: CopyWhat::All, format: CopyFormat::SqlInsert, into: Some(into.to_string()) };
        self.copy_request(r, false);
    }

    fn copy_request(&mut self, req: CopyRequest, confirmed: bool) {
        let format = req.format;
        let Some(b) = self.block(req.what) else {
            return self.flash(Notice::new(Label::CopyNothing, Level::Warning));
        };
        let count = b.rows.len();
        if count > LARGE_COPY && !confirmed {
            self.pending_copy = Some((Intent::of(self.tab()), req));
            self.confirm(Label::CopyConfirmTitle, Label::CopyConfirmTitle, Label::CopyConfirmKeys, ConfirmAction::Copy);
            if let Some(c) = self.overlays.confirm_mut() {
                c.text = Msg::CopyConfirmText { count: count as u64 };
            }
            return;
        }
        // Generated columns left out of an SQL INSERT copy.
        let mut skipped = String::new();
        // `:copy insert` of rows that are not a projection of that table: said after the copy.
        let mut as_is = None;
        let text = {
            let t = self.tab();
            let Results::Rows(rs) = &t.results else { return };
            // The SQL formats are written in the tab's dialect.
            let d = self.tab_dialect(t.id);
            let columns: Vec<export::Column> = b
                .cols
                .iter()
                .map(|&c| {
                    let m = &rs.columns[c].meta;
                    export::Column { name: &m.name, kind: Kind::of_column(m) }
                })
                .collect();
            let header = self.prefs.copy_header.applies(count);
            // The chosen columns of the rows of `b`, `COPY_CHUNK` at a time, into `write`.
            let stream = |write: &mut dyn FnMut(&[export::Row])| {
                rs.rows.for_each_chunk(b.rows.clone(), COPY_CHUNK, |chunk| {
                    let rows: Vec<export::Row> = chunk
                        .iter()
                        .map(|r| b.cols.iter().map(|&c| r.get(c).and_then(|v| v.as_deref())).collect())
                        .collect();
                    write(&rows);
                })
            };
            let format = match format {
                CopyFormat::Tsv if b.cell && !header => None,
                CopyFormat::TsvPlain if b.cell => None,
                CopyFormat::Tsv => Some(export::Format::Tsv { header }),
                CopyFormat::TsvPlain => Some(export::Format::Tsv { header: false }),
                CopyFormat::TsvHeader => Some(export::Format::Tsv { header: true }),
                // CSV is a table: its header row unless `copy_header = off`.
                CopyFormat::Csv => Some(export::Format::Csv { header: self.prefs.copy_header != CopyHeader::Off }),
                CopyFormat::Json => Some(export::Format::Json),
                CopyFormat::JsonPretty => Some(export::Format::JsonPretty),
                CopyFormat::Markdown => Some(export::Format::Markdown),
                CopyFormat::List => Some(export::Format::List),
                CopyFormat::Html => Some(export::Format::Html),
                CopyFormat::Xml => Some(export::Format::Xml),
                CopyFormat::SqlIn | CopyFormat::SqlInsert | CopyFormat::SqlUpdate => None,
            };
            let written: Result<String, Fault> = match (format, req.format) {
                (Some(f), _) => {
                    let mut w = export::Writer::new(f, &columns);
                    stream(&mut |rows| w.rows(rows)).map(|()| w.finish())
                }
                (None, CopyFormat::Tsv | CopyFormat::TsvPlain) => {
                    let mut cell = String::new();
                    stream(&mut |rows| {
                        cell = rows.first().and_then(|r| r[0]).unwrap_or_default().to_string();
                    })
                    .map(|()| cell)
                }
                (None, CopyFormat::SqlIn) => {
                    // One column's values, each once: collected first (they are deduplicated).
                    if columns.len() != 1 {
                        return self.flash(Notice::new(Label::CopyInSeveralColumns, Level::Warning));
                    }
                    let mut values: Vec<Option<String>> = Vec::new();
                    let read = stream(&mut |rows| values.extend(rows.iter().map(|r| r[0].map(str::to_string))));
                    let rows: Vec<export::Row> = values.iter().map(|v| vec![v.as_deref()]).collect();
                    match (read, export::sql_in(d, &columns, &rows)) {
                        (Err(fault), _) => Err(fault),
                        (Ok(()), Ok(text)) => Ok(text),
                        (Ok(()), Err(export::NotInList::SeveralColumns)) => {
                            return self.flash(Notice::new(Label::CopyInSeveralColumns, Level::Warning));
                        }
                        (Ok(()), Err(export::NotInList::NoValues)) => {
                            return self.flash(Notice::new(Label::CopyInNoValues, Level::Warning));
                        }
                    }
                }
                (None, CopyFormat::SqlUpdate) => {
                    let origins: Vec<_> = b.cols.iter().map(|&c| rs.columns[c].meta.origin.clone()).collect();
                    let names: Vec<&str> = columns.iter().map(|c| c.name).collect();
                    let keys = self.tab_keys(t).catalog();
                    let plan = match update_source(d, keys, &origins, &names, t.shown_sql()) {
                        Ok(p) => p,
                        Err(why) => {
                            let reason = self.why_not_update(&why);
                            return self.flash(Notice::new(Msg::CopyUpdateRefused { reason }, Level::Warning));
                        }
                    };
                    let target = export::UpdateTarget {
                        schema: &plan.table.schema,
                        name: &plan.table.name,
                        set: plan.set.clone(),
                        keys: plan.keys.clone(),
                    };
                    let mut w = export::Writer::new(export::Format::Update(d, target), &columns);
                    stream(&mut |rows| w.rows(rows)).map(|()| w.finish())
                }
                (None, _) => {
                    let origins: Vec<_> = b.cols.iter().map(|&c| rs.columns[c].meta.origin.clone()).collect();
                    let names: Vec<&str> = columns.iter().map(|c| c.name).collect();
                    let keys = self.tab_keys(t).catalog();
                    let source = insert_source(d, keys, &origins, &names, t.shown_sql());
                    let plan = match &req.into {
                        None => source.map_err(|why| {
                            Notice::new(Msg::CopyInsertRefused { reason: self.why(&why) }, Level::Warning)
                        }),
                        Some(into) => insert_into(d, keys, into, &names)
                            .inspect(|p| {
                                if !source.as_ref().is_ok_and(|s| std::ptr::eq(s.table, p.table)) {
                                    as_is = Some(qualified(d, p.table));
                                }
                            })
                            .map_err(|why| Notice::new(self.copy_into_failed(into, &why), Level::Warning)),
                    };
                    let InsertPlan { table, columns: written, skipped: s, overriding } = match plan {
                        Ok(p) => p,
                        Err(refused) => return self.flash(refused),
                    };
                    skipped = s.join(", ");
                    let target = Target::Table {
                        schema: &table.schema,
                        name: &table.name,
                        columns: written.iter().map(|w| w.1).collect(),
                        overriding,
                    };
                    let columns: Vec<_> = written.iter().map(|w| columns[w.0]).collect();
                    let mut w = export::Writer::new(export::Format::Sql(d, target), &columns);
                    stream(&mut |rows| {
                        let rows: Vec<export::Row> =
                            rows.iter().map(|r| written.iter().map(|w| r[w.0]).collect()).collect();
                        w.rows(&rows);
                    })
                    .map(|()| w.finish())
                }
            };
            match written {
                Ok(text) => text,
                Err(fault) => {
                    let error = self.fault_text("copy.read_failed", &fault);
                    let failed = Notice::new(Msg::CopyFailed { error }, Level::Error);
                    self.show_status(failed.clone());
                    return self.flash(failed);
                }
            }
        };
        let method = match self.deliver(&text) {
            Ok(m) => m,
            Err(msg) => {
                // Said at once, and in the status bar instead of an older copy's success.
                let failed = Notice::new(msg, Level::Error);
                self.show_status(failed.clone());
                return self.flash(failed);
            }
        };
        let method = self.method_text(method);
        let format_name = self.i18n.label(format.label()).to_string();
        let done = if b.cell && matches!(format, CopyFormat::Tsv | CopyFormat::TsvPlain) {
            Msg::CopyDoneCell { method }
        } else if b.partial {
            Msg::CopyDoneFetched { count: count as u64, format: format_name, method }
        } else {
            Msg::CopyDoneRows { count: count as u64, format: format_name, method }
        };
        self.tab_mut().grid.anchor = None;
        // Said at once (over an older flash), and kept in the status bar.
        let done = Notice::new(done, Level::Success);
        self.show_status(done.clone());
        self.flash(done);
        if !skipped.is_empty() {
            self.flash(Notice::new(Msg::CopyGeneratedSkipped { columns: skipped }, Level::Info));
        }
        if let Some(table) = as_is {
            let warning = Notice::new(Msg::CopyIntoAsIs { table }, Level::Warning);
            self.show_status(warning.clone());
            self.flash(warning);
        }
    }

    /// Whether the rest of the active tab's rows can be fetched: its portal is open, the
    /// server has more and nothing runs.
    pub(super) fn can_fetch_rest(&self) -> bool {
        let t = self.tab();
        matches!(&t.results, Results::Rows(rs) if rs.more)
            && matches!(t.exec.paging, Paging::Open { .. })
            && t.exec.session.is_some()
            && t.exec.running.is_none()
    }

    /// Why the rest of the active tab's rows cannot be fetched: nothing was held after the
    /// first page (`paging = "no_hold"`), or the portal closed.
    fn fetch_unavailable(&self) -> Label {
        if self.tab().exec.paging == Paging::Released { Label::CopyFetchNoHold } else { Label::CopyFetchUnavailable }
    }

    /// "Fetch every row, then copy as `format`": ask first, with the rows fetched so far.
    pub(super) fn ask_fetch_then_copy(&mut self, format: CopyFormat) {
        if !self.can_fetch_rest() {
            return self.flash(Notice::new(self.fetch_unavailable(), Level::Warning));
        }
        let count = match &self.tab().results {
            Results::Rows(rs) => rs.rows.len() as u64,
            _ => 0,
        };
        self.pending_fetch_copy = Some((Intent::of(self.tab()), format));
        self.confirm(Label::CopyFetchTitle, Label::CopyFetchTitle, Label::CopyFetchKeys, ConfirmAction::FetchThenCopy);
        let format = self.i18n.label(format.label()).to_string();
        if let Some(c) = self.overlays.confirm_mut() {
            c.text = Msg::CopyFetchText { count, format };
        }
    }

    /// Fetching the rest was confirmed: fetch page after page; the copy runs when the result
    /// is complete, or fetching stopped at the spill limit (then of the rows fetched, and it
    /// says so). The large-copy question still comes when there are many rows.
    pub(super) fn fetch_then_copy(&mut self) {
        let Some((intent, format)) = self.pending_fetch_copy.take() else { return };
        if !intent.holds(self) || self.tab().id != intent.tab {
            return self.tab_status(intent.tab, Notice::new(Label::CopyStale, Level::Warning));
        }
        if !self.can_fetch_rest() {
            return self.flash(Notice::new(self.fetch_unavailable(), Level::Warning));
        }
        self.fetch_copy = Some((intent, CopyRequest { what: CopyWhat::Every, format, into: None }));
        self.fetch_page(intent.tab);
    }

    /// A page of tab `id`'s rows arrived (or fetching ended): the next page for a copy that
    /// waits for every row, or the copy once there is none. `true` when such a copy waits.
    pub(super) fn fetch_copy_page(&mut self, id: TabId) -> bool {
        let Some((intent, _)) = self.fetch_copy.as_ref().filter(|(i, _)| i.tab == id) else { return false };
        // Not the result (or the session, or the connection) it was asked for: dropped.
        if !intent.holds(self) {
            self.fetch_copy = None;
            self.tab_status(id, Notice::new(Label::CopyStale, Level::Warning));
            return false;
        }
        let Some(t) = self.tabs.get(id) else { return false };
        let more = matches!(&t.results, Results::Rows(rs) if rs.more) && matches!(t.exec.paging, Paging::Open { .. });
        if more {
            self.fetch_page(id);
            return true;
        }
        let Some((_, req)) = self.fetch_copy.take() else { return false };
        if self.tab().id == id {
            self.copy_request(req, false);
        } else {
            self.tab_status(id, Notice::new(Label::CopyFetchFailed, Level::Warning));
        }
        true
    }

    /// Fetching for a copy failed or was cancelled: nothing is copied.
    pub(super) fn fetch_copy_failed(&mut self, id: TabId) {
        if self.fetch_copy.as_ref().is_some_and(|(i, _)| i.tab == id) {
            self.fetch_copy = None;
            self.tab_status(id, Notice::new(Label::CopyFetchFailed, Level::Warning));
        }
    }

    /// The large copy was confirmed.
    pub(super) fn copy_confirmed(&mut self) {
        if let Some((intent, r)) = self.pending_copy.take() {
            if !intent.holds(self) || self.tab().id != intent.tab {
                return self.tab_status(intent.tab, Notice::new(Label::CopyStale, Level::Warning));
            }
            self.copy_request(r, true);
        }
    }

    /// Stop the copy that waits for tab `id`'s rows (the user cancelled): nothing more is
    /// fetched for it and nothing is copied. Whether one waited.
    pub(super) fn stop_fetch_copy(&mut self, id: TabId) -> bool {
        if self.fetch_copy.as_ref().is_some_and(|(i, _)| i.tab == id) {
            self.fetch_copy = None;
            return true;
        }
        false
    }

    /// How the status bar names `m`.
    pub(super) fn method_text(&self, m: Method) -> String {
        let tmux = (self.env)("TMUX").is_some_and(|v| !v.is_empty());
        let label = match m {
            Method::System => Label::CopyMethodSystem,
            Method::Osc52 if tmux => Label::CopyMethodOsc52Tmux,
            Method::Osc52 => Label::CopyMethodOsc52,
        };
        self.i18n.label(label).to_string()
    }

    /// Put `text` on the clipboard the `clipboard` setting picks; why not, as a message.
    ///
    /// A copy longer than `osc52_max_bytes` of base64 never goes through OSC 52 (the terminal
    /// would drop it silently): `auto` tries the system clipboard instead, also over SSH.
    pub(super) fn deliver(&mut self, text: &str) -> Result<Method, Msg> {
        let ssh = clipboard::in_ssh(|k| (self.env)(k));
        let plan = clipboard::plan(self.prefs.clipboard, ssh);
        let (len, max) = (clipboard::osc52_len(text), self.prefs.osc52_max_bytes);
        let fits = len <= max;
        if plan != Plan::Osc52 || (!fits && self.prefs.clipboard == ClipboardSetting::Auto) {
            match self.system_copy(text) {
                Ok(()) => return Ok(Method::System),
                Err(e) if plan == Plan::System => {
                    ErrorLog::new(self.paths.errors_log()).record("copy.system", &Fault::other(e));
                    return Err(Msg::CopyFailed { error: self.i18n.label(Label::CopySystemUnavailable).to_string() });
                }
                Err(e) => {
                    ErrorLog::new(self.paths.errors_log()).record("copy.system", &Fault::other(e));
                }
            }
        }
        if !fits {
            return Err(Msg::CopyTooLongForOsc52 { size: kilobytes(len), limit: kilobytes(max) });
        }
        self.terminal_out.push(clipboard::osc52(text));
        Ok(Method::Osc52)
    }

    /// A register write of the editor: to the system clipboard the `clipboard` setting picks,
    /// when `[editor] clipboard` is on or the write names `"+` / `"*`. One the clipboard cannot
    /// take stays in the register, and the notice says why (each reason once a session); the
    /// edit is done either way.
    pub(super) fn editor_yanked(&mut self, y: Yank) {
        if y.register == '"' && self.prefs.editor_clipboard == EditorClipboard::Off {
            return;
        }
        let msg = match self.deliver(&y.reg.clipboard_text()) {
            Ok(_) => return,
            Err(Msg::CopyTooLongForOsc52 { size, limit }) => {
                if std::mem::replace(&mut self.yank_notices.too_long, true) {
                    return;
                }
                Msg::EditorClipboardTooLong { size, limit }
            }
            Err(Msg::CopyFailed { error }) => {
                if std::mem::replace(&mut self.yank_notices.failed, true) {
                    return;
                }
                Msg::EditorClipboardFailed { error }
            }
            Err(other) => other,
        };
        self.flash(Notice::new(msg, Level::Warning));
    }

    /// The system clipboard's text for `"+p` / `"*p` / `Ctrl+R +`, read now; `None` with a
    /// notice when it cannot be: with the `osc52` plan (over SSH) it is never read (the
    /// terminal's paste is the way), and a failed read says why.
    pub(super) fn editor_clipboard_text(&mut self) -> Option<String> {
        let ssh = clipboard::in_ssh(|k| (self.env)(k));
        if clipboard::plan(self.prefs.clipboard, ssh) == Plan::Osc52 {
            self.flash(Notice::new(Label::EditorClipboardNoRead, Level::Warning));
            return None;
        }
        match self.system_paste() {
            Ok(text) => Some(text),
            Err(e) => {
                ErrorLog::new(self.paths.errors_log()).record("paste.system", &Fault::other(e.clone()));
                self.flash(Notice::new(Msg::EditorClipboardReadFailed { error: e }, Level::Warning));
                None
            }
        }
    }

    /// The system clipboard's text, opening it on first use (a failure drops it, so the next
    /// use opens it again).
    fn system_paste(&mut self) -> Result<String, String> {
        if self.clipboard.is_none() {
            self.clipboard = Some((self.clipboard_opener)()?);
        }
        let result = self.clipboard.as_mut().map_or_else(|| Err("no system clipboard".to_string()), |c| c.get_text());
        if result.is_err() {
            self.clipboard = None;
        }
        result
    }

    /// Put `text` on the system clipboard, opening it on first use (a failure drops it, so the
    /// next copy opens it again).
    fn system_copy(&mut self, text: &str) -> Result<(), String> {
        if self.clipboard.is_none() {
            self.clipboard = Some((self.clipboard_opener)()?);
        }
        let result = self.clipboard.as_mut().map_or(Ok(()), |c| c.set_text(text));
        if result.is_err() {
            self.clipboard = None;
        }
        result
    }

    /// Text for the terminal that the binary writes out before the next frame (OSC 52).
    pub fn take_terminal_output(&mut self) -> Vec<String> {
        std::mem::take(&mut self.terminal_out)
    }

    /// Use `open` to reach the system clipboard (the binary passes the OS one; without this
    /// call there is none, so tests and headless runs never touch it).
    pub fn set_clipboard(&mut self, open: clipboard::Opener) {
        self.clipboard_opener = open;
        self.clipboard = None;
    }
}

/// `n` bytes in kilobytes (1000 bytes), rounded up: `100 KB`.
fn kilobytes(n: usize) -> String {
    format!("{} KB", n.div_ceil(1000))
}

/// `schema.table` as dialect `d` writes it, each name quoted, for messages.
fn qualified(d: Dialect, t: &datarig_core::driver::keys::TableKeys) -> String {
    format!("{}.{}", d.force_quote_ident(&t.schema), d.force_quote_ident(&t.name))
}

impl App {
    /// Why an automatic copy as SQL INSERT was refused, in words.
    fn why(&self, why: &NotInsertable) -> String {
        use NotInsertable::*;
        // The key cache could not be read: say why (an old server, a permission).
        let failed = Some(self.tab_keys(self.tab())).and_then(|k| match k {
            Keys::Failed(error) => Some(error.clone()),
            _ => None,
        });
        if let (NoCatalog, Some(error)) = (why, failed) {
            return self.i18n.msg(&Msg::CopyInsertWhyKeysFailed { error }).to_string();
        }
        let label = match why {
            NotSelect => Label::CopyInsertWhyNotSelect,
            With => Label::CopyInsertWhyWith,
            SeveralTables => Label::CopyInsertWhySeveralTables,
            Subquery => Label::CopyInsertWhySubquery,
            SetOperation => Label::CopyInsertWhySetOperation,
            Grouping => Label::CopyInsertWhyGrouping,
            Window => Label::CopyInsertWhyWindow,
            NotATable => Label::CopyInsertWhyNotATable,
            Computed => Label::CopyInsertWhyComputed,
            OtherTable => Label::CopyInsertWhyOtherTable,
            RepeatedColumn => Label::CopyInsertWhyRepeatedColumn,
            View => Label::CopyInsertWhyView,
            NoCatalog => Label::CopyInsertWhyNoCatalog,
            DuplicateNames => Label::CopyInsertWhyDuplicateNames,
            OnlyGenerated => Label::CopyInsertWhyOnlyGenerated,
            UnknownTable => Label::CopyInsertWhyUnknownTable,
            // Only from `:copy insert`, which says them itself ([`App::copy_into_failed`]).
            NoSuchTable | AmbiguousTable => Label::CopyInsertWhyUnknownTable,
            NoSuchColumn(_) => Label::CopyInsertWhyOtherTable,
            GeneratedColumn(_) => Label::CopyInsertWhyOnlyGenerated,
        };
        self.i18n.label(label).to_string()
    }

    /// Why a copy as SQL UPDATE was refused, in words.
    fn why_not_update(&self, why: &NotUpdatable) -> String {
        match why {
            NotUpdatable::NotInsertable(n) => self.why(n),
            NotUpdatable::NoPrimaryKey => self.i18n.label(Label::CopyUpdateWhyNoPrimaryKey).to_string(),
            NotUpdatable::MissingKey(cols) => {
                self.i18n.msg(&Msg::CopyUpdateWhyMissingKey { columns: cols.join(", ") }).to_string()
            }
            NotUpdatable::NothingToSet => self.i18n.label(Label::CopyUpdateWhyNothingToSet).to_string(),
        }
    }

    /// Why `:copy insert <into>` was refused, as a message.
    fn copy_into_failed(&self, into: &str, why: &NotInsertable) -> Msg {
        let table = into.to_string();
        match why {
            NotInsertable::NoSuchTable | NotInsertable::UnknownTable => Msg::CopyIntoNoTable { table },
            NotInsertable::AmbiguousTable => Msg::CopyIntoAmbiguous { table },
            NotInsertable::NoSuchColumn(column) => Msg::CopyIntoNoColumn { table, column: column.clone() },
            NotInsertable::GeneratedColumn(column) => Msg::CopyIntoGenerated { table, column: column.clone() },
            other => Msg::CopyIntoFailed { reason: self.why(other) },
        }
    }
}

#[cfg(test)]
mod tests;
