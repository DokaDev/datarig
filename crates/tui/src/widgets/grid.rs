//! Result grid: virtualized rows, horizontal scroll by character (the columns at the view's
//! edges partly shown), CJK-safe cells. A
//! header shows the column's key marks (PK/FK/UQ) before its name. The
//! grid shows one page of the result at a time ([`GridState::page`]); row
//! numbers stay those of the whole result.
//!
//! New rows not applied yet are shown after the last row read, on the last page: row `i` of the
//! grid is row `i` of the result while `i` is less than the rows read, else new row
//! `i - rows read` ([`GridState::new_rows`]). Read by a row's index, a new row is no row of the
//! result ([`CellRef::Missing`]). The new rows an Apply added come first among them, and the
//! values it wrote show over the rows as read, until the query runs again.

use crate::text::{Align, fit, sanitize_cell, slice_cols, width};
use crate::theme;
use datarig_core::driver::{Cell, ColumnMeta, KeyMarks};
use datarig_core::edit::{DefaultShown, Value};
use datarig_core::fault::Fault;
use datarig_core::i18n::{I18n, Label, Msg};
use datarig_core::results::{Row, RowStore};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use std::ops::Range;

pub const MAX_COL_WIDTH: usize = 40;
/// Characters the view moves sideways per wheel event (and per drag event past its edge).
pub const SIDE_STEP: usize = 4;

#[derive(Clone, Debug)]
pub struct Column {
    pub meta: ColumnMeta,
    pub width: usize,
}

/// A result: its columns and its rows ([`RowStore`]: a window in memory, the rest spilled).
#[derive(Debug)]
pub struct ResultSet {
    /// Tells this result apart from every other one of the process (a chart follows the result
    /// it was made for).
    pub id: u64,
    pub columns: Vec<Column>,
    pub rows: RowStore,
    pub more: bool,
    /// How many rows the result has, when the user had them counted.
    pub counted: Option<u64>,
    /// The count was of the rows committed when it ran, which may differ from the rows paged
    /// (not in the snapshot the pages were read in).
    pub counted_now: bool,
    /// Which transaction every page so far was read in: a count in that same
    /// transaction, when it has one snapshot, counts what the pages show.
    pub pages_in: PagesIn,
    /// Rows of it were read inside the user's transaction, and what became
    /// of that transaction.
    pub tx: Option<TxMark>,
    /// What its values were read in (the session's time zone and character set when its first
    /// page came, and whether a later page came under others): what a copy as SQL judges it by.
    pub read_in: datarig_core::driver::ReadIn,
    /// The user's changes of its cells and what may change them: kept with the result, so a
    /// parked result keeps its own.
    pub edit: Box<crate::app::edit::EditState>,
}

/// Where the pages of a result were read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PagesIn {
    /// No page yet.
    #[default]
    Nothing,
    /// Every page in the user's transaction `n` (the tab's `n`th).
    Block(u64),
    /// Outside the user's transaction, or in more than one.
    Other,
}

/// What a result read inside the user's transaction says about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxMark {
    /// The transaction (the tab's `n`th) is still open: the rows may show uncommitted
    /// changes.
    InTx(u64),
    /// It ended, and the app cannot tell it rolled back (a `COMMIT`, a `PREPARE TRANSACTION`).
    Ended,
    /// It rolled back: `ROLLBACK`, a `COMMIT` of an aborted block, a failed `COMMIT`, a session
    /// that ended (lost, closed, switched).
    RolledBack,
}

impl ResultSet {
    /// Rows came while the user's transaction `epoch` is open (`in_block`): the result says so.
    pub fn mark_read(&mut self, in_block: bool, epoch: u64) {
        if in_block && !matches!(self.tx, Some(TxMark::InTx(e)) if e == epoch) {
            self.tx = Some(TxMark::InTx(epoch));
        }
        self.pages_in = match (self.pages_in, in_block) {
            (PagesIn::Nothing, true) => PagesIn::Block(epoch),
            (PagesIn::Block(e), true) if e == epoch => PagesIn::Block(e),
            _ => PagesIn::Other,
        };
    }

    /// The user's transaction `epoch` ended (`end`: how). Whether what an Apply did in it is
    /// gone, with the label that says why.
    pub fn tx_ended(&mut self, epoch: u64, end: crate::app::tabs::TxEnd) -> Option<datarig_core::i18n::Label> {
        if self.tx == Some(TxMark::InTx(epoch)) {
            let rolled_back = end == crate::app::tabs::TxEnd::RolledBack;
            self.tx = Some(if rolled_back { TxMark::RolledBack } else { TxMark::Ended });
        }
        self.edit.tx_ended(epoch, end)
    }
}

/// A cell of a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellRef<'a> {
    Here(&'a Cell),
    /// Its row is not in memory (not loaded yet, or the spill file could not be read): the
    /// value is unknown, never shown as empty or NULL.
    NotRead,
    /// No such row or column.
    Missing,
}

