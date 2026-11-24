//! Reading every row of a table in chunks by its key (keyset pagination), for an export: each
//! chunk is a short statement of its own, `SELECT * … WHERE key > (the last key read) ORDER BY
//! key LIMIT n`, so no statement holds a lock or a snapshot for the whole export (a long
//! `SELECT *` would keep the table's lock and, on MySQL, its metadata lock for as long as the
//! file takes to write).
//!
//! Only a key that finds every row exactly once in a fixed order is used: the primary key, or a
//! unique constraint whose columns are all `NOT NULL` (a NULL would never be past the last
//! key), its columns in the constraint's order, which is the order of its index, so the server
//! reads the index and never sorts. A key whose index orders a column otherwise (`DESC`, a
//! collation or operator class of its own) is not used, nor a key column of a kind whose text
//! does not compare back exactly as the column ([`key_kind`]): floats (rounded), a MySQL
//! `TIMESTAMP` (an hour a clock goes back is two instants), MySQL text read in a character set
//! that is not `utf8mb4`, and anything not on the list. Rows changed while the export runs may
//! be read in a later state than rows read before them: each chunk sees the table as it is then.
//!
//! The key's literals are the values the server sent for the last row ([`super::sql_value`]):
//! numbers bare (a MySQL `BIGINT` compared with a string would be compared as a double), the
//! rest as string literals the server reads in the column's type and collation.

use super::{Kind, sql_value};
use crate::driver::structure::TableStructure;
use crate::driver::{Cell, ColumnMeta};
use crate::engine::Fact;
use crate::sql::dialect::Dialect;

/// How long a chunk waits for a lock (an `ALTER TABLE` in the way) before it gives up, in
/// seconds. The drivers bound it when they send it: MySQL's with an optimizer hint, MariaDB's with
/// `SET STATEMENT`, PostgreSQL's with `lock_timeout` in their own transaction. The chunk's text
/// is a plain `SELECT` for every engine, so that the driver reads it as a read.
pub const LOCK_WAIT_SECS: u32 = 5;

/// Rows a chunk reads by default.
pub const CHUNK_ROWS: u64 = 10_000;

/// A column of the key: its name in the table, its kind and its place in the result's rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyColumn {
    pub name: String,
    pub kind: Kind,
    pub index: usize,
}

/// Why a table is not read by its key (it is then read with one statement).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotKeyset {
    /// No primary key, nor a unique constraint of `NOT NULL` columns.
    NoKey,
    /// The result's columns are not the table's (in its order).
    NotTheTable,
    /// The key's index orders this column otherwise than ascending in the column's own
    /// collation.
    Ordered(String),
    /// This key column's values do not compare back exactly as written.
    Kind(String),
}

/// The kind of key column `c` as a keyset literal is written, or why it cannot be one, by the
/// export rules of dialect `d`'s engine ([`ExportRules::key_kind`]; refused on an engine whose
/// tables are not read so). `text_ok`: text read in this session is the text (MySQL: `utf8mb4`
/// results; always on PostgreSQL).
///
/// [`ExportRules::key_kind`]: crate::engine::ExportRules::key_kind
pub fn key_kind(d: Dialect, c: &ColumnMeta, text_ok: bool) -> Result<Kind, NotKeyset> {
    match d.engine().spec().export {
        Fact::Known(rules) => rules.key_kind(c, text_ok),
        Fact::No(_) | Fact::Unknown(_) => Err(NotKeyset::Kind(c.name.clone())),
    }
}

/// The key a table is read by: its primary key, else its first unique constraint whose
/// columns are all `NOT NULL`, the columns in the constraint's order; `None` when it has none
/// (or tables inherit from it, whose rows the key does not bind),
/// or the key's index orders a column otherwise than ascending (`DESC`, a collation of its own:
/// [`NotKeyset::Ordered`]).
pub fn table_key(s: &TableStructure) -> Result<Vec<String>, NotKeyset> {
    // Rows of tables inheriting from it are read with it, and its keys do not bind them: a key
    // value may repeat, and a chunk would skip the others.
    if s.inherited {
        return Err(NotKeyset::NoKey);
    }
    let not_null = |name: &str| s.columns.iter().any(|c| c.name == name && c.not_null);
    let key = s
        .primary_key
        .iter()
        .chain(s.unique_constraints.iter().filter(|k| k.columns.iter().all(|c| not_null(c))))
        .find(|k| !k.columns.is_empty())
        .ok_or(NotKeyset::NoKey)?;
    // The index behind it, by its columns: one that says more than the column (an order, a
    // collation, an operator class) is not read in the order the comparison uses.
    for i in &s.indexes {
        let cols: Vec<&str> = i.key_columns.iter().map(|c| c.as_deref().unwrap_or("")).collect();
        if !(i.unique && cols == key.columns.iter().map(String::as_str).collect::<Vec<_>>()) {
            continue;
        }
        if let Some((c, _)) = key.columns.iter().zip(&i.options).find(|(_, o)| !o.trim().is_empty()) {
            return Err(NotKeyset::Ordered(c.clone()));
        }
        // A MySQL prefix key (`name(10)`) is not the column's order: every chunk would sort.
        if let Some((c, _)) = key.columns.iter().zip(&i.columns).find(|(_, t)| t.ends_with(')')) {
            return Err(NotKeyset::Ordered(c.clone()));
        }
    }
    Ok(key.columns.clone())
}

