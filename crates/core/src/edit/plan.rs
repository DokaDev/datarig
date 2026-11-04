//! A result's pending changes as the statements that apply them, each with the count it must
//! give, fixed before anything runs: what the preview shows is what is sent.
//!
//! In order: a check of every existing row the changes touch (it locks the row and counts the
//! rows its condition matches, before anything is written), then the deletes, the updates and
//! the inserts. Deleting first frees unique values the new rows may take.
//!
//! This is the only code that asks a format for an `UPDATE` or a `DELETE`, and the condition it
//! gives is never empty: every statement is read back as a statement of the format's language
//! and kept only when it is one statement of the kind asked for, and a write is limited by the
//! condition as it was given and nothing else ([`shaped`]). Anything else is an internal error,
//! and nothing runs.

use super::batch::{Counts, Draft, EditBatch, RowEdit};
use super::sql::{EditSql, Exactness, Scope, Value};
use super::target::{ColumnReason, Duplicates, EditTarget, Identity, TableColumn, TableRef, Write};
use crate::driver::structure::ColumnFill;
use crate::sql::dialect::Language;
use crate::sql::lexer::{Tok, Token, lex_in};
use crate::sql::risk::{Class, Classifier};
use crate::sql::split::split_in;
use std::collections::{BTreeMap, BTreeSet};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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
    /// Whether the value of cell (row, column) was read whole, so it may go in a condition: not a
    /// value cut short, nor one read in a character set or a time zone that changes it (a MySQL
    /// `TIMESTAMP` shown in a zone with daylight saving is two instants in the hour the clock goes
    /// back), as the copy as SQL refuses them (`export::NotWhole`).
    pub whole: &'a dyn Fn(usize, usize) -> bool,
    /// For rows found by their values: how many rows of the table matched, as counted
    /// ([`count_statements`]).
    pub matches: &'a BTreeMap<usize, u64>,
    /// For rows that matched several rows the same in every column: the user's choice.
    pub choices: &'a BTreeMap<usize, Choice>,
    /// Cells (row or new row, result column) known to hold a value that is not one of their
    /// column's type, as checked where they were set: none of them is ever sent.
    pub invalid: &'a BTreeSet<(Origin, usize)>,
    /// The batch revision ([`EditBatch::revision`]) `invalid` was found for: for any other, it is
    /// not known which cells are invalid, and nothing is planned.
    pub invalid_for: u64,
}

/// Why the changes cannot be applied as they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    /// A cell set holds a value its column does not take ([`Facts::invalid`]).
    Invalid { origin: Origin, column: usize },
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
    /// A new row leaves a column to its default that has none and takes no NULL: the server
    /// would refuse the row, or (MySQL, not strict) store an empty value of its own.
    NoDefault { origin: Origin, column: usize },
    /// A new row leaves `column` of the table, which the result does not show, to its default,
    /// which it lacks (as [`PlanError::NoDefault`]).
    NotShown { origin: Origin, column: String },
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
    // Every value as read is known: a missing one is not NULL.
    if e.orig.len() != t.columns.len() {
        return Err(internal("a row without its values as read"));
    }
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
        verify(f, &t.table, &sql, Kind::Count(&w))?;
        out.push((row, sql));
    }
    Ok(out)
}

/// The cells of new rows left to their column's default (new row, result column) where the
/// column has none: `NOT NULL`, no default expression, not numbered nor computed by the table.
pub fn no_default(t: &EditTarget, b: &EditBatch) -> Vec<(u64, usize)> {
    let none = |c: usize| {
        t.columns
            .get(c)
            .and_then(Option::as_ref)
            .is_some_and(|c| c.column.not_null && c.column.default.is_none() && c.column.fill == ColumnFill::Default)
    };
    b.drafts()
        .iter()
        .flat_map(|d| d.cells.iter().enumerate().map(move |(c, v)| (d.id, c, v)))
        .filter(|&(_, c, v)| *v == Value::Default && none(c))
        .map(|(id, c, _)| (id, c))
        .collect()
}

