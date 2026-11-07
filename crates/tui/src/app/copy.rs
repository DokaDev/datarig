//! Copying results: `y` copies the cell, `Y` the row, `v` starts a rectangular selection that
//! `y` copies as TSV, `Ctrl+A` selects every cell of the page. A copy takes a scope (the
//! selection, or every fetched row: [`CopyScope`]) and a format ([`CopyFormat::MENU`]: TSV
//! without or with headers, a comma list, CSV, JSON, indented JSON, Markdown, HTML, XML, an SQL
//! IN list, SQL INSERT or UPDATE statements; `core::export`), from the grid's menu,
//! `Space r y`/`Space r a` or `:copy <format> [selection|all]`. The text goes to the clipboard
//! the `clipboard` setting picks ([`crate::clipboard`]) and the status bar says how.
//!
//! The editor's registers reach the clipboard here too ([`App::editor_yanked`]): yanks and
//! deletes that name no register when `[editor] clipboard` is on, and `"+` / `"*` always; `"+p`
//! reads it ([`App::editor_clipboard_text`]), only then.
//!
//! `y` and `Y` copy a block in the format `copy_format` names (TSV unless set; one cell without
//! a selection is its value): the menu's and `Space r y`'s formats mark it, and `Space r y d`
//! changes it ([`CopyFormat::of_setting`]).
//!
//! `y` and `Y` also keep the cells themselves ([`edit::GridCopy`]), which `g p` pastes: whatever
//! the clipboard does with the text, for the session.
//!
//! Only rows already fetched are copied; when the server has more, the notice says so. More
//! than [`LARGE_COPY`] rows ask first. The rows are written a chunk at a time
//! ([`export::Writer`]), read from the result's spill file when they are not in memory, so a
//! large copy never holds every row twice.

use super::*;
use crate::clipboard::{self, Method, Plan};
use crate::widgets::editor::Yank;
use crate::widgets::grid::{Over, Shape};
use datarig_core::config::{ClipboardSetting, CopyFormatSetting, CopyHeader, EditorClipboard};
use datarig_core::driver::keys::{
    Generated, InsertPlan, NotInsertable, NotUpdatable, insert_into, insert_source, update_source,
};
use datarig_core::edit::Value;
use datarig_core::edit::paste::MAX_ROWS as MAX_PASTE_ROWS;
use datarig_core::export::{self, Kind, Target};
use datarig_core::fault::{ErrorLog, Fault};
use datarig_core::i18n::fmt_count;
use datarig_core::sql::dialect::Dialect;
use std::ops::Range;
use std::sync::Arc;

/// The reasons a register write did not reach the clipboard that were said already: each is
/// said once a session (else every `x` would say it again).
#[derive(Debug, Default)]
pub(super) struct YankNotices {
    too_long: bool,
    failed: bool,
}

/// Copies of more rows than this ask first.
pub const LARGE_COPY: usize = 10_000;

