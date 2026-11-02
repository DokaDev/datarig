//! A result's pending changes as the statements that apply them, each with the count it must
//! give, fixed before anything runs: what the preview shows is what is sent.
//!
//! In order: a check of every existing row the changes touch (it locks the row and counts the
//! rows its condition matches, before anything is written), then the deletes, the updates and
//! the inserts. Deleting first frees unique values the new rows may take.
//!
//! This is the only code that asks a format for an `UPDATE` or a `DELETE`, and the condition it
//! gives is never empty: every statement is read back as a statement of the format's language
//! and kept only when it is one statement, a write with a condition that reads a column, and
//! holds the condition as it was given. Anything else is an internal error, and nothing runs.

use super::batch::{Counts, EditBatch, RowEdit};
use super::sql::{EditSql, Exactness, Scope, Value};
use super::target::{ColumnReason, Duplicates, EditTarget, Identity, Write};
use crate::sql::dialect::Language;
use crate::sql::risk::Classifier;
use crate::sql::split::split_in;
use std::collections::BTreeMap;

/// The condition that finds a row: never empty, and made only here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Where(String);

impl Where {
    /// Every one of `parts`; `None` for none.
    fn all(parts: Vec<String>) -> Option<Self> {
        (!parts.is_empty()).then(|| Self(parts.join(" AND ")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a statement must give for the changes to stand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    /// A check: this many rows returned.
    Rows(u64),
    /// A write: this many rows changed (matched).
    Affected(u64),
}

/// What a statement is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A row of the result (its index in the row store).
    Row(usize),
    /// A new row (its id in the batch).
    New(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub sql: String,
    pub expect: Expect,
    pub origin: Origin,
}

/// The statements of one Apply, in the order they run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    pub counts: Counts,
}

/// Which of several rows the same in every column a change is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    One,
    All,
}

/// What the planner needs beyond the batch.
pub struct Facts<'a> {
    /// Whether the value of cell (row, column) was read whole, so it may go in a condition (a
    /// value cut short, or read in a character set or time zone that changes it, may not).
    pub whole: &'a dyn Fn(usize, usize) -> bool,
    /// For rows found by their values: how many rows of the table matched, as counted
    /// ([`count_statements`]).
    pub matches: &'a BTreeMap<usize, u64>,
    /// For rows that matched several rows the same in every column: the user's choice.
    pub choices: &'a BTreeMap<usize, Choice>,
}

/// Why the changes cannot be applied as they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    /// A cell is set that may not be.
    Column { origin: Origin, column: usize, why: ColumnReason },
    /// A value that finds the row was not read whole.
    NotWhole { row: usize, column: usize },
    /// The row was not counted (rows found by their values are, before a plan).
    NotCounted { row: usize },
    /// No row of the table matches the row any more: it changed or was deleted.
    Gone { row: usize },
    /// The row matches several rows that may differ in what the result does not show.
    Ambiguous { row: usize, matches: u64 },
    /// The row matches several rows the same in every column: one, or all?
    Choose { row: usize, matches: u64 },
    /// The format cannot change just one of several rows.
    NoOneOfK { row: usize },
    /// A statement the format wrote is not what it must be. A bug: nothing runs.
    Internal(String),
}

/// The condition that finds row `row` (`e` its changes): the values of its identity as read,
/// and, so that a change made elsewhere since is not overwritten, each changed column's (an
/// update's) or every column's (a delete's) that compares exactly and was read whole.
fn locate(
    t: &EditTarget,
    row: usize,
    e: &RowEdit,
    f: &dyn EditSql,
    whole: &dyn Fn(usize, usize) -> bool,
) -> Result<Where, PlanError> {
    let (identity, extra): (&[usize], Vec<usize>) = match &t.identity {
        Identity::Key { columns, .. } => {
            let checked: Vec<usize> = if e.delete { t.exact_columns() } else { e.set.keys().copied().collect() };
            (columns, checked.into_iter().filter(|c| !columns.contains(c)).collect())
        }
        Identity::Values { columns, .. } => (columns, Vec::new()),
    };
    let mut parts = Vec::new();
    for &c in identity {
        if !whole(row, c) {
            return Err(PlanError::NotWhole { row, column: c });
        }
        parts.push((c, e.orig.get(c).cloned().flatten()));
    }
    for c in extra {
        let exact = t.columns.get(c).and_then(Option::as_ref).is_some_and(|x| x.exact == Exactness::Exact);
        if exact && whole(row, c) {
            parts.push((c, e.orig.get(c).cloned().flatten()));
        }
    }
    let parts = parts
        .into_iter()
        .map(|(c, v)| {
            let col = t
                .columns
                .get(c)
                .and_then(Option::as_ref)
                .ok_or_else(|| internal("a condition on a computed column"))?;
            Ok(f.eq_exact(col, v.as_deref()))
        })
        .collect::<Result<Vec<String>, PlanError>>()?;
    Where::all(parts).ok_or_else(|| internal("an empty condition"))
}

