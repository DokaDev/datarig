//! `<state>/table-view.toml`: how each table's grid is shown, as far as it is not its query:
//! the columns hidden from it. A view only: what is hidden is still read (the query stays
//! `SELECT *`), it is never in the workspace file nor in a query, and it follows the table into
//! any tab. Written with mode 0600 on Unix, as the filter history is.
//!
//! A table is named as the filter history names it ([`TableKey`]). Each keeps the names of its
//! hidden columns (at most [`NAMES`]; names, not places, so a column added or dropped leaves
//! the others as they were) and when they last changed (Unix seconds). At most [`TABLES`]
//! tables are kept, the one changed longest ago goes first, and the file stays under
//! [`FILE_BUDGET`] the same way. A name longer than [`NAME_MAX_BYTES`] is not kept.
//!
//! ```toml
//! version = 1
//!
//! [[tables]]
//! profile = "3f0b8f5e-…"
//! database = "shop"
//! schema = "public"
//! table = "orders"
//! hidden = ["memo", "shipping"]
//! used = 1762790400
//! ```
//!
//! A file that is not there holds nothing hidden. A file that is there but cannot be read, is
//! not this shape, was written by a newer version, is larger than [`READ_MAX`] or is anything
//! else than a plain file (a symbolic link too, never followed) is [`Unreadable`]: never taken
//! for an empty view, and never written over.

pub use crate::filter_history::{TableKey, Unreadable};
use crate::fsutil::is_plain_file;
use crate::profile::ProfileId;
use std::io::{self, Read};
use std::path::Path;

/// The file's name, in the state directory.
pub const FILE: &str = "table-view.toml";

/// The file format this build writes and reads.
pub const VERSION: i64 = 1;

/// The tables kept.
pub const TABLES: usize = 500;

/// The hidden columns kept per table.
pub const NAMES: usize = 1000;

/// A column's (or the table's) name longer than this (bytes) is not kept, and a file holding
/// one is not read.
pub const NAME_MAX_BYTES: usize = 4096;

/// The file written stays under this size: the tables changed longest ago are left out.
pub const FILE_BUDGET: usize = 1 << 20;

/// A file larger than this is not read.
pub const READ_MAX: u64 = 4 << 20;

/// The view of one table.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Table {
    key: TableKey,
    /// The hidden columns' names, in the order they were hidden, each once.
    hidden: Vec<String>,
    /// When they last changed, in Unix seconds.
    used: i64,
}

/// What the file holds: each table's view, the one changed last first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableView {
    tables: Vec<Table>,
}

impl TableView {
    /// The names of `key`'s hidden columns (none: every column shown).
    pub fn hidden(&self, key: &TableKey) -> &[String] {
        self.tables.iter().find(|t| t.key == *key).map_or(&[], |t| t.hidden.as_slice())
    }

    /// How many tables have hidden columns.
    pub fn table_count(&self) -> usize {
        self.tables.len()
    }

    /// `key`'s hidden columns become `hidden` at `now` (the table moves to the front; none:
    /// it is dropped). A name met twice is kept once; names longer than [`NAME_MAX_BYTES`] and
    /// those past [`NAMES`] are not kept, nor is anything of a table whose names are too long.
    /// Whether what is kept differs from what was.
    pub fn set(&mut self, key: TableKey, hidden: &[String], now: i64) -> bool {
        let before = self.hidden(&key).to_vec();
        let mut names: Vec<String> = Vec::new();
        for n in hidden {
            if n.len() <= NAME_MAX_BYTES && !names.contains(n) && names.len() < NAMES {
                names.push(n.clone());
            }
        }
        let too_long = [&key.database, &key.schema, &key.table].iter().any(|t| t.len() > NAME_MAX_BYTES);
        if too_long {
            names.clear();
        }
        self.tables.retain(|t| t.key != key);
        let changed = names != before;
        if !names.is_empty() {
            self.tables.insert(0, Table { key, hidden: names, used: now });
            self.tables.truncate(TABLES);
        }
        changed
    }

    /// Of `key`'s hidden columns, keep only those `columns` names (an authoritative list of
    /// the table's columns: a name not in it is no column of the table any more). Whether any
    /// was dropped. The table keeps its place.
    pub fn prune(&mut self, key: &TableKey, columns: &[&str]) -> bool {
        let Some(i) = self.tables.iter().position(|t| t.key == *key) else { return false };
        let before = self.tables[i].hidden.len();
        self.tables[i].hidden.retain(|n| columns.contains(&n.as_str()));
        let dropped = self.tables[i].hidden.len() != before;
        if self.tables[i].hidden.is_empty() {
            self.tables.remove(i);
        }
        dropped
    }

