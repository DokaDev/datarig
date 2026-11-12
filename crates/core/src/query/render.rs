//! A [`Cond`] as the text an app action writes into the filter bar.
//!
//! Literals are written for the column they are compared with: a number bare only for a number
//! column and a valid number literal (else a string the server converts), and on MySQL valid
//! number text too (the server compares a number with a string as two doubles), an exponent
//! written out as the exact decimal (MySQL reads a number with one as a double); bytes as PostgreSQL's
//! `'\x…'::pg_catalog.bytea` (an escape string, see below; the type is named with its schema, so
//! that a domain of that name earlier on the search path is not the one cast to) or MySQL's
//! `X'…'`; text as a string literal. A PostgreSQL string with a backslash is an escape string (`E'…'`), which reads the same whatever
//! `standard_conforming_strings` is. A MySQL string is plain only when the session's character
//! set is known to be `utf8mb4` and it holds no backslash or NUL (which `NO_BACKSLASH_ESCAPES`
//! reads otherwise); else it is its UTF-8 bytes, `_utf8mb4 X'…'`.
//!
//! A MySQL `TIMESTAMP` value with the offset of the zone it was read in (as a copy as SQL takes
//! it) is written as that instant in the session's zone when the statement runs,
//! `CONVERT_TZ('…', '+00:00', @@session.time_zone)`: every MySQL and MariaDB reads it, and it is
//! the same instant in a session of another zone, as when it runs again from the history.
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
    /// A MySQL `FLOAT` compared for equality (`=`, `<>`, `<=>`, `IN`), which a literal never
    /// meets exactly: the column.
    InexactEquality(String),
    /// MySQL text that is not a number compared with a number column, which the server reads
    /// as a number without a word (`'abc'` as 0, `'12abc'` as 12, `'true'` as 0): the column.
    NotNumber(String),
    /// A MySQL number compared with a number column that has no exact decimal spelling within
    /// MySQL's `DECIMAL` (65 digits, 30 after the point; `1e300`, `1e-40`): the server would
    /// read it as a double, which other values of the column round to as well. The column.
    NotExact(String),
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
    if let Some(e) = refused_values(c, t) {
        return Err(e);
    }
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
        // MySQL reads a number with an exponent as a double, and compares a number column with a
        // string as two doubles: a number for a number column is written as its exact decimal
        // (a number that has none is refused before, `RenderError::NotExact`).
        Value::Number(n) | Value::Text(n)
            if matches!(t.dialect, Dialect::MySql(_))
                && c.is_some_and(|c| c.kind == ColumnKind::Number)
                && sql_number(n)
                && !negative_zero(n) =>
        {
            exact_decimal(n).unwrap_or_else(|| n.clone())
        }
        Value::Number(n)
            if c.is_some_and(|c| bare_numbers(t.dialect, c.kind)) && sql_number(n) && !negative_zero(n) =>
        {
            n.clone()
        }
        Value::Number(n) => text_literal(t, n),
        // A MySQL `FLOAT`: compared as a double whatever is written, the number as typed.
        Value::Text(s)
            if matches!(t.dialect, Dialect::MySql(_))
                && c.is_some_and(|c| bare_numbers(t.dialect, c.kind))
                && sql_number(s)
                && !negative_zero(s) =>
        {
            s.clone()
        }
        // A MySQL TIMESTAMP's value with the offset of the zone it was read in: that instant in
        // the session's zone when the statement runs. Every MySQL and MariaDB reads this (a
        // literal with an offset only MySQL 8.0.19 and later), and it stays the same instant
        // after a change of the session's zone and run again from the history.
        Value::Text(s) if matches!(t.dialect, Dialect::MySql(_)) && c.is_some_and(|c| c.zoned) => match zoned(s) {
            Some((at, offset)) => {
                format!("CONVERT_TZ({}, {}, @@session.time_zone)", text_literal(t, at), text_literal(t, offset))
            }
            None => text_literal(t, s),
        },
        Value::Text(s) => text_literal(t, s),
        Value::Bytes(b) => match t.dialect {
            Dialect::Postgres => format!("{}::pg_catalog.bytea", text_literal(t, &format!("\\x{}", hex(b, false)))),
            Dialect::MySql(_) => format!("X'{}'", hex(b, true)),
        },
    }
}

/// A date and time with a time zone offset, `YYYY-MM-DD HH:MM:SS[.ffffff]±HH:MM`: the date and
/// time, and the offset.
pub(super) fn zoned(s: &str) -> Option<(&str, &str)> {
    let at = s.len().checked_sub(6)?;
    let (when, offset) = (s.get(..at)?, s.get(at..)?);
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    let shaped = |text: &str, shape: &str| {
        text.len() == shape.len()
            && text.bytes().zip(shape.bytes()).all(|(b, c)| if c == b'9' { b.is_ascii_digit() } else { b == c })
    };
    let (whole, fraction) = when.split_once('.').unwrap_or((when, ""));
    let fraction_ok = fraction.is_empty() && !when.contains('.') || digits(fraction) && fraction.len() <= 6;
    let offset_ok = (offset.starts_with('+') || offset.starts_with('-')) && shaped(&offset[1..], "99:99");
    (shaped(whole, "9999-99-99 99:99:99") && fraction_ok && offset_ok).then_some((when, offset))
}