fn internal(what: &str) -> PlanError {
    PlanError::Internal(what.to_string())
}

/// The statements that count the rows each changed row matches, for the rows found by their
/// values (none for rows found by a key): plain reads, run before the plan is made.
pub fn count_statements(
    t: &EditTarget,
    b: &EditBatch,
    f: &dyn EditSql,
    whole: &dyn Fn(usize, usize) -> bool,
) -> Result<Vec<(usize, String)>, PlanError> {
    if matches!(t.identity, Identity::Key { .. }) {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (row, e) in b.rows() {
        let w = locate(t, row, e, f, whole)?;
        let sql = f.count(&t.table, &w);
        verify(f, &sql, Kind::Read)?;
        out.push((row, sql));
    }
    Ok(out)
}

/// The statements that apply batch `b` to target `t` in format `f`.
pub fn plan(t: &EditTarget, b: &EditBatch, f: &dyn EditSql, facts: &Facts) -> Result<Plan, PlanError> {
    let (mut checks, mut deletes, mut updates, mut inserts) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (row, e) in b.rows() {
        let origin = Origin::Row(row);
        for &c in e.set.keys() {
            t.allows(c, Write::Update).map_err(|why| PlanError::Column { origin, column: c, why })?;
        }
        let w = locate(t, row, e, f, facts.whole)?;
        let (n, scope) = match &t.identity {
            Identity::Key { .. } => (1, Scope::All),
            Identity::Values { duplicates, .. } => match facts.matches.get(&row).copied() {
                None => return Err(PlanError::NotCounted { row }),
                Some(0) => return Err(PlanError::Gone { row }),
                Some(1) => (1, Scope::All),
                Some(k) => match (duplicates, facts.choices.get(&row)) {
                    (Duplicates::Refuse, _) => return Err(PlanError::Ambiguous { row, matches: k }),
                    (Duplicates::Choose, None) => return Err(PlanError::Choose { row, matches: k }),
                    (Duplicates::Choose, Some(Choice::All)) => (k, Scope::All),
                    (Duplicates::Choose, Some(Choice::One)) => (k, Scope::One),
                },
            },
        };
        let check = f.check(&t.table, &w);
        verify(f, &check, Kind::Read)?;
        checks.push(Step { sql: check, expect: Expect::Rows(n), origin });
        let affected = Expect::Affected(if scope == Scope::One { 1 } else { n });
        let write = if e.delete {
            f.delete(&t.table, &w, scope)
        } else {
            let set = e
                .set
                .iter()
                .map(|(c, v)| {
                    let col =
                        t.columns.get(*c).and_then(Option::as_ref).ok_or_else(|| internal("a computed column set"))?;
                    Ok((col, f.value(col, v)))
                })
                .collect::<Result<Vec<_>, PlanError>>()?;
            f.update(&t.table, &set, &w, scope)
        };
        let sql = write.ok_or(PlanError::NoOneOfK { row })?;
        verify(f, &sql, Kind::Write(&w))?;
        let step = Step { sql, expect: affected, origin };
        if e.delete { deletes.push(step) } else { updates.push(step) }
    }
    for d in b.drafts() {
        let origin = Origin::New(d.id);
        let mut cells = Vec::new();
        for (c, v) in d.cells.iter().enumerate() {
            if *v == Value::Default {
                continue;
            }
            let col = t.allows(c, Write::Insert).map_err(|why| PlanError::Column { origin, column: c, why })?;
            cells.push((col, f.value(col, v)));
        }
        let sql = f.insert(&t.table, &cells);
        verify(f, &sql, Kind::Insert)?;
        inserts.push(Step { sql, expect: Expect::Affected(1), origin });
    }
    let steps = checks.into_iter().chain(deletes).chain(updates).chain(inserts).collect();
    Ok(Plan { steps, counts: b.counts() })
}

/// What a statement must be.
enum Kind<'a> {
    /// A read that changes nothing.
    Read,
    /// An `UPDATE` or a `DELETE` with condition `w`.
    Write(&'a Where),
    Insert,
}

/// Read `sql` back in the format's language: one statement, of the kind it must be.
fn verify(f: &dyn EditSql, sql: &str, kind: Kind) -> Result<(), PlanError> {
    let lang = f.language();
    let Language::Sql(d) = lang;
    let bad = || PlanError::Internal(sql.to_string());
    if split_in(sql, d).len() != 1 {
        return Err(bad());
    }
    let r = Classifier::classify_once(lang, sql);
    let ok = match kind {
        Kind::Read => !r.writes,
        Kind::Write(w) => r.writes && r.no_where.is_none() && sql.contains(w.as_str()),
        Kind::Insert => r.writes,
    };
    if ok { Ok(()) } else { Err(bad()) }
}

#[cfg(test)]
mod tests;
