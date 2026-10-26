//! A Parquet file written as a stream ([`ParquetWriter`], a [`RowSink`]) with the file format's
//! own column writers (no Arrow), compressed with Snappy. The rows are kept a row group at a
//! time, column by column, and written out when the group is full ([`GROUP_ROWS`] rows or
//! [`GROUP_BYTES`] of values), so memory stays at one row group; the footer that makes the file
//! readable is written when the writer ends ([`RowSink::end`]), also after a cancel or a value
//! that does not fit: the file then has the rows before it.
//!
//! Every column is OPTIONAL (NULL is a missing value, whatever NULL text is given). Its type
//! follows the result's column, decided before the first row ([`Physical::of`]):
//! * an integer is INT32 when its type has at most 32 bits (`int2`, `int4`, MySQL's `tinyint`,
//!   `smallint`, `mediumint`, `int`, `year`, and their unsigned forms but `int unsigned`), else
//!   INT64; MySQL's `bigint unsigned` is INT64 marked unsigned (64-bit);
//! * a float is FLOAT for a 32-bit type (`float4`, MySQL's `float`), else DOUBLE (`NaN` and the
//!   infinities as they are);
//! * a column of a PostgreSQL domain is known by the domain's name only, so a domain over an
//!   integer is INT64 and one over a float DOUBLE, whatever its base type (wider, never cut);
//! * a boolean is BOOLEAN;
//! * a date is INT32 DATE; a time of day without a zone INT64 TIME (microseconds); a timestamp
//!   INT64 TIMESTAMP (microseconds), adjusted to UTC for PostgreSQL's `timestamptz` (its offset
//!   applied) and not otherwise (a MySQL `TIMESTAMP` is the time shown in the session's zone);
//! * binary data is BYTE_ARRAY of its bytes (the text `\x0102` or `0x0102` decoded);
//! * everything else is BYTE_ARRAY UTF8 (String), the text the driver gives: text, JSON, UUIDs,
//!   arrays, intervals, bits, a time with a zone, and decimals, every digit kept (a result does
//!   not say a decimal's precision and scale, so a DECIMAL type could not be chosen up front).
//!
//! A value its column's type cannot hold (PostgreSQL's `infinity` dates and timestamps, MySQL's
//! zero date, `24:00:00`, a timestamp past what a 64-bit count of microseconds holds, bytes not in
//! hex) is never written as NULL nor as another type: the export fails naming its column and row
//! ([`BadValue`]), and the file keeps the rows before it. Dates and timestamps BC are held.
//!
//! A row group that fails to be written (a full disk) leaves a file that cannot be finished: every
//! later call fails with that first error (its kind and its text), [`RowSink::end`] included.

use super::stream::RowSink;
use crate::driver::ValueKind;
use parquet::basic::{Compression, LogicalType, Repetition, TimeUnit, Type as PhysicalType};
use parquet::basic::{IntType, TimeType, TimestampType};
use parquet::data_type::{BoolType, ByteArray, ByteArrayType, DoubleType, FloatType, Int32Type, Int64Type};
use parquet::errors::ParquetError;
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::types::Type;
use std::fmt;
use std::io::{self, Write};
use std::sync::Arc;

/// The rows of a row group at most.
pub const GROUP_ROWS: usize = 64 * 1024;

/// The bytes of the values a row group keeps in memory at most (as written: 4 for an INT32, the
/// text of a string).
pub const GROUP_BYTES: usize = 32 * 1024 * 1024;

/// The characters of a value a [`BadValue`] shows.
const SHOWN_CHARS: usize = 64;

/// A column to write: its name, the kind of value it holds and its type as the server names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParquetColumn<'a> {
    pub name: &'a str,
    pub kind: ValueKind,
    pub type_name: &'a str,
}

/// How a column is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Physical {
    Int32,
    Int64,
    /// INT64 marked unsigned: the bits of a `u64`.
    UInt64,
    Float,
    Double,
    Bool,
    Date,
    Time,
    Timestamp {
        utc: bool,
    },
    Bytes,
    Text,
}

