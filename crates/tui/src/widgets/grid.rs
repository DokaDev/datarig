//! Result grid: virtualized rows, horizontal scroll by character (the columns at the view's
//! edges partly shown), CJK-safe cells. A
//! header shows the column's key marks (PK/FK/UQ) before its name. The
//! grid shows one page of the result at a time ([`GridState::page`]); row
//! numbers stay those of the whole result.

use crate::text::{Align, fit, sanitize_cell, slice_cols, width};
use crate::theme;
use datarig_core::driver::{Cell, ColumnMeta, KeyMarks};
use datarig_core::fault::Fault;
use datarig_core::i18n::{I18n, Label};
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

    /// The user's transaction `epoch` ended (`rolled_back` when the app knows it rolled back).
    pub fn tx_ended(&mut self, epoch: u64, rolled_back: bool) {
        if self.tx == Some(TxMark::InTx(epoch)) {
            self.tx = Some(if rolled_back { TxMark::RolledBack } else { TxMark::Ended });
        }
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

    /// The rows the grid shows of a result of `total` rows: its page (the last one when the
    /// page is past the end).
    pub fn window(&self, total: usize) -> std::ops::Range<usize> {
        if self.page_size == 0 || total == 0 {
            return 0..total;
        }
        let last = (total - 1) / self.page_size;
        let start = self.page.min(last) * self.page_size;
        start..(start + self.page_size).min(total)
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
    /// than the view from its start. A column already whole on screen moves nothing.
    fn follow(&mut self) {
        let (Some(&start), Some(&end)) = (self.col_x.get(self.col), self.col_x.get(self.col + 1)) else { return };
        let view = self.view_w as usize;
        if start < self.x_off || end - start > view {
            self.x_off = start;
        } else if end > self.x_off + view {
            self.x_off = end - view;
        }
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

/// How the grid draws: the column headers' key marks (by column, empty: none) and whether they
/// are Nerd Font glyphs.
#[derive(Clone, Copy, Default)]
pub struct Look<'a> {
    pub marks: &'a [KeyMarks],
    pub icons: bool,
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
    if area.height < 3 || area.width < 8 || rs.columns.is_empty() {
        return;
    }
    let total = rs.rows.len();
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

    let gutter = win.end.max(1).to_string().len().max(2) + 1;
    let avail = (area.width as usize).saturating_sub(gutter);
    // A column is as wide as its values, or its header with the key marks before the name.
    let marks: Vec<String> = (0..rs.columns.len())
        .map(|i| look.marks.get(i).map(|m| crate::icons::key_marks_text(*m, look.icons)).unwrap_or_default())
        .collect();
    let widths: Vec<usize> = rs
        .columns
        .iter()
        .zip(&marks)
        .map(|(c, m)| c.width.max(width(m) + width(&c.meta.name)).min(MAX_COL_WIDTH))
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
        let to = (x + 3 + w as i32).min(i32::from(right));
        st.hit_cols.push((x.max(i32::from(from)) as u16, to as u16, ci));
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

    // A selected range (`v`, `V`, the gutter, a header), from its anchor to the cursor.
    let range = st.selection(total, rs.columns.len());
    for &(cx, ci, w) in &layout {
        let col = &rs.columns[ci];
        // Whole selected columns: their headers too.
        let whole = st.shape == Shape::Cols && range.as_ref().is_some_and(|(_, cr)| cr.contains(&ci));
        let (header_style, type_style) =
            if whole { (header_style.patch(th.range), type_style.patch(th.range)) } else { (header_style, type_style) };
        if whole {
            let (a, b) = ((cx + 1).max(i32::from(from)), (cx + 3 + w as i32).min(i32::from(right)));
            buf.set_style(Rect::new(a as u16, y0, (b - a).max(0) as u16, 2), th.range);
        }
        put(buf, cx, y0, "│", sep_style(th.surface));
        put(buf, cx, y0 + 1, "│", sep_style(th.surface));
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
        let zebra = if r % 2 == 1 { th.surface_alt } else { th.bg };
        buf.set_style(Rect::new(area.x, y, area.width, 1), Style::new().bg(zebra));
        // Whole selected rows: their numbers too.
        let whole = st.shape == Shape::Rows && range.as_ref().is_some_and(|(rr, _)| rr.contains(&r));
        let num_bg = if whole { th.range } else { Style::new().bg(zebra) };
        let num_style = num_bg.fg(if r == st.row { th.accent } else { th.fg_muted });
        buf.set_stringn(area.x, y, fit(&(r + 1).to_string(), gutter - 1, Align::Right), gutter - 1, num_style);
        for &(cx, ci, w) in &layout {
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
            put(buf, cx, y, "│", sep_style(zebra));
            let (text, is_null) = match rs.cell(r, ci) {
                CellRef::Here(c) => cell_text(c, &null_label),
                CellRef::NotRead => (not_read.to_string(), true),
                CellRef::Missing => cell_text(&None, &null_label),
            };
            let align = if col.meta.numeric && !is_null { Align::Right } else { Align::Left };
            let style = if is_null {
                Style::new().fg(th.null_fg).add_modifier(Modifier::ITALIC)
            } else {
                Style::new().fg(th.fg)
            }
            .patch(bg);
            put(buf, cx + 1, y, &" ".repeat(w + 2), bg);
            put(buf, cx + 2, y, &fit(&text, w, align), style);
        }
    }
}

/// Full value for the cell viewer; json/jsonb pretty-printed. A cell whose row is not in memory
/// is `not_read`.
pub fn viewer_text(rs: &ResultSet, st: &GridState, null_label: &str, not_read: &str) -> Option<(String, String)> {
    let col = rs.columns.get(st.col)?;
    let cell = match rs.cell(st.row, st.col) {
        CellRef::Here(c) => c,
        CellRef::NotRead => return Some((col.meta.name.clone(), not_read.to_string())),
        CellRef::Missing => return None,
    };
    let text = match cell {
        None => null_label.to_string(),
        Some(v) if col.meta.json => serde_json::from_str::<serde_json::Value>(v)
            .ok()
            .and_then(|j| serde_json::to_string_pretty(&j).ok())
            .unwrap_or_else(|| v.clone()),
        Some(v) => v.clone(),
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
