//! The rows of one result: up to a window of rows
//! in memory, the rest in the result's spill file ([`spill`]), so a result paged to the end of
//! millions of rows keeps the process's memory flat.
//!
//! * A result with at most [`Limits::window`] rows stays in memory and writes nothing.
//! * Past that, every row is written to a spill file (the rows so far at once, then each page
//!   as it arrives) and memory keeps a window of `window` rows. Paging appends at the end, where
//!   the window follows it; [`RowStore::load`] moves the window to the rows the grid shows,
//!   reading ahead so the next rows are there before they are needed.
//! * The spill file stops growing at [`Limits::cap`] bytes: [`RowStore::append`] keeps the rows
//!   that fit and says it stopped ([`Appended::capped`]).
//! * A row that is not in memory is [`Row::NotRead`], never an empty row; a failed read is an
//!   error for the caller to show.

pub mod spill;

use crate::driver::Cell;
use crate::fault::Fault;
use spill::{SpillDir, SpillFile};
use std::ops::Range;
use std::sync::Arc;

/// Where rows may spill and how much.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Rows kept in memory (at least [`MIN_WINDOW`]).
    pub window: usize,
    /// The most bytes a result's spill file may take (`None`: no limit).
    pub cap: Option<u64>,
}

/// Default of [`Limits::window`] (config `result_window_rows`).
pub const WINDOW: usize = 10_000;
/// The smallest window the config accepts.
pub const MIN_WINDOW: usize = 1_000;
/// Default of [`Limits::cap`] (config and policy `spill_limit`): 1 GiB.
pub const CAP: u64 = 1 << 30;

impl Default for Limits {
    fn default() -> Self {
        Self { window: WINDOW, cap: Some(CAP) }
    }
}

/// A row of a [`RowStore`].
#[derive(Debug, PartialEq, Eq)]
pub enum Row<'a> {
    /// In memory.
    Here(&'a [Cell]),
    /// Fetched, but not in memory now (load it first, or the read failed): unknown, not empty.
    NotRead,
    /// Beyond the rows fetched.
    Missing,
}

/// What [`RowStore::append`] kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Appended {
    pub kept: usize,
    /// The spill file reached [`Limits::cap`]: the rows after `kept` were dropped, and no more
    /// should be fetched.
    pub capped: bool,
}

pub struct RowStore {
    len: usize,
    /// Rows `start..start + window.len()` are in memory.
    start: usize,
    window: Vec<Vec<Cell>>,
    spill: Option<SpillFile>,
    dir: Option<Arc<SpillDir>>,
    limits: Limits,
}

impl std::fmt::Debug for RowStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowStore")
            .field("len", &self.len)
            .field("window", &(self.start..self.start + self.window.len()))
            .field("spilled", &self.spill.as_ref().map(SpillFile::bytes))
            .finish()
    }
}

impl RowStore {
    /// The first page of a result. `dir`: where rows spill (`None`: all rows stay in memory).
    pub fn new(rows: Vec<Vec<Cell>>, dir: Option<Arc<SpillDir>>, limits: Limits) -> Self {
        let mut s = Self {
            len: 0,
            start: 0,
            window: Vec::new(),
            spill: None,
            dir,
            limits: Limits { window: limits.window.max(MIN_WINDOW), ..limits },
        };
        // A first page never spills on its own: pages are far smaller than the window.
        s.len = rows.len();
        s.window = rows;
        s
    }

