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
//! reads otherwise); else it is its UTF-8 bytes, `_utf8mb4 X'…'`. A line break is never written
//! as it is (the filter line holds one line): PostgreSQL writes it as an escape (`E'…\n…'`),
//! MySQL the string's bytes.
//!
//! A MySQL `TIMESTAMP` value with the offset of the zone it was read in (as a copy as SQL takes
//! it) is written as that instant in the session's zone when the statement runs, the instant
//! worked out here in UTC: `CONVERT_TZ('<UTC date and time>', '+00:00', @@session.time_zone)`.
//! Every MySQL and MariaDB reads it, and it is the same instant in a session of another zone, as
//! when it runs again from the history. A value whose offset or instant the server would not
//! convert is refused ([`RenderError::NotInstant`]), never written as a conversion that can
//! come out NULL.
//!
//! NULL is never compared with `=`: [`Cond::IsNull`] and the NULL-safe [`Cond::NotDistinct`]
//! (PostgreSQL `IS [NOT] DISTINCT FROM`, MySQL `<=>` and `NOT (… <=> …)`). A NULL among the
//! values of [`Cond::In`] is taken out of the list and tested on its own, so it matches as the
//! user expects: `(col IN (…) OR col IS NULL)`, `(col NOT IN (…) AND col IS NOT NULL)`.
//!
//! Patterns are escaped with `!` and written with `ESCAPE '!'`: a backslash would read
//! differently under MySQL's `NO_BACKSLASH_ESCAPES`. A PostgreSQL pattern without case is
//! `ILIKE`, written only on a column whose collation is known to be deterministic (the server
//! refuses it on a nondeterministic one).

