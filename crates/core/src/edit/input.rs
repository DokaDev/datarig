//! Whether a value typed into a cell is a value of its column's type, in the form the result
//! shows that type's values in ([`check_input`]).
//!
//! What would be stored as something else than the text the user sees (rounded, cut, read as
//! another date) is refused; what this code cannot judge is left to the server. The checks that
//! differ by engine are the engine's input rules ([`crate::engine::InputRules`]); the parts they
//! share are here.

use super::target::TableColumn;
use crate::driver::ValueKind;
use crate::sql::dialect::Dialect;
use std::ops::RangeInclusive;

/// Why a typed value is not a value of its column's type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invalid {
    /// Not one of the column's enum labels (exactly, case included).
    NotLabel,
    /// Not a whole number (an optional sign and digits).
    NotWholeNumber,
    /// A number outside the range of the column's type (as the server names it).
    OutOfRange { type_name: String },
    /// Not a number in the form the column's type takes.
    NotNumber,
    /// More digits than `numeric(precision, scale)` holds: before the point, or after it
    /// (which the server would round away).
    TooManyDigits { precision: u32, scale: u32 },
    /// Not `true` or `false`.
    NotBool,
    /// Not a date (`YYYY-MM-DD`), or a day the calendar does not have.
    NotDate,
    /// Not a time of day (`HH:MM:SS`, with an offset for a type with a time zone).
    NotTime,
    /// Not a date and time (`YYYY-MM-DD HH:MM:SS`, with an offset for a type with a time zone).
    NotTimestamp,
    /// More digits of a second than the column keeps (the server would round them away).
    TooPrecise { digits: u32 },
    /// Not a UUID (`8-4-4-4-12` hexadecimal digits).
    NotUuid,
    /// Not bytes as the result shows them, in the form `form`.
    NotBytes { form: BytesForm },
    /// Not bits as the result shows them, in the form `form`.
    NotBits { form: BitsForm },
    /// Not as many bits as the column takes: exactly `count`, or at most `count`.
    BitCount { count: u32, exact: bool },
    /// Longer than the column's `max` characters.
    TooLong { max: u32 },
    /// A number the column's floating-point type does not hold as typed: it would be stored
    /// as `stored`.
    Rounded { type_name: String, stored: String },
    /// Text ending in spaces for a MySQL `CHAR` column, which drops them.
    TrailingSpaces,
}

/// The form a result shows bytes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BytesForm {
    /// `\x` and hexadecimal digits in pairs, either case.
    BackslashX,
    /// `0x` and upper-case hexadecimal digits in pairs.
    ZeroX,
}

/// The form a result shows bits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitsForm {
    /// Binary digits.
    Digits,
    /// Binary digits in `b'…'`.
    BQuoted,
}

/// Whether `text`, typed for table column `c` on a server of dialect `d`, is a value of the
/// column's type in the form the result shows that type's values in: as the engine's input
/// rules ([`crate::engine::InputRules`]) judge it.
pub fn check_input(d: Dialect, c: &TableColumn, text: &str) -> Result<(), Invalid> {
    let rules = d.engine().spec().values.input();
    let col = &c.column;
    if !col.enum_labels.is_empty() && !matches!(c.kind, ValueKind::Array(_)) {
        return col.enum_labels.iter().any(|l| l == text).then_some(()).ok_or(Invalid::NotLabel);
    }
    match c.kind {
        ValueKind::Integer => rules.integer(c, text),
        ValueKind::Decimal => rules.decimal(c, text),
        ValueKind::Float => rules.float(c, text),
        ValueKind::Bool => matches!(text, "true" | "false").then_some(()).ok_or(Invalid::NotBool),
        ValueKind::Date => rules.date(text),
        ValueKind::Time => time_of_day(&TypeName::read(&col.type_name), text),
        ValueKind::Timestamp => rules.timestamp(c, false, text),
        ValueKind::TimestampTz => rules.timestamp(c, true, text),
        ValueKind::Bytes => rules.bytes(text),
        ValueKind::Bit => rules.bits(c, text),
        ValueKind::Text => rules.text(c, text),
        ValueKind::Other => rules.other(c, text),
        ValueKind::Interval => rules.interval(c, text),
        ValueKind::Json | ValueKind::Array(_) => Ok(()),
    }
}

