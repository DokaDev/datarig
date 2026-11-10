//! `<state>/filter-history.toml`: the filters and orders a table tab ran, per table, so an
//! earlier one can be picked again. Local only, never sent anywhere, never part of the
//! workspace file; written with mode 0600 on Unix (a filter can hold personal values).
//!
//! A table is its profile's id, its database (the profile's own named as such), its schema and
//! its name. Each table keeps at most [`PER_TABLE`] entries, most recent first; an entry is a
//! filter text, an order text and when it was last used (Unix seconds). Running the same pair
//! again moves it to the front. At most [`TABLES`] tables are kept, the least recently used one
//! goes first. A text longer than [`ENTRY_MAX_BYTES`] is not kept, and the file stays under
//! [`FILE_BUDGET`] by leaving out the tables used longest ago.
//!
//! ```toml
//! version = 1
//!
//! [[tables]]
//! profile = "3f0b8f5e-…"
//! database = "shop"
//! schema = "public"
//! table = "orders"
//!
//! [[tables.entries]]
//! where = "status = 'done'"
//! order = "id DESC"
//! used = 1762790400
//! ```
//!
//! A file that is not there is an empty history. A file that is there but cannot be read, is
//! not this shape (anything missing, of another type or not known), was written by a newer
//! version or is larger than [`READ_MAX`] is [`Unreadable`]: never taken for an empty history,
//! and never written over.

use crate::profile::ProfileId;
use std::io::{self, Read};
use std::path::Path;

/// The file's name, in the state directory.
pub const FILE: &str = "filter-history.toml";

/// The file format this build writes and reads.
pub const VERSION: i64 = 1;

/// The entries kept per table.
pub const PER_TABLE: usize = 50;

/// The tables kept.
pub const TABLES: usize = 200;

/// A filter or order text longer than this (bytes) is not kept.
pub const ENTRY_MAX_BYTES: usize = 4096;

/// The file written stays under this size: the tables used longest ago are left out.
pub const FILE_BUDGET: usize = 1 << 20;

/// A file larger than this is not read.
pub const READ_MAX: u64 = 4 << 20;

/// A table whose rows a filter ran over.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TableKey {
    pub profile: ProfileId,
    /// The database by name, the profile's own too.
    pub database: String,
    pub schema: String,
    pub table: String,
}

/// A filter and order that ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The filter's text (empty: none).
    pub filter: String,
    /// The order's text (empty: none).
    pub order: String,
    /// When it last ran, in Unix seconds.
    pub used: i64,
}

/// What the file holds: each table's entries, the table used last first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    tables: Vec<(TableKey, Vec<Entry>)>,
}

/// Why the file could not be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unreadable {
    /// It could not be read.
    Io(io::ErrorKind),
    /// It is not the shape of the file; `line`: where, when known (1-based).
    Malformed { line: Option<usize> },
    /// It is larger than [`READ_MAX`].
    TooLarge,
    /// A newer version wrote it (its version).
    Newer(i64),
}

impl History {
    /// The entries of `key`, the most recent first.
    pub fn entries(&self, key: &TableKey) -> &[Entry] {
        self.tables.iter().find(|(k, _)| k == key).map_or(&[], |(_, e)| e.as_slice())
    }

    /// How many tables have entries.
    pub fn table_count(&self) -> usize {
        self.tables.len()
    }

    /// `filter` and `order` ran on `key` at `now`: the entry (new, or the same texts run
    /// earlier) moves to the front, and so does the table. Whether anything was kept: both
    /// texts blank, or one longer than [`ENTRY_MAX_BYTES`], keeps nothing.
    pub fn record(&mut self, key: TableKey, filter: &str, order: &str, now: i64) -> bool {
        let (filter, order) = (filter.trim(), order.trim());
        if (filter.is_empty() && order.is_empty()) || filter.len() > ENTRY_MAX_BYTES || order.len() > ENTRY_MAX_BYTES {
            return false;
        }
        let mut entries = match self.tables.iter().position(|(k, _)| *k == key) {
            Some(i) => self.tables.remove(i).1,
            None => Vec::new(),
        };
        entries.retain(|e| !(e.filter == filter && e.order == order));
        entries.insert(0, Entry { filter: filter.to_string(), order: order.to_string(), used: now });
        entries.truncate(PER_TABLE);
        self.tables.insert(0, (key, entries));
        self.tables.truncate(TABLES);
        true
    }

    /// Forget the entries of `key`; how many there were.
    pub fn clear_table(&mut self, key: &TableKey) -> usize {
        match self.tables.iter().position(|(k, _)| k == key) {
            Some(i) => self.tables.remove(i).1.len(),
            None => 0,
        }
    }

    /// Forget every entry; how many there were.
    pub fn clear_all(&mut self) -> usize {
        let n = self.tables.iter().map(|(_, e)| e.len()).sum();
        self.tables.clear();
        n
    }

