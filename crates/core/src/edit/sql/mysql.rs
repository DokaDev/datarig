//! MySQL's edits, 8.0.17 or later (the binary collation `utf8mb4_0900_bin` it compares text
//! under).
//!
//! A value is written as the copy as SQL writes it (`export::sql_value`): a number bare, bytes
//! as `X'…'`, bits as `b'…'`, text as a string literal that reads the same in any sql mode. A row
//! is compared with `<=>` (NULL-safe). A text value (and an enum's or a set's) is also compared
//! byte for byte: the column converted to `utf8mb4` under the binary collation that does not pad,
//! against the value's own UTF-8 bytes, so neither a `_ci` collation (`'a'` is `'A'`) nor padding
//! (`'a'` is `'a '`) makes another row match; only in a character set that converts one to one
//! ([`ONE_TO_ONE`]). One of several rows the same in every column is the first `LIMIT 1` finds.

use super::{ColumnInfo, EditSql, Exactness, Inexact, Offer, Scope, Value};
use crate::driver::MySqlEdit;
use crate::driver::structure::StructureColumn;
use crate::edit::plan::Where;
use crate::edit::target::{TableColumn, TableRef};
use crate::export::{Kind, sql_value};
use crate::sql::dialect::{Dialect, Language, MySqlMode};

const D: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: false });

pub struct MySql(pub MySqlEdit);

/// Column types (the first word of `COLUMN_TYPE`) whose values, as the result shows them,
/// compare equal only to themselves.
const EXACT: &[&str] = &[
    "tinyint",
    "smallint",
    "mediumint",
    "int",
    "bigint",
    "decimal",
    "bit",
    "year",
    "date",
    "time",
    "datetime",
    "timestamp",
    "char",
    "varchar",
    "tinytext",
    "text",
    "mediumtext",
    "longtext",
    "enum",
    "set",
    "binary",
    "varbinary",
    "tinyblob",
    "blob",
    "mediumblob",
    "longblob",
];

/// Types compared as text (their characters), not as their value.
const TEXT: &[&str] = &["char", "varchar", "tinytext", "text", "mediumtext", "longtext", "enum", "set"];

/// Types of bytes.
const BYTES: &[&str] = &["binary", "varbinary", "tinyblob", "blob", "mediumblob", "longblob"];

/// Character sets whose every value converts to `utf8mb4` as a value of its own (no two
/// stored values become one: `sjis` and `cp932` map several byte sequences to one character).
const ONE_TO_ONE: &[&str] = &["utf8mb4", "utf8mb3", "utf8", "latin1", "ascii", "ucs2", "utf16", "utf16le", "utf32"];

/// The first word of a column's type (`varchar(20)` → `varchar`, `int unsigned` → `int`).
fn base(c: &StructureColumn) -> String {
    let ty = c.type_name.to_ascii_lowercase();
    ty.split(|ch: char| ch == '(' || ch.is_whitespace()).next().unwrap_or_default().to_string()
}

/// `s` as UTF-8 bytes in hexadecimal.
fn hex(s: &[u8]) -> String {
    s.iter().map(|b| format!("{b:02X}")).collect()
}

impl MySql {
    fn tail(&self, scope: Scope) -> &'static str {
        match scope {
            Scope::All => "",
            Scope::One => " LIMIT 1",
        }
    }
}