/// The most digits of a MySQL `DECIMAL`, and of them the most after the point: an exact value
/// literal within them is read as a `DECIMAL`.
const DECIMAL_DIGITS: usize = 65;
const DECIMAL_SCALE: usize = 30;

/// Valid number text `s` ([`sql_number`]) as an exact MySQL decimal literal: as written when it
/// has no exponent, else with its exponent written out (`9007199254740993e0` is
/// `9007199254740993`, `15e-1` is `1.5`), never through a float. `None` when it needs more than
/// [`DECIMAL_DIGITS`] digits or [`DECIMAL_SCALE`] after the point, where MySQL reads a double.
pub(super) fn exact_decimal(s: &str) -> Option<String> {
    let (sign, t) = match s.strip_prefix('-') {
        Some(t) => ("-", t),
        None => ("", s),
    };
    let (mantissa, exponent) = match t.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e)),
        None => (t, None),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{int}{frac}");
    if digits.bytes().all(|b| b == b'0') {
        return Some(if exponent.is_some() { "0".to_string() } else { s.to_string() });
    }
    let e: i64 = match exponent {
        None => 0,
        // Beyond this the value has far more digits than a DECIMAL either way.
        Some(e) if e.trim_start_matches(['+', '-']).len() > 4 => return None,
        Some(e) => e.strip_prefix('+').unwrap_or(e).parse().ok()?,
    };
    let point = int.len() as i64 + e;
    let (whole, fraction) = if point <= 0 {
        ("0".to_string(), format!("{}{digits}", "0".repeat(usize::try_from(-point).ok()?)))
    } else if point as usize >= digits.len() {
        (format!("{digits}{}", "0".repeat(point as usize - digits.len())), String::new())
    } else {
        (digits[..point as usize].to_string(), digits[point as usize..].to_string())
    };
    let whole = whole.trim_start_matches('0');
    let fraction = fraction.trim_end_matches('0');
    if whole.len() + fraction.len() > DECIMAL_DIGITS || fraction.len() > DECIMAL_SCALE {
        return None;
    }
    if exponent.is_none() {
        return Some(s.to_string());
    }
    let whole = if whole.is_empty() { "0" } else { whole };
    Some(match fraction.is_empty() {
        true => format!("{sign}{whole}"),
        false => format!("{sign}{whole}.{fraction}"),
    })
}

/// A number compared with a column of kind `kind` of dialect `d` is written bare: a number
/// column, or a MySQL `FLOAT` (a PostgreSQL `real` takes it as a string, see [`ColumnKind::Real`]).
pub(super) fn bare_numbers(d: Dialect, kind: ColumnKind) -> bool {
    match kind {
        ColumnKind::Number => true,
        ColumnKind::Real => matches!(d, Dialect::MySql(_)),
        _ => false,
    }
}

/// Why the values of comparison `c` are not written for its column on `t`'s dialect: on MySQL
/// a `FLOAT` compared for equality ([`RenderError::InexactEquality`]), text that is not a
/// number compared with a number column ([`RenderError::NotNumber`]), or a number with no exact
/// decimal spelling compared with one ([`RenderError::NotExact`]). PostgreSQL refuses such
/// text itself (an error, never a silent conversion).
fn refused_values(c: &Cond, t: &Target) -> Option<RenderError> {
    if t.dialect == Dialect::Postgres {
        return None;
    }
    let (name, values, equality): (&str, Vec<&Value>, bool) = match c {
        Cond::Cmp { col, op, value } => (col, vec![value], matches!(op, CmpOp::Eq | CmpOp::Ne)),
        Cond::NotDistinct { col, value, .. } => (col, vec![value], true),
        Cond::In { col, values, .. } => (col, values.iter().collect(), true),
        Cond::Between { col, lo, hi, .. } => (col, vec![lo, hi], false),
        _ => return None,
    };
    let kind = t.column(name)?.kind;
    let values: Vec<&Value> = values.into_iter().filter(|v| **v != Value::Null).collect();
    match kind {
        ColumnKind::Real if equality && !values.is_empty() => Some(RenderError::InexactEquality(name.to_string())),
        ColumnKind::Number | ColumnKind::Real
            if values.iter().any(|v| matches!(v, Value::Text(s) if !sql_number(s))) =>
        {
            Some(RenderError::NotNumber(name.to_string()))
        }
        ColumnKind::Number
            if values.iter().any(|v| {
                matches!(v, Value::Number(n) | Value::Text(n)
                    if sql_number(n) && !negative_zero(n) && exact_decimal(n).is_none())
            }) =>
        {
            Some(RenderError::NotExact(name.to_string()))
        }
        _ => None,
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