/// The most bytes of values `y` and `Y` keep for `g p` (more are not kept, and said).
pub const GRID_COPY_MAX_BYTES: usize = 32 << 20;

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

    /// The format `y` and `Y` copy a block in under `copy_format` `s`.
    pub fn of_setting(s: CopyFormatSetting) -> CopyFormat {
        match s {
            CopyFormatSetting::Tsv => CopyFormat::Tsv,
            CopyFormatSetting::Csv => CopyFormat::Csv,
            CopyFormatSetting::Json => CopyFormat::Json,
            CopyFormatSetting::JsonPretty => CopyFormat::JsonPretty,
            CopyFormatSetting::Markdown => CopyFormat::Markdown,
            CopyFormatSetting::Html => CopyFormat::Html,
            CopyFormatSetting::Xml => CopyFormat::Xml,
            CopyFormatSetting::Insert => CopyFormat::SqlInsert,
        }
    }

    /// The `copy_format` that makes `y` and `Y` copy in this format (of the menu), if one does.
    /// TSV is `tsv` with or without headers: its header row follows `copy_header`.
    pub fn setting(self) -> Option<CopyFormatSetting> {
        Some(match self {
            CopyFormat::Tsv | CopyFormat::TsvPlain | CopyFormat::TsvHeader => CopyFormatSetting::Tsv,
            CopyFormat::Csv => CopyFormatSetting::Csv,
            CopyFormat::Json => CopyFormatSetting::Json,
            CopyFormat::JsonPretty => CopyFormatSetting::JsonPretty,
            CopyFormat::Markdown => CopyFormatSetting::Markdown,
            CopyFormat::Html => CopyFormatSetting::Html,
            CopyFormat::Xml => CopyFormatSetting::Xml,
            CopyFormat::SqlInsert => CopyFormatSetting::Insert,
            CopyFormat::List | CopyFormat::SqlIn | CopyFormat::SqlUpdate => return None,
        })
    }

    /// The menu's format that stands for `copy_format` `s` (marked as the default of `y` and
    /// `Y`): TSV with headers when `copy_header = on`, else TSV without.
    pub fn menu_default(s: CopyFormatSetting, header: CopyHeader) -> CopyFormat {
        match CopyFormat::of_setting(s) {
            CopyFormat::Tsv if header == CopyHeader::On => CopyFormat::TsvHeader,
            CopyFormat::Tsv => CopyFormat::TsvPlain,
            f => f,
        }
    }

    /// The formats `y` and `Y` may copy in (the choices of `Space r y d`), as the menu shows
    /// them.
    pub const DEFAULTS: [CopyFormat; 8] = [
        CopyFormat::Tsv,
        CopyFormat::Csv,
        CopyFormat::Json,
        CopyFormat::JsonPretty,
        CopyFormat::Markdown,
        CopyFormat::Html,
        CopyFormat::Xml,
        CopyFormat::SqlInsert,
    ];

    /// Its name as a choice of `copy_format` (the setting's own words).
    pub fn default_label(self) -> Label {
        match self.setting().unwrap_or_default() {
            CopyFormatSetting::Tsv => Label::SettingCopyFormatTsv,
            CopyFormatSetting::Csv => Label::SettingCopyFormatCsv,
            CopyFormatSetting::Json => Label::SettingCopyFormatJson,
            CopyFormatSetting::JsonPretty => Label::SettingCopyFormatJsonPretty,
            CopyFormatSetting::Markdown => Label::SettingCopyFormatMarkdown,
            CopyFormatSetting::Html => Label::SettingCopyFormatHtml,
            CopyFormatSetting::Xml => Label::SettingCopyFormatXml,
            CopyFormatSetting::Insert => Label::SettingCopyFormatInsert,
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
    /// `y` or `Y`: the cells are kept for `g p`, one cell without a selection is its value, and
    /// SQL INSERT statements for rows not certainly of one table are TSV instead (and said).
    pub keys: bool,
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
    pub(super) fn of(t: &Tab) -> Self {
        Self {
            tab: t.id,
            profile: t.profile,
            binding: t.binding,
            generation: t.exec.generation,
            query: t.exec.query_id,
        }
    }

    /// The tab it belongs to still shows the same result on the same session.
    pub(super) fn holds(&self, app: &App) -> bool {
        app.tabs.get(self.tab).is_some_and(|t| Self::of(t) == *self)
    }
}

/// The rows and columns a copy takes, and whether it is one cell without a selection.
pub(super) struct Block {
    pub(super) rows: Range<usize>,
    pub(super) cols: Vec<usize>,
    pub(super) cell: bool,
    /// Every fetched row, and the server has more.
    pub(super) partial: bool,
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
            self.say_selecting();
        } else {
            self.selecting_done();
        }
    }

    /// `Ctrl+A` in the grid: every cell of the page shown (its rows, every column) as a range
    /// of cells, which `y` copies and `v` or `Esc` drops like any other. The cursor goes to the
    /// page's first cell. An empty result has nothing to select.
    pub(super) fn select_all(&mut self) {
        let t = self.tabs.active_mut();
        let Results::Rows(rs) = &t.results else { return };
        let (total, ncols) = (rs.rows.len(), rs.columns.len());
        let g = &mut t.grid;
        // The rows read of the page: the new rows after them are not copied.
        let page = g.window(total);
        let page = page.start..page.end.min(total);
        if page.is_empty() || ncols == 0 {
            return;
        }
        g.shape = Shape::Cells;
        g.anchor = Some((page.end - 1, ncols - 1));
        (g.row, g.col) = (page.start, 0);
        g.detached = false;
        self.say_selecting();
    }

    /// A selection started: the status bar says which keys copy and drop it.
    fn say_selecting(&mut self) {
        let (copy, cancel) =
            (self.key_for(Action::Grid(GridAction::CopyCell), Ctx::Grid), self.key_for(Action::PaneBack, Ctx::Grid));
        self.flash(Notice::new(Msg::CopySelecting { copy, cancel }, Level::Info));
    }

    /// The selection was dropped: its "Selecting…" notice goes with it.
    pub(super) fn selecting_done(&mut self) {
        if self.transient.as_ref().is_some_and(|(m, _)| matches!(m.msg, Msg::CopySelecting { .. })) {
            self.transient = None;
        }
    }

    /// The cursor or the selection is on a new row (shown after the rows read): what it holds
    /// is not a row of the result, and is not copied.
    pub(super) fn on_new_rows(&self) -> bool {
        let t = self.tab();
        let Results::Rows(rs) = &t.results else { return false };
        let (g, len) = (&t.grid, rs.rows.len());
        let anchor = g.anchor.filter(|_| g.shape != Shape::Cols).is_some_and(|a| a.0 >= len);
        g.row >= len || anchor
    }

    pub(super) fn block(&self, what: CopyWhat) -> Option<Block> {
        let t = self.tab();
        let Results::Rows(rs) = &t.results else { return None };
        if rs.rows.is_empty() || rs.columns.is_empty() {
            return None;
        }
        let g = &t.grid;
        // What the cursor's row decides is never the row read before a new row (the cursor is
        // clamped below): nothing then.
        let by_cursor = match (what, g.anchor) {
            (CopyWhat::Every, _) | (CopyWhat::All, None) => false,
            (_, Some(_)) => g.shape != Shape::Cols,
            _ => true,
        };
        if by_cursor && self.on_new_rows() {
            return None;
        }
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
        self.copy_request(CopyRequest { what, format, into: None, keys: false }, confirmed);
    }

    /// The menu's format `y` and `Y` copy in (marked in the menu and after `Space r y`).
    pub(super) fn copy_default_item(&self) -> CopyFormat {
        CopyFormat::menu_default(self.prefs.copy_format, self.prefs.copy_header)
    }

    /// `label` of a format, marked as the one `y` and `Y` copy in.
    pub(super) fn copy_default_marked(&self, label: Localized) -> Localized {
        self.i18n.msg(&Msg::CopyMenuDefault { label: label.to_string() })
    }

    /// `y` (`Cell`) and `Y` (`Row`): copy in the format `copy_format` names.
    pub(super) fn copy_keys(&mut self, what: CopyWhat) {
        let format = CopyFormat::of_setting(self.prefs.copy_format);
        self.copy_request(CopyRequest { what, format, into: None, keys: true }, false);
    }

    /// Copy `scope` as `format` (the menu, `Space r`, `:copy <format> <scope>`).
    pub(super) fn copy_scoped(&mut self, scope: CopyScope, format: CopyFormat) {
        self.copy(scope.what(), format, false);
    }

    /// `:copy insert <table>`: copy the result (or the selected range) as SQL INSERT statements
    /// for table `into`, which the user names.
    pub(super) fn copy_into(&mut self, into: &str) {
        let r = CopyRequest {
            what: CopyWhat::All,
            format: CopyFormat::SqlInsert,
            into: Some(into.to_string()),
            keys: false,
        };
        self.copy_request(r, false);
    }

    fn copy_request(&mut self, mut req: CopyRequest, confirmed: bool) {
        // `y` and `Y` replace the cells `g p` pastes, with nothing when they keep none (refused,
        // asked about, not kept): `g p` never pastes an older copy in their place.
        if req.keys {
            self.grid_copy = None;
        }
        let Some(b) = self.block(req.what) else {
            let why = if self.on_new_rows() { Label::CopyNewRows } else { Label::CopyNothing };
            return self.flash(Notice::new(why, Level::Warning));
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
        // `y` on one cell without a selection copies its value, whatever `copy_format` says.
        if req.keys && b.cell {
            req.format = CopyFormat::Tsv;
        }
        // `y`/`Y` as SQL INSERT: TSV when the rows are not certainly of one table, and why.
        let mut fell_back = None;
        if req.keys
            && req.format == CopyFormat::SqlInsert
            && let Err(why) = self.insert_source_of(&b)
        {
            fell_back = Some(self.why(&why));
            req.format = CopyFormat::Tsv;
        }
        let format = req.format;
        // What the grid shows over the rows as read is copied: said when some is pending; a
        // value it shows that has none yet is not copied.
        let (pending, deleted) = {
            let Results::Rows(rs) = &self.tab().results else { return };
            match over_block(rs, &b, format == CopyFormat::SqlInsert) {
                Ok(p) => p,
                Err(why) => return self.flash(Notice::new(why, Level::Warning)),
            }
        };
        // `y` and `Y` keep the cells for `g p`, before the clipboard is tried; why not, said
        // after the copy.
        let mut not_kept = None;
        if req.keys {
            let kept = {
                let Results::Rows(rs) = &self.tab().results else { return };
                self.grid_copy_of(rs, &b)
            };
            match kept {
                Ok(c) => self.grid_copy = Some(Arc::new(c)),
                Err(why) => not_kept = why,
            }
        }
        // Generated columns left out of an SQL INSERT copy.
        let mut skipped = String::new();
        // `:copy insert` of rows that are not a projection of that table: said after the copy.
        let mut as_is = None;
        // The columns an SQL INSERT copy wrote (generated ones and those not of the table left
        // out): only they flash.
        let mut wrote_cols = None;
        // A value of an SQL copy that would not be written whole ([`export::sql_cells`]): the
        // copy is refused once the rows were read.
        let mut refused: Option<export::Refused> = None;
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
            // How the SQL formats write each value ([`export::sql_cells`]), or the column they refuse;
            // `insert`: an `AUTO_INCREMENT` value of 0 is refused (an `INSERT` takes it as the next
            // number).
            let keys = self.tab_keys(t).catalog();
            let sql_cells = |insert: bool| {
                let cols: Vec<export::SqlColumn> = b
                    .cols
                    .iter()
                    .map(|&c| {
                        let m = &rs.columns[c].meta;
                        let auto = keys.and_then(|k| m.origin.as_ref().and_then(|o| k.column(o)));
                        export::SqlColumn {
                            name: &m.name,
                            type_name: &m.type_name,
                            kind: m.kind,
                            auto_increment: insert && auto.is_some_and(|c| c.generated == Generated::AutoIncrement),
                        }
                    })
                    .collect();
                export::sql_cells(d, &cols, &rs.read_in, keys.and_then(|k| k.server_zone()))
            };
            // The chosen columns of the rows of `b` as the grid shows them, `COPY_CHUNK` at a
            // time, into `write` with the row the chunk starts at.
            let overlaid = !rs.edit.batch.is_empty() || rs.edit.applied.is_some();
            let stream = |write: &mut dyn FnMut(&[export::Row], usize)| {
                let mut at = b.rows.start;
                rs.rows.for_each_chunk(b.rows.clone(), COPY_CHUNK, |chunk| {
                    let rows: Vec<export::Row> = chunk
                        .iter()
                        .enumerate()
                        .map(|(i, r)| {
                            b.cols
                                .iter()
                                .map(|&c| match overlaid.then(|| rs.over(at + i, c)).flatten() {
                                    Some(o) => o.cell().flatten(),
                                    None => r.get(c).and_then(|v| v.as_deref()),
                                })
                                .collect()
                        })
                        .collect();
                    write(&rows, at);
                    at += chunk.len();
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
                    stream(&mut |rows, _| w.rows(rows)).map(|()| w.finish())
                }
                (None, CopyFormat::Tsv | CopyFormat::TsvPlain) => {
                    let mut cell = String::new();
                    stream(&mut |rows, _| {
                        cell = rows.first().and_then(|r| r[0]).unwrap_or_default().to_string();
                    })
                    .map(|()| cell)
                }
                (None, CopyFormat::SqlIn) => {
                    // One column's values, each once: collected first (they are deduplicated).
                    if columns.len() != 1 {
                        return self.flash(Notice::new(Label::CopyInSeveralColumns, Level::Warning));
                    }
                    let cells = match sql_cells(false) {
                        Ok(c) => c,
                        Err(r) => return self.flash(self.not_whole(&r)),
                    };
                    let mut values: Vec<Option<String>> = Vec::new();
                    let read = stream(&mut |rows, _| {
                        let rows = written_as(&cells, rows, &mut refused);
                        values.extend(rows.iter().map(|r| r[0].as_deref().map(str::to_string)));
                    });
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
                    let cells = match sql_cells(false) {
                        Ok(c) => c,
                        Err(r) => return self.flash(self.not_whole(&r)),
                    };
                    let mut w = export::Writer::new(export::Format::Update(d, target), &columns);
                    stream(&mut |rows, _| {
                        let rows = written_as(&cells, rows, &mut refused);
                        let rows: Vec<export::Row> =
                            rows.iter().map(|r| r.iter().map(|v| v.as_deref()).collect()).collect();
                        w.rows(&rows);
                    })
                    .map(|()| w.finish())
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
                    wrote_cols = Some(written.iter().map(|w| b.cols[w.0]).collect());
                    let target = Target::Table {
                        schema: &table.schema,
                        name: &table.name,
                        columns: written.iter().map(|w| w.1).collect(),
                        overriding,
                    };
                    let cells = match sql_cells(true) {
                        Ok(c) => c,
                        Err(r) => return self.flash(self.not_whole(&r)),
                    };
                    let columns: Vec<_> = written.iter().map(|w| columns[w.0]).collect();
                    // A value left to its pending DEFAULT is written `DEFAULT`.
                    let default = |r: usize, j: usize| {
                        matches!(rs.over(r, b.cols[written[j].0]), Some(Over::Pending(Value::Default)))
                    };
                    let defaults = overlaid && b.rows.clone().any(|r| (0..written.len()).any(|j| default(r, j)));
                    let mut w = export::Writer::new(export::Format::Sql(d, target.clone()), &columns);
                    let mut parts: Vec<String> = Vec::new();
                    stream(&mut |rows, at| {
                        let rows = written_as(&cells, rows, &mut refused);
                        let rows: Vec<export::Row> =
                            rows.iter().map(|r| written.iter().map(|w| r[w.0].as_deref()).collect()).collect();
                        if defaults {
                            parts
                                .push(export::sql_insert_with(d, &target, &columns, &rows, &|i, j| default(at + i, j)));
                        } else {
                            w.rows(&rows);
                        }
                    })
                    .map(|()| if defaults { parts.join("\n") } else { w.finish() })
                }
            };
            match written {
                Ok(_) if refused.is_some() => {
                    let notice = refused.as_ref().map(|r| self.not_whole(r));
                    return self.flash(notice.unwrap_or_else(|| Notice::new(Label::CopyNothing, Level::Warning)));
                }
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
                self.flash(failed);
                // The cells are kept whatever the clipboard did: when they are not, why.
                if let Some(why) = not_kept {
                    self.flash(Notice::new(why, Level::Info));
                }
                return;
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
        let done = match (pending, deleted) {
            (false, _) => done,
            (true, 0) => Msg::CopyWithPending { done: self.i18n.msg(&done).to_string() },
            (true, count) => Msg::CopyWithDeletes { done: self.i18n.msg(&done).to_string(), count },
        };
        let (done, level) = match fell_back {
            None => (done, Level::Success),
            Some(reason) => (Msg::CopyFellBack { done: self.i18n.msg(&done).to_string(), reason }, Level::Warning),
        };
        self.flash_copied(b.rows, wrote_cols.unwrap_or(b.cols));
        self.tab_mut().grid.anchor = None;
        // Said at once (over an older flash), and kept in the status bar.
        let done = Notice::new(done, level);
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
        if let Some(why) = not_kept {
            self.flash(Notice::new(why, Level::Info));
        }
    }

    /// Whether the rows of block `b` are certainly of one table, as SQL INSERT statements need
    /// (the menu's rule): the reason when not.
    fn insert_source_of(&self, b: &Block) -> Result<(), NotInsertable> {
        let t = self.tab();
        let Results::Rows(rs) = &t.results else { return Ok(()) };
        let d = self.tab_dialect(t.id);
        let origins: Vec<_> = b.cols.iter().map(|&c| rs.columns[c].meta.origin.clone()).collect();
        let names: Vec<&str> = b.cols.iter().map(|&c| rs.columns[c].meta.name.as_str()).collect();
        insert_source(d, self.tab_keys(t).catalog(), &origins, &names, t.shown_sql()).map(drop)
    }

    /// The cells of block `b` of `rs` as `g p` pastes them: each value as the grid shows it (a
    /// pending one over the value read; NULL as NULL), and the fixed offset each MySQL
    /// `TIMESTAMP` column was read in. Not kept, and why (`None`: the rows could not be read,
    /// which the copy says): more rows than a paste takes, more than [`GRID_COPY_MAX_BYTES`] of
    /// values, a row not read whole, or a value read that is not shown as stored
    /// ([`export::sql_cells`]: a MySQL `FLOAT`, bytes not in their form, text or a time read in a
    /// session that shows them otherwise), whose text would store another one.
    fn grid_copy_of(&self, rs: &ResultSet, b: &Block) -> Result<edit::GridCopy, Option<Msg>> {
        let key = || self.key_for(Action::Edit(edit::EditAction::PasteRows), Ctx::Grid);
        if b.rows.len() > MAX_PASTE_ROWS {
            return Err(Some(Msg::CopyRegisterTooMany { key: key(), max: fmt_count(MAX_PASTE_ROWS as u64) }));
        }
        let t = self.tab();
        let d = self.tab_dialect(t.id);
        let zone = self.tab_keys(t).catalog().and_then(|k| k.server_zone());
        let cells: Vec<Result<export::SqlCells, export::Refused>> = b
            .cols
            .iter()
            .map(|&c| {
                let m = &rs.columns[c].meta;
                let col =
                    export::SqlColumn { name: &m.name, type_name: &m.type_name, kind: m.kind, auto_increment: false };
                export::sql_cells(d, &[col], &rs.read_in, zone)
            })
            .collect();
        // A MySQL `TIMESTAMP` is shown in the session's time zone: the offset it was read in
        // goes with it (one not fixed refused it above, through `sql_cells`).
        let offsets: Vec<Option<String>> = b
            .cols
            .iter()
            .map(|&c| match (&rs.read_in, rs.columns[c].meta.kind) {
                (datarig_core::driver::ReadIn::Values(v), datarig_core::driver::ValueKind::TimestampTz)
                    if matches!(d, Dialect::MySql(_)) =>
                {
                    export::fixed_offset(&v.time_zone, zone)
                }
                _ => None,
            })
            .collect();
        let overlaid = !rs.edit.batch.is_empty() || rs.edit.applied.is_some();
        let mut rows: Vec<Vec<Value>> = Vec::with_capacity(b.rows.len());
        let mut refused: Option<export::Refused> = None;
        let (mut short, mut bytes) = (None, 0usize);
        let mut default: Option<usize> = None;
        let mut at = b.rows.start;
        rs.rows
            .for_each_chunk(b.rows.clone(), COPY_CHUNK, |chunk| {
                for (i, r) in chunk.iter().enumerate() {
                    // Past the most kept, nothing more is collected.
                    if bytes > GRID_COPY_MAX_BYTES {
                        return;
                    }
                    let row = b.cols.iter().enumerate().map(|(j, &c)| {
                        let v = match overlaid.then(|| rs.over(at + i, c)).flatten() {
                            // A pending DEFAULT (an SQL INSERT copy takes it): a paste writes none,
                            // so the cells are not kept.
                            Some(Over::Pending(Value::Default)) => {
                                default.get_or_insert(c);
                                Value::Null
                            }
                            // Another value with none to show (one only a run shows) refused the
                            // copy before.
                            Some(o) => o.cell().flatten().map_or(Value::Null, |v| Value::Text(v.to_string())),
                            None => match r.get(c) {
                                None => {
                                    short.get_or_insert(at + i);
                                    Value::Null
                                }
                                Some(None) => Value::Null,
                                Some(Some(v)) => {
                                    if refused.is_none() {
                                        refused = cells[j]
                                            .as_ref()
                                            .map_err(Clone::clone)
                                            .and_then(|x| x.cell(0, v).map(drop))
                                            .err();
                                    }
                                    Value::Text(v.to_string())
                                }
                            },
                        };
                        if let Value::Text(t) = &v {
                            bytes += t.len();
                        }
                        v
                    });
                    rows.push(row.collect());
                }
                at += chunk.len();
            })
            .map_err(|_| None)?;
        if bytes > GRID_COPY_MAX_BYTES {
            let max = format!("{} MiB", GRID_COPY_MAX_BYTES >> 20);
            return Err(Some(Msg::CopyRegisterTooLarge { key: key(), max }));
        }
        if let Some(c) = default {
            return Err(Some(Msg::CopyRegisterDefault { key: key(), column: rs.columns[c].meta.name.clone() }));
        }
        if let Some(row) = short {
            return Err(Some(Msg::CopyRegisterNotRead { key: key(), row: (row + 1).to_string() }));
        }
        // A pending value of a MySQL `TIMESTAMP` read in a zone without a fixed offset (or one not
        // known) is no instant either: its text is read in the zone it lands in.
        let zoned = |j: usize| {
            matches!(d, Dialect::MySql(_))
                && rs.columns[b.cols[j]].meta.kind == datarig_core::driver::ValueKind::TimestampTz
        };
        let refused = refused.or_else(|| {
            (0..b.cols.len())
                .filter(|&j| zoned(j) && offsets_none(&offsets, j))
                .filter(|&j| rows.iter().any(|r| matches!(r.get(j), Some(Value::Text(_)))))
                .find_map(|j| cells[j].as_ref().err().cloned())
        });
        if let Some(r) = refused {
            let reason = self.not_whole_reason(&r.why);
            return Err(Some(Msg::CopyRegisterNotWhole { key: key(), column: r.column, reason }));
        }
        let columns = b.cols.iter().map(|&c| rs.columns[c].meta.clone()).collect();
        // Without a profile the cells are no table's rows: never matched by column.
        let source = t.profile.map(|p| (p, self.other_database(t).map(str::to_string)));
        Ok(edit::GridCopy { source, columns, offsets, rows })
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

    /// Why "every row" cannot be copied now while the server has more (nothing runs): paging
    /// closed, or nothing was held; `None` when it can, or there is no more to fetch.
    pub(super) fn fetch_off(&self) -> Option<super::menu::Off> {
        let t = self.tab();
        let more = matches!(&t.results, Results::Rows(rs) if rs.more);
        if !more || t.exec.running.is_some() || self.can_fetch_rest() {
            return None;
        }
        let short = if t.exec.paging == Paging::Released { Label::MenuOffNotHeld } else { Label::MenuOffPagingClosed };
        Some(super::menu::Off::new(short, Notice::new(self.fetch_unavailable(), Level::Warning)))
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
        self.fetch_copy = Some((intent, CopyRequest { what: CopyWhat::Every, format, into: None, keys: false }));
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
    pub(super) fn why(&self, why: &NotInsertable) -> String {
        use NotInsertable::*;
        // The key cache could not be read: say why (an old server, a permission).
        let failed = Some(self.tab_keys(self.tab())).and_then(|k| match k {
            Keys::Failed(error) => Some(error.clone()),
            _ => None,
        });
        if let (NoCatalog, Some(error)) = (why, failed) {
            return self.i18n.msg(&Msg::CopyInsertWhyKeysFailed { error }).to_string();
        }
        // MySQL's reasons in its own words: the keys are read at connect, so a missing table is
        // one created later or one the user may not see.
        if matches!(self.tab_dialect(self.tab().id), Dialect::MySql(_)) {
            match why {
                NoCatalog => return self.i18n.label(Label::CopyInsertWhyKeysLoading).to_string(),
                UnknownTable => return self.i18n.label(Label::CopyInsertWhyNotInKeys).to_string(),
                _ => {}
            }
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

    /// Why a copy as SQL was refused for a value of a column ([`export::sql_cells`]), in words.
    pub(super) fn not_whole(&self, r: &export::Refused) -> Notice {
        let reason = self.not_whole_reason(&r.why);
        Notice::new(Msg::CopySqlNotWhole { column: r.column.clone(), reason }, Level::Warning)
    }

    /// Why a value is not written whole, in words.
    pub(super) fn not_whole_reason(&self, why: &export::NotWhole) -> String {
        match why {
            export::NotWhole::Float => self.i18n.label(Label::CopySqlWhyFloat).to_string(),
            export::NotWhole::Charset(charset) => {
                self.i18n.msg(&Msg::CopySqlWhyCharset { charset: charset.clone() }).to_string()
            }
            export::NotWhole::TimeZone(zone) => {
                self.i18n.msg(&Msg::CopySqlWhyTimeZone { zone: zone.clone() }).to_string()
            }
            export::NotWhole::AutoIncrementZero => self.i18n.label(Label::CopySqlWhyAutoIncrementZero).to_string(),
            export::NotWhole::NotShownWhole => self.i18n.label(Label::CopySqlWhyNotShownWhole).to_string(),
            export::NotWhole::SessionUnknown => self.i18n.label(Label::CopySqlWhySessionUnknown).to_string(),
            export::NotWhole::SessionChanged => self.i18n.label(Label::CopySqlWhySessionChanged).to_string(),
        }
    }

    /// Why a copy as SQL UPDATE was refused, in words.
    pub(super) fn why_not_update(&self, why: &NotUpdatable) -> String {
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

/// The values of `rows` as the SQL formats write them ([`export::SqlCells::cell`]); the first
/// one that is not written whole is kept in `refused` (the copy is then refused).
/// What the grid shows over the rows as read in block `b` of `rs`: whether some of it is pending
/// (a value, a row to be deleted) and how many of its rows are to be deleted; or why it is not
/// copied: a value it shows has none yet (a pending DEFAULT, except in an INSERT (`insert`),
/// where it is written `DEFAULT`; one only a run shows).
fn over_block(rs: &ResultSet, b: &Block, insert: bool) -> Result<(bool, u64), Msg> {
    let in_block = |r: &usize| b.rows.contains(r);
    let mut rows: std::collections::BTreeSet<usize> = rs.edit.batch.rows().map(|(r, _)| r).filter(in_block).collect();
    if let Some(a) = rs.edit.applied.as_deref() {
        rows.extend(a.set.keys().copied().filter(in_block));
        rows.extend(a.rerun.iter().map(|&(r, _)| r).filter(in_block));
    }
    let deleted = rows.iter().filter(|&&r| rs.edit.batch.deleted(r)).count() as u64;
    let mut pending = deleted > 0;
    for &r in &rows {
        for &c in &b.cols {
            let column = || rs.columns[c].meta.name.clone();
            match rs.over(r, c) {
                Some(Over::Pending(Value::Default)) if !insert => {
                    return Err(Msg::CopyPendingDefault { column: column() });
                }
                Some(Over::Rerun) => return Err(Msg::CopyRerun { column: column() }),
                Some(Over::Applied(Value::Default)) => return Err(Msg::CopyAppliedDefault { column: column() }),
                Some(Over::Pending(_)) => pending = true,
                Some(Over::Applied(_)) | None => {}
            }
        }
    }
    Ok((pending, deleted))
}

fn written_as<'a>(
    cells: &export::SqlCells,
    rows: &'a [export::Row],
    refused: &mut Option<export::Refused>,
) -> Vec<Vec<Option<std::borrow::Cow<'a, str>>>> {
    rows.iter()
        .map(|r| {
            r.iter()
                .enumerate()
                .map(|(i, v)| {
                    v.and_then(|v| match cells.cell(i, v) {
                        Ok(c) => Some(c),
                        Err(r) => {
                            refused.get_or_insert(r);
                            None
                        }
                    })
                })
                .collect()
        })
        .collect()
}

/// Column `j` has no offset kept.
fn offsets_none(offsets: &[Option<String>], j: usize) -> bool {
    offsets.get(j).is_none_or(Option::is_none)
}
