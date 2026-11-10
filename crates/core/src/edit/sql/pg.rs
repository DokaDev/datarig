//! PostgreSQL's edits.
//!
//! A value is written as a string literal the server reads as the column's type (`'5'`, `'t'`,
//! `'2026-11-03'`, `'\x0102'`), in the form the result showed it, so it is the value read
//! back; one holding a backslash is an escape string (`E'…'`), which reads the same whatever
//! `standard_conforming_strings` is. A row is compared with `=` (`IS NULL` for NULL), which an
//! index serves; a text column whose collation is not deterministic is also compared under
//! `"C"`, byte for byte. Operators, functions and collations are named with their schema: a
//! `search_path` of the user's may list its own `=` first, and a row must never be found by it
//! (only columns of built-in types and enums are compared, whose `=` is `pg_catalog`'s).
//! Statements change the rows they find and give them back (`RETURNING *`); one of several rows
//! the same in every column is picked by its physical place, `(tableoid, ctid)`, which holds
//! across partitions and inheritance.

use super::{ColumnInfo, EditSql, Exactness, Inexact, Offer, Scope, Value};
use crate::edit::plan::Where;
use crate::edit::target::{TableColumn, TableRef};
use crate::sql::dialect::{Dialect, Language};

const D: Dialect = Dialect::Postgres;

/// `=`, whatever the session's `search_path` lists first.
const EQ: &str = "OPERATOR(pg_catalog.=)";

pub struct Postgres;

/// Column types whose values, as the result shows them, compare equal only to themselves: the
/// type as `format_type` names it, without its modifier (`character varying(20)`).
const EXACT: &[&str] = &[
    "smallint",
    "integer",
    "bigint",
    "oid",
    "boolean",
    "text",
    "character varying",
    "character",
    "\"char\"",
    "name",
    "uuid",
    "date",
    "time without time zone",
    "timestamp without time zone",
    "timestamp with time zone",
    "bytea",
    "bit",
    "bit varying",
];

/// `s` as a string literal that reads the same whatever `standard_conforming_strings` is.
fn literal(s: &str) -> String {
    if s.contains('\\') {
        format!("E'{}'", s.replace('\\', "\\\\").replace('\'', "''"))
    } else {
        format!("'{}'", s.replace('\'', "''"))
    }
}

impl Postgres {
    /// `w`, or the one row it picks among several the same.
    fn scoped(&self, t: &TableRef, w: &Where, scope: Scope) -> String {
        match scope {
            Scope::All => format!("WHERE {}", w.as_str()),
            Scope::One => format!(
                "WHERE (tableoid, ctid) OPERATOR(pg_catalog.=) (SELECT tableoid, ctid FROM {} WHERE {} LIMIT 1)",
                self.table(t),
                w.as_str()
            ),
        }
    }
}

impl EditSql for Postgres {
    fn offers(&self) -> &'static [Offer] {
        &[Offer::Primary, Offer::Unique, Offer::Values]
    }

    /// The types of [`EXACT`]; `numeric` only with a scale (every value has it, so `1.5` is not
    /// also `1.50`); an enum (its labels); arrays, floats, JSON, XML, intervals (`1 day` is
    /// `24 hours`), time zones of times, network addresses, geometry and every other type
    /// (domains, extension types like `citext`) are not.
    fn exactness(&self, c: &ColumnInfo) -> Exactness {
        let ty = c.column.type_name.as_str();
        // Without its modifier, wherever it is (`timestamp(3) without time zone`).
        let base = match (ty.find('('), ty.find(')')) {
            (Some(a), Some(b)) if a < b => format!("{}{}", &ty[..a], &ty[b + 1..]),
            _ => ty.to_string(),
        };
        let base = base.trim();
        if !c.column.enum_labels.is_empty() || EXACT.contains(&base) || (base == "numeric" && ty.contains(',')) {
            return Exactness::Exact;
        }
        let inexact = match base {
            "real" | "double precision" => Inexact::Approximate,
            "json" | "jsonb" | "xml" => Inexact::Document,
            "point" | "line" | "lseg" | "box" | "path" | "polygon" | "circle" => Inexact::Geometry,
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

    fn value(&self, _: &TableColumn, v: &Value) -> String {
        match v {
            Value::Text(s) => literal(s),
            Value::Null => "NULL".into(),
            Value::Default => "DEFAULT".into(),
        }
    }

    /// `col = '…'` (which an index serves), and for a collation that is not deterministic also
    /// `col COLLATE "C" = '…'` (byte for byte: the first matches a superset), each named with
    /// its schema; `col IS NULL`.
    fn eq_exact(&self, c: &TableColumn, v: Option<&str>) -> String {
        let col = self.column(&c.column.name);
        let Some(v) = v else { return format!("{col} IS NULL") };
        let lit = literal(v);
        if c.column.collation.as_ref().is_some_and(|k| !k.deterministic) {
            format!("{col} {EQ} {lit} AND {col} COLLATE pg_catalog.\"C\" {EQ} {lit}")
        } else {
            format!("{col} {EQ} {lit}")
        }
    }

    fn count(&self, t: &TableRef, w: &Where) -> String {
        format!("SELECT pg_catalog.count(*) FROM {} WHERE {}", self.table(t), w.as_str())
    }

    fn check(&self, t: &TableRef, w: &Where) -> String {
        format!("SELECT 1 FROM {} WHERE {} FOR UPDATE NOWAIT", self.table(t), w.as_str())
    }

    fn update(&self, t: &TableRef, set: &[(&TableColumn, String)], w: &Where, scope: Scope) -> Option<String> {
        let set: Vec<String> = set.iter().map(|(c, v)| format!("{} = {v}", self.column(&c.column.name))).collect();
        Some(format!("UPDATE {} SET {} {} RETURNING *", self.table(t), set.join(", "), self.scoped(t, w, scope)))
    }

    fn delete(&self, t: &TableRef, w: &Where, scope: Scope) -> Option<String> {
        Some(format!("DELETE FROM {} {}", self.table(t), self.scoped(t, w, scope)))
    }

    fn tails(&self) -> &'static [&'static str] {
        &["", " RETURNING *", " LIMIT 1)", " LIMIT 1) RETURNING *"]
    }

    /// A `GENERATED ALWAYS AS IDENTITY` column given a value takes it with
    /// `OVERRIDING SYSTEM VALUE`.
    fn insert(&self, t: &TableRef, cells: &[(&TableColumn, String)]) -> String {
        if cells.is_empty() {
            return format!("INSERT INTO {} DEFAULT VALUES RETURNING *", self.table(t));
        }
        let cols: Vec<String> = cells.iter().map(|(c, _)| self.column(&c.column.name)).collect();
        let vals: Vec<&str> = cells.iter().map(|(_, v)| v.as_str()).collect();
        let identity = cells
            .iter()
            .any(|(c, v)| c.column.fill == crate::driver::structure::ColumnFill::IdentityAlways && v != "DEFAULT");
        let overriding = if identity { " OVERRIDING SYSTEM VALUE" } else { "" };
        format!(
            "INSERT INTO {} ({}){overriding} VALUES ({}) RETURNING *",
            self.table(t),
            cols.join(", "),
            vals.join(", ")
        )
    }
}

#[cfg(test)]
mod tests;
