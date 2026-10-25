//! An Excel workbook (`.xlsx`) written as a stream ([`XlsxWriter`], a [`RowSink`]):
//! rust_xlsxwriter in its constant memory mode. Each row goes to the sheet's temporary file as it
//! comes (in the system's temporary folder, `TMPDIR`; the file has no name and goes away with
//! the writer), so memory stays at one row; the workbook itself is made when the writer ends
//! ([`RowSink::end`]), zipping those files into the output. Until then the output is empty.
//!
//! The first row of every sheet holds the column names, bold, and stays in view (frozen). A sheet
//! holds 1,048,576 rows with that header: the rows that follow go to `Sheet2`, `Sheet3`, …, each
//! with the header again. A workbook holds 16,384 columns at most ([`XlsxWriter::new`] refuses
//! more) and a cell 32,767 characters as Excel counts them (UTF-16 units: a character outside the
//! Basic Multilingual Plane, an emoji, counts two): a longer value fails the export ([`TooLong`])
//! and the rows before it make the workbook.
//!
//! The workbook is a ZIP64 container: a sheet of more than 4 GiB of XML, or a workbook of more than
//! 4 GiB, is made like any other (without ZIP64 the library fails at the end, when every row was
//! written already). A stop asked while the workbook is made ([`XlsxWriter::end_unless`]) drops it.
//!
//! The cells:
//! * a number column's value is a number when Excel holds it exactly: at most 15 significant
//!   digits (what Excel keeps and shows), within a double's normal range. Any other value (a
//!   `bigint` past 15 digits, a `numeric` with more digits, `NaN`, `Infinity`) is text, every
//!   digit kept;
//! * a boolean is a boolean (`TRUE`/`FALSE`);
//! * NULL is an empty cell, or the NULL text when one is given (an empty string is an empty cell
//!   too: the NULL text tells them apart);
//! * every other value is text as the driver gives it: text, JSON, UUIDs, arrays, bytes (as a
//!   copy writes them, `\x0102`), and dates, times and timestamps (Excel's dates have no time
//!   zone and a range of their own, so they stay the text the database wrote).

use super::stream::RowSink;
use super::{Column, Kind, Row};
use crate::panics;
use rust_xlsxwriter::{Format, Workbook, Worksheet, XlsxError};
use std::fmt;
use std::io::{self, Write};

/// The rows of a sheet, its header among them.
pub const MAX_ROWS: u32 = 1_048_576;

/// The columns of a sheet.
pub const MAX_COLUMNS: usize = 16_384;

/// The characters of a cell, as Excel counts them: UTF-16 units.
pub const MAX_CELL_CHARS: usize = 32_767;

/// The significant digits of a number Excel holds and shows as it is.
const EXCEL_DIGITS: usize = 15;

/// A result too wide for a sheet: its number of columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooWide {
    pub columns: usize,
}

/// A value too long for a cell: its column, its row (the first row of the result is 1), its
/// length in characters as Excel counts them (UTF-16 units). It is the error inside the [`io::Error`] that [`XlsxWriter`]'s
/// [`RowSink::rows`] fails with ([`TooLong::of`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TooLong {
    pub column: String,
    pub row: u64,
    pub chars: usize,
}

impl fmt::Display for TooLong {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a value of column {} (row {}) has {} characters; a cell holds {MAX_CELL_CHARS}",
            self.column, self.row, self.chars
        )
    }
}

impl std::error::Error for TooLong {}

impl TooLong {
    /// The value too long in `e`, if that is what it is.
    pub fn of(e: &io::Error) -> Option<&TooLong> {
        e.get_ref().and_then(|inner| inner.downcast_ref::<TooLong>())
    }
}

/// The workbook was dropped while it was made: [`XlsxWriter::end_unless`] was told to stop. It is
/// the error inside the [`io::Error`] that it fails with ([`Dropped::of`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dropped;

impl fmt::Display for Dropped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the workbook was dropped before it was made")
    }
}

impl std::error::Error for Dropped {}

impl Dropped {
    /// Whether `e` says the workbook was dropped.
    pub fn of(e: &io::Error) -> bool {
        e.get_ref().is_some_and(|inner| inner.is::<Dropped>())
    }
}

/// The output while the workbook is zipped into it. A write fails once `stop` says so, and once
/// the output failed; that failure ends the save, and what is written after it (the zip writer
/// finishing the archive as it is dropped) goes nowhere: a zip writer that cannot finish prints
/// so on stderr, over the screen.
struct Stoppable<'s, W> {
    out: &'s mut W,
    stop: &'s mut (dyn FnMut() -> bool + Send),
    stopped: bool,
    failed: bool,
}