impl ResultSet {
    /// Column width = min(max(header width, first-page value widths), 40).
    pub fn new(cols: Vec<ColumnMeta>, rows: RowStore, more: bool, null_label: &str) -> Self {
        let first = rows.resident();
        let columns = cols
            .into_iter()
            .enumerate()
            .map(|(i, meta)| {
                let header = width(&meta.name).max(width(&meta.type_name));
                let vals = first
                    .clone()
                    .map(|r| match rows.row(r) {
                        Row::Here(r) => match r.get(i) {
                            Some(Some(v)) => width(&sanitize_cell(v)),
                            _ => width(null_label),
                        },
                        _ => 0,
                    })
                    .max()
                    .unwrap_or(0);
                Column { meta, width: header.max(vals).clamp(1, MAX_COL_WIDTH) }
            })
            .collect();
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            id,
            columns,
            rows,
            more,
            counted: None,
            counted_now: false,
            pages_in: PagesIn::Nothing,
            tx: None,
            read_in: Default::default(),
            edit: Default::default(),
        }
    }

    /// How many rows the result has, when that is known: all of them are fetched, or the
    /// user had them counted.
    pub fn total(&self) -> Option<u64> {
        if self.more { self.counted } else { Some(self.rows.len() as u64) }
    }

    /// Rows in memory only (tests, and results that never grow).
    pub fn in_memory(cols: Vec<ColumnMeta>, rows: Vec<Vec<Cell>>, more: bool, null_label: &str) -> Self {
        Self::new(cols, RowStore::in_memory(rows), more, null_label)
    }

    pub fn cell(&self, row: usize, col: usize) -> CellRef<'_> {
        match self.rows.row(row) {
            Row::Here(r) => r.get(col).map_or(CellRef::Missing, CellRef::Here),
            Row::NotRead => CellRef::NotRead,
            Row::Missing => CellRef::Missing,
        }
    }

    /// What the grid shows instead of the value as read in cell (`row`, `col`) of a row read:
    /// a pending value, else what an Apply wrote there (or the table holds since), or that a
    /// run shows it. `None`: the value as read.
    pub fn over(&self, row: usize, col: usize) -> Option<Over<'_>> {
        if row >= self.rows.len() {
            return None;
        }
        if let Some(v) = self.edit.batch.value(row, col) {
            return Some(Over::Pending(v));
        }
        let a = self.edit.applied.as_deref()?;
        if a.rerun.contains(&(row, col)) {
            return Some(Over::Rerun);
        }
        a.set.get(&row)?.get(&col).map(Over::Applied)
    }

    /// Cell (`row`, `col`) as the grid shows it: the value as read, or the value over it.
    pub fn shown(&self, row: usize, col: usize) -> Shown<'_> {
        match self.over(row, col) {
            Some(o) => match o.cell() {
                Some(v) => Shown::Text(v),
                None if o == Over::Rerun => Shown::Label(Label::EditRerunToSee),
                None => Shown::Label(Label::EditDefault),
            },
            None => match self.cell(row, col) {
                CellRef::Here(c) => Shown::Text(c.as_deref()),
                CellRef::NotRead => Shown::NotRead,
                CellRef::Missing => Shown::Missing,
            },
        }
    }
}

/// A cell as the grid shows it ([`ResultSet::shown`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown<'a> {
    /// A value (`None`: NULL).
    Text(Option<&'a str>),
    /// No value yet, said by this label (`default`, `re-run to see`).
    Label(Label),
    NotRead,
    Missing,
}

/// What the grid shows over a value as read ([`ResultSet::over`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Over<'a> {
    /// A change not applied yet.
    Pending(&'a Value),
    /// What an Apply wrote, or the table returned for it (`Value::Default`: not known).
    Applied(&'a Value),
    /// The table returned it in another form than the result shows: only a run shows it.
    Rerun,
}

impl<'a> Over<'a> {
    /// Its value as a cell (`Some(None)`: NULL); `None` when it has none to show (a DEFAULT, a
    /// value only a run shows).
    pub fn cell(self) -> Option<Option<&'a str>> {
        match self {
            Over::Pending(Value::Text(v)) | Over::Applied(Value::Text(v)) => Some(Some(v)),
            Over::Pending(Value::Null) | Over::Applied(Value::Null) => Some(None),
            _ => None,
        }
    }
}

#[derive(Default, Clone, Debug)]
pub struct GridState {
    pub row: usize,
    pub col: usize,
    pub top: usize,
    /// How many characters of the row of all columns (each `"│ " + value + " "`) the view
    /// starts after, right of the row-number gutter.
    pub x_off: usize,
    /// Where each column starts in that row, and where the last one ends (set on render).
    pub col_x: Vec<usize>,
    /// How many characters of it the view shows (set on render).
    pub view_w: u16,
    /// The column whose separator is hidden left of the view, as drawn last (it shows a part).
    pub cut_col: Option<usize>,
    /// Visible data rows (set on render).
    pub page_rows: usize,
    /// Where a selection started (`v`, `V`, a drag): the range is from here to the cursor.
    pub anchor: Option<(usize, usize)>,
    /// What the selection spans ([`Shape`]); only meaningful with an `anchor`.
    pub shape: Shape,
    /// The row-number gutter's columns (x from, x to) and the headers' first row, as drawn
    /// last (a click there selects whole rows or whole columns).
    pub gutter_x: (u16, u16),
    pub header_y: u16,
    /// Hit-test info from the last render: (x_start, x_end, column index) of the part of each
    /// column on screen, data origin y.
    pub hit_cols: Vec<(u16, u16, usize)>,
    pub data_y: u16,
    /// The last render could not read the rows it shows from the result's spill file.
    pub read_error: Option<Fault>,
    /// The page shown (from 0), of [`GridState::page_size`] rows each.
    pub page: usize,
    /// Rows per page; 0: the whole result is one page.
    pub page_size: usize,
    /// The view was scrolled by the wheel: it stays where it was put, even with
    /// the selected cell off screen, until a key or a click moves the selection (which brings
    /// the view back to it).
    pub detached: bool,
    /// How many cells (a column's part on screen on one row, its header's two rows counted as
    /// one) the last render drew: the benchmark's measure of a frame's work.
    pub drawn_cells: usize,
    /// A search's match the next render shows: (row, column, where the match starts in the
    /// cell's text as drawn, in columns). Taken by that render; only for the cursor's cell.
    pub reveal: Option<(usize, usize, usize)>,
    /// How many new rows the grid shows after the result's last row read (set on render and by
    /// every edit).
    pub new_rows: usize,
}