impl Physical {
    /// How a column of `kind` and type `type_name` is written (the module says why).
    pub fn of(kind: ValueKind, type_name: &str) -> Physical {
        let name = type_name.to_ascii_lowercase();
        match kind {
            ValueKind::Integer => match name.as_str() {
                "bigint unsigned" => Physical::UInt64,
                "int unsigned" | "integer unsigned" => Physical::Int64,
                n if matches!(
                    n.strip_suffix(" unsigned").unwrap_or(n),
                    "int2" | "int4" | "smallint" | "int" | "integer" | "mediumint" | "tinyint" | "tinyint(1)" | "year"
                ) =>
                {
                    Physical::Int32
                }
                _ => Physical::Int64,
            },
            ValueKind::Float => match name.strip_suffix(" unsigned").unwrap_or(&name) {
                "float4" | "float" | "real" => Physical::Float,
                _ => Physical::Double,
            },
            ValueKind::Bool => Physical::Bool,
            ValueKind::Date => Physical::Date,
            ValueKind::Time if name == "timetz" || name.contains("with time zone") => Physical::Text,
            ValueKind::Time => Physical::Time,
            ValueKind::Timestamp => Physical::Timestamp { utc: false },
            ValueKind::TimestampTz => {
                Physical::Timestamp { utc: name == "timestamptz" || name.contains("with time zone") }
            }
            ValueKind::Bytes => Physical::Bytes,
            ValueKind::Text
            | ValueKind::Decimal
            | ValueKind::Json
            | ValueKind::Bit
            | ValueKind::Interval
            | ValueKind::Array(_)
            | ValueKind::Other => Physical::Text,
        }
    }

    /// The column's Parquet type, as a message names it.
    pub fn name(self) -> &'static str {
        match self {
            Physical::Int32 => "INT32",
            Physical::Int64 => "INT64",
            Physical::UInt64 => "INT64 (unsigned)",
            Physical::Float => "FLOAT",
            Physical::Double => "DOUBLE",
            Physical::Bool => "BOOLEAN",
            Physical::Date => "DATE",
            Physical::Time => "TIME",
            Physical::Timestamp { utc: true } => "TIMESTAMP (UTC)",
            Physical::Timestamp { utc: false } => "TIMESTAMP",
            Physical::Bytes => "BYTE_ARRAY",
            Physical::Text => "STRING",
        }
    }

    /// The schema field of a column named `name`.
    fn field(self, name: &str) -> Result<Type, ParquetError> {
        let (physical, logical) = match self {
            Physical::Int32 => (PhysicalType::INT32, None),
            Physical::Int64 => (PhysicalType::INT64, None),
            Physical::UInt64 => {
                (PhysicalType::INT64, Some(LogicalType::Integer(IntType { bit_width: 64, is_signed: false })))
            }
            Physical::Float => (PhysicalType::FLOAT, None),
            Physical::Double => (PhysicalType::DOUBLE, None),
            Physical::Bool => (PhysicalType::BOOLEAN, None),
            Physical::Date => (PhysicalType::INT32, Some(LogicalType::Date)),
            Physical::Time => (
                PhysicalType::INT64,
                Some(LogicalType::Time(TimeType { is_adjusted_to_u_t_c: false, unit: TimeUnit::MICROS })),
            ),
            Physical::Timestamp { utc } => (
                PhysicalType::INT64,
                Some(LogicalType::Timestamp(TimestampType { is_adjusted_to_u_t_c: utc, unit: TimeUnit::MICROS })),
            ),
            Physical::Bytes => (PhysicalType::BYTE_ARRAY, None),
            Physical::Text => (PhysicalType::BYTE_ARRAY, Some(LogicalType::String)),
        };
        Type::primitive_type_builder(name, physical)
            .with_repetition(Repetition::OPTIONAL)
            .with_logical_type(logical)
            .build()
    }
}

/// A value its column's type cannot hold: its column, its row (the first row of the result is 1),
/// the value (its first characters) and the type. It is the error inside the [`io::Error`] that
/// [`ParquetWriter`]'s [`RowSink::rows`] fails with ([`BadValue::of`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadValue {
    pub column: String,
    pub row: u64,
    pub value: String,
    pub parquet_type: &'static str,
}

impl fmt::Display for BadValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a value of column {} (row {}), {}, does not fit its Parquet type {}",
            self.column, self.row, self.value, self.parquet_type
        )
    }
}

impl std::error::Error for BadValue {}

impl BadValue {
    /// The value that does not fit in `e`, if that is what it is.
    pub fn of(e: &io::Error) -> Option<&BadValue> {
        e.get_ref().and_then(|inner| inner.downcast_ref::<BadValue>())
    }
}

