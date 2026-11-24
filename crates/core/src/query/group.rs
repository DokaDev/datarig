//! The values of one column of a table and how many rows hold each, counted by the server
//! ([`group_count`]): `SELECT <preview>, <length>, count(*) FROM <table> [WHERE (<other>)]
//! GROUP BY <column> ORDER BY count(*) DESC LIMIT 201`. On PostgreSQL every function and operator
//! the app writes there is named with `pg_catalog`, so that none of them depends on the session's
//! search path (the filter text inside stays as the user sees it).
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
//! most [`GROUP_VALUES`] + 1 values of that size. On PostgreSQL, types of a short value of fixed
//! size (numbers, dates and times, booleans, `uuid`, …) are sent as they are; on MySQL every
//! value is measured (`OCTET_LENGTH`), whatever its type is said to be. Only a column whose values a
//! filter compares for equality is grouped ([`compares`]).

use super::check::{BarRefusal, FilterVerdict, check_filter};
use super::{TableName, Target};
use crate::driver::ColumnMeta;
use crate::engine::{Fact, FilterSql, GroupParts};
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
/// with an order, or for equality only, by the engine's filter rules
/// ([`FilterSql::compares`]); not at all ([`NoEquality`]) on an engine without them.
/// `enum_known`: the column's type is known to be a PostgreSQL enum (no kind of its own,
/// ordered by its labels).
pub fn compares(d: Dialect, m: &ColumnMeta, enum_known: bool) -> Result<Compares, NoEquality> {
    super::filter_sql(d).map_err(|_| NoEquality)?.compares(m, enum_known)
}

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
    counted(g, &g.target.dialect.engine().spec().filter)
}

/// [`group_count`] by the filter rules of `group`, the engine's filter group: refused as the
/// filter bar refuses a filter where it has none.
pub(super) fn counted(g: &GroupCount<'_>, group: &Fact<&'static dyn FilterSql>) -> Result<String, GroupRefusal> {
    let t = g.target;
    let d = t.dialect;
    let name = g.column.name.as_str();
    if t.column(name).is_none() {
        return Err(GroupRefusal::UnknownColumn);
    }
    let rules = super::known(group).map_err(|why| GroupRefusal::Filter(BarRefusal::Engine(why)))?;
    rules.compares(g.column, g.enum_known).map_err(|_| GroupRefusal::NoEquality)?;
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
    let sql = statement(d, rules, g.table, g.column, other);
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

/// The statement of [`group_count`], `other` checked already: the items `rules` read of the
/// column ([`FilterSql::group_parts`]) around one grouped read of the table.
fn statement(d: Dialect, rules: &dyn FilterSql, table: &TableName, column: &ColumnMeta, other: Option<&str>) -> String {
    let c = d.force_quote_ident(&column.name);
    let from = format!("{}.{}", d.force_quote_ident(&table.schema), d.force_quote_ident(&table.name));
    let filter = other.map_or(String::new(), |o| format!(" WHERE ({o})"));
    let GroupParts { preview, length, count } = rules.group_parts(column, &c);
    let limit = GROUP_VALUES + 1;
    format!("SELECT {preview}, {length}, {count} FROM {from}{filter} GROUP BY {c} ORDER BY {count} DESC LIMIT {limit}")
}
