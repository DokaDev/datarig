//! A [`Cond`] as the text an app action writes into the filter bar.
//!
//! Literals are written for the column they are compared with: a number bare only for a number
//! column and a valid number literal (else a string the server converts); bytes as PostgreSQL's
//! `'\x…'::pg_catalog.bytea` (an escape string, see below; the type is named with its schema, so
//! that a domain of that name earlier on the search path is not the one cast to) or MySQL's
//! `X'…'`; text as a string literal. A PostgreSQL string with a backslash is an escape string (`E'…'`), which reads the same whatever
//! `standard_conforming_strings` is. A MySQL string is plain only when the session's character
//! set is known to be `utf8mb4` and it holds no backslash or NUL (which `NO_BACKSLASH_ESCAPES`
//! reads otherwise); else it is its UTF-8 bytes, `_utf8mb4 X'…'`.
//!
//! NULL is never compared with `=`: [`Cond::IsNull`] and the NULL-safe [`Cond::NotDistinct`]
//! (PostgreSQL `IS [NOT] DISTINCT FROM`, MySQL `<=>` and `NOT (… <=> …)`). A NULL among the
//! values of [`Cond::In`] is taken out of the list and tested on its own, so it matches as the
//! user expects: `(col IN (…) OR col IS NULL)`, `(col NOT IN (…) AND col IS NOT NULL)`.
//!
//! Patterns are escaped with `!` and written with `ESCAPE '!'`: a backslash would read
//! differently under MySQL's `NO_BACKSLASH_ESCAPES`.

use super::{CaseMode, CmpOp, ColumnFacts, ColumnKind, Cond, Target, Value};
use crate::export::{negative_zero, sql_number};
use crate::sql::dialect::{Dialect, LiteralOpen};

/// Why a condition is not written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// A comparison or a `BETWEEN` with NULL, which matches no row: the column.
    NullCompare(String),
    /// `IN` of no value: the column.
    EmptyList(String),
    /// A case-insensitive pattern on a PostgreSQL column whose collation is not deterministic,
    /// which the server refuses for `ILIKE`: the column.
    CaseInsensitiveRefused(String),
}

/// The escape character of the patterns written here.
const ESCAPE: char = '!';

/// `value` escaped for a pattern ([`ESCAPE`]), so that its `%` and `_` match themselves.
fn escape_like(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if matches!(c, '%' | '_') || c == ESCAPE {
            out.push(ESCAPE);
        }
        out.push(c);
    }
    out
}

/// The pattern of the values that start with `value` (for [`Cond::Like`]).
pub fn like_starts_with(value: &str) -> String {
    format!("{}%", escape_like(value))
}

/// The pattern of the values that contain `value` (for [`Cond::Like`]).
pub fn like_contains(value: &str) -> String {
    format!("%{}%", escape_like(value))
}

/// Where a condition is written: the operator around it decides whether it needs parentheses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Top,
    Operand,
}

/// `c` as filter text of `t`'s dialect. A condition of several parts that is not all `AND`s is
/// in parentheses, so that `AND` may be written after it.
pub fn render(c: &Cond, t: &Target) -> Result<String, RenderError> {
    render_at(c, t, Place::Top)
}