/// MySQL's `TIME` in the forms the server reads exactly: `[-]H:M:S[.f]` (the hours one to three
/// digits, minutes and seconds one or two: `838:59:59`, `8:3:2`), `[-]H:M` (seconds zero), and
/// either with days first (`D HH:MM:SS[.f]`, `D HH:MM`). At most as many digits of a second as
/// the column keeps: the server rounds the others away without a warning. Out of the type's
/// range the server warns, which fails the Apply.
pub(crate) fn mysql_time(ty: &TypeName, s: &str) -> Result<(), Invalid> {
    let digits = |p: &str, most: usize| !p.is_empty() && p.len() <= most && p.bytes().all(|b| b.is_ascii_digit());
    let sixty = |p: &str| digits(p, 2) && p.parse::<u32>().is_ok_and(|n| n < 60);
    let s = s.strip_prefix('-').unwrap_or(s);
    let (s, most_hours) = match s.split_once(' ') {
        Some((days, rest)) if digits(days, 2) => (rest, 2),
        Some(_) => return Err(Invalid::NotTime),
        None => (s, 3),
    };
    let (clock, frac) = match s.split_once('.') {
        Some((c, f)) if digits(f, usize::MAX) => (c, f),
        Some(_) => return Err(Invalid::NotTime),
        None => (s, ""),
    };
    let fields: Vec<&str> = clock.split(':').collect();
    let ok = match fields[..] {
        [h, m] => frac.is_empty() && digits(h, most_hours) && sixty(m),
        [h, m, sec] => digits(h, most_hours) && sixty(m) && sixty(sec),
        _ => false,
    };
    if !ok {
        return Err(Invalid::NotTime);
    }
    let keep = fraction_digits(ty, 0);
    if frac.trim_end_matches('0').len() > keep as usize { Err(Invalid::TooPrecise { digits: keep }) } else { Ok(()) }
}

/// A type name as the server formats it, read for its checks: `int(11) unsigned` is `int`
/// with arguments `[11]`, unsigned; `timestamp(3) with time zone` is `timestamp with time
/// zone` with `[3]`.
pub(crate) struct TypeName {
    /// Lower case, without the arguments, `unsigned` and `zerofill`.
    pub(crate) name: String,
    /// The numbers in parentheses; empty when there are none (or they are not numbers).
    pub(crate) args: Vec<u32>,
    pub(crate) unsigned: bool,
}

impl TypeName {
    pub(crate) fn read(type_name: &str) -> TypeName {
        let lower = type_name.to_ascii_lowercase();
        let (rest, args) = match (lower.find('('), lower.rfind(')')) {
            (Some(open), Some(close)) if open < close => {
                let args = lower[open + 1..close].split(',').map(|a| a.trim().parse().ok()).collect();
                (format!("{} {}", &lower[..open], &lower[close + 1..]), args)
            }
            _ => (lower.clone(), None),
        };
        let words: Vec<&str> = rest.split_whitespace().collect();
        TypeName {
            name: words
                .iter()
                .filter(|w| !matches!(**w, "unsigned" | "zerofill"))
                .copied()
                .collect::<Vec<_>>()
                .join(" "),
            args: args.unwrap_or_default(),
            unsigned: words.contains(&"unsigned"),
        }
    }
}

/// The digits of a whole number `s` (an optional sign and digits), without its sign.
pub(crate) fn whole_digits(s: &str) -> Result<&str, Invalid> {
    let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Invalid::NotWholeNumber);
    }
    Ok(digits)
}

/// Whether whole number `s` (its `digits` without the sign) is in `range`, the range of column
/// type `type_name`; any number when the range is not known.
pub(crate) fn whole_in(s: &str, digits: &str, range: Option<(i128, i128)>, type_name: &str) -> Result<(), Invalid> {
    let Some((min, max)) = range else { return Ok(()) };
    let value = (digits.trim_start_matches('0').len() <= 30).then(|| s.parse::<i128>().ok()).flatten();
    value.filter(|v| (min..=max).contains(v)).map(|_| ()).ok_or_else(|| out_of_range(type_name))
}

pub(crate) fn out_of_range(type_name: &str) -> Invalid {
    Invalid::OutOfRange { type_name: type_name.to_string() }
}