    /// The file's text, never larger than [`FILE_BUDGET`]: tables added whole, the one changed
    /// last first, while they fit.
    pub fn to_toml(&self) -> String {
        let mut out = format!("version = {VERSION}\n");
        for t in &self.tables {
            let part = table_text(t);
            if out.len() + part.len() > FILE_BUDGET {
                break;
            }
            out.push_str(&part);
        }
        out
    }
}

/// One table of the file as text.
fn table_text(t: &Table) -> String {
    let s = |v: &str| toml_edit::Value::from(v).to_string();
    let names: Vec<String> = t.hidden.iter().map(|n| s(n)).collect();
    format!(
        "\n[[tables]]\nprofile = {}\ndatabase = {}\nschema = {}\ntable = {}\nhidden = [{}]\nused = {}\n",
        s(&t.key.profile.to_string()),
        s(&t.key.database),
        s(&t.key.schema),
        s(&t.key.table),
        names.join(", "),
        t.used,
    )
}

/// Read the file: nothing hidden when there is none. Only a plain file is read: a symbolic
/// link at the path (one to nothing too) is never followed.
pub fn load(path: &Path) -> Result<TableView, Unreadable> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(TableView::default()),
        Err(e) => return Err(Unreadable::Io(e.kind())),
        Ok(m) if !is_plain_file(&m) => return Err(Unreadable::NotAFile),
        Ok(_) => {}
    }
    let not_a_file = |e: io::Error| match std::fs::symlink_metadata(path) {
        Ok(m) if !is_plain_file(&m) => Unreadable::NotAFile,
        _ => Unreadable::Io(e.kind()),
    };
    let file = crate::fsutil::open_no_follow(path).map_err(not_a_file)?;
    if !is_plain_file(&file.metadata().map_err(|e| Unreadable::Io(e.kind()))?) {
        return Err(Unreadable::NotAFile);
    }
    let mut bytes = Vec::new();
    file.take(READ_MAX + 1).read_to_end(&mut bytes).map_err(|e| Unreadable::Io(e.kind()))?;
    if bytes.len() as u64 > READ_MAX {
        return Err(Unreadable::TooLarge);
    }
    let text = String::from_utf8(bytes).map_err(|_| Unreadable::Malformed { line: None })?;
    parse(&text)
}

/// The view in `text`, read strictly (see the module's doc).
pub fn parse(text: &str) -> Result<TableView, Unreadable> {
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
        None => return Ok(TableView::default()),
        Some(toml::Value::Array(a)) => a,
        Some(_) => return Err(malformed),
    };
    let mut v = TableView::default();
    for t in tables {
        let t = t.as_table().ok_or(malformed)?;
        if !t.keys().all(|k| ["profile", "database", "schema", "table", "hidden", "used"].contains(&k.as_str())) {
            return Err(malformed);
        }
        let text = |k: &str| bounded(t.get(k)).ok_or(malformed);
        let profile = t.get("profile").and_then(toml::Value::as_str).and_then(ProfileId::parse).ok_or(malformed)?;
        let key = TableKey { profile, database: text("database")?, schema: text("schema")?, table: text("table")? };
        let used = t.get("used").and_then(toml::Value::as_integer).ok_or(malformed)?;
        let mut hidden = Vec::new();
        for n in t.get("hidden").and_then(toml::Value::as_array).ok_or(malformed)? {
            let n = bounded(Some(n)).ok_or(malformed)?;
            if !hidden.contains(&n) && hidden.len() < NAMES {
                hidden.push(n);
            }
        }
        // A table named twice (a hand edit): the first is the one kept.
        if !hidden.is_empty() && v.tables.len() < TABLES && !v.tables.iter().any(|x| x.key == key) {
            v.tables.push(Table { key, hidden, used });
        }
    }
    Ok(v)
}

/// `v` as a text no longer than [`NAME_MAX_BYTES`].
fn bounded(v: Option<&toml::Value>) -> Option<String> {
    v.and_then(toml::Value::as_str).filter(|t| t.len() <= NAME_MAX_BYTES).map(str::to_string)
}

/// Write `text` (a [`TableView::to_toml`]) as the file, atomically and with mode 0600 on Unix,
/// never through a link nor into a file already there ([`crate::fsutil::replace_private`]).
pub fn write(path: &Path, text: &str) -> io::Result<()> {
    crate::fsutil::replace_private(path, text.as_bytes())
}

#[cfg(test)]
mod tests;