use super::{BarRefusal, CaseMode, CmpOp, ColumnFacts, ColumnKind, Cond, Target, Value};
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
    /// A value with a time zone offset compared with a MySQL `TIMESTAMP` that is no instant the
    /// column holds: a date or time that does not exist, an offset no session has (beyond
    /// `-13:59` to `+14:00`), an instant outside 1970-01-01 00:00:01 to 2038-01-19 03:14:07 UTC
    /// (which the server would not convert). The column.
    NotInstant(String),
    /// A comparison or a `BETWEEN` with NULL, which matches no row: the column.
    NullCompare(String),
    /// `IN` of no value: the column.
    EmptyList(String),
    /// A case-insensitive pattern on a PostgreSQL column whose collation is not deterministic,
    /// which the server refuses for `ILIKE`: the column.
    CaseInsensitiveRefused(String),
    /// A case-insensitive pattern on a PostgreSQL column whose collation is not known to be
    /// deterministic (it may be one the server refuses `ILIKE` on): the column.
    CollationUnknown(String),
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
                    match facts.and_then(|f| f.deterministic) {
                        Some(true) => {}
                        Some(false) => return Err(RenderError::CaseInsensitiveRefused(name.clone())),
                        None => return Err(RenderError::CollationUnknown(name.clone())),
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
        // The instant is written in UTC (the source zone `+00:00`, which every server reads); a
        // value with an offset that is no such instant is refused before
        // (`RenderError::NotInstant`).
        Value::Text(s) if matches!(t.dialect, Dialect::MySql(_)) && c.is_some_and(|c| c.zoned) => match instant(s) {
            Some(Some(utc)) => format!("CONVERT_TZ({}, '+00:00', @@session.time_zone)", text_literal(t, &utc)),
            _ => text_literal(t, s),
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

/// The offsets a MySQL session may have, in minutes: `-13:59` to `+14:00` (MySQL 8.0.19 and
/// later; before, and MariaDB, `-12:59` to `+13:00`).
const OFFSET_MINUTES: std::ops::RangeInclusive<i64> = -(13 * 60 + 59)..=14 * 60;

/// The instants a MySQL `TIMESTAMP` holds, in seconds since 1970-01-01 00:00:00 UTC: from
/// `1970-01-01 00:00:01` to `2038-01-19 03:14:07` (with any fraction of that second).
const TIMESTAMP_SECONDS: std::ops::RangeInclusive<i64> = 1..=i32::MAX as i64;

/// Text `s` with a time zone offset ([`zoned`]) as the UTC date and time of its instant
/// (`2026-11-10 14:00:00+14:00` is `2026-11-10 00:00:00`), the fraction as written: `Some(None)`
/// when it has an offset but is no instant a MySQL `TIMESTAMP` holds (a date or time that does
/// not exist, an offset beyond [`OFFSET_MINUTES`] or of 60 minutes or more, an instant outside
/// [`TIMESTAMP_SECONDS`], where the server would not convert it); `None` without an offset.
pub(super) fn instant(s: &str) -> Option<Option<String>> {
    let (when, offset) = zoned(s)?;
    Some(utc(when, offset))
}

fn utc(when: &str, offset: &str) -> Option<String> {
    let n = |r: std::ops::Range<usize>| when[r].parse::<i64>().ok();
    let (year, month, day) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hour, minute, second) = (n(11..13)?, n(14..16)?, n(17..19)?);
    let fraction = &when[19..];
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if !(1..=days_in_month).contains(&day) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let (off_hours, off_minutes) = (offset[1..3].parse::<i64>().ok()?, offset[4..6].parse::<i64>().ok()?);
    if off_minutes > 59 {
        return None;
    }
    let sign = if offset.starts_with('-') { -1 } else { 1 };
    let shift = sign * (off_hours * 60 + off_minutes);
    if !OFFSET_MINUTES.contains(&shift) {
        return None;
    }
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - shift * 60;
    if !TIMESTAMP_SECONDS.contains(&seconds) {
        return None;
    }
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    Some(format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}{fraction}", rest / 3600, rest % 3600 / 60, rest % 60))
}

/// Days from 1970-01-01 to a date of the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of the proleptic Gregorian calendar `days` after 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
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
/// decimal spelling compared with one ([`RenderError::NotExact`]); a value with an offset that is
/// no instant compared with a `TIMESTAMP` ([`RenderError::NotInstant`]). PostgreSQL refuses such
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
    let facts = t.column(name)?;
    let values: Vec<&Value> = values.into_iter().filter(|v| **v != Value::Null).collect();
    if facts.zoned && values.iter().any(|v| matches!(v, Value::Text(s) if instant(s) == Some(None))) {
        return Some(RenderError::NotInstant(name.to_string()));
    }
    match facts.kind {
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

/// Characters a literal does not hold as they are: the filter line is one line, and would put
/// a space for them, so that the text it shows would not be the text that runs.
const LINE_BREAKS: [char; 2] = ['\n', '\r'];

/// `s` as a string literal that the server reads as `s`, on one line (see the module docs).
fn text_literal(t: &Target, s: &str) -> String {
    let d = t.dialect;
    match d {
        Dialect::Postgres if s.contains('\\') || s.contains(LINE_BREAKS) => {
            let open = LiteralOpen { prefix: 1, quote: '\'', backslash: true };
            // The backslashes are doubled first: the escapes written after them stay escapes.
            let body = d.literal_body(s, open).replace('\n', "\\n").replace('\r', "\\r");
            format!("E'{body}'")
        }
        Dialect::Postgres => d.quote_literal(s),
        Dialect::MySql(_) if t.utf8mb4 && !s.contains(['\\', '\0']) && !s.contains(LINE_BREAKS) => d.quote_literal(s),
        Dialect::MySql(_) => format!("_utf8mb4 X'{}'", hex(s.as_bytes(), true)),
    }
}

/// Filter text `base` with condition `cond` (as [`render`] writes one) added to it: `cond` alone
/// when `base` is blank, else `base AND cond`. `base` is put in parentheses first unless every
/// operator outside its own parentheses binds tighter than `AND` (read with the dialect's lexer):
/// an `OR`, an `XOR`, a `|` (MySQL's `||` is `OR` unless `PIPES_AS_CONCAT`) or an assignment
/// wraps it, so that the condition added never changes what `base` means.
///
/// `base` must stand on its own as one condition: one that the filter's lexical check refuses (a
/// parenthesis that closes one it did not open or is left open, a comment, a statement's end, a
/// string left open, a text read otherwise under another server setting) is refused with why,
/// never put in parentheses (which could make it read as another condition). Only the blanks
/// the dialect's lexer reads as blanks are left out at its end; the rest of it is kept as
/// written (PostgreSQL reads a space outside ASCII as part of a name).
pub fn and_filter(base: &str, cond: &str, d: Dialect) -> Result<String, BarRefusal> {
    use crate::sql::lexer::{Tok, lex_in};
    let end = lex_in(base, d).iter().rev().find(|t| t.kind != Tok::Whitespace).map_or(0, |t| t.end.min(base.len()));
    let base = &base[..end];
    if base.is_empty() {
        return Ok(cond.to_string());
    }
    super::check::lexical(base, d)?;
    Ok(match plain_and_chain(base, d) {
        true => format!("{base} AND {cond}"),
        false => format!("({base}) AND {cond}"),
    })
}

/// Whether `AND` written after `text` takes the whole of it as its left operand: nothing outside
/// its parentheses binds more loosely than `AND`, and nothing there hides what follows.
fn plain_and_chain(text: &str, d: Dialect) -> bool {
    use crate::sql::lexer::{Tok, lex_in};
    let mut depth = 0i64;
    // The end of a `:` the lexer gave as an operator of its own (`:=` comes as `:` then `=`).
    let mut colon = None;
    for t in lex_in(text, d) {
        let s = t.text(text);
        let after_colon = colon == Some(t.start);
        colon = (t.kind == Tok::Op && s == ":").then_some(t.end);
        match t.kind {
            Tok::LParen => depth += 1,
            Tok::RParen => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            // A comment, a statement's end, code in a comment: what follows may be cut off.
            Tok::LineComment | Tok::BlockComment | Tok::ExecComment | Tok::Semi | Tok::Directive => return false,
            _ if depth > 0 => {}
            Tok::Keyword | Tok::Ident if s.eq_ignore_ascii_case("OR") || s.eq_ignore_ascii_case("XOR") => {
                return false;
            }
            Tok::Op if s == "|" || (s == "=" && after_colon) => return false,
            _ => {}
        }
    }
    depth == 0
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