/// Whether a decimal number's digits before the point (`int`) and after it (`frac`) fit the
/// type's `(precision, scale)`; any when the type has no precision.
pub(crate) fn decimal_digits(ty: &TypeName, int: &str, frac: &str) -> Result<(), Invalid> {
    let (precision, scale) = match ty.args[..] {
        [p] => (p, 0),
        [p, s] => (p, s),
        _ => return Ok(()),
    };
    let int_digits = int.trim_start_matches('0').len();
    let frac_digits = frac.trim_end_matches('0').len();
    if int_digits > precision.saturating_sub(scale) as usize || frac_digits > scale as usize {
        return Err(Invalid::TooManyDigits { precision, scale });
    }
    Ok(())
}

/// `-12.50` as its sign, its digits before the point and after it; digits on at least one side.
pub(crate) fn plain_decimal(s: &str) -> Option<(bool, &str, &str)> {
    let (negative, unsigned) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (int, frac) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    (digits(int) && digits(frac) && !(int.is_empty() && frac.is_empty())).then_some((negative, int, frac))
}

/// Whether `s` is a decimal number with an optional exponent.
pub(crate) fn float_form(s: &str) -> Result<(), Invalid> {
    let (mantissa, exp) = match s.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e.strip_prefix(['-', '+']).unwrap_or(e))),
        None => (s, None),
    };
    let exp_ok = exp.is_none_or(|e| !e.is_empty() && e.bytes().all(|b| b.is_ascii_digit()));
    if plain_decimal(mantissa).is_none() || !exp_ok {
        return Err(Invalid::NotNumber);
    }
    Ok(())
}

/// The number `s` is read as in single or double precision, and its shortest digits (`{:e}`);
/// out of the range of column type `type_name` when it is not finite.
pub(crate) fn float_read(s: &str, single: bool, type_name: &str) -> Result<(f64, String), Invalid> {
    let (value, stored) = if single {
        let v: f32 = s.parse().map_err(|_| Invalid::NotNumber)?;
        (f64::from(v), v.is_finite().then(|| format!("{v:e}")))
    } else {
        let v: f64 = s.parse().map_err(|_| Invalid::NotNumber)?;
        (v, v.is_finite().then(|| format!("{v:e}")))
    };
    let Some(stored) = stored else { return Err(out_of_range(type_name)) };
    Ok((value, stored))
}

/// A number written `-1.25e3` as its sign, its significant digits and the power of ten of the
/// first one: `(true, "125", 3)`; zero has no digits. `None` when the exponent is too large to
/// read.
pub(crate) fn digits_of(s: &str) -> Option<(bool, String, i64)> {
    let (negative, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (mantissa, exp) = match s.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i64>().ok()?),
        None => (s, 0),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let all = format!("{int}{frac}");
    let lead = all.len() - all.trim_start_matches('0').len();
    let digits = all[lead..].trim_end_matches('0').to_string();
    if digits.is_empty() {
        return Some((negative, digits, 0));
    }
    Some((negative, digits, exp.checked_add(int.len() as i64 - 1 - lead as i64)?))
}

/// Rust's `{:e}` text of a float as a person writes it: plain unless it is very large or small.
pub(crate) fn plain_float(sci: &str) -> String {
    match sci.split_once('e').map(|(_, e)| e.parse::<i32>()) {
        Some(Ok(exp)) if (-4..17).contains(&exp) => sci.parse::<f64>().map_or(sci.to_string(), |v| v.to_string()),
        _ => sci.to_string(),
    }
}