/// [`ParquetError`] as an I/O error: its own when it has one.
fn io_error(e: ParquetError) -> io::Error {
    match e {
        ParquetError::External(e) => match e.downcast::<io::Error>() {
            Ok(e) => *e,
            Err(e) => io::Error::other(e.to_string()),
        },
        e => io::Error::other(e.to_string()),
    }
}

/// What a failure to write a row group (its kind and text) leaves: the file cannot be finished.
fn broken(kind: io::ErrorKind, text: &str) -> io::Error {
    io::Error::new(kind, format!("the Parquet file could not be written (a row group failed: {text})"))
}

/// The days from 1970-01-01 to the proleptic Gregorian date `y-m-d` (a valid date).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

/// `n` digits of `s` at its start as a number, and the rest.
fn digits(s: &str, n: usize) -> Option<(u32, &str)> {
    let head = s.get(..n)?;
    if !head.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((head.parse().ok()?, &s[n..]))
}

/// A date `YYYY-MM-DD` (four digits of year or more) at the start of `s`, as days since
/// 1970-01-01 (before a `BC` the caller has taken off: `bc`), and the rest.
fn date_days(s: &str, bc: bool) -> Option<(i64, &str)> {
    let year_len = s.bytes().take_while(u8::is_ascii_digit).count();
    if year_len < 4 {
        return None;
    }
    let year: i64 = s[..year_len].parse().ok()?;
    let s = s[year_len..].strip_prefix('-')?;
    let (month, s) = digits(s, 2)?;
    let (day, s) = digits(s.strip_prefix('-')?, 2)?;
    // 1 BC is year 0 of the proleptic calendar.
    let year = if bc { 1 - year } else { year };
    if bc && year > 0 {
        return None;
    }
    let days_in = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap(year) => 29,
        2 => 28,
        _ => return None,
    };
    if day == 0 || day > days_in {
        return None;
    }
    Some((days_from_civil(year, month, day), s))
}

/// A time of day `HH:MM:SS[.ffffff]` at the start of `s` as microseconds since midnight (below a
/// day), and the rest.
fn time_micros(s: &str) -> Option<(i64, &str)> {
    let (h, s) = digits(s, 2)?;
    let (m, s) = digits(s.strip_prefix(':')?, 2)?;
    let (sec, mut s) = digits(s.strip_prefix(':')?, 2)?;
    if h > 23 || m > 59 || sec > 59 {
        return None;
    }
    let mut micros = 0i64;
    if let Some(frac) = s.strip_prefix('.') {
        let n = frac.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 || n > 6 {
            return None;
        }
        micros = frac[..n].parse::<i64>().ok()? * 10i64.pow(6 - n as u32);
        s = &frac[n..];
    }
    Some(((i64::from(h) * 3600 + i64::from(m) * 60 + i64::from(sec)) * 1_000_000 + micros, s))
}

/// A UTC offset `+HH[:MM[:SS]]` (or `-`) at the start of `s` in seconds, and the rest.
fn offset_secs(s: &str) -> Option<(i64, &str)> {
    let (sign, s) = match s.as_bytes().first()? {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => return None,
    };
    let (h, mut s) = digits(s, 2)?;
    let mut secs = i64::from(h) * 3600;
    for unit in [60, 1] {
        match s.strip_prefix(':') {
            Some(rest) => {
                let (n, rest) = digits(rest, 2)?;
                if n > 59 {
                    return None;
                }
                secs += i64::from(n) * unit;
                s = rest;
            }
            None => break,
        }
    }
    Some((sign * secs, s))
}

/// `s` without a trailing ` BC`, and whether it had one.
fn era(s: &str) -> (&str, bool) {
    match s.strip_suffix(" BC") {
        Some(s) => (s, true),
        None => (s, false),
    }
}

/// A date as days since 1970-01-01.
fn parse_date(v: &str) -> Option<i32> {
    let (v, bc) = era(v);
    let (days, rest) = date_days(v, bc)?;
    if !rest.is_empty() {
        return None;
    }
    i32::try_from(days).ok()
}

/// A time of day as microseconds since midnight.
fn parse_time(v: &str) -> Option<i64> {
    match time_micros(v)? {
        (t, "") => Some(t),
        _ => None,
    }
}

