//! Rows of the monitor's statements (each driver's `meta::monitor`) read into the model. The
//! driver hands each statement's rows back as text cells; each decoder names the columns it
//! reads, in order (its shape: `pg::SESSIONS` and the like), and reads them by name.
//!
//! A value that is not what the statement gives (a missing column, text where a number belongs)
//! is a [`DecodeError`], never a default: a wrong reading must not look like an idle server.
//! NULL is read as "not there" only where the statement says it may be.

pub mod mysql;
pub mod pg;

use super::model::Timestamp;
use crate::driver::{Cell, DbError};
use crate::fault::Fault;
use std::fmt;

/// A statement's rows, as the driver hands them back.
pub type Rows = Vec<Vec<Cell>>;

/// How a statement of the probe ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Rows(Rows),
    /// The server refused or failed it: its error code, where it has one, and message.
    Error {
        code: Option<u32>,
        message: String,
    },
}

/// Why rows could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The rows are not what the statement gives.
    Malformed { what: &'static str, detail: String },
    /// The server is a version the monitor does not read (its version line).
    Unsupported(String),
    /// The server is a product the monitor does not read at any version (`MariaDB 11.4.13`).
    NotRead(String),
    /// The probe's first statement failed, so nothing is known of the server.
    ProbeFailed(String),
    /// A read failed for another reason than a privilege refused: what it would show is not
    /// known.
    Failed(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Malformed { what, detail } => write!(f, "unexpected monitor rows ({what}): {detail}"),
            DecodeError::Unsupported(version) => write!(f, "server not supported by the monitor: {version}"),
            DecodeError::NotRead(server) => write!(f, "server not read by the monitor: {server}"),
            DecodeError::ProbeFailed(message) => write!(f, "monitor probe failed: {message}"),
            DecodeError::Failed(message) => write!(f, "monitor read failed: {message}"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl DecodeError {
    /// As the session's error: a server too old names `oldest`, the oldest version the monitor
    /// reads.
    pub fn into_db(self, oldest: &str) -> DbError {
        match self {
            DecodeError::Unsupported(server) => DbError::VersionUnsupported { server, needed: oldest.to_owned() },
            DecodeError::NotRead(server) => DbError::MonitorUnsupported(server),
            DecodeError::ProbeFailed(message) | DecodeError::Failed(message) => DbError::Server(message.into()),
            e @ DecodeError::Malformed { .. } => DbError::Connection(Fault::other(e.to_string())),
        }
    }
}

fn malformed(what: &'static str, detail: impl Into<String>) -> DecodeError {
    DecodeError::Malformed { what, detail: detail.into() }
}

/// The one row a statement gives.
fn one<'a>(rows: &'a Rows, what: &'static str, shape: &'static [&'static str]) -> Result<Row<'a>, DecodeError> {
    match rows.as_slice() {
        [row] => Row::new(row, what, shape),
        _ => Err(malformed(what, format!("{} rows, expected 1", rows.len()))),
    }
}

/// A row of a statement whose columns are named by its shape (the decoder's list of the
/// columns it reads, in order: `pg::SESSIONS` and the like, which the drivers' tests hold their
/// statements to). Values are read by column name.
struct Row<'a> {
    cells: &'a [Cell],
    what: &'static str,
    shape: &'static [&'static str],
}

impl<'a> Row<'a> {
    fn new(cells: &'a [Cell], what: &'static str, shape: &'static [&'static str]) -> Result<Self, DecodeError> {
        if cells.len() != shape.len() {
            return Err(malformed(what, format!("{} columns, expected {}", cells.len(), shape.len())));
        }
        Ok(Row { cells, what, shape })
    }

    /// The cell of column `name`. A name not in the shape is a decoder's own mistake, which
    /// every decoder's tests reach: it reads as NULL in a release build.
    fn text(&self, name: &str) -> Option<&'a str> {
        let i = self.shape.iter().position(|c| *c == name);
        debug_assert!(i.is_some(), "column {name} is not in the shape of {}", self.what);
        i.and_then(|i| self.cells[i].as_deref())
    }

    fn string(&self, i: &str) -> Option<String> {
        self.text(i).map(str::to_owned)
    }

    fn required(&self, i: &str) -> Result<&'a str, DecodeError> {
        self.text(i).ok_or_else(|| malformed(self.what, format!("column {i} is NULL")))
    }

    fn int(&self, i: &str) -> Result<Option<i64>, DecodeError> {
        self.text(i)
            .map(|t| t.parse::<i64>().map_err(|_| malformed(self.what, format!("column {i}: {t:?} is not an integer"))))
            .transpose()
    }

    fn uint(&self, i: &str) -> Result<Option<u64>, DecodeError> {
        self.text(i)
            .map(|t| t.parse::<u64>().map_err(|_| malformed(self.what, format!("column {i}: {t:?} is not a count"))))
            .transpose()
    }

    fn required_int(&self, i: &str) -> Result<i64, DecodeError> {
        self.int(i)?.ok_or_else(|| malformed(self.what, format!("column {i} is NULL")))
    }

    fn required_uint(&self, i: &str) -> Result<u64, DecodeError> {
        self.uint(i)?.ok_or_else(|| malformed(self.what, format!("column {i} is NULL")))
    }

    fn time(&self, i: &str) -> Result<Option<Timestamp>, DecodeError> {
        Ok(self.int(i)?.map(Timestamp))
    }

    fn required_time(&self, i: &str) -> Result<Timestamp, DecodeError> {
        Ok(Timestamp(self.required_int(i)?))
    }

    /// A boolean as either engine writes it: PostgreSQL `t`/`f`, MySQL `1`/`0`.
    fn bool(&self, i: &str) -> Result<Option<bool>, DecodeError> {
        self.text(i)
            .map(|t| match t {
                "t" | "true" | "1" | "ON" => Ok(true),
                "f" | "false" | "0" | "OFF" => Ok(false),
                _ => Err(malformed(self.what, format!("column {i}: {t:?} is not a boolean"))),
            })
            .transpose()
    }

    fn required_bool(&self, i: &str) -> Result<bool, DecodeError> {
        self.bool(i)?.ok_or_else(|| malformed(self.what, format!("column {i} is NULL")))
    }
}

/// `Variable_name`, `Value` rows (`SHOW GLOBAL STATUS`, `SHOW GLOBAL VARIABLES`) by name.
fn name_values<'a>(rows: &'a Rows, what: &'static str) -> Result<Vec<(&'a str, Option<&'a str>)>, DecodeError> {
    rows.iter()
        .map(|r| {
            let row = Row::new(r, what, mysql::NAME_VALUE)?;
            Ok((row.required("Variable_name")?, row.text("Value")))
        })
        .collect()
}