/// Whether `s` is `YYYY-MM-DD`, a day of the calendar, the year with as many digits as
/// `year_digits` allows, `bc` when it is a year before Christ. With `zeros`, a zero month or
/// day is a date too (each within its range) and so is year 0; without, year 0 is none.
pub(crate) fn ymd(s: &str, bc: bool, year_digits: RangeInclusive<usize>, zeros: bool) -> bool {
    let mut parts = s.split('-');
    let (Some(y), Some(m), Some(d), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let digits = |p: &str, len: bool| len && p.bytes().all(|b| b.is_ascii_digit());
    let year_len = year_digits.contains(&y.len());
    if !(digits(y, year_len) && digits(m, m.len() == 2) && digits(d, d.len() == 2)) {
        return false;
    }
    let (Ok(y), Ok(m), Ok(d)) = (y.parse::<i64>(), m.parse::<u32>(), d.parse::<u32>()) else { return false };
    if zeros && (m == 0 || d == 0) {
        return m <= 12 && d <= 31;
    }
    if !zeros && y == 0 {
        return false;
    }
    // 1 BC is year 0 of the proleptic Gregorian calendar, a leap year.
    let year = if bc { 1 - y } else { y };
    (1..=12).contains(&m) && d >= 1 && d <= days_in(year, m)
}

fn days_in(year: i64, month: u32) -> u32 {
    let leap = year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0);
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// How many digits of a second the type keeps: its argument, else the server's `default`
/// (PostgreSQL 6, MySQL 0).
pub(crate) fn fraction_digits(ty: &TypeName, default: u32) -> u32 {
    ty.args.first().copied().unwrap_or(default)
}

/// A time of day (`time`, `time with time zone`, PostgreSQL's: MySQL's `TIME` is an interval).
fn time_of_day(ty: &TypeName, s: &str) -> Result<(), Invalid> {
    let zoned = ty.name == "timetz" || ty.name.ends_with("with time zone");
    let (time, offset) = split_offset(s);
    if offset.is_some() != zoned || !offset.is_none_or(utc_offset) {
        return Err(Invalid::NotTime);
    }
    hms(time, fraction_digits(ty, 6), true).ok_or(Invalid::NotTime)?
}

/// `12:00:00+09` as the time and the offset.
pub(crate) fn split_offset(s: &str) -> (&str, Option<&str>) {
    match s.find(['+', '-']) {
        Some(i) => (&s[..i], Some(&s[i..])),
        None => (s, None),
    }
}

/// `HH:MM:SS` and at most `keep` significant digits of a second (`24:00:00` too when `midnight`).
/// `None` when it is no time; else whether the fraction fits.
pub(crate) fn hms(s: &str, keep: u32, midnight: bool) -> Option<Result<(), Invalid>> {
    let (clock, frac) = match s.split_once('.') {
        Some((c, f)) if !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()) => (c, f),
        Some(_) => return None,
        None => (s, ""),
    };
    let [h, m, sec] = two_digit_fields(clock)?;
    let zero_frac = frac.bytes().all(|b| b == b'0');
    let ok = m <= 59 && sec <= 59 && (h <= 23 || (midnight && h == 24 && m == 0 && sec == 0 && zero_frac));
    if !ok {
        return None;
    }
    let digits = frac.trim_end_matches('0').len();
    Some(if digits > keep as usize { Err(Invalid::TooPrecise { digits: keep }) } else { Ok(()) })
}

/// `HH:MM:SS` as its three numbers.
fn two_digit_fields<const N: usize>(s: &str) -> Option<[u32; N]> {
    let mut out = [0; N];
    let mut parts = s.split(':');
    for slot in &mut out {
        let p = parts.next().filter(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_digit()))?;
        *slot = p.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

/// A UTC offset as PostgreSQL shows it: `+09`, `-03:30`, `+08:27:52`.
pub(crate) fn utc_offset(s: &str) -> bool {
    let Some(rest) = s.strip_prefix(['+', '-']) else { return false };
    let fields = match rest.matches(':').count() {
        0 => two_digit_fields::<1>(rest).map(|[h]| [h, 0, 0]),
        1 => two_digit_fields::<2>(rest).map(|[h, m]| [h, m, 0]),
        2 => two_digit_fields::<3>(rest),
        _ => None,
    };
    fields.is_some_and(|[h, m, s]| h <= 15 && m <= 59 && s <= 59)
}

/// Whether `s` is binary digits only.
pub(crate) fn binary(s: &str) -> bool {
    s.bytes().all(|b| b == b'0' || b == b'1')
}

/// Whether the type is a text of a fixed length (`char(n)`).
pub(crate) fn fixed_text(ty: &TypeName) -> bool {
    matches!(ty.name.as_str(), "character" | "char" | "bpchar")
}

/// Text no longer than a `varchar(n)` or `char(n)` holds, in characters.
pub(crate) fn text_length(ty: &TypeName, s: &str) -> Result<(), Invalid> {
    let limited = fixed_text(ty) || matches!(ty.name.as_str(), "character varying" | "varchar");
    match ty.args[..] {
        [max] if limited && s.chars().count() > max as usize => Err(Invalid::TooLong { max }),
        _ => Ok(()),
    }
}

/// `8-4-4-4-12` hexadecimal digits, either case.
pub(crate) fn uuid(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    groups.len() == 5
        && groups.iter().zip([8, 4, 4, 4, 12]).all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests;