impl<W: Write> Write for Stoppable<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.failed {
            return Ok(buf.len());
        }
        if (self.stop)() {
            (self.stopped, self.failed) = (true, true);
            return Err(io::Error::other(Dropped));
        }
        self.out.write(buf).inspect_err(|_| self.failed = true)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.failed {
            return Ok(());
        }
        self.out.flush().inspect_err(|_| self.failed = true)
    }
}

/// [`XlsxError`] as an I/O error: its own when it has one.
fn io_error(e: XlsxError) -> io::Error {
    match e {
        XlsxError::IoError(e) => e,
        e => io::Error::other(e.to_string()),
    }
}

/// What a panic of the library leaves: the workbook cannot be trusted any more.
fn broken() -> io::Error {
    io::Error::other("the workbook could not be written (its temporary file failed)")
}

/// A number column's value as the number Excel holds, when it holds it exactly: plain decimal
/// text (a sign, digits, a point, an exponent) of at most [`EXCEL_DIGITS`] significant digits,
/// whose double is finite and not below the normal range.
fn excel_number(v: &str) -> Option<f64> {
    let body = v.strip_prefix('-').unwrap_or(v);
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(i) => (&body[..i], Some(&body[i + 1..])),
        None => (body, None),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if (int.is_empty() && frac.is_empty()) || !digits(int) || !digits(frac) {
        return None;
    }
    if let Some(e) = exponent {
        let e = e.strip_prefix(['-', '+']).unwrap_or(e);
        if e.is_empty() || !digits(e) {
            return None;
        }
    }
    let all: String = int.chars().chain(frac.chars()).collect();
    let significant = all.trim_start_matches('0').trim_end_matches('0').len();
    if significant > EXCEL_DIGITS {
        return None;
    }
    let n: f64 = v.parse().ok()?;
    // A value that is not zero must not become zero, or lose digits below the normal range.
    let ok = n.is_finite() && (significant == 0 || n.abs() >= f64::MIN_POSITIVE);
    ok.then_some(n)
}

/// A boolean column's value as a boolean.
fn excel_bool(v: &str) -> Option<bool> {
    match v {
        "true" | "t" => Some(true),
        "false" | "f" => Some(false),
        _ => None,
    }
}

/// An Excel workbook written a row at a time into `out` ([`RowSink`]); the module says how.
pub struct XlsxWriter<'a, W: Write + Send> {
    columns: &'a [Column<'a>],
    out: Option<W>,
    workbook: Workbook,
    bold: Format,
    /// What NULL is written as: nothing (an empty cell) when empty.
    null: String,
    /// The rows of a sheet, its header among them.
    max_rows: u32,
    sheets: u32,
    /// The next row of the current sheet.
    sheet_row: u32,
    rows: u64,
    bytes: u64,
    /// The library panicked: nothing more is written, and the workbook is not made.
    broken: bool,
    ended: bool,
}

