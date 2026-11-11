//! The values of one column of a table and how many rows hold each, counted by the server
//! ([`group_count`]): `SELECT <preview>, <length>, count(*) FROM <table> [WHERE (<other>)]
//! GROUP BY <column> ORDER BY count(*) DESC LIMIT 201`.
//!
//! The filter text it counts under (the other conditions of the table's filter) passes every
//! check of [`check_filter`](super::check_filter) on the table's usual statement, `SELECT * FROM
//! <table> WHERE (<other>)`; a call it would ask the user about is refused here instead (a
//! count is not worth a question), as on a read-only profile. The statement written around it is
//! then read by the classifier (a plain read, nothing dangerous) and must be on the allowlist of
//! statements the app runs on its own ([`Classifier::repeatable`]).
//!
//! Values are grouped whole, by the column itself, but a value is sent only when it is at most
//! [`GROUP_PREVIEW`] bytes long: a longer one comes as its length alone, so the answer holds at
//! most [`GROUP_VALUES`] + 1 values of that size. Types of a short value of fixed size (numbers,
//! dates and times, booleans, `uuid`, …) are sent as they are. Only a column whose values a
//! filter compares for equality is grouped ([`compares`]).

use super::check::{BarRefusal, FilterVerdict, check_filter};
use super::{TableName, Target};
use crate::driver::{ArrayElement, ColumnMeta, ValueKind};
use crate::sql::dialect::Dialect;
use crate::sql::risk::repeat::NotRepeatable;
use crate::sql::risk::{Class, Classifier, Danger};

/// The most values a group count lists; the statement asks for one more, to tell whether there
/// are more.
pub const GROUP_VALUES: usize = 200;

/// The longest value, in bytes, a group count sends; a longer one comes as its length alone.
pub const GROUP_PREVIEW: usize = 4096;

/// A column's values compared by a filter ([`compares`]): with an order (`<`, `>`, `BETWEEN`
/// too), or for equality alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compares {
    Order,
    Equality,
}

/// Why a column's values are not compared by a filter ([`compares`]): its type has no equality
/// a filter's literal reaches (PostgreSQL `json`, `xml`, `point` and the other geometric types,
/// arrays of them; MySQL JSON, spatial and vector types, bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoEquality;

/// How a filter compares the values of a result column `m` of dialect `d`, as its type says:
/// with an order, for equality only, or not at all ([`NoEquality`]). `enum_known`: the
/// column's type is known to be a PostgreSQL enum (no kind of its own, ordered by its labels).
pub fn compares(d: Dialect, m: &ColumnMeta, enum_known: bool) -> Result<Compares, NoEquality> {
    let mysql = matches!(d, Dialect::MySql(_));
    let ty = m.type_name.to_ascii_lowercase();
    match m.kind {
        ValueKind::Integer
        | ValueKind::Decimal
        | ValueKind::Float
        | ValueKind::Text
        | ValueKind::Bool
        | ValueKind::Date
        | ValueKind::Time
        | ValueKind::Timestamp
        | ValueKind::TimestampTz
        | ValueKind::Interval => Ok(Compares::Order),
        // MySQL's spatial and vector types come as bytes but do not compare as such.
        ValueKind::Bytes if mysql && matches!(ty.as_str(), "geometry" | "vector") => Err(NoEquality),
        ValueKind::Bytes => Ok(Compares::Order),
        // MySQL shows bits as `b'…'`, which is no string literal of them.
        ValueKind::Bit if mysql => Err(NoEquality),
        ValueKind::Bit => Ok(Compares::Order),
        // `jsonb` has equality; `json` has none, and MySQL compares JSON with a string as a
        // JSON string.
        ValueKind::Json if !mysql && ty == "jsonb" => Ok(Compares::Equality),
        ValueKind::Json => Err(NoEquality),
        // An array compares through its element type's equality: only one known to have it.
        ValueKind::Array(ArrayElement::Number | ArrayElement::Bool | ArrayElement::Text)
            if !mysql && ty.strip_suffix("[]").is_some_and(|e| PG_ELEMENT_EQ.contains(&e)) =>
        {
            Ok(Compares::Equality)
        }
        ValueKind::Array(_) => Err(NoEquality),
        ValueKind::Other if !mysql && (enum_known || PG_ORDERED.contains(&ty.as_str())) => Ok(Compares::Order),
        ValueKind::Other => Err(NoEquality),
    }
}

/// PostgreSQL types of no kind of their own whose values compare as their text reads back
/// (equality and order).
const PG_ORDERED: &[&str] = &["uuid", "inet", "cidr", "macaddr", "macaddr8", "citext"];

/// Built-in PostgreSQL element types (as an array's type names them, `text` of `text[]`) known
/// to have the equality an array's `=` compares its elements with. Any other (`xml`, `json`,
/// `point` and the other geometric types have none) is not compared.
const PG_ELEMENT_EQ: &[&str] = &[
    "int2",
    "int4",
    "int8",
    "float4",
    "float8",
    "numeric",
    "oid",
    "money",
    "text",
    "varchar",
    "bpchar",
    "char",
    "name",
    "bool",
    "date",
    "time",
    "timetz",
    "timestamp",
    "timestamptz",
    "interval",
    "uuid",
    "bytea",
    "inet",
    "cidr",
    "macaddr",
    "macaddr8",
    "bit",
    "varbit",
];