    /// The file's text. Tables are added the most recent first while the text stays within
    /// [`FILE_BUDGET`]; the most recent table is always there (its size is bounded by the
    /// limits of an entry).
    pub fn to_toml(&self) -> String {
        let mut out = format!("version = {VERSION}\n");
        for (i, (key, entries)) in self.tables.iter().enumerate() {
            let part = table_text(key, entries);
            if i > 0 && out.len() + part.len() > FILE_BUDGET {
                break;
            }
            out.push_str(&part);
        }
        out
    }
}

/// One table of the file, as text.
fn table_text(key: &TableKey, entries: &[Entry]) -> String {
    let s = |v: &str| toml_edit::Value::from(v).to_string();
    let mut out = format!(
        "\n[[tables]]\nprofile = {}\ndatabase = {}\nschema = {}\ntable = {}\n",
        s(&key.profile.to_string()),
        s(&key.database),
        s(&key.schema),
        s(&key.table),
    );
    for e in entries {
        out.push_str(&format!(
            "\n[[tables.entries]]\nwhere = {}\norder = {}\nused = {}\n",
            s(&e.filter),
            s(&e.order),
            e.used
        ));
    }
    out
}

/// Read the file: an empty history when there is none.
pub fn load(path: &Path) -> Result<History, Unreadable> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(History::default()),
        Err(e) => return Err(Unreadable::Io(e.kind())),
    };
    let mut bytes = Vec::new();
    file.take(READ_MAX + 1).read_to_end(&mut bytes).map_err(|e| Unreadable::Io(e.kind()))?;
    if bytes.len() as u64 > READ_MAX {
        return Err(Unreadable::TooLarge);
    }
    let text = String::from_utf8(bytes).map_err(|_| Unreadable::Malformed { line: None })?;
    parse(&text)
}

/// The history in `text`, read strictly (see the module's doc).
pub fn parse(text: &str) -> Result<History, Unreadable> {
    let malformed = Unreadable::Malformed { line: None };
    let doc: toml::Table = text.parse().map_err(|e: toml::de::Error| Unreadable::Malformed {
        line: e.span().map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1),
    })?;
    match doc.get("version") {
        Some(toml::Value::Integer(v)) if *v > VERSION => return Err(Unreadable::Newer(*v)),
        Some(toml::Value::Integer(v)) if *v == VERSION => {}
        _ => return Err(malformed),
    }
    if doc.keys().any(|k| k != "version" && k != "tables") {
        return Err(malformed);
    }
    let tables = match doc.get("tables") {
        None => return Ok(History::default()),
        Some(toml::Value::Array(a)) => a,
        Some(_) => return Err(malformed),
    };
    let mut h = History::default();
    for t in tables {
        let t = t.as_table().ok_or(malformed)?;
        only(t, &["profile", "database", "schema", "table", "entries"])?;
        let text = |k: &str| t.get(k).and_then(toml::Value::as_str).map(str::to_string).ok_or(malformed);
        let profile = t.get("profile").and_then(toml::Value::as_str).and_then(ProfileId::parse).ok_or(malformed)?;
        let key = TableKey { profile, database: text("database")?, schema: text("schema")?, table: text("table")? };
        let mut entries = Vec::new();
        for e in t.get("entries").and_then(toml::Value::as_array).ok_or(malformed)? {
            let e = e.as_table().ok_or(malformed)?;
            only(e, &["where", "order", "used"])?;
            let text = |k: &str| e.get(k).and_then(toml::Value::as_str).map(str::to_string).ok_or(malformed);
            let used = e.get("used").and_then(toml::Value::as_integer).ok_or(malformed)?;
            entries.push(Entry { filter: text("where")?, order: text("order")?, used });
        }
        entries.truncate(PER_TABLE);
        // A table named twice (a hand edit): its first entries are the ones kept.
        if !entries.is_empty() && h.tables.len() < TABLES && !h.tables.iter().any(|(k, _)| *k == key) {
            h.tables.push((key, entries));
        }
    }
    Ok(h)
}

/// `t` has no key but `keys`.
fn only(t: &toml::Table, keys: &[&str]) -> Result<(), Unreadable> {
    if t.keys().all(|k| keys.contains(&k.as_str())) { Ok(()) } else { Err(Unreadable::Malformed { line: None }) }
}

/// Write `text` (a [`History::to_toml`]) as the file, atomically and with mode 0600 on Unix.
pub fn write(path: &Path, text: &str) -> io::Result<()> {
    crate::fsutil::atomic_write_private(path, text.as_bytes())
}

/// Write `history` as the file ([`write`]).
pub fn save(path: &Path, history: &History) -> io::Result<()> {
    write(path, &history.to_toml())
}

#[cfg(test)]
mod tests;