/// What a grid selection spans.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shape {
    /// The rectangle between the anchor and the cursor (`v`, a drag over cells).
    #[default]
    Cells,
    /// Whole rows, from the anchor's row to the cursor's (`V`, the row-number gutter).
    Rows,
    /// Whole columns, from the anchor's column to the cursor's, every fetched row (a header).
    Cols,
}

impl Shape {
    /// The rows and columns between `anchor` and `cursor` in this shape, of a result of
    /// `total` rows and `ncols` columns.
    pub fn span(
        self,
        anchor: (usize, usize),
        cursor: (usize, usize),
        total: usize,
        ncols: usize,
    ) -> (Range<usize>, Range<usize>) {
        let (rows, cols) = crate::app::copy::range(anchor, cursor);
        match self {
            Shape::Cells => (rows, cols),
            Shape::Rows => (rows, 0..ncols),
            Shape::Cols => (0..total, cols),
        }
    }
}

impl GridState {
    /// A grid on page 0 with `page_size` rows a page (0: one page), drawn `page_rows` high.
    pub fn paged(page_size: usize, page_rows: usize) -> Self {
        Self { page_size, page_rows, ..Self::default() }
    }

    /// The rows the grid shows of a result of `total` rows read: its page (the last one when the
    /// page is past the end), the new rows after the last page's rows.
    pub fn window(&self, total: usize) -> std::ops::Range<usize> {
        if self.page_size == 0 || total == 0 {
            return 0..total + self.new_rows;
        }
        let last = (total - 1) / self.page_size;
        let start = self.page.min(last) * self.page_size;
        let end = (start + self.page_size).min(total);
        start..if end == total { total + self.new_rows } else { end }
    }

    /// `added` rows were read after the `read` there were: a cursor, a view or a selection's
    /// anchor among the new rows (shown after the rows read) stays on the same new row, and the
    /// page shown is the cursor's. A selection from a row read to a new row goes (it would take
    /// in the rows that came).
    pub fn rows_came(&mut self, read: usize, added: usize) {
        if added == 0 || self.new_rows == 0 {
            return;
        }
        let on_new = self.row >= read;
        if self.shape != Shape::Cols && self.anchor.is_some_and(|(r, _)| (r >= read) != on_new) {
            self.anchor = None;
        }
        let shift = |r: &mut usize| {
            if *r >= read {
                *r += added;
            }
        };
        shift(&mut self.row);
        shift(&mut self.top);
        if let Some((r, _)) = self.anchor.as_mut() {
            shift(r);
        }
        if on_new && self.page_size > 0 {
            self.page = (read + added - 1) / self.page_size;
        }
    }

    /// Show page `page`: the cursor goes to its first row, a selected range goes.
    pub fn set_page(&mut self, page: usize) {
        self.page = page;
        self.row = page * self.page_size;
        self.top = self.row;
        self.anchor = None;
        self.detached = false;
    }

    /// Scroll the view `rows` rows and `chars` characters (the wheel): the selected cell
    /// stays on its data row, the view stops at the page's ends and at the columns' ends
    /// (as drawn last) and fetches nothing.
    pub fn scroll(&mut self, rows: isize, chars: isize, total: usize) {
        let w = self.window(total);
        let last_top = w.end.saturating_sub(self.page_rows.max(1)).max(w.start);
        self.top = (self.top as isize + rows).clamp(w.start as isize, last_top as isize) as usize;
        self.x_off = self.x_off.saturating_add_signed(chars).min(self.x_max());
        self.detached = true;
    }

    /// The farthest the view can be scrolled right: the last column's end at the view's right
    /// edge (as laid out last).
    pub fn x_max(&self) -> usize {
        self.col_x.last().map_or(0, |end| end.saturating_sub(self.view_w as usize))
    }

    /// Bring the selected column into view, moving the view as little as it takes: a column
    /// cut on the left shows from its start, one cut on the right up to its end, one wider
    /// than the view from its start unless it fills the view already. A column already whole
    /// on screen moves nothing.
    fn follow(&mut self) {
        let (Some(&start), Some(&end)) = (self.col_x.get(self.col), self.col_x.get(self.col + 1)) else { return };
        let view = self.view_w as usize;
        if end - start > view {
            if !(start <= self.x_off && self.x_off + view <= end) {
                self.x_off = start;
            }
        } else if start < self.x_off {
            self.x_off = start;
        } else if end > self.x_off + view {
            self.x_off = end - view;
        }
    }

    /// Whether column `x` of the screen is left or right of the view (as laid out last).
    pub fn beside(&self, x: u16) -> bool {
        x < self.gutter_x.1 || x >= self.gutter_x.1 + self.view_w
    }