/// A timestamp `date time` as microseconds since 1970-01-01 00:00: with an offset (`utc`, made
/// UTC) or without one.
fn parse_timestamp(v: &str, utc: bool) -> Option<i64> {
    let (v, bc) = era(v);
    let (days, rest) = date_days(v, bc)?;
    let (time, rest) = time_micros(rest.strip_prefix([' ', 'T'])?)?;
    let offset = match (utc, rest) {
        (false, "") => 0,
        (true, rest) => match offset_secs(rest)? {
            (o, "") => o,
            _ => return None,
        },
        (false, _) => return None,
    };
    days.checked_mul(86_400_000_000)?.checked_add(time)?.checked_sub(offset.checked_mul(1_000_000)?)
}

/// Binary data as its text gives it: `\x` (PostgreSQL) or `0x` (MySQL) then hex digits.
fn parse_bytes(v: &str) -> Option<Vec<u8>> {
    let hex = v.strip_prefix("\\x").or_else(|| v.strip_prefix("0x")).or_else(|| v.strip_prefix("0X"))?;
    if hex.len() % 2 != 0 {
        return None;
    }
    let nibble = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    hex.as_bytes().chunks(2).map(|p| Some(nibble(p[0])? << 4 | nibble(p[1])?)).collect()
}

fn parse_bool(v: &str) -> Option<bool> {
    match v {
        "t" | "true" => Some(true),
        "f" | "false" => Some(false),
        _ => None,
    }
}

/// A parsed value of a row, before it is kept.
enum Value<'v> {
    Null,
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    Bool(bool),
    Bytes(Vec<u8>),
    Text(&'v str),
}

/// The values of a column kept for the row group being made: the values that are not NULL, and a
/// definition level per row (1 a value, 0 NULL).
struct Buffer {
    defs: Vec<i16>,
    values: Values,
}

enum Values {
    I32(Vec<i32>),
    I64(Vec<i64>),
    F32(Vec<f32>),
    F64(Vec<f64>),
    Bool(Vec<bool>),
    /// Byte strings back to back, and where each ends.
    Bytes(Vec<u8>, Vec<usize>),
}

impl Buffer {
    fn new(p: Physical) -> Self {
        let values = match p {
            Physical::Int32 | Physical::Date => Values::I32(Vec::new()),
            Physical::Int64 | Physical::UInt64 | Physical::Time | Physical::Timestamp { .. } => Values::I64(Vec::new()),
            Physical::Float => Values::F32(Vec::new()),
            Physical::Double => Values::F64(Vec::new()),
            Physical::Bool => Values::Bool(Vec::new()),
            Physical::Bytes | Physical::Text => Values::Bytes(Vec::new(), Vec::new()),
        };
        Self { defs: Vec::new(), values }
    }

    /// Keep `v` (of this column's type); the bytes it takes.
    fn push(&mut self, v: Value<'_>) -> usize {
        let mut size = 2;
        match (&mut self.values, v) {
            (_, Value::Null) => {
                self.defs.push(0);
                return size;
            }
            (Values::I32(vs), Value::I32(n)) => {
                vs.push(n);
                size += 4;
            }
            (Values::I64(vs), Value::I64(n)) => {
                vs.push(n);
                size += 8;
            }
            (Values::F32(vs), Value::F32(n)) => {
                vs.push(n);
                size += 4;
            }
            (Values::F64(vs), Value::F64(n)) => {
                vs.push(n);
                size += 8;
            }
            (Values::Bool(vs), Value::Bool(b)) => {
                vs.push(b);
                size += 1;
            }
            (Values::Bytes(data, ends), Value::Bytes(b)) => {
                data.extend_from_slice(&b);
                ends.push(data.len());
                size += b.len() + 8;
            }
            (Values::Bytes(data, ends), Value::Text(t)) => {
                data.extend_from_slice(t.as_bytes());
                ends.push(data.len());
                size += t.len() + 8;
            }
            // `parse` makes each column's values of its own type.
            _ => unreachable!("a value of another type than its column's"),
        }
        self.defs.push(1);
        size
    }
}

/// A Parquet file written a row at a time into `out` ([`RowSink`]); the module says how.
pub struct ParquetWriter<'a, W: Write + Send> {
    columns: &'a [ParquetColumn<'a>],
    physical: Vec<Physical>,
    file: Option<SerializedFileWriter<W>>,
    out: Option<W>,
    buffers: Vec<Buffer>,
    group_rows: usize,
    group_bytes: usize,
    /// The rows and bytes kept for the row group being made.
    kept_rows: usize,
    kept_bytes: usize,
    groups: u32,
    rows: u64,
    bytes: u64,
    /// The first failure to write a row group or the footer (its kind and text): nothing more is
    /// written, and the file cannot be finished.
    broken: Option<(io::ErrorKind, String)>,
    ended: bool,
}