fn render_at(c: &Cond, t: &Target, place: Place) -> Result<String, RenderError> {
    let d = t.dialect;
    let col = |name: &str| d.quote_ident(name);
    let lit = |name: &str, v: &Value| literal(t, t.column(name), v);
    let not = |negated: bool| if negated { "NOT " } else { "" };
    Ok(match c {
        Cond::All(cs) if cs.len() == 1 => render_at(&cs[0], t, place)?,
        Cond::Any(cs) if cs.len() == 1 => render_at(&cs[0], t, place)?,
        Cond::All(cs) if cs.is_empty() => "TRUE".to_string(),
        Cond::Any(cs) if cs.is_empty() => "FALSE".to_string(),
        Cond::All(cs) => {
            let parts = cs.iter().map(|c| render_at(c, t, Place::Operand)).collect::<Result<Vec<_>, _>>()?;
            let s = parts.join(" AND ");
            if place == Place::Top { s } else { format!("({s})") }
        }
        Cond::Any(cs) => {
            let parts = cs.iter().map(|c| render_at(c, t, Place::Operand)).collect::<Result<Vec<_>, _>>()?;
            format!("({})", parts.join(" OR "))
        }
        Cond::Not(inner) => format!("NOT {}", grouped(d, render_at(inner, t, Place::Operand)?)),
        Cond::Raw(s) => grouped(d, s.clone()),
        Cond::Cmp { col: name, op, value } => {
            if *value == Value::Null {
                return Err(RenderError::NullCompare(name.clone()));
            }
            format!("{} {} {}", col(name), op.sql(), lit(name, value))
        }
        Cond::NotDistinct { col: name, value, negated } => match (d, negated) {
            (Dialect::Postgres, false) => format!("{} IS NOT DISTINCT FROM {}", col(name), lit(name, value)),
            (Dialect::Postgres, true) => format!("{} IS DISTINCT FROM {}", col(name), lit(name, value)),
            (Dialect::MySql(_), false) => format!("{} <=> {}", col(name), lit(name, value)),
            (Dialect::MySql(_), true) => format!("NOT ({} <=> {})", col(name), lit(name, value)),
        },
        Cond::IsNull { col: name, negated } => format!("{} IS {}NULL", col(name), not(*negated)),
        Cond::In { col: name, values, negated } => {
            let null = values.contains(&Value::Null);
            let rest: Vec<String> = values.iter().filter(|v| **v != Value::Null).map(|v| lit(name, v)).collect();
            let c = col(name);
            match (rest.is_empty(), null, negated) {
                (true, false, _) => return Err(RenderError::EmptyList(name.clone())),
                (true, true, _) => format!("{c} IS {}NULL", not(*negated)),
                (false, false, _) => format!("{c} {}IN ({})", not(*negated), rest.join(", ")),
                (false, true, false) => format!("({c} IN ({}) OR {c} IS NULL)", rest.join(", ")),
                (false, true, true) => format!("({c} NOT IN ({}) AND {c} IS NOT NULL)", rest.join(", ")),
            }
        }
        Cond::Like { col: name, pattern, negated, case } => {
            let p = text_literal(t, pattern);
            let e = d.quote_literal(&ESCAPE.to_string());
            let facts = t.column(name);
            match (d, case) {
                (_, CaseMode::Server) => format!("{} {}LIKE {p} ESCAPE {e}", col(name), not(*negated)),
                (Dialect::Postgres, CaseMode::Insensitive) => {
                    if facts.is_some_and(|f| !f.deterministic) {
                        return Err(RenderError::CaseInsensitiveRefused(name.clone()));
                    }
                    format!("{} {}ILIKE {p} ESCAPE {e}", col(name), not(*negated))
                }
                // A `_ci` collation compares without case already (and keeps an index usable).
                (Dialect::MySql(_), CaseMode::Insensitive) if facts.is_some_and(case_insensitive) => {
                    format!("{} {}LIKE {p} ESCAPE {e}", col(name), not(*negated))
                }
                (Dialect::MySql(_), CaseMode::Insensitive) => {
                    format!("LOWER({}) {}LIKE LOWER({p}) ESCAPE {e}", col(name), not(*negated))
                }
            }
        }
        Cond::Between { col: name, lo, hi, negated } => {
            if *lo == Value::Null || *hi == Value::Null {
                return Err(RenderError::NullCompare(name.clone()));
            }
            format!("{} {}BETWEEN {} AND {}", col(name), not(*negated), lit(name, lo), lit(name, hi))
        }
    })
}

/// `s` in parentheses, unless it is in a pair around the whole of it already.
fn grouped(d: Dialect, s: String) -> String {
    if d.unparenthesized(&s).len() < s.trim().len() { s } else { format!("({s})") }
}

/// A MySQL collation that compares without case (`utf8mb4_0900_ai_ci`).
fn case_insensitive(c: &ColumnFacts) -> bool {
    c.collation.as_deref().is_some_and(|n| n.to_ascii_lowercase().ends_with("_ci"))
}

/// `v` as a literal compared with column `c` (unknown: no kind).
fn literal(t: &Target, c: Option<&ColumnFacts>, v: &Value) -> String {
    match v {
        Value::Null => "NULL".to_string(),
        Value::Bool(true) => "TRUE".to_string(),
        Value::Bool(false) => "FALSE".to_string(),
        Value::Number(n) if c.is_some_and(|c| c.kind == ColumnKind::Number) && sql_number(n) && !negative_zero(n) => {
            n.clone()
        }
        Value::Number(n) => text_literal(t, n),
        Value::Text(s) => text_literal(t, s),
        Value::Bytes(b) => match t.dialect {
            Dialect::Postgres => format!("{}::pg_catalog.bytea", text_literal(t, &format!("\\x{}", hex(b, false)))),
            Dialect::MySql(_) => format!("X'{}'", hex(b, true)),
        },
    }
}

/// `s` as a string literal that the server reads as `s` (see the module docs).
fn text_literal(t: &Target, s: &str) -> String {
    let d = t.dialect;
    match d {
        Dialect::Postgres if s.contains('\\') => {
            let open = LiteralOpen { prefix: 1, quote: '\'', backslash: true };
            format!("E'{}'", d.literal_body(s, open))
        }
        Dialect::Postgres => d.quote_literal(s),
        Dialect::MySql(_) if t.utf8mb4 && !s.contains(['\\', '\0']) => d.quote_literal(s),
        Dialect::MySql(_) => format!("_utf8mb4 X'{}'", hex(s.as_bytes(), true)),
    }
}

/// Bytes in hexadecimal, two digits each.
fn hex(b: &[u8], upper: bool) -> String {
    b.iter().map(|b| if upper { format!("{b:02X}") } else { format!("{b:02x}") }).collect()
}

impl CmpOp {
    /// The operator written as `sql` (`!=` is `<>`).
    pub(super) fn from_sql(sql: &str) -> Option<Self> {
        Some(match sql {
            "=" => Self::Eq,
            "<>" | "!=" => Self::Ne,
            "<" => Self::Lt,
            "<=" => Self::Le,
            ">" => Self::Gt,
            ">=" => Self::Ge,
            _ => return None,
        })
    }
}