    /// A drag at column `x` of the screen past the left or right edge of the view: the view
    /// moves [`SIDE_STEP`] characters that way and the cursor goes to the column at that edge
    /// that shows whole (the one cut there when none does). `None` within the view.
    pub fn drag_edge(&mut self, x: u16) -> Option<usize> {
        let ncols = self.col_x.len().checked_sub(1).filter(|n| *n > 0)?;
        let (from, view) = (self.gutter_x.1, self.view_w as usize);
        let at = |vx: usize| (0..ncols).find(|&c| self.col_x[c + 1] > vx).unwrap_or(ncols - 1);
        if x < from {
            self.x_off = self.x_off.saturating_sub(SIDE_STEP);
            let whole = (0..ncols).find(|&c| self.col_x[c] >= self.x_off && self.col_x[c + 1] <= self.x_off + view);
            Some(whole.unwrap_or_else(|| at(self.x_off)))
        } else if x >= from + self.view_w {
            self.x_off = (self.x_off + SIDE_STEP).min(self.x_max());
            let whole =
                (0..ncols).rev().find(|&c| self.col_x[c] >= self.x_off && self.col_x[c + 1] <= self.x_off + view);
            Some(whole.unwrap_or_else(|| at((self.x_off + view).saturating_sub(1))))
        } else {
            None
        }
    }

    pub fn move_row(&mut self, delta: isize, total: usize) {
        self.detached = false;
        let w = self.window(total);
        if w.is_empty() {
            return;
        }
        self.row = (self.row as isize + delta).clamp(w.start as isize, w.end as isize - 1) as usize;
    }

    pub fn move_col(&mut self, delta: isize, total: usize) {
        self.detached = false;
        if total == 0 {
            return;
        }
        self.col = (self.col as isize + delta).clamp(0, total as isize - 1) as usize;
    }

    /// The cursor is on the last row of its page.
    pub fn at_page_end(&self, total: usize) -> bool {
        self.window(total).end == self.row + 1
    }

    /// The cell at (x, y) as drawn last, if there is one.
    pub fn cell_at(&self, x: u16, y: u16, total: usize) -> Option<(usize, usize)> {
        if y < self.data_y {
            return None;
        }
        let r = self.top + (y - self.data_y) as usize;
        if r >= self.window(total).end {
            return None;
        }
        self.hit_cols.iter().find(|(a, b, _)| x >= *a && x < *b).map(|&(_, _, c)| (r, c))
    }

    /// Where cell (row, col) was drawn last (between its separators; of a column at an edge of
    /// the view, the part on screen), if it was.
    pub fn cell_rect(&self, row: usize, col: usize, total: usize) -> Option<Rect> {
        let shown = self.top..(self.top + self.page_rows).min(self.window(total).end);
        let &(a, b, _) = self.hit_cols.iter().find(|h| h.2 == col).filter(|_| shown.contains(&row))?;
        let x = if self.cut_col == Some(col) { a } else { a + 1 };
        Some(Rect::new(x, self.data_y + (row - self.top) as u16, b.saturating_sub(x), 1)).filter(|r| r.width > 0)
    }

    /// Where a drag at (x, y) takes the cursor: the cell there, clamped to
    /// the result; past an edge of the grid one row or column beyond the last one shown, so
    /// the grid scrolls that way (over rows not in memory too: drawing reads them).
    pub fn drag_target(&self, x: u16, y: u16, total: usize, cols: usize) -> (usize, usize) {
        let w = self.window(total);
        let last_row = w.end.saturating_sub(1);
        let row = if y < self.data_y {
            self.top.saturating_sub(1).max(w.start)
        } else {
            let r = self.top + (y - self.data_y) as usize;
            if r >= self.top + self.page_rows.max(1) {
                (self.top + self.page_rows).min(last_row)
            } else {
                r.min(last_row)
            }
        };
        let (first, last) = match (self.hit_cols.first(), self.hit_cols.last()) {
            (Some(f), Some(l)) => (*f, *l),
            _ => return (row, self.col),
        };
        let col = if x < first.0 {
            first.2.saturating_sub(1)
        } else if x >= last.1 {
            (last.2 + 1).min(cols.saturating_sub(1))
        } else {
            self.hit_cols.iter().find(|(a, b, _)| x >= *a && x < *b).map_or(self.col, |&(_, _, c)| c)
        };
        (row, col)
    }

    /// The selected rows and columns of a result of `total` rows and `ncols` columns, if
    /// something is selected: whole rows span every column, whole columns every fetched row.
    pub fn selection(&self, total: usize, ncols: usize) -> Option<(Range<usize>, Range<usize>)> {
        self.anchor.map(|a| self.shape.span(a, (self.row, self.col), total, ncols))
    }

    /// Whether (row, col) is in the selected range.
    pub fn in_range(&self, (row, col): (usize, usize), total: usize, ncols: usize) -> bool {
        self.selection(total, ncols).is_some_and(|(rows, cols)| rows.contains(&row) && cols.contains(&col))
    }

    /// The row whose number is at (x, y) in the gutter, as drawn last.
    pub fn gutter_row(&self, x: u16, y: u16, total: usize) -> Option<usize> {
        if x < self.gutter_x.0 || x >= self.gutter_x.1 || y < self.data_y {
            return None;
        }
        let r = self.top + (y - self.data_y) as usize;
        (r < self.window(total).end).then_some(r)
    }

    /// The column whose header is at (x, y), as drawn last.
    pub fn header_col(&self, x: u16, y: u16) -> Option<usize> {
        if y < self.header_y || y >= self.data_y {
            return None;
        }
        self.hit_cols.iter().find(|(a, b, _)| x >= *a && x < *b).map(|&(_, _, c)| c)
    }

    /// Select whole rows or columns (`shape`) from `anchor` (row, column) to the cursor at
    /// `to`; `extend` keeps an anchor there is (Shift+click).
    pub fn select_whole(&mut self, shape: Shape, to: (usize, usize), extend: bool) {
        let keep = extend && self.anchor.is_some();
        if !keep {
            self.anchor = Some(to);
        }
        self.shape = shape;
        (self.row, self.col) = to;
        self.detached = false;
    }