impl<'a, W: Write + Send> ParquetWriter<'a, W> {
    /// A writer of `columns` into `out`: the schema and the file's first bytes are written.
    pub fn new(columns: &'a [ParquetColumn<'a>], out: W) -> io::Result<Self> {
        let physical: Vec<Physical> = columns.iter().map(|c| Physical::of(c.kind, c.type_name)).collect();
        let fields = columns
            .iter()
            .zip(&physical)
            .map(|(c, p)| p.field(c.name).map(Arc::new))
            .collect::<Result<Vec<_>, _>>()
            .map_err(io_error)?;
        let schema = Type::group_type_builder("schema").with_fields(fields).build().map_err(io_error)?;
        let props = WriterProperties::builder().set_compression(Compression::SNAPPY).build();
        let file = SerializedFileWriter::new(out, Arc::new(schema), Arc::new(props)).map_err(io_error)?;
        Ok(Self {
            columns,
            buffers: physical.iter().map(|&p| Buffer::new(p)).collect(),
            physical,
            file: Some(file),
            out: None,
            group_rows: GROUP_ROWS,
            group_bytes: GROUP_BYTES,
            kept_rows: 0,
            kept_bytes: 0,
            groups: 0,
            rows: 0,
            bytes: 0,
            broken: None,
            ended: false,
        })
    }

    /// Row groups of at most `rows` rows and `bytes` bytes of values (at least one row each): a
    /// test sees the rows go on in the next group without writing 65,536 of them.
    pub fn row_groups(mut self, rows: usize, bytes: usize) -> Self {
        self.group_rows = rows.max(1);
        self.group_bytes = bytes.max(1);
        self
    }

    /// How each column is written.
    pub fn physical(&self) -> &[Physical] {
        &self.physical
    }

    /// The row groups written so far.
    pub fn groups(&self) -> u32 {
        self.groups
    }

    /// The output, once the writer ended (`None` before, or once it failed).
    pub fn into_inner(self) -> Option<W> {
        self.out
    }

    /// Value `v` of the `n`th row in column `c`, as its type holds it.
    fn parse<'v>(&self, c: usize, v: Option<&'v str>, n: u64) -> io::Result<Value<'v>> {
        let Some(v) = v else { return Ok(Value::Null) };
        let p = self.physical[c];
        let value = match p {
            Physical::Int32 => v.parse().ok().map(Value::I32),
            Physical::Int64 => v.parse().ok().map(Value::I64),
            // The bits of the `u64`, as Parquet keeps an unsigned 64-bit integer.
            Physical::UInt64 => v.parse::<u64>().ok().map(|n| Value::I64(n as i64)),
            Physical::Float => v.parse().ok().map(Value::F32),
            Physical::Double => v.parse().ok().map(Value::F64),
            Physical::Bool => parse_bool(v).map(Value::Bool),
            Physical::Date => parse_date(v).map(Value::I32),
            Physical::Time => parse_time(v).map(Value::I64),
            Physical::Timestamp { utc } => parse_timestamp(v, utc).map(Value::I64),
            Physical::Bytes => parse_bytes(v).map(Value::Bytes),
            Physical::Text => Some(Value::Text(v)),
        };
        value.ok_or_else(|| {
            let mut shown: String = v.chars().take(SHOWN_CHARS).collect();
            if shown.len() < v.len() {
                shown.push('\u{2026}');
            }
            io::Error::other(BadValue {
                column: self.columns[c].name.to_string(),
                row: n,
                value: shown,
                parquet_type: p.name(),
            })
        })
    }

    /// The first failure, as an error, once there was one.
    fn failed(&self) -> Option<io::Error> {
        self.broken.as_ref().map(|(kind, text)| broken(*kind, text))
    }

    /// `e`, kept as the first failure (a later one is what the first left).
    fn fail(&mut self, e: io::Error) -> io::Error {
        if self.broken.is_none() {
            self.broken = Some((e.kind(), e.to_string()));
        }
        e
    }

    /// Write the rows kept as a row group, and keep none. A failure leaves a group that never
    /// ends: it is kept ([`ParquetWriter::fail`]).
    fn flush(&mut self) -> io::Result<()> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        self.write_group().map_err(|e| self.fail(e))
    }

    fn write_group(&mut self) -> io::Result<()> {
        let Some(file) = self.file.as_mut() else { return Err(io::Error::other("the Parquet file is ended")) };
        let mut group = file.next_row_group().map_err(io_error)?;
        for buffer in &mut self.buffers {
            let Some(mut col) = group.next_column().map_err(io_error)? else {
                return Err(io::Error::other("the Parquet schema has fewer columns than the rows"));
            };
            let defs = Some(buffer.defs.as_slice());
            match &mut buffer.values {
                Values::I32(vs) => col.typed::<Int32Type>().write_batch(vs, defs, None),
                Values::I64(vs) => col.typed::<Int64Type>().write_batch(vs, defs, None),
                Values::F32(vs) => col.typed::<FloatType>().write_batch(vs, defs, None),
                Values::F64(vs) => col.typed::<DoubleType>().write_batch(vs, defs, None),
                Values::Bool(vs) => col.typed::<BoolType>().write_batch(vs, defs, None),
                Values::Bytes(data, ends) => {
                    let all = bytes::Bytes::from(std::mem::take(data));
                    let mut start = 0;
                    let vs: Vec<ByteArray> = ends
                        .iter()
                        .map(|&end| {
                            let b = ByteArray::from(all.slice(start..end));
                            start = end;
                            b
                        })
                        .collect();
                    ends.clear();
                    col.typed::<ByteArrayType>().write_batch(&vs, defs, None)
                }
            }
            .map_err(io_error)?;
            col.close().map_err(io_error)?;
            buffer.defs.clear();
            match &mut buffer.values {
                Values::I32(vs) => vs.clear(),
                Values::I64(vs) => vs.clear(),
                Values::F32(vs) => vs.clear(),
                Values::F64(vs) => vs.clear(),
                Values::Bool(vs) => vs.clear(),
                Values::Bytes(..) => {}
            }
        }
        group.close().map_err(io_error)?;
        self.groups += 1;
        self.kept_rows = 0;
        self.kept_bytes = 0;
        Ok(())
    }
}