/// The new rows of batch `b` and the columns of the table they leave to a default it lacks
/// because the result does not show them ([`EditTarget::unshown_required`]), each new row with
/// each column.
pub fn not_shown(t: &EditTarget, b: &EditBatch) -> Vec<(u64, String)> {
    b.drafts().iter().flat_map(|d| t.unshown_required.iter().map(move |c| (d.id, c.clone()))).collect()
}

/// The cells of batch `b` that [`Facts::invalid`] has, each as its problem, in order: cells set
/// in rows of the result (not ones a row to be deleted had), then cells of new rows that are not
/// left to their default. When the set was found for another revision of `b`, that alone.
fn invalid(b: &EditBatch, facts: &Facts) -> Vec<PlanError> {
    if facts.invalid_for != b.revision() {
        return vec![PlanError::Internal(format!(
            "invalid cells found for revision {}, the changes are at {}",
            facts.invalid_for,
            b.revision()
        ))];
    }
    let touched = |o: Origin, c: usize| match o {
        Origin::Row(r) => b.value(r, c).is_some(),
        Origin::New(id) => {
            b.drafts().iter().any(|d| d.id == id && d.cells.get(c).is_some_and(|v| *v != Value::Default))
        }
    };
    facts
        .invalid
        .iter()
        .filter(|&&(o, c)| touched(o, c))
        .map(|&(origin, column)| PlanError::Invalid { origin, column })
        .collect()
}

/// The statements that apply batch `b` to target `t` in format `f`. Nothing is made while a cell
/// it sets is invalid, nor while the invalid cells are not known for this revision of `b`.
pub fn plan(t: &EditTarget, b: &EditBatch, f: &dyn EditSql, facts: &Facts) -> Result<Plan, PlanError> {
    if let Some(e) = invalid(b, facts).into_iter().next() {
        return Err(e);
    }
    if let Some(&(id, column)) = no_default(t, b).first() {
        return Err(PlanError::NoDefault { origin: Origin::New(id), column });
    }
    if let Some((id, column)) = not_shown(t, b).into_iter().next() {
        return Err(PlanError::NotShown { origin: Origin::New(id), column });
    }
    let (mut checks, mut deletes, mut updates, mut inserts) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (row, e) in b.rows() {
        let origin = Origin::Row(row);
        let (w, n, scope) = row_checks(t, row, e, f, facts)?;
        let check = f.check(&t.table, &w);
        verify(f, &t.table, &check, Kind::Check(&w))?;
        checks.push(Step { sql: check, expect: Expect::Rows(n), origin });
        let affected = Expect::Affected(if scope == Scope::One { 1 } else { n });
        let sql = write_sql(t, row, e, f, &w, scope)?;
        let kind = if e.delete { Kind::Delete(&w, scope) } else { Kind::Update(&w, scope) };
        verify(f, &t.table, &sql, kind)?;
        let step = Step { sql, expect: affected, origin };
        if e.delete { deletes.push(step) } else { updates.push(step) }
    }
    for d in b.drafts() {
        let cells = insert_cells(t, d)?.into_iter().map(|(col, v)| (col, f.value(col, v))).collect::<Vec<_>>();
        let sql = f.insert(&t.table, &cells);
        verify(f, &t.table, &sql, Kind::Insert)?;
        inserts.push(Step { sql, expect: Expect::Affected(1), origin: Origin::New(d.id) });
    }
    let steps = checks.into_iter().chain(deletes).chain(updates).chain(inserts).collect();
    Ok(Plan { steps, counts: b.counts() })
}