    pub fn click(&mut self, x: u16, y: u16, total: usize) -> bool {
        if y < self.data_y {
            return false;
        }
        let r = self.top + (y - self.data_y) as usize;
        if r >= self.window(total).end {
            return false;
        }
        if let Some(&(_, _, c)) = self.hit_cols.iter().find(|(a, b, _)| x >= *a && x < *b) {
            self.row = r;
            self.col = c;
            self.detached = false;
            return true;
        }
        false
    }
}

fn cell_text(cell: &Cell, null_label: &str) -> (String, bool) {
    match cell {
        Some(v) => (sanitize_cell(v), false),
        None => (null_label.to_string(), true),
    }
}

/// How the grid draws: the column headers' key marks (by column, empty: none), whether they
/// are Nerd Font glyphs, and the cells an Apply is refused for: changed cells whose value their
/// column does not take, and cells of new rows left to a default their column lacks.
#[derive(Clone, Copy, Default)]
pub struct Look<'a> {
    pub marks: &'a [KeyMarks],
    pub icons: bool,
    pub invalid: Option<&'a crate::app::edit::Invalids>,
    /// By result column, what a new row's cell left to its default shows (none: the column's
    /// default is not known, `default`).
    pub defaults: &'a [Option<DefaultShown>],
}

/// How wide a default's expression is shown in a new row's cell, at most (`<default: now()>`).
const EXPR_W: usize = 20;

/// The mark of a row in the gutter's free column: `!` a cell an Apply is refused for (a value
/// its column does not take, or a default it lacks), else `+` a new row, `-` a row to be
/// deleted, `~` cells changed.
fn row_mark(problem: bool, new: bool, deleted: bool, changed: bool) -> Option<&'static str> {
    match (new, deleted) {
        _ if problem => Some("!"),
        (true, _) => Some("+"),
        (false, true) => Some("-"),
        (false, false) => changed.then_some("~"),
    }
}