/// A table read in chunks by its key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keyset {
    pub dialect: Dialect,
    pub schema: String,
    pub table: String,
    pub key: Vec<KeyColumn>,
    /// Rows a chunk reads.
    pub chunk: u64,
}

impl Keyset {
    /// The keyset of table `schema.table` by `key` (its columns' names in order, [`table_key`])
    /// over a result with `columns` (the table's own, in its order: `SELECT *`). `text_ok` as
    /// for [`key_kind`].
    pub fn new(
        dialect: Dialect,
        schema: &str,
        table: &str,
        key: &[String],
        columns: &[ColumnMeta],
        table_columns: &[String],
        text_ok: bool,
    ) -> Result<Self, NotKeyset> {
        if columns.len() != table_columns.len() || columns.iter().zip(table_columns).any(|(c, t)| c.name != *t) {
            return Err(NotKeyset::NotTheTable);
        }
        let key = key
            .iter()
            .map(|name| {
                let index = columns.iter().position(|c| c.name == *name).ok_or(NotKeyset::NotTheTable)?;
                Ok(KeyColumn { name: name.clone(), kind: key_kind(dialect, &columns[index], text_ok)?, index })
            })
            .collect::<Result<Vec<_>, NotKeyset>>()?;
        if key.is_empty() {
            return Err(NotKeyset::NoKey);
        }
        Ok(Self { dialect, schema: schema.to_string(), table: table.to_string(), key, chunk: CHUNK_ROWS })
    }

    fn select(&self) -> String {
        let d = self.dialect;
        format!("SELECT * FROM {}.{}", d.force_quote_ident(&self.schema), d.force_quote_ident(&self.table))
    }

    fn order(&self) -> String {
        let cols: Vec<String> = self.key.iter().map(|k| self.dialect.force_quote_ident(&k.name)).collect();
        format!(" ORDER BY {} LIMIT {}", cols.join(", "), self.chunk)
    }

    /// The first chunk.
    pub fn first(&self) -> String {
        format!("{}{}", self.select(), self.order())
    }

    /// The chunk after the row `last` (a row of the result, the last one read); `None` when a
    /// key value of it is NULL or cannot be written as a literal that compares back exactly
    /// (bytes not in the driver's form).
    pub fn after(&self, last: &[Cell]) -> Option<String> {
        let d = self.dialect;
        let Fact::Known(rules) = d.engine().spec().export else { return None };
        let mut names = Vec::with_capacity(self.key.len());
        let mut values = Vec::with_capacity(self.key.len());
        let mut kinds = Vec::with_capacity(self.key.len());
        for k in &self.key {
            let v = last.get(k.index)?.as_deref()?;
            // Text the driver could not read as UTF-8 (replaced characters) is not the key.
            if v.contains('\u{FFFD}') {
                return None;
            }
            names.push(d.force_quote_ident(&k.name));
            values.push(sql_value(d, k.kind, Some(v)));
            kinds.push(k.kind);
        }
        let cond = rules.after(&names, &values, &kinds)?;
        Some(format!("{} WHERE {cond}{}", self.select(), self.order()))
    }
}

/// `(k1, k2, …) > (v1, v2, …)` written out: `k1 > v1 OR (k1 = v1 AND (k2 > v2 OR …))`.
pub(crate) fn expanded(names: &[String], values: &[String]) -> String {
    match (names, values) {
        ([n], [v]) => format!("{n} > {v}"),
        ([n, ns @ ..], [v, vs @ ..]) => format!("{n} > {v} OR ({n} = {v} AND ({}))", expanded(ns, vs)),
        _ => "FALSE".to_string(),
    }
}

#[cfg(test)]
mod tests;