/// Every reason the changes cannot be applied, of every row, in the order [`plan`] meets them:
/// every invalid cell first, then the checks [`plan`] makes of each row and new row, without the
/// statements, so the first is the one [`plan`] stops at. A row or a target that is not as it
/// must be ([`PlanError::Internal`]: a row without its values as read, a condition on a computed
/// column) is among them; a statement the format writes wrong (also [`PlanError::Internal`]) is
/// found only by [`plan`], which reads each one back. Invalid cells found for another revision
/// are the only problem ([`PlanError::Internal`]).
pub fn plan_problems(t: &EditTarget, b: &EditBatch, f: &dyn EditSql, facts: &Facts) -> Vec<PlanError> {
    let mut out: Vec<PlanError> = invalid(b, facts);
    if matches!(out.first(), Some(PlanError::Internal(_))) {
        return out;
    }
    out.extend(
        no_default(t, b)
            .into_iter()
            .map(|(id, column)| PlanError::NoDefault { origin: Origin::New(id), column })
            .chain(
                not_shown(t, b).into_iter().map(|(id, column)| PlanError::NotShown { origin: Origin::New(id), column }),
            ),
    );
    for (row, e) in b.rows() {
        // The write is made only to learn whether the format can change one of several rows.
        let checked = row_checks(t, row, e, f, facts).and_then(|(w, _, scope)| {
            if scope == Scope::One { write_sql(t, row, e, f, &w, scope).map(drop) } else { Ok(()) }
        });
        out.extend(checked.err());
    }
    out.extend(b.drafts().iter().filter_map(|d| insert_cells(t, d).err()));
    out
}

/// The checks of changed row `row` (`e` its changes) before any statement: the cells it sets may
/// be set, the condition that finds it, and how many rows of the table that is (with the scope of
/// its write).
fn row_checks(
    t: &EditTarget,
    row: usize,
    e: &RowEdit,
    f: &dyn EditSql,
    facts: &Facts,
) -> Result<(Where, u64, Scope), PlanError> {
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
    Ok((w, n, scope))
}

/// The `DELETE` or `UPDATE` of changed row `row`, as the format writes it (not yet read back).
fn write_sql(
    t: &EditTarget,
    row: usize,
    e: &RowEdit,
    f: &dyn EditSql,
    w: &Where,
    scope: Scope,
) -> Result<String, PlanError> {
    let write = if e.delete {
        f.delete(&t.table, w, scope)
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
        f.update(&t.table, &set, w, scope)
    };
    write.ok_or(PlanError::NoOneOfK { row })
}

/// The cells new row `d` gives (every one not left to its default), each with its column, which
/// must take it.
fn insert_cells<'a>(t: &'a EditTarget, d: &'a Draft) -> Result<Vec<(&'a TableColumn, &'a Value)>, PlanError> {
    let origin = Origin::New(d.id);
    d.cells
        .iter()
        .enumerate()
        .filter(|(_, v)| **v != Value::Default)
        .map(|(c, v)| {
            let col = t.allows(c, Write::Insert).map_err(|why| PlanError::Column { origin, column: c, why })?;
            Ok((col, v))
        })
        .collect()
}