pub fn render(
    rs: &mut ResultSet,
    st: &mut GridState,
    area: Rect,
    buf: &mut Buffer,
    i18n: &I18n,
    focused: bool,
    look: Look,
) {
    let th = theme::cur();
    buf.set_style(area, th.base());
    st.hit_cols.clear();
    st.cut_col = None;
    st.drawn_cells = 0;
    if area.height < 3 || area.width < 8 || rs.columns.is_empty() {
        return;
    }
    let total = rs.rows.len();
    st.new_rows = rs.edit.new_shown();
    // The page shown: the rows of the window, numbered as in the whole result.
    let win = st.window(total);
    let null_label = i18n.label(Label::ResultsNull);
    let page_rows = area.height as usize - 2;
    st.page_rows = page_rows;
    st.row = st.row.clamp(win.start, win.end.saturating_sub(1).max(win.start));
    st.col = st.col.min(rs.columns.len() - 1);
    st.top = st.top.max(win.start);
    // The view stays within the page, filled to its last row (a jump to another page, as a
    // search's, may leave it past the rows there).
    st.top = st.top.min(win.end.saturating_sub(page_rows).max(win.start));
    // Scrolled by the wheel (detached), the view stays; else it follows the selected row.
    if !st.detached && st.row < st.top {
        st.top = st.row;
    } else if !st.detached && st.row >= st.top + page_rows {
        st.top = st.row + 1 - page_rows;
    }
    // The rows on screen (the selected one among them) from the spill file, when they are not
    // in memory.
    if let Err(e) = rs.rows.load(st.top..st.top + page_rows) {
        st.read_error = Some(e);
    }
    let not_read = i18n.label(Label::ResultsNotRead);
    let default_label = i18n.label(Label::EditDefault);
    let needed_label = i18n.label(Label::EditValueNeeded);
    let rerun_label = i18n.label(Label::EditRerunToSee);
    // What each column's cells of new rows left to their default show, and whether it is a
    // value (aligned as one); none for a cell that needs a value (`! value needed`).
    let generated_label = i18n.label(Label::EditDefaultGenerated);
    let draft_defaults: Vec<Option<(String, bool)>> = (0..rs.columns.len())
        .map(|c| match look.defaults.get(c)?.as_ref()? {
            DefaultShown::Needed => None,
            DefaultShown::Generated => Some((generated_label.to_string(), false)),
            DefaultShown::Null => Some((null_label.to_string(), false)),
            // A string that reads like a marker (empty, NULL, `<…>`) is quoted.
            DefaultShown::Value(v) if v.is_empty() || v.eq_ignore_ascii_case(&null_label) || v.starts_with('<') => {
                Some((format!("'{}'", sanitize_cell(v)), false))
            }
            DefaultShown::Value(v) => Some((sanitize_cell(v), true)),
            DefaultShown::Expression(e) => {
                let e = sanitize_cell(e);
                let expr = if width(&e) > EXPR_W { fit(&e, EXPR_W, Align::Left) } else { e };
                // In angle brackets, as `<generated>`: no value of the column.
                Some((format!("<{}>", i18n.msg(&Msg::EditDefaultExpr { expr })), false))
            }
        })
        .collect();
    // The columns where a new row is left to its default, wide enough for what it shows.
    let mut defaulted = std::collections::BTreeSet::new();
    for d in rs.edit.batch.drafts() {
        for c in 0..rs.columns.len() {
            if d.cells.get(c).is_none_or(|v| *v == Value::Default) {
                defaulted.insert(c);
            }
        }
    }

    let gutter = win.end.max(1).to_string().len().max(2) + 1;
    let avail = (area.width as usize).saturating_sub(gutter);
    // A column is as wide as its values, or its header with the key marks before the name.
    // A column with a new row's cell that needs a value is wide enough for its marker.
    let needs: std::collections::BTreeSet<usize> =
        look.invalid.map(|m| m.no_default.iter().map(|&(_, c)| c).collect()).unwrap_or_default();
    let marks: Vec<String> = (0..rs.columns.len())
        .map(|i| look.marks.get(i).map(|m| crate::icons::key_marks_text(*m, look.icons)).unwrap_or_default())
        .collect();
    let widths: Vec<usize> = rs
        .columns
        .iter()
        .zip(&marks)
        .enumerate()
        .map(|(i, (c, m))| {
            let needed = if needs.contains(&i) { width(&needed_label) } else { 0 };
            let shown = match draft_defaults.get(i) {
                Some(Some((t, _))) if defaulted.contains(&i) => width(t),
                _ => 0,
            };
            c.width.max(width(m) + width(&c.meta.name)).max(needed).max(shown).min(MAX_COL_WIDTH)
        })
        .collect();
    // The row of all columns, scrolled by character: the selected column whole in view unless
    // the wheel moved the view, never past the last column's end (also when the widths or the
    // view's width changed).
    st.col_x.clear();
    let mut end = 0;
    for w in &widths {
        st.col_x.push(end);
        end += w + 3;
    }
    st.col_x.push(end);
    st.view_w = avail as u16;
    if !st.detached {
        st.follow();
    }
    // A search's match in a column wider than the view: its start in view, a little of what
    // comes before it too (past a value's cut, the cut's `…`).
    if let Some((r, c, at)) = st.reveal.take()
        && (r, c) == (st.row, st.col)
        && !st.detached
        && let Shown::Text(Some(v)) = rs.shown(r, c)
    {
        let (text, w) = (sanitize_cell(v), widths[c]);
        let tw = width(&text);
        let k = if tw > w {
            at.min(w - 1)
        } else if rs.columns[c].meta.numeric {
            w - tw + at
        } else {
            at
        };
        let p = st.col_x[c] + 2 + k;
        if p < st.x_off || p >= st.x_off + avail {
            st.x_off = p.saturating_sub((avail / 4).min(8)).max(st.col_x[c]);
        }
    }
    st.x_off = st.x_off.min(st.x_max());

    let header_style = Style::new().bg(th.surface).fg(th.fg).add_modifier(Modifier::BOLD);
    let type_style = Style::new().bg(th.surface).fg(th.fg_muted);
    let sep_style = |bg| Style::new().fg(th.border).bg(bg);
    let y0 = area.y;
    buf.set_style(Rect::new(area.x, y0, area.width, 2), Style::new().bg(th.surface));
    st.data_y = y0 + 2;
    st.header_y = y0;
    st.gutter_x = (area.x, area.x + gutter as u16);

    let right = area.x + area.width;
    // Column layout: "│ " + cell + " ", from the gutter's end, `x_off` characters of it hidden;
    // the columns at the edges show their part on screen.
    let from = area.x + gutter as u16;
    let mut layout: Vec<(i32, usize, usize)> = Vec::new(); // (x of its separator, col index, width)
    for (ci, &w) in widths.iter().enumerate() {
        let (start, end) = (st.col_x[ci], st.col_x[ci + 1]);
        if end <= st.x_off {
            continue;
        }
        if start >= st.x_off + avail {
            break;
        }
        let x = i32::from(from) + start as i32 - st.x_off as i32;
        if x < i32::from(from) {
            st.cut_col = Some(ci);
        }
        layout.push((x, ci, w));
        // A column of which only the separator shows is no cell to point at.
        if x + 1 < i32::from(right) {
            let to = (x + 3 + w as i32).min(i32::from(right));
            st.hit_cols.push((x.max(i32::from(from)) as u16, to as u16, ci));
        }
    }
    // `s` drawn from column `x` of the screen (left of the view when the view cut it), only
    // what falls between the gutter and the right edge.
    let put = |buf: &mut Buffer, x: i32, y: u16, s: &str, style: Style| {
        let at = x.max(i32::from(from));
        let room = (i32::from(right) - at).max(0) as usize;
        let part = slice_cols(s, (at - x) as usize, room);
        if !part.is_empty() {
            buf.set_stringn(at as u16, y, &part, room, style);
        }
    };
    // A cell cut by the right edge whose text columns on screen are blank though its text is
    // not (a right-aligned number under the edge, a value after blanks): a dim `›` in its last
    // column on screen (the muted text, made to read on the cell's background), so it never
    // reads as empty. Only where at least one of its text columns shows (not its separator or
    // blank alone). `s` is its text as drawn from column `x`, `w` wide; text of blanks only (an
    // empty value) is not hidden and gets none.
    let edge_mark = |buf: &mut Buffer, x: i32, w: usize, s: &str, y: u16, style: Style| {
        let r = i32::from(right);
        if x + w as i32 <= r || x >= r || s.trim().is_empty() {
            return;
        }
        let at = x.max(i32::from(from));
        if slice_cols(s, (at - x) as usize, (r - at).max(0) as usize).trim().is_empty() {
            let fg = theme::Theme::readable_on(th.fg_muted, style.bg.unwrap_or(th.bg), 3.0);
            buf.set_stringn(right - 1, y, "›", 1, style.fg(fg));
        }
    };

    // A selected range (`v`, `V`, the gutter, a header), from its anchor to the cursor.
    let range = st.selection(total, rs.columns.len());
    let range_cols = range.as_ref().map(|(_, cr)| cr.clone());
    let selected_col = |c: usize| range_cols.as_ref().is_some_and(|cr| cr.contains(&c));
    // Between two selected columns the separator is the range's too: the block reads as one.
    let inner_sep = |c: usize| c > 0 && selected_col(c) && selected_col(c - 1);
    for &(cx, ci, w) in &layout {
        st.drawn_cells += 1;
        let col = &rs.columns[ci];
        // The headers of the selected columns, else the cursor's column's header: tinted.
        let tint = if selected_col(ci) {
            Some(th.range)
        } else if ci == st.col {
            Some(th.grid_cursor_header)
        } else {
            None
        };
        let (header_style, type_style) = match tint {
            Some(t) => (header_style.patch(t), type_style.patch(t)),
            None => (header_style, type_style),
        };
        if let Some(t) = tint {
            let (a, b) = ((cx + 1).max(i32::from(from)), (cx + 3 + w as i32).min(i32::from(right)));
            buf.set_style(Rect::new(a as u16, y0, (b - a).max(0) as u16, 2), t);
        }
        let sep = if inner_sep(ci) { sep_style(th.surface).patch(th.range) } else { sep_style(th.surface) };
        put(buf, cx, y0, "│", sep);
        put(buf, cx, y0 + 1, "│", sep);
        // The key marks in their colors, then the name.
        let mut hx = cx + 2;
        let mut left = w;
        for m in crate::icons::key_marks(look.marks.get(ci).copied().unwrap_or_default()) {
            let text = format!("{} ", m.text(look.icons));
            let mw = width(&text);
            if mw >= left {
                break;
            }
            put(buf, hx, y0, &text, header_style.fg(th.key_color(m)));
            hx += mw as i32;
            left -= mw;
        }
        put(buf, hx, y0, &fit(&col.meta.name, left, Align::Left), header_style);
        put(buf, cx + 2, y0 + 1, &fit(&col.meta.type_name, w, Align::Left), type_style);
    }

    for vy in 0..page_rows {
        let r = st.top + vy;
        if r >= win.end {
            break;
        }
        let y = st.data_y + vy as u16;
        // A new row is after the rows read and those an Apply added.
        let applied = rs.edit.applied.as_deref();
        let added = r.checked_sub(total);
        let applied_new = added.and_then(|i| applied?.new.get(i));
        let new = added.and_then(|i| i.checked_sub(rs.edit.applied_new())).and_then(|i| rs.edit.batch.drafts().get(i));
        // A row of the selected range; its row tint and number only for a range of rows or cells
        // (whole columns take in every row).
        let in_block_rows = range.as_ref().is_some_and(|(rr, _)| rr.contains(&r));
        let in_rows = in_block_rows && st.shape != Shape::Cols;
        // A new row is tinted as a whole: its cells, the separators and its number; the cursor's
        // row and the rows of a selected range get the cursor row's tint the same way (a new row
        // keeps its own: its number says where the cursor is).
        let stripe = if r % 2 == 1 { th.surface_alt } else { th.bg };
        let zebra = match new {
            Some(_) => th.edit_inserted_row,
            None if r == st.row || in_rows => th.grid_cursor_row.bg.unwrap_or(stripe),
            None => stripe,
        };
        buf.set_style(Rect::new(area.x, y, area.width, 1), Style::new().bg(zebra));
        // Selected rows (whole, or of a range of cells): their numbers too.
        let num_bg = if in_rows { th.range } else { Style::new().bg(zebra) };
        let num_style = num_bg.fg(if r == st.row { th.accent } else { th.fg_muted });
        // Deleted by an Apply: it stays, crossed out.
        let deleted = added.is_none()
            && (rs.edit.batch.deleted(r)
                || (applied.is_some_and(|a| a.deleted.contains(&r)) && rs.edit.batch.row(r).is_none()));
        // Rows an Apply added and new rows continue the numbers of the rows read.
        let number = (r + 1).to_string();
        buf.set_stringn(area.x, y, fit(&number, gutter - 1, Align::Right), gutter - 1, num_style);
        // What is pending in the row, in the gutter's free column ([`row_mark`]).
        use crate::app::edit::RowAt;
        let at = match (added, new) {
            (None, _) => Some(RowAt::Read(r)),
            (Some(_), Some(d)) => Some(RowAt::New(d.id)),
            (Some(_), None) => None,
        };
        let found = at.zip(look.invalid);
        let bad = |c: usize| found.is_some_and(|(a, m)| m.cells.contains_key(&(a, c)));
        // A cell of a new row left to a default its column lacks.
        let needed = |c: usize| found.is_some_and(|(a, m)| m.no_default.contains(&(a, c)));
        // Or a new row of a table with a column the result does not show that needs a value.
        let row_bad = found.is_some_and(|(a, m)| {
            let row = (a, 0)..=(a, usize::MAX);
            m.cells.range(row.clone()).next().is_some()
                || m.no_default.range(row).next().is_some()
                || matches!(a, RowAt::New(id) if m.not_shown.contains(&id))
        });
        let mark = row_mark(row_bad, new.is_some(), deleted, rs.edit.batch.row(r).is_some());
        if let Some(mark) = mark {
            let style = if row_bad { num_bg.fg(th.error) } else { num_style };
            buf.set_stringn(area.x + gutter as u16 - 1, y, mark, 1, style);
        }
        for &(cx, ci, w) in &layout {
            st.drawn_cells += 1;
            let col = &rs.columns[ci];
            let selected = r == st.row && ci == st.col;
            let in_range = range.as_ref().is_some_and(|(rr, cr)| rr.contains(&r) && cr.contains(&ci));
            let bg = if selected {
                if focused { th.selection } else { th.selection_unfocused() }
            } else if in_range {
                th.range
            } else {
                Style::new().bg(zebra)
            };
            let in_block = in_block_rows && inner_sep(ci);
            put(buf, cx, y, "│", if in_block { sep_style(zebra).patch(th.range) } else { sep_style(zebra) });
            // A pending value is shown instead of the value as read; a new row's own values. So is
            // a value an Apply wrote, when nothing is pending there.
            let pending = match new {
                Some(d) => Some(d.cells.get(ci).unwrap_or(&Value::Default)),
                None if added.is_some() => None,
                None => rs.edit.batch.value(r, ci),
            };
            let done = match applied_new {
                Some(cells) => Some(cells.get(ci).unwrap_or(&Value::Default)),
                None if added.is_some() || pending.is_some() => None,
                None => applied.and_then(|a| a.set.get(&r)?.get(&ci)),
            };
            // Shown as applied: what the Apply wrote, every value of a row it added. Returned in
            // another form than the result shows: dim, a run shows it.
            let (written, rerun) = match (applied, added) {
                (Some(a), Some(i)) if applied_new.is_some() => (true, a.rerun_new.contains(&(i, ci))),
                (Some(a), None) if pending.is_none() => (a.written.contains(&(r, ci)), a.rerun.contains(&(r, ci))),
                _ => (false, false),
            };
            let needed = needed(ci);
            // A new row's cell left to its default: what the server will put there.
            let drafted = match (new, pending) {
                (Some(_), Some(Value::Default)) if !needed => draft_defaults.get(ci).and_then(Option::as_ref),
                _ => None,
            };
            let (text, is_null, default) = match (pending.or(done), rs.cell(r, ci)) {
                _ if rerun => (rerun_label.to_string(), true, true),
                // Not the dim `default`: an Apply is refused until it has a value.
                _ if needed => (needed_label.to_string(), true, false),
                _ if drafted.is_some() => drafted.map(|(t, value)| (t.clone(), !value, true)).unwrap_or_default(),
                (Some(Value::Text(v)), _) => (sanitize_cell(v), false, false),
                (Some(Value::Null), _) => (null_label.to_string(), true, false),
                (Some(Value::Default), _) => (default_label.to_string(), true, true),
                (None, CellRef::Here(c)) => {
                    let (t, n) = cell_text(c, &null_label);
                    (t, n, false)
                }
                (None, CellRef::NotRead) => (not_read.to_string(), true, false),
                (None, CellRef::Missing) => {
                    let (t, n) = cell_text(&None, &null_label);
                    (t, n, false)
                }
            };
            let align = if col.meta.numeric && !is_null { Align::Right } else { Align::Left };
            let style = if needed {
                Style::new().fg(th.fg)
            } else if drafted.is_some() {
                // Muted, not dim: it must read on the new row's background.
                Style::new().fg(th.fg_muted).add_modifier(Modifier::ITALIC)
            } else if default {
                Style::new().fg(th.fg_muted).add_modifier(Modifier::DIM)
            } else if is_null {
                Style::new().fg(th.null_fg).add_modifier(Modifier::ITALIC)
            } else {
                Style::new().fg(th.fg)
            };
            let style = match (new, pending) {
                _ if needed || bad(ci) => style.patch(th.edit_invalid),
                // A default keeps its dim look in a new row.
                (Some(_), _) if default => style,
                (Some(_), _) => style.patch(th.edit_inserted),
                (None, Some(_)) => style.patch(th.edit_changed),
                (None, None) if deleted => style.patch(th.edit_deleted),
                // What the table filled in for a new row is not known: its dim `default`.
                (None, None) if default => style,
                (None, None) if done.is_some() && written => style.patch(th.edit_applied),
                (None, None) => style,
            }
            .patch(bg);
            // On the cursor or in a range too: the selection's text color would hide the problem.
            let problem = needed || bad(ci);
            let style = match th.edit_invalid.fg {
                Some(fg) if problem => style.fg(fg),
                _ => style,
            };
            put(buf, cx + 1, y, &" ".repeat(w + 2), bg);
            let text = fit(&text, w, align);
            put(buf, cx + 2, y, &text, style);
            // NULL (and a row not read) keeps its own look; what a new row's default shows is
            // marked where the view cuts it off, as a value is.
            if !is_null || drafted.is_some() {
                edge_mark(buf, cx + 2, w, &text, y, bg);
            }
        }
    }
}