impl EditSql for MySql {
    fn offers(&self) -> &'static [Offer] {
        &[Offer::Primary, Offer::Unique, Offer::Values]
    }

    /// The types of [`EXACT`] (a text in a character set of its own included: it is compared
    /// as UTF-8); floats, JSON, spatial types and any other are not.
    fn exactness(&self, c: &ColumnInfo) -> Exactness {
        let base = base(c.column);
        let base = base.as_str();
        if TEXT.contains(&base) {
            // Its characters, in a character set that converts one to one (unknown: not).
            let charset = c.column.collation.as_ref().and_then(|k| k.charset.as_deref());
            return match charset {
                Some(cs) if ONE_TO_ONE.contains(&cs.to_ascii_lowercase().as_str()) => Exactness::Exact,
                _ => Exactness::Inexact(Inexact::Unknown),
            };
        }
        if EXACT.contains(&base) {
            return Exactness::Exact;
        }
        let inexact = match base {
            "float" | "double" | "real" => Inexact::Approximate,
            "json" => Inexact::Document,
            "geometry" | "point" | "linestring" | "polygon" | "multipoint" | "multilinestring" | "multipolygon"
            | "geometrycollection" | "geomcollection" => Inexact::Geometry,
            _ => Inexact::Unknown,
        };
        Exactness::Inexact(inexact)
    }

    fn language(&self) -> Language {
        Language::Sql(D)
    }

    fn table(&self, t: &TableRef) -> String {
        format!("{}.{}", D.force_quote_ident(&t.schema), D.force_quote_ident(&t.name))
    }

    fn column(&self, name: &str) -> String {
        D.force_quote_ident(name)
    }

    fn value(&self, c: &TableColumn, v: &Value) -> String {
        match v {
            Value::Text(s) => sql_value(D, Kind::from(c.kind), Some(s)),
            Value::Null => "NULL".into(),
            Value::Default => "DEFAULT".into(),
        }
    }

    /// `col <=> value`; a text, an enum or a set also byte for byte, after `col = '…'` under its
    /// own collation, which an index serves and matches a superset (without it the server reads,
    /// and a locking read locks, every row).
    fn eq_exact(&self, c: &TableColumn, v: Option<&str>) -> String {
        let col = self.column(&c.column.name);
        let base = base(&c.column);
        let Some(v) = v else { return format!("{col} <=> NULL") };
        if TEXT.contains(&base.as_str()) && c.column.collation.is_some() {
            let near = sql_value(D, Kind::Text, Some(v));
            return format!(
                "{col} = {near} AND CONVERT({col} USING utf8mb4) COLLATE utf8mb4_0900_bin <=> _utf8mb4 X'{}'",
                hex(v.as_bytes())
            );
        }
        if BYTES.contains(&base.as_str()) || TEXT.contains(&base.as_str()) {
            // Bytes as the driver shows them (`0x…`), else the text's own bytes.
            let bytes = v.strip_prefix("0x").filter(|h| h.len() % 2 == 0 && h.bytes().all(|b| b.is_ascii_hexdigit()));
            let hex = bytes.map_or_else(|| hex(v.as_bytes()), str::to_ascii_uppercase);
            return format!("{col} <=> X'{hex}'");
        }
        format!("{col} <=> {}", sql_value(D, Kind::from(c.kind), Some(v)))
    }

    fn count(&self, t: &TableRef, w: &Where) -> String {
        format!("SELECT count(*) FROM {} WHERE {}", self.table(t), w.as_str())
    }

    /// `FOR UPDATE NOWAIT` where the server has it; else `FOR UPDATE`, which waits for a row
    /// another session holds as long as the session's lock wait limit allows.
    fn check(&self, t: &TableRef, w: &Where) -> String {
        let nowait = if self.0.nowait { " NOWAIT" } else { "" };
        format!("SELECT 1 FROM {} WHERE {} FOR UPDATE{nowait}", self.table(t), w.as_str())
    }

    fn update(&self, t: &TableRef, set: &[(&TableColumn, String)], w: &Where, scope: Scope) -> Option<String> {
        let set: Vec<String> = set.iter().map(|(c, v)| format!("{} = {v}", self.column(&c.column.name))).collect();
        Some(format!("UPDATE {} SET {} WHERE {}{}", self.table(t), set.join(", "), w.as_str(), self.tail(scope)))
    }

    fn delete(&self, t: &TableRef, w: &Where, scope: Scope) -> Option<String> {
        Some(format!("DELETE FROM {} WHERE {}{}", self.table(t), w.as_str(), self.tail(scope)))
    }

    fn tails(&self) -> &'static [&'static str] {
        &["", " LIMIT 1"]
    }

    fn insert(&self, t: &TableRef, cells: &[(&TableColumn, String)]) -> String {
        let cols: Vec<String> = cells.iter().map(|(c, _)| self.column(&c.column.name)).collect();
        let vals: Vec<&str> = cells.iter().map(|(_, v)| v.as_str()).collect();
        format!("INSERT INTO {} ({}) VALUES ({})", self.table(t), cols.join(", "), vals.join(", "))
    }
}

#[cfg(test)]
mod tests;
