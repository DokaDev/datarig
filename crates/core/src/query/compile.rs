//! A [`Query`] as one `SELECT` statement of the target's dialect.

use super::{ColName, Dir, KeyOrigin, Nulls, OrderKey, Query, Source, Target};
use crate::sql::dialect::Dialect;
use std::ops::Range;

/// A query written as SQL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compiled {
    pub sql: String,
    /// Where the filter's text is in `sql` (bytes): a server's error position inside it maps
    /// back to the text the user typed. `None` without a filter.
    pub filter_span: Option<Range<usize>>,
    /// The sort keys written, the tie-breaker keys appended ([`KeyOrigin::TieBreak`]).
    pub order: Vec<OrderKey>,
    /// The order is total: its columns hold the primary key or a unique key whose columns are
    /// all `NOT NULL`, so no two rows tie and pages read again follow one order. `false` when
    /// that is not known.
    pub total: bool,
}

/// `q` as the statement the grid runs: `SELECT * FROM <table> [WHERE (<filter>)] [ORDER BY …]`.
/// Neither `limit` nor `offset` is written: the pager reads the rows a page at a time and stops
/// at the cap (on MySQL a `LIMIT` in the text would override the cap the driver sets per page).
pub fn compile(q: &Query, t: &Target) -> Compiled {
    build(q, t, false)
}

/// `q` as a statement that reads the whole set at once (an export, a count around it): as
/// [`compile`], with `LIMIT` and `OFFSET` written when `q` sets them.
pub fn compile_capped(q: &Query, t: &Target) -> Compiled {
    build(q, t, true)
}

fn build(q: &Query, t: &Target, capped: bool) -> Compiled {
    let d = t.dialect;
    let Source::Table(table) = &q.source;
    let mut sql = format!("SELECT * FROM {}.{}", d.force_quote_ident(&table.schema), d.force_quote_ident(&table.name));
    let mut filter_span = None;
    if let Some(f) = q.filter.as_ref().filter(|f| !f.text.trim().is_empty()) {
        sql.push_str(" WHERE (");
        let start = sql.len();
        sql.push_str(&f.text);
        filter_span = Some(start..sql.len());
        sql.push(')');
    }
    let order = keys(&q.order, t);
    if !order.is_empty() {
        let keys: Vec<String> = order.iter().map(|k| key_sql(d, k)).collect();
        sql.push_str(" ORDER BY ");
        sql.push_str(&keys.join(", "));
    }
    if capped {
        match (q.limit, q.offset, d) {
            (Some(n), Some(m), _) => sql.push_str(&format!(" LIMIT {n} OFFSET {m}")),
            (Some(n), None, _) => sql.push_str(&format!(" LIMIT {n}")),
            (None, Some(m), Dialect::Postgres) => sql.push_str(&format!(" OFFSET {m}")),
            // MySQL has no `OFFSET` without a `LIMIT`: the largest one stands for none.
            (None, Some(m), Dialect::MySql(_)) => sql.push_str(&format!(" LIMIT {} OFFSET {m}", u64::MAX)),
            (None, None, _) => {}
        }
    }
    let total = !order.is_empty() && total(&order, t);
    Compiled { sql, filter_span, order, total }
}

/// The user's keys of `order`, then (with [`Target::tie_break`]) the primary key's columns
/// that are not among them, ascending. No keys at all stay none: an unordered read.
fn keys(order: &[OrderKey], t: &Target) -> Vec<OrderKey> {
    let mut out: Vec<OrderKey> = order.iter().filter(|k| k.origin == KeyOrigin::User).cloned().collect();
    if out.is_empty() || !t.tie_break {
        return out;
    }
    for col in &t.primary_key {
        if !out.iter().any(|k| &k.col == col) {
            out.push(OrderKey { col: col.clone(), dir: Dir::Asc, nulls: None, origin: KeyOrigin::TieBreak });
        }
    }
    out
}

/// Whether the columns of `order` hold a key no two rows share.
fn total(order: &[OrderKey], t: &Target) -> bool {
    let has = |key: &[ColName]| !key.is_empty() && key.iter().all(|c| order.iter().any(|k| &k.col == c));
    has(&t.primary_key) || t.unique_keys.iter().any(|k| has(k))
}

/// One sort key as SQL. MySQL has no `NULLS FIRST`/`LAST`: it sorts on whether the value is
/// NULL first.
fn key_sql(d: Dialect, k: &OrderKey) -> String {
    let col = d.force_quote_ident(&k.col);
    let dir = match k.dir {
        Dir::Asc => "ASC",
        Dir::Desc => "DESC",
    };
    match (d, k.nulls) {
        (_, None) => format!("{col} {dir}"),
        (Dialect::Postgres, Some(Nulls::First)) => format!("{col} {dir} NULLS FIRST"),
        (Dialect::Postgres, Some(Nulls::Last)) => format!("{col} {dir} NULLS LAST"),
        (Dialect::MySql(_), Some(Nulls::First)) => format!("{col} IS NULL DESC, {col} {dir}"),
        (Dialect::MySql(_), Some(Nulls::Last)) => format!("{col} IS NULL ASC, {col} {dir}"),
    }
}