impl<'a, W: Write + Send> XlsxWriter<'a, W> {
    /// A writer of `columns` into `out`; more columns than a sheet holds are refused.
    pub fn new(columns: &'a [Column<'a>], out: W) -> Result<Self, TooWide> {
        if columns.len() > MAX_COLUMNS {
            return Err(TooWide { columns: columns.len() });
        }
        let mut workbook = Workbook::new();
        workbook.use_zip_large_file(true);
        Ok(Self {
            columns,
            out: Some(out),
            workbook,
            bold: Format::new().set_bold(),
            null: String::new(),
            max_rows: MAX_ROWS,
            sheets: 0,
            sheet_row: 0,
            rows: 0,
            bytes: 0,
            broken: false,
            ended: false,
        })
    }

    /// NULL written as `text` (an empty cell when it is empty).
    pub fn null_text(mut self, text: &str) -> Self {
        self.null = text.to_string();
        self
    }

    /// Sheets of `rows` rows (the header among them), at least 2, at most [`MAX_ROWS`]: a test
    /// sees the rows go on to the next sheet without writing a million of them.
    pub fn sheet_rows(mut self, rows: u32) -> Self {
        self.max_rows = rows.clamp(2, MAX_ROWS);
        self
    }

    /// The sheets begun so far.
    pub fn sheets(&self) -> u32 {
        self.sheets
    }

    /// The output, once the writer is done with it (`None` once it failed to be given back).
    pub fn into_inner(self) -> Option<W> {
        self.out
    }

    /// Run `f` on the workbook, a panic of the library caught: the writer is then broken.
    fn guarded<R>(&mut self, f: impl FnOnce(&mut Self) -> Result<R, XlsxError>) -> io::Result<R> {
        if self.broken {
            return Err(broken());
        }
        match panics::catch(|| f(self)) {
            Ok(r) => r.map_err(io_error),
            Err(_) => {
                self.broken = true;
                Err(broken())
            }
        }
    }

    /// A new sheet, its header row written.
    fn next_sheet(&mut self) -> io::Result<()> {
        self.guarded(|w| {
            let sheet = w.workbook.add_worksheet_with_constant_memory()?;
            for (c, col) in w.columns.iter().enumerate() {
                sheet.write_string_with_format(0, c as u16, col.name, &w.bold)?;
            }
            sheet.set_freeze_panes(1, 0)?;
            Ok(())
        })?;
        self.sheets += 1;
        self.sheet_row = 1;
        Ok(())
    }

    /// The current sheet.
    fn sheet(workbook: &mut Workbook, sheets: u32) -> Result<&mut Worksheet, XlsxError> {
        workbook.worksheet_from_index(sheets as usize - 1)
    }

    /// Fail with the first value of `row` (the `n`th of the result) too long for a cell.
    fn check(&self, row: &Row, n: u64) -> io::Result<()> {
        for (v, c) in row.iter().zip(self.columns) {
            let v = v.unwrap_or(self.null.as_str());
            // Bytes first: a value of fewer bytes has fewer UTF-16 units.
            if v.len() > MAX_CELL_CHARS {
                let chars = v.encode_utf16().count();
                if chars > MAX_CELL_CHARS {
                    return Err(io::Error::other(TooLong { column: c.name.to_string(), row: n, chars }));
                }
            }
        }
        Ok(())
    }

    /// Make the workbook of the rows so far into the output ([`RowSink::end`]), unless `stop`
    /// says to stop while it is made (it is asked before each write to the output): the workbook
    /// is then dropped, the output holds part of it (not a workbook), and the error is
    /// [`Dropped`].
    pub fn end_unless(&mut self, stop: &mut (dyn FnMut() -> bool + Send)) -> io::Result<()> {
        if self.ended {
            return Ok(());
        }
        self.ended = true;
        if self.sheets == 0 && !self.broken {
            self.next_sheet()?;
        }
        let Some(mut out) = self.out.take() else { return Err(broken()) };
        let mut to = Stoppable { out: &mut out, stop, stopped: false, failed: false };
        let saved = self.guarded(|w| w.workbook.save_to_writer(&mut to));
        let (stopped, failed) = (to.stopped, to.failed);
        self.out = Some(out);
        if stopped {
            // Whatever error the library made of the failed write: the stop is what happened.
            return Err(io::Error::other(Dropped));
        }
        saved?;
        if failed {
            // The library went on past a failed write.
            return Err(io::Error::other("the workbook could not be written to its file"));
        }
        if let Some(out) = self.out.as_mut() {
            out.flush()?;
        }
        Ok(())
    }
}

impl<W: Write + Send> RowSink for XlsxWriter<'_, W> {
    fn begin(&mut self) -> io::Result<()> {
        self.next_sheet()
    }

    fn rows(&mut self, rows: &[Row]) -> io::Result<()> {
        for row in rows {
            self.check(row, self.rows + 1)?;
            if self.sheet_row >= self.max_rows || self.sheets == 0 {
                self.next_sheet()?;
            }
            let (r, sheets) = (self.sheet_row, self.sheets);
            let mut bytes = 0;
            self.guarded(|w| {
                let sheet = Self::sheet(&mut w.workbook, sheets)?;
                for (c, (v, col)) in row.iter().zip(w.columns).enumerate() {
                    let c = c as u16;
                    let Some(v) = v else {
                        if !w.null.is_empty() {
                            bytes += w.null.len() as u64;
                            sheet.write_string(r, c, &w.null)?;
                        }
                        continue;
                    };
                    bytes += v.len() as u64;
                    let number = if col.kind == Kind::Number { excel_number(v) } else { None };
                    let boolean = if col.kind == Kind::Bool { excel_bool(v) } else { None };
                    match (number, boolean) {
                        (Some(n), _) => sheet.write_number(r, c, n)?,
                        (_, Some(b)) => sheet.write_boolean(r, c, b)?,
                        _ => sheet.write_string(r, c, *v)?,
                    };
                }
                Ok(())
            })?;
            self.sheet_row += 1;
            self.rows += 1;
            self.bytes += bytes;
        }
        Ok(())
    }

    /// Make the workbook of the rows so far into the output. A workbook the library could not
    /// write ([`Self::rows`] failed in it) is not made: the output is then not a workbook.
    fn end(&mut self) -> io::Result<()> {
        self.end_unless(&mut || false)
    }

    fn rows_written(&self) -> u64 {
        self.rows
    }

    /// The bytes of the values written so far (the workbook is made, compressed, at the end).
    fn bytes(&self) -> u64 {
        self.bytes
    }
}

#[cfg(any(test, feature = "test-util"))]
pub mod read;
#[cfg(test)]
mod tests;
