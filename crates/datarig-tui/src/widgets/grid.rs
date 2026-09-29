//! Result grid: virtualized rows, column-wise horizontal scroll, CJK-safe cells. A
//! header shows the column's key marks (PK/FK/UQ) before its name. The
//! grid shows one page of the result at a time ([`GridState::page`]); row
//! numbers stay those of the whole result.

use crate::text::{Align, fit, sanitize_cell, width};
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

#[derive(Clone, Debug)]
pub struct Column {
    pub meta: ColumnMeta,
    pub width: usize,
}

/// A result: its columns and its rows ([`RowStore`]: a window in memory, the rest spilled).
#[derive(Debug)]
pub struct ResultSet {
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
        Self { columns, rows, more, counted: None, counted_now: false, pages_in: PagesIn::Nothing, tx: None }
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
    pub left: usize,
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
    /// Hit-test info from the last render: (x_start, x_end, column index), data origin y.
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

    /// Scroll the view `rows` rows and `cols` columns (the wheel): the selected cell
    /// stays on its data row, the view stops at the page's ends and fetches nothing.
    pub fn scroll(&mut self, rows: isize, cols: isize, total: usize, ncols: usize) {
        let w = self.window(total);
        let last_top = w.end.saturating_sub(self.page_rows.max(1)).max(w.start);
        self.top = (self.top as isize + rows).clamp(w.start as isize, last_top as isize) as usize;
        self.left = (self.left as isize + cols).clamp(0, ncols.saturating_sub(1) as isize) as usize;
        self.detached = true;
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
    buf.set_style(area, theme::base());
    st.hit_cols.clear();
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
    if st.detached {
        // Scrolled by the wheel: the view stays, within the page.
        st.top = st.top.min(win.end.saturating_sub(page_rows).max(win.start));
    } else if st.row < st.top {
        st.top = st.row;
    } else if st.row >= st.top + page_rows {
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
    // Keep the selected column fully visible (column-wise horizontal scroll).
    let span = |from: usize, to: usize| -> usize { widths[from..=to].iter().map(|w| w + 3).sum() };
    st.left = st.left.min(rs.columns.len() - 1);
    if !st.detached {
        if st.col < st.left {
            st.left = st.col;
        }
        while st.left < st.col && span(st.left, st.col) > avail {
            st.left += 1;
        }
    }

    let header_style = Style::new().bg(theme::SURFACE).fg(theme::FG).add_modifier(Modifier::BOLD);
    let type_style = Style::new().bg(theme::SURFACE).fg(theme::FG_MUTED);
    let sep_style = |bg| Style::new().fg(theme::BORDER).bg(bg);
    let y0 = area.y;
    buf.set_style(Rect::new(area.x, y0, area.width, 2), Style::new().bg(theme::SURFACE));
    st.data_y = y0 + 2;
    st.header_y = y0;
    st.gutter_x = (area.x, area.x + gutter as u16);

    let right = area.x + area.width;
    // Column layout: "│ " + cell + " "
    let mut x = area.x + gutter as u16;
    let mut layout: Vec<(u16, usize, usize)> = Vec::new(); // (x, col index, drawn width)
    for (ci, cw) in widths.iter().enumerate().skip(st.left) {
        if x + 2 >= right {
            break;
        }
        let w = (*cw).min((right - x - 2) as usize);
        layout.push((x, ci, w));
        st.hit_cols.push((x, (x + 3 + w as u16).min(right), ci));
        x += 3 + w as u16;
    }

    // A selected range (`v`, `V`, the gutter, a header), from its anchor to the cursor.
    let range = st.selection(total, rs.columns.len());
    for &(cx, ci, w) in &layout {
        let col = &rs.columns[ci];
        // Whole selected columns: their headers too.
        let whole = st.shape == Shape::Cols && range.as_ref().is_some_and(|(_, cr)| cr.contains(&ci));
        let (header_style, type_style) = if whole {
            (header_style.bg(theme::RANGE_BG), type_style.bg(theme::RANGE_BG))
        } else {
            (header_style, type_style)
        };
        if whole {
            buf.set_style(Rect::new(cx + 1, y0, (w + 2) as u16, 2), Style::new().bg(theme::RANGE_BG));
        }
        buf.set_stringn(cx, y0, "│", 1, sep_style(theme::SURFACE));
        buf.set_stringn(cx, y0 + 1, "│", 1, sep_style(theme::SURFACE));
        // The key marks in their colors, then the name.
        let mut hx = cx + 2;
        let mut left = w;
        for m in crate::icons::key_marks(look.marks.get(ci).copied().unwrap_or_default()) {
            let text = format!("{} ", m.text(look.icons));
            let mw = width(&text);
            if mw >= left {
                break;
            }
            buf.set_stringn(hx, y0, &text, mw, header_style.fg(theme::key_color(m)));
            hx += mw as u16;
            left -= mw;
        }
        buf.set_stringn(hx, y0, fit(&col.meta.name, left, Align::Left), left, header_style);
        buf.set_stringn(cx + 2, y0 + 1, fit(&col.meta.type_name, w, Align::Left), w, type_style);
    }

    for vy in 0..page_rows {
        let r = st.top + vy;
        if r >= win.end {
            break;
        }
        let y = st.data_y + vy as u16;
        let zebra = if r % 2 == 1 { theme::SURFACE_ALT } else { theme::BG };
        buf.set_style(Rect::new(area.x, y, area.width, 1), Style::new().bg(zebra));
        // Whole selected rows: their numbers too.
        let whole = st.shape == Shape::Rows && range.as_ref().is_some_and(|(rr, _)| rr.contains(&r));
        let num_bg = if whole { theme::RANGE_BG } else { zebra };
        let num_style = Style::new().bg(num_bg).fg(if r == st.row { theme::ACCENT } else { theme::FG_MUTED });
        buf.set_stringn(area.x, y, fit(&(r + 1).to_string(), gutter - 1, Align::Right), gutter - 1, num_style);
        for &(cx, ci, w) in &layout {
            let col = &rs.columns[ci];
            let selected = r == st.row && ci == st.col;
            let in_range = range.as_ref().is_some_and(|(rr, cr)| rr.contains(&r) && cr.contains(&ci));
            let bg = if selected {
                if focused { theme::SELECTION_BG } else { theme::CURSOR_LINE_BG }
            } else if in_range {
                theme::RANGE_BG
            } else {
                zebra
            };
            buf.set_stringn(cx, y, "│", 1, sep_style(zebra));
            let (text, is_null) = match rs.cell(r, ci) {
                CellRef::Here(c) => cell_text(c, &null_label),
                CellRef::NotRead => (not_read.to_string(), true),
                CellRef::Missing => cell_text(&None, &null_label),
            };
            let align = if col.meta.numeric && !is_null { Align::Right } else { Align::Left };
            let style = if is_null {
                Style::new().fg(theme::NULL_FG).add_modifier(Modifier::ITALIC)
            } else {
                Style::new().fg(theme::FG)
            }
            .bg(bg);
            let cell_w = (w + 2).min((right - cx - 1) as usize);
            buf.set_stringn(cx + 1, y, " ".repeat(cell_w), cell_w, Style::new().bg(bg));
            buf.set_stringn(cx + 2, y, fit(&text, w, align), w, style);
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