/// What a group count is asked for.
#[derive(Clone, Copy, Debug)]
pub struct GroupCount<'a> {
    /// The table, and what is known of it and its session.
    pub table: &'a TableName,
    pub target: &'a Target,
    /// The column, as the table's result describes it (its name is the table's column's).
    pub column: &'a ColumnMeta,
    /// The column's type is known to be a PostgreSQL enum ([`compares`]).
    pub enum_known: bool,
    /// The filter text the rows are counted under, as it is sent: the table's other conditions;
    /// `None` for every row.
    pub other: Option<&'a str>,
    /// The session's classifier (its dialect is the target's).
    pub classifier: &'a Classifier,
    /// The profile is read-only.
    pub read_only: bool,
}

/// Why no group count is written ([`group_count`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GroupRefusal {
    /// The column is not one of the table's (as `target` knows them).
    UnknownColumn,
    /// Its values are not compared for equality ([`compares`]).
    NoEquality,
    /// The filter text is refused, as the filter bar refuses it.
    Filter(BarRefusal),
    /// The filter text calls functions whose effects are not known (as written), which a count
    /// does not ask the user about.
    Calls(Vec<String>),
    /// The classifier does not read the statement as a plain read.
    NotRead { class: Class, danger: Option<Danger> },
    /// The statement is not one the app runs on its own.
    NotRepeatable(NotRepeatable),
}

/// The statement that counts the rows of each value of `g.column` (see the module docs), or why
/// it is not written.
pub fn group_count(g: &GroupCount<'_>) -> Result<String, GroupRefusal> {
    let t = g.target;
    let d = t.dialect;
    let name = g.column.name.as_str();
    if t.column(name).is_none() {
        return Err(GroupRefusal::UnknownColumn);
    }
    compares(d, g.column, g.enum_known).map_err(|_| GroupRefusal::NoEquality)?;
    let other = g.other.filter(|o| !o.trim().is_empty());
    if let Some(text) = other {
        match check_filter(text, g.table, t, g.classifier, g.read_only).map_err(GroupRefusal::Filter)? {
            FilterVerdict::Clear => {}
            FilterVerdict::NeedsConfirm(confirms) => {
                let calls = confirms.into_iter().flat_map(|c| match c {
                    super::Confirm::Calls(calls) => calls,
                });
                return Err(GroupRefusal::Calls(calls.collect()));
            }
        }
    }
    let sql = statement(d, g.table, g.column, other);
    let risk = g.classifier.clone().classify(&sql);
    if risk.class != Class::Read || risk.danger.is_some() || risk.read_write {
        return Err(GroupRefusal::NotRead { class: risk.class, danger: risk.danger });
    }
    if g.read_only {
        risk.read_only().map_err(|block| GroupRefusal::Filter(BarRefusal::ReadOnly(block)))?;
    }
    g.classifier.repeatable(&sql).map_err(GroupRefusal::NotRepeatable)?;
    Ok(sql)
}

/// The statement of [`group_count`], `other` checked already.
fn statement(d: Dialect, table: &TableName, column: &ColumnMeta, other: Option<&str>) -> String {
    let c = d.force_quote_ident(&column.name);
    let from = format!("{}.{}", d.force_quote_ident(&table.schema), d.force_quote_ident(&table.name));
    let filter = other.map_or(String::new(), |o| format!(" WHERE ({o})"));
    let (preview, length) = match (d, length(d, column, &c)) {
        (_, None) => (c.clone(), "NULL".to_string()),
        (Dialect::Postgres, Some(len)) => (format!("CASE WHEN {len} <= {GROUP_PREVIEW} THEN {c} END"), len),
        (Dialect::MySql(_), Some(len)) => (format!("IF({len} <= {GROUP_PREVIEW}, {c}, NULL)"), len),
    };
    let count = match d {
        Dialect::Postgres => "count(*)",
        Dialect::MySql(_) => "COUNT(*)",
    };
    let limit = GROUP_VALUES + 1;
    format!("SELECT {preview}, {length}, {count} FROM {from}{filter} GROUP BY {c} ORDER BY {count} DESC LIMIT {limit}")
}

/// The byte length of value `c` (column `column` quoted) when its type may hold a long one; `None`
/// for a type whose values are short (a number, a date, …). PostgreSQL measures text and bytes
/// as they are; JSON, arrays and `numeric` by their text, written by the type's own output
/// function (no cast, which could be one of the user's).
fn length(d: Dialect, column: &ColumnMeta, c: &str) -> Option<String> {
    let ty = column.type_name.to_ascii_lowercase();
    match d {
        Dialect::Postgres => match column.kind {
            ValueKind::Text | ValueKind::Bytes | ValueKind::Bit => Some(format!("octet_length({c})")),
            ValueKind::Other if ty == "citext" => Some(format!("octet_length({c})")),
            ValueKind::Json if ty == "jsonb" => Some(format!("octet_length(textin(jsonb_out({c})))")),
            ValueKind::Array(_) => Some(format!("octet_length(textin(array_out({c})))")),
            ValueKind::Decimal if ty == "numeric" => Some(format!("octet_length(textin(numeric_out({c})))")),
            _ => None,
        },
        Dialect::MySql(_) => {
            matches!(column.kind, ValueKind::Text | ValueKind::Bytes | ValueKind::Json).then(|| format!("LENGTH({c})"))
        }
    }
}
