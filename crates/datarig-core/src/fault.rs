//! Why something failed, as data: a [`FaultKind`] the UI turns into a short message in the
//! user's language, and the raw `detail` (the OS's or a parser's text) that only goes to the
//! error log ([`ErrorLog`]). Core never renders a fault for the UI itself.
//!
//! Database server messages are not faults: they are data and are shown as they are.

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The kind of a failure, as far as the UI needs to tell it apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FaultKind {
    /// A file operation failed.
    Io(io::ErrorKind),
    /// A host name could not be resolved to an address.
    HostNotFound,
    /// A TOML file does not parse (or a value has the wrong type); the line (1-based) when
    /// known.
    Toml { line: Option<usize> },
    /// A TOML file parses but `key` does not have the form datarig needs (a table, …).
    Shape { key: String },
    /// The OS keychain failed.
    Keychain(KeychainFault),
    /// A folder datarig keeps private data in (result spill files) is there already and is
    /// not private to this user: another user owns it, or others may open it and it could not
    /// be restricted. Nothing is written into it.
    NotPrivate,
    /// Anything else; only the detail says what.
    Other,
}

/// How the OS keychain failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeychainFault {
    /// Locked, or access was denied.
    Locked,
    /// There is no keychain on this system (or none datarig can use).
    NoStore,
    /// A stored entry (or the store) is damaged or not text.
    Damaged,
    /// The keychain refused the entry (too long, invalid).
    Refused,
    /// More than one entry matches.
    Ambiguous,
    /// It took a password and gave back another one.
    Mismatch,
    /// Anything else the platform reported.
    Failure,
    /// It did not answer in time ([`crate::secret::Guarded`]): it may be waiting for a
    /// permission or an unlock asked on a screen the user does not see (a remote session).
    /// Unknown, not "no entry".
    NoAnswer,
}

/// A failure: its kind, and the raw text for the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    pub kind: FaultKind,
    /// The raw text (OS, parser, platform), for the error log only.
    pub detail: String,
}

impl Fault {
    pub fn new(kind: FaultKind, detail: impl Into<String>) -> Self {
        Self { kind, detail: detail.into() }
    }

    /// A failed file operation.
    pub fn io(e: &io::Error) -> Self {
        Self::new(FaultKind::Io(e.kind()), e.to_string())
    }

    /// A failed file operation on `path` (the path goes into the detail).
    pub fn io_at(e: &io::Error, path: &Path) -> Self {
        Self::new(FaultKind::Io(e.kind()), format!("{}: {e}", path.display()))
    }

    /// `text` does not parse as TOML (or has a value of the wrong type): the line of `span`.
    pub fn toml(text: &str, span: Option<std::ops::Range<usize>>, message: &str) -> Self {
        let line = span.map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1);
        Self::new(FaultKind::Toml { line }, message.trim().to_string())
    }

    /// A `toml` (serde) error of `text`.
    pub fn toml_de(text: &str, e: &toml::de::Error) -> Self {
        Self::toml(text, e.span(), e.message())
    }

    /// A `toml_edit` error of `text`.
    pub fn toml_edit(text: &str, e: &toml_edit::TomlError) -> Self {
        Self::toml(text, e.span(), e.message())
    }

    /// Anything else.
    pub fn other(detail: impl Into<String>) -> Self {
        Self::new(FaultKind::Other, detail)
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

/// Tests build faults from plain text (the kind is [`FaultKind::Other`]).
#[cfg(any(test, feature = "test-util"))]
impl From<&str> for Fault {
    fn from(detail: &str) -> Self {
        Self::other(detail)
    }
}

impl From<io::Error> for Fault {
    fn from(e: io::Error) -> Self {
        Self::io(&e)
    }
}

/// `<state>/errors.log`: the raw detail of every failure the UI reported in words, one line
/// each (`<unix seconds> <context>: <detail>`), so nothing is lost by keeping the status line
/// short. Appending is best effort; without a state directory nothing is written.
#[derive(Clone, Debug, Default)]
pub struct ErrorLog {
    path: Option<PathBuf>,
}

impl ErrorLog {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { path }
    }

    /// Where it writes.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Append `fault` as having happened while `context` (a message key).
    pub fn record(&self, context: &str, fault: &Fault) {
        let Some(path) = &self.path else { return };
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let detail = fault.detail.replace('\n', " | ");
        // A fault without a detail says so instead of ending the line after its kind.
        let line = match detail.trim() {
            "" => format!("{secs} {context}: {:?} (no detail given)\n", fault.kind),
            _ => format!("{secs} {context}: {:?}: {detail}\n", fault.kind),
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests;