/// Full value for the cell viewer, as the grid shows it; json/jsonb pretty-printed. A cell
/// whose row is not in memory is "not read".
pub fn viewer_text(rs: &ResultSet, st: &GridState, i18n: &I18n) -> Option<(String, String)> {
    let col = rs.columns.get(st.col)?;
    let text = match rs.shown(st.row, st.col) {
        Shown::Missing => return None,
        Shown::NotRead => i18n.label(Label::ResultsNotRead).to_string(),
        Shown::Label(l) => i18n.label(l).to_string(),
        Shown::Text(None) => i18n.label(Label::ResultsNull).to_string(),
        Shown::Text(Some(v)) if col.meta.json => serde_json::from_str::<serde_json::Value>(v)
            .ok()
            .and_then(|j| serde_json::to_string_pretty(&j).ok())
            .unwrap_or_else(|| v.to_string()),
        Shown::Text(Some(v)) => v.to_string(),
    };
    Some((col.meta.name.clone(), text))
}

impl GridState {
    /// Make the selected row resident (before reading it outside a render).
    pub fn load_selected(&mut self, rs: &mut ResultSet) {
        if let Err(e) = rs.rows.load(self.row..self.row + 1) {
            self.read_error = Some(e);
        }
    }
}

#[cfg(test)]
mod tests;