impl<W: Write + Send> RowSink for ParquetWriter<'_, W> {
    /// Nothing: the file's first bytes are written by [`ParquetWriter::new`].
    fn begin(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn rows(&mut self, rows: &[super::Row]) -> io::Result<()> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        if self.ended {
            return Err(io::Error::other("the Parquet file is ended"));
        }
        for row in rows {
            if row.len() != self.columns.len() {
                let (values, columns) = (row.len(), self.columns.len());
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("a row of {values} values for a Parquet file of {columns} columns"),
                ));
            }
            let n = self.rows + 1;
            // Every value parsed before any is kept: a value that does not fit leaves no part
            // of its row behind.
            let values = row.iter().enumerate().map(|(c, v)| self.parse(c, *v, n)).collect::<io::Result<Vec<_>>>()?;
            let mut bytes = 0;
            for (buffer, v) in self.buffers.iter_mut().zip(values) {
                bytes += buffer.push(v);
            }
            self.kept_rows += 1;
            self.kept_bytes += bytes;
            self.rows += 1;
            self.bytes += row.iter().map(|v| v.map_or(0, str::len) as u64).sum::<u64>();
            if self.kept_rows >= self.group_rows || self.kept_bytes >= self.group_bytes {
                self.flush()?;
            }
        }
        Ok(())
    }

    /// Write the rows kept and the footer: the file of the rows so far reads. A file whose row
    /// group or footer failed to be written is not finished (the output is then not a Parquet
    /// file), and ending it again fails the same way.
    fn end(&mut self) -> io::Result<()> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        if self.ended {
            return Ok(());
        }
        self.ended = true;
        if self.kept_rows > 0 {
            self.flush()?;
        }
        let Some(file) = self.file.take() else { return Err(io::Error::other("the Parquet file is ended")) };
        let out = file.into_inner().map_err(io_error).and_then(|mut out| out.flush().map(|()| out));
        self.out = Some(out.map_err(|e| self.fail(e))?);
        Ok(())
    }

    fn rows_written(&self) -> u64 {
        self.rows
    }

    /// The bytes of the values written so far, as the driver gave them (the file is encoded and
    /// compressed a row group at a time).
    fn bytes(&self) -> u64 {
        self.bytes
    }
}

#[cfg(any(test, feature = "test-util"))]
pub mod read;
#[cfg(test)]
mod tests;