/// What a statement must be.
#[derive(Clone, Copy)]
enum Kind<'a> {
    /// A plain read that counts the rows `w` matches.
    Count(&'a Where),
    /// A read that locks the rows `w` matches.
    Check(&'a Where),
    Update(&'a Where, Scope),
    Delete(&'a Where, Scope),
    Insert,
}

/// Read `sql` back in the format's language: one statement, without comments, of the kind it
/// must be, a read that changes nothing or a write of that kind; an `UPDATE` or a `DELETE` of
/// the shape [`shaped`] allows.
fn verify(f: &dyn EditSql, t: &TableRef, sql: &str, kind: Kind) -> Result<(), PlanError> {
    let lang = f.language();
    let Language::Sql(d) = lang;
    let bad = || PlanError::Internal(sql.to_string());
    let toks: Vec<Token> = lex_in(sql, d).into_iter().filter(|t| t.kind != Tok::Whitespace).collect();
    let noise = |t: &Token| {
        matches!(t.kind, Tok::LineComment | Tok::BlockComment | Tok::ExecComment | Tok::Directive | Tok::Semi)
    };
    if split_in(sql, d).len() != 1 || toks.iter().any(noise) || !names(f, t) {
        return Err(bad());
    }
    let first = toks.first().filter(|t| t.is_word()).map(|t| t.text(sql).to_ascii_uppercase()).unwrap_or_default();
    let r = Classifier::classify_once(lang, sql);
    // A read of the table's rows that `w` matches.
    let reads = |w: &Where| sql.contains(&format!(" FROM {} WHERE {}", f.table(t), w.as_str()));
    let ok = match kind {
        Kind::Count(w) => first == "SELECT" && r.class == Class::Read && !r.writes && reads(w),
        Kind::Check(w) => first == "SELECT" && matches!(r.class, Class::Read | Class::Write) && !r.writes && reads(w),
        Kind::Update(w, scope) => {
            let head = format!("UPDATE {} SET ", f.table(t));
            // What it sets: values only, from no other table.
            let set = sql.strip_prefix(&head).and_then(|rest| rest.find(" WHERE ").map(|i| &rest[..i]));
            let plain = set.is_some_and(|set| !has_word(set, d, &["FROM", "JOIN", "USING", "SELECT", "WHERE"]));
            plain && write(&r) && shaped(f, sql, w, scope)
        }
        Kind::Delete(w, scope) => {
            sql.starts_with(&format!("DELETE FROM {} WHERE ", f.table(t))) && write(&r) && shaped(f, sql, w, scope)
        }
        Kind::Insert => {
            let rest = sql.strip_prefix(&format!("INSERT INTO {} ", f.table(t)));
            rest.is_some_and(|rest| !has_word(rest, d, &["SELECT", "WHERE"])) && r.writes && r.class == Class::Write
        }
    };
    if ok { Ok(()) } else { Err(bad()) }
}

/// The format's name of table `t` ([`EditSql::table`]) is `t`'s, as the server reads it: a schema,
/// a dot and a name, each quoted and unquoted to the catalog's name, or bare and folded to it.
fn names(f: &dyn EditSql, t: &TableRef) -> bool {
    let Language::Sql(d) = f.language();
    let text = f.table(t);
    let toks: Vec<Token> = lex_in(&text, d).into_iter().filter(|x| !x.is_trivia()).collect();
    let ident = |x: &Token| match x.kind {
        Tok::QuotedIdent => d.unquote(x.text(&text)),
        Tok::Ident | Tok::Keyword => Some(d.fold(x.text(&text))),
        _ => None,
    };
    match toks.as_slice() {
        [s, dot, n] if dot.kind == Tok::Dot => {
            ident(s).is_some_and(|s| s == t.schema) && ident(n).is_some_and(|n| n == t.name)
        }
        _ => false,
    }
}

/// `text` (in dialect `d`) has one of `words` as a word of its own (not in a string or a quoted
/// name).
fn has_word(text: &str, d: crate::sql::dialect::Dialect, words: &[&str]) -> bool {
    lex_in(text, d).iter().any(|t| t.is_word() && words.iter().any(|w| t.text(text).eq_ignore_ascii_case(w)))
}

/// A write with a condition the classifier finds reads a column.
fn write(r: &crate::sql::risk::Risk) -> bool {
    r.class == Class::Write && r.writes && r.no_where.is_none()
}

/// The condition `w` is what limits the write: it is at the end, right after a `WHERE`, but for
/// one of the format's own ends ([`EditSql::tails`]); the rest of the text has no `AND`, `OR` or
/// `NOT` of its own (a second copy of the condition would), and one `WHERE` (two for one of
/// several rows, whose second is in the subquery that picks the row).
fn shaped(f: &dyn EditSql, sql: &str, w: &Where, scope: Scope) -> bool {
    let Language::Sql(d) = f.language();
    let w = w.as_str();
    // The last: a value set may hold the condition's text in a literal.
    let Some(at) = sql.rfind(w) else { return false };
    let (before, after) = (&sql[..at], &sql[at + w.len()..]);
    if !before.ends_with(" WHERE ") || !f.tails().contains(&after) {
        return false;
    }
    let words = |part: &str| -> Vec<String> {
        lex_in(part, d).into_iter().filter(|t| t.is_word()).map(|t| t.text(part).to_ascii_uppercase()).collect()
    };
    let outside: Vec<String> = words(before).into_iter().chain(words(after)).collect();
    let wheres = outside.iter().filter(|x| *x == "WHERE").count();
    let most = if scope == Scope::One { 2 } else { 1 };
    !outside.iter().any(|x| matches!(x.as_str(), "AND" | "OR" | "NOT")) && (1..=most).contains(&wheres)
}

#[cfg(test)]
mod tests;