    /// Rows in memory only, no spilling (tests, small results).
    pub fn in_memory(rows: Vec<Vec<Cell>>) -> Self {
        Self::new(rows, None, Limits { window: usize::MAX, cap: None })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Bytes in the spill file (0 while the rows are all in memory).
    pub fn spilled(&self) -> u64 {
        self.spill.as_ref().map_or(0, SpillFile::bytes)
    }

    /// The spill file, if the rows spilled.
    pub fn spill_path(&self) -> Option<&std::path::Path> {
        self.spill.as_ref().map(SpillFile::path)
    }

    /// Rows in memory now.
    pub fn resident(&self) -> Range<usize> {
        self.start..self.start + self.window.len()
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    pub fn row(&self, i: usize) -> Row<'_> {
        if i >= self.len {
            Row::Missing
        } else if i >= self.start && i < self.start + self.window.len() {
            Row::Here(&self.window[i - self.start])
        } else {
            Row::NotRead
        }
    }

    /// Add a page of rows. Nothing is added when writing the spill file fails (the error
    /// comes back); fetching should stop then, as on [`Appended::capped`].
    pub fn append(&mut self, rows: Vec<Vec<Cell>>) -> Result<Appended, Fault> {
        let n = rows.len();
        if self.spill.is_none() {
            let (fits, dir) = (self.len + n <= self.limits.window, self.dir.clone());
            let Some(dir) = dir.filter(|_| !fits) else {
                self.window.extend(rows);
                self.len += n;
                return Ok(Appended { kept: n, capped: false });
            };
            // Past the window: every row so far goes to the new spill file, then this page.
            let mut file = dir.create().map_err(|e| spill::create_fault(&e, dir.path()))?;
            let written = file.append(&self.window, self.limits.cap).map_err(|e| Fault::io_at(&e, file.path()))?;
            if written < self.window.len() {
                // The rows already shown do not fit under the limit: keep them in memory, and
                // fetch no more.
                return Ok(Appended { kept: 0, capped: true });
            }
            self.spill = Some(file);
        }
        let Some(file) = self.spill.as_mut() else { return Ok(Appended { kept: 0, capped: true }) };
        let kept = file.append(&rows, self.limits.cap).map_err(|e| Fault::io_at(&e, file.path()))?;
        let at_end = self.start + self.window.len() == self.len;
        self.len += kept;
        if at_end {
            self.window.extend(rows.into_iter().take(kept));
            let over = self.window.len().saturating_sub(self.limits.window);
            if over > 0 {
                self.window.drain(..over);
                self.start += over;
            }
        }
        Ok(Appended { kept, capped: kept < n })
    }

    /// Make rows `range` resident. The window moves to be centered on them when they come
    /// within an eighth of the window of its edge (read-ahead), so scrolling reads a large
    /// block now and then rather than a few rows on every step.
    pub fn load(&mut self, range: Range<usize>) -> Result<(), Fault> {
        let (a, b) = (range.start.min(self.len), range.end.min(self.len));
        if a >= b {
            return Ok(());
        }
        let Some(file) = &self.spill else { return Ok(()) };
        let w = self.limits.window;
        let (start, end) = (self.start, self.start + self.window.len());
        let margin = w / 8;
        let low = a < start || (a < start + margin && start > 0);
        let high = b > end || (b + margin > end && end < self.len);
        if !low && !high {
            return Ok(());
        }
        let center = a + (b - a) / 2;
        let ns = center.saturating_sub(w / 2).min(self.len.saturating_sub(w));
        let ne = (ns + w).min(self.len);
        // Keep the rows of the old window that the new one has, read the rest.
        let (ks, ke) = (ns.max(start), ne.min(end));
        let mut rows = Vec::with_capacity(ne - ns);
        if ks < ke {
            rows.extend(file.read(ns..ks).map_err(|e| Fault::io_at(&e, file.path()))?);
            let old = std::mem::take(&mut self.window);
            rows.extend(old.into_iter().skip(ks - start).take(ke - ks));
            rows.extend(file.read(ke..ne).map_err(|e| Fault::io_at(&e, file.path()))?);
        } else {
            rows = file.read(ns..ne).map_err(|e| Fault::io_at(&e, file.path()))?;
        }
        self.window = rows;
        self.start = ns;
        Ok(())
    }

    /// Call `f` with the rows of `range` in order, `chunk` rows at a time, reading the rows that
    /// are not in memory from the spill file without moving the window.
    pub fn for_each_chunk(
        &self,
        range: Range<usize>,
        chunk: usize,
        mut f: impl FnMut(&[Vec<Cell>]),
    ) -> Result<(), Fault> {
        let (mut a, b) = (range.start.min(self.len), range.end.min(self.len));
        let chunk = chunk.max(1);
        let res = self.resident();
        while a < b {
            if res.contains(&a) {
                let e = b.min(res.end).min(a + chunk);
                f(&self.window[a - res.start..e - res.start]);
                a = e;
            } else {
                // Up to the window (or the end), from the file.
                let limit = if a < res.start { res.start.min(b) } else { b };
                let e = limit.min(a + chunk);
                let file =
                    self.spill.as_ref().ok_or_else(|| Fault::other("rows outside memory without a spill file"))?;
                let rows = file.read(a..e).map_err(|err| Fault::io_at(&err, file.path()))?;
                if rows.len() != e - a {
                    return Err(Fault::other("spill file: fewer rows than written"));
                }
                f(&rows);
                a = e;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
