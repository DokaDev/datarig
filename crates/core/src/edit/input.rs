//! Whether a value typed into a cell is a value of its column's type, in the form the result
//! shows that type's values in ([`check_input`]).
//!
//! What would be stored as something else than the text the user sees (rounded, cut, read as
//! another date) is refused here; what this code cannot judge is left to the server.

use super::target::TableColumn;
use crate::driver::ValueKind;
use crate::sql::dialect::Dialect;

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
    /// Not bytes as the result shows them: `\x` and hexadecimal digits in pairs; MySQL's `0x`
    /// and upper-case hexadecimal digits in pairs.
    NotBytes { mysql: bool },
    /// Not bits as the result shows them: binary digits; MySQL's `b'…'`.
    NotBits { mysql: bool },
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

/// Whether `text`, typed for table column `c` on a server of dialect `d`, is a value of the
/// column's type in the form the result shows that type's values in.
pub fn check_input(d: Dialect, c: &TableColumn, text: &str) -> Result<(), Invalid> {
    let mysql = matches!(d, Dialect::MySql(_));
    let col = &c.column;
    if !col.enum_labels.is_empty() && !matches!(c.kind, ValueKind::Array(_)) {
        return col.enum_labels.iter().any(|l| l == text).then_some(()).ok_or(Invalid::NotLabel);
    }
    let ty = TypeName::read(&col.type_name);
    match c.kind {
        ValueKind::Integer => integer(mysql, &ty, &col.type_name, text),
        ValueKind::Decimal => decimal(mysql, &ty, &col.type_name, text),
        ValueKind::Float => float(mysql, &ty, &col.type_name, text),
        ValueKind::Bool => matches!(text, "true" | "false").then_some(()).ok_or(Invalid::NotBool),
        ValueKind::Date => date(mysql, text),
        ValueKind::Time => time_of_day(&ty, text),
        ValueKind::Timestamp => timestamp(mysql, &ty, false, text),
        ValueKind::TimestampTz => timestamp(mysql, &ty, true, text),
        ValueKind::Bytes => bytes(mysql, text),
        ValueKind::Bit => bits(mysql, &ty, text),
        ValueKind::Text => self::text(mysql, &ty, text),
        ValueKind::Other if !mysql && ty.name == "uuid" => uuid(text).then_some(()).ok_or(Invalid::NotUuid),
        ValueKind::Other | ValueKind::Json | ValueKind::Interval | ValueKind::Array(_) => Ok(()),
    }
}

/// A type name as the server formats it, read for its checks: `int(11) unsigned` is `int`
/// with arguments `[11]`, unsigned; `timestamp(3) with time zone` is `timestamp with time
/// zone` with `[3]`.
struct TypeName {
    /// Lower case, without the arguments, `unsigned` and `zerofill`.
    name: String,
    /// The numbers in parentheses; empty when there are none (or they are not numbers).
    args: Vec<u32>,
    unsigned: bool,
}

impl TypeName {
    fn read(type_name: &str) -> TypeName {
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

/// A whole number, in the range of the column's type when the type is known.
fn integer(mysql: bool, ty: &TypeName, type_name: &str, s: &str) -> Result<(), Invalid> {
    let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Invalid::NotWholeNumber);
    }
    let range: Option<(i128, i128)> = if mysql {
        let bits = match ty.name.as_str() {
            "tinyint" | "bool" | "boolean" => 8,
            "smallint" => 16,
            "mediumint" => 24,
            "int" | "integer" => 32,
            "bigint" => 64,
            // Two-digit years and years before 1901 are read as other years.
            "year" => {
                let year = (digits.len() <= 10).then(|| s.parse::<i64>().ok()).flatten();
                let fits = year.is_some_and(|y| y == 0 || (1901..=2155).contains(&y));
                return fits.then_some(()).ok_or_else(|| out_of_range(type_name));
            }
            _ => 0,
        };
        (bits > 0).then(|| if ty.unsigned { (0, (1 << bits) - 1) } else { (-(1 << (bits - 1)), (1 << (bits - 1)) - 1) })
    } else {
        match ty.name.as_str() {
            "smallint" | "int2" => Some((i16::MIN.into(), i16::MAX.into())),
            "integer" | "int" | "int4" => Some((i32::MIN.into(), i32::MAX.into())),
            "bigint" | "int8" => Some((i64::MIN.into(), i64::MAX.into())),
            "oid" => Some((0, u32::MAX.into())),
            _ => None,
        }
    };
    let Some((min, max)) = range else { return Ok(()) };
    let value = (digits.trim_start_matches('0').len() <= 30).then(|| s.parse::<i128>().ok()).flatten();
    value.filter(|v| (min..=max).contains(v)).map(|_| ()).ok_or_else(|| out_of_range(type_name))
}

fn out_of_range(type_name: &str) -> Invalid {
    Invalid::OutOfRange { type_name: type_name.to_string() }
}

/// A decimal number: a sign, digits and a fraction, as many as `numeric(p, s)` holds. `money`
/// and types this code does not know are left to the server.
fn decimal(mysql: bool, ty: &TypeName, type_name: &str, s: &str) -> Result<(), Invalid> {
    let known = if mysql {
        matches!(ty.name.as_str(), "decimal" | "numeric" | "dec" | "fixed")
    } else {
        matches!(ty.name.as_str(), "numeric" | "decimal")
    };
    if !known {
        return Ok(());
    }
    // PostgreSQL's `numeric` shows these; a `numeric(p, s)` holds NaN but no infinity.
    if !mysql && (s == "NaN" || (ty.args.is_empty() && matches!(s, "Infinity" | "-Infinity"))) {
        return Ok(());
    }
    let (negative, int, frac) = plain_decimal(s).ok_or(Invalid::NotNumber)?;
    if mysql && ty.unsigned && negative && !(int.bytes().chain(frac.bytes()).all(|b| b == b'0')) {
        return Err(out_of_range(type_name));
    }
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
fn plain_decimal(s: &str) -> Option<(bool, &str, &str)> {
    let (negative, unsigned) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (int, frac) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    (digits(int) && digits(frac) && !(int.is_empty() && frac.is_empty())).then_some((negative, int, frac))
}

/// A floating-point number: a decimal one with an optional exponent; PostgreSQL's also `NaN`,
/// `Infinity` and `-Infinity`. It must be a value the type holds exactly as typed: the number
/// it is read as (single or double precision) has the same shortest digits; MySQL's
/// `FLOAT(M,D)` and `DOUBLE(M,D)` keep `D` digits after the point, and take the text the server
/// shows for a value too (its digits there, of the binary value); an unsigned one no negative
/// number.
fn float(mysql: bool, ty: &TypeName, type_name: &str, s: &str) -> Result<(), Invalid> {
    if !mysql && matches!(s, "NaN" | "Infinity" | "-Infinity") {
        return Ok(());
    }
    let (mantissa, exp) = match s.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e.strip_prefix(['-', '+']).unwrap_or(e))),
        None => (s, None),
    };
    let exp_ok = exp.is_none_or(|e| !e.is_empty() && e.bytes().all(|b| b.is_ascii_digit()));
    if plain_decimal(mantissa).is_none() || !exp_ok {
        return Err(Invalid::NotNumber);
    }
    // PostgreSQL's `float(p)` is `real` up to 24 bits, MySQL's `FLOAT(p)` too; a type this code
    // does not know is taken as the narrower one.
    let single = match (mysql, ty.name.as_str(), &ty.args[..]) {
        (_, "float", [p]) => *p <= 24,
        (false, "float" | "double precision" | "float8", _) => false,
        (true, "double" | "double precision" | "real", _) => false,
        _ => true,
    };
    let (value, stored) = if single {
        let v: f32 = s.parse().map_err(|_| Invalid::NotNumber)?;
        (f64::from(v), v.is_finite().then(|| format!("{v:e}")))
    } else {
        let v: f64 = s.parse().map_err(|_| Invalid::NotNumber)?;
        (v, v.is_finite().then(|| format!("{v:e}")))
    };
    let Some(stored) = stored else { return Err(out_of_range(type_name)) };
    let typed = digits_of(s);
    if mysql && ty.unsigned && value < 0.0 {
        return Err(out_of_range(type_name));
    }
    let rounded = |stored: String| Invalid::Rounded { type_name: type_name.to_string(), stored };
    let Some((_, digits, exp)) = &typed else { return Err(rounded(plain_float(&stored))) };
    let exact = typed == digits_of(&stored);
    let (true, &[m, d]) = (mysql, &ty.args[..]) else {
        return if exact { Ok(()) } else { Err(rounded(plain_float(&stored))) };
    };
    // MySQL's `FLOAT(M,D)` and `DOUBLE(M,D)` are shown with `D` digits after the point, as many
    // as the binary value has there (0.1 in a `DOUBLE(30,20)` is `0.10000000000000000555`): the
    // text read back is taken when it is what the server shows for the number it is read as.
    let shown = format!("{value:.*}", d as usize);
    let after = (digits.len() as i64 - 1 - exp).max(0);
    if after > i64::from(d) || !(exact || typed == digits_of(&shown)) {
        return Err(rounded(shown));
    }
    if (exp + 1).max(0) > i64::from(m.saturating_sub(d)) {
        return Err(out_of_range(type_name));
    }
    Ok(())
}

/// A number written `-1.25e3` as its sign, its significant digits and the power of ten of the
/// first one: `(true, "125", 3)`; zero has no digits. `None` when the exponent is too large to
/// read.
fn digits_of(s: &str) -> Option<(bool, String, i64)> {
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
fn plain_float(sci: &str) -> String {
    match sci.split_once('e').map(|(_, e)| e.parse::<i32>()) {
        Some(Ok(exp)) if (-4..17).contains(&exp) => sci.parse::<f64>().map_or(sci.to_string(), |v| v.to_string()),
        _ => sci.to_string(),
    }
}

/// A date as the result shows it: `YYYY-MM-DD`, a day of the calendar. PostgreSQL's also
/// `infinity`, `-infinity` and years `BC` (and past 9999); MySQL's zero date and zero month or
/// day, which the server takes or refuses by its mode.
fn date(mysql: bool, s: &str) -> Result<(), Invalid> {
    if !mysql && matches!(s, "infinity" | "-infinity") {
        return Ok(());
    }
    let (s, bc) = if mysql { (s, false) } else { s.strip_suffix(" BC").map_or((s, false), |d| (d, true)) };
    ymd(mysql, s, bc).then_some(()).ok_or(Invalid::NotDate)
}

fn ymd(mysql: bool, s: &str, bc: bool) -> bool {
    let mut parts = s.split('-');
    let (Some(y), Some(m), Some(d), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let digits = |p: &str, len: bool| len && p.bytes().all(|b| b.is_ascii_digit());
    let year_len = if mysql { y.len() == 4 } else { (4..=7).contains(&y.len()) };
    if !(digits(y, year_len) && digits(m, m.len() == 2) && digits(d, d.len() == 2)) {
        return false;
    }
    let (Ok(y), Ok(m), Ok(d)) = (y.parse::<i64>(), m.parse::<u32>(), d.parse::<u32>()) else { return false };
    if mysql && (m == 0 || d == 0) {
        return m <= 12 && d <= 31;
    }
    if !mysql && y == 0 {
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

/// How many digits of a second the type keeps: its argument, else the server's default
/// (PostgreSQL 6, MySQL 0).
fn fraction_digits(mysql: bool, ty: &TypeName) -> u32 {
    ty.args.first().copied().unwrap_or(if mysql { 0 } else { 6 })
}

/// A time of day (`time`, `time with time zone`, PostgreSQL's: MySQL's `TIME` is an interval).
fn time_of_day(ty: &TypeName, s: &str) -> Result<(), Invalid> {
    let zoned = ty.name == "timetz" || ty.name.ends_with("with time zone");
    let (time, offset) = split_offset(s);
    if offset.is_some() != zoned || !offset.is_none_or(utc_offset) {
        return Err(Invalid::NotTime);
    }
    hms(time, fraction_digits(false, ty), true).ok_or(Invalid::NotTime)?
}

/// A date and time as the result shows it: PostgreSQL's `timestamp with time zone` with the
/// offset it is shown in, a year `BC` after it, `infinity` and `-infinity`; MySQL's without an
/// offset (a `TIMESTAMP` is shown in the session's time zone).
fn timestamp(mysql: bool, ty: &TypeName, zoned: bool, s: &str) -> Result<(), Invalid> {
    if !mysql && matches!(s, "infinity" | "-infinity") {
        return Ok(());
    }
    let (s, bc) = if mysql { (s, false) } else { s.strip_suffix(" BC").map_or((s, false), |t| (t, true)) };
    let (date, rest) = s.split_once(' ').ok_or(Invalid::NotTimestamp)?;
    let (time, offset) = split_offset(rest);
    let offset_ok = match offset {
        Some(o) => zoned && !mysql && utc_offset(o),
        None => !zoned || mysql,
    };
    if !offset_ok || !ymd(mysql, date, bc) {
        return Err(Invalid::NotTimestamp);
    }
    hms(time, fraction_digits(mysql, ty), !mysql).ok_or(Invalid::NotTimestamp)?
}

/// `12:00:00+09` as the time and the offset.
fn split_offset(s: &str) -> (&str, Option<&str>) {
    match s.find(['+', '-']) {
        Some(i) => (&s[..i], Some(&s[i..])),
        None => (s, None),
    }
}

/// `HH:MM:SS` and at most `keep` significant digits of a second (`24:00:00` too when `midnight`).
/// `None` when it is no time; else whether the fraction fits.
fn hms(s: &str, keep: u32, midnight: bool) -> Option<Result<(), Invalid>> {
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
fn utc_offset(s: &str) -> bool {
    let Some(rest) = s.strip_prefix(['+', '-']) else { return false };
    let fields = match rest.matches(':').count() {
        0 => two_digit_fields::<1>(rest).map(|[h]| [h, 0, 0]),
        1 => two_digit_fields::<2>(rest).map(|[h, m]| [h, m, 0]),
        2 => two_digit_fields::<3>(rest),
        _ => None,
    };
    fields.is_some_and(|[h, m, s]| h <= 15 && m <= 59 && s <= 59)
}

/// Bytes as the result shows them: PostgreSQL's `\x` and hexadecimal digits (either case);
/// MySQL's `0x` and upper-case ones (other text would be written as a string of that text).
fn bytes(mysql: bool, s: &str) -> Result<(), Invalid> {
    let hex = if mysql {
        s.strip_prefix("0x").filter(|h| h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'A'..=b'F')))
    } else {
        s.strip_prefix("\\x").filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
    };
    hex.is_some_and(|h| h.len() % 2 == 0).then_some(()).ok_or(Invalid::NotBytes { mysql })
}

/// Bits as the result shows them: PostgreSQL's binary digits, exactly `n` for `bit(n)` and at
/// most `n` for `bit varying(n)`; MySQL's `b'…'`, no more significant digits than `bit(n)` has.
fn bits(mysql: bool, ty: &TypeName, s: &str) -> Result<(), Invalid> {
    let binary = |d: &str| d.bytes().all(|b| b == b'0' || b == b'1');
    if mysql {
        let digits = s.strip_prefix("b'").and_then(|r| r.strip_suffix('\'')).filter(|d| !d.is_empty() && binary(d));
        let digits = digits.ok_or(Invalid::NotBits { mysql })?;
        return match ty.args[..] {
            [n] if digits.trim_start_matches('0').len() > n as usize => {
                Err(Invalid::BitCount { count: n, exact: false })
            }
            _ => Ok(()),
        };
    }
    if !binary(s) {
        return Err(Invalid::NotBits { mysql });
    }
    let exact = ty.name == "bit";
    match ty.args[..] {
        [n] if (exact && s.len() != n as usize) || s.len() > n as usize => Err(Invalid::BitCount { count: n, exact }),
        _ => Ok(()),
    }
}

/// Text no longer than a `varchar(n)` or `char(n)` holds, in characters; for MySQL's `CHAR`,
/// which drops trailing spaces when it reads a value back, without them.
fn text(mysql: bool, ty: &TypeName, s: &str) -> Result<(), Invalid> {
    let fixed = matches!(ty.name.as_str(), "character" | "char" | "bpchar");
    let limited = fixed || matches!(ty.name.as_str(), "character varying" | "varchar");
    if mysql && fixed && s.ends_with(' ') {
        return Err(Invalid::TrailingSpaces);
    }
    match ty.args[..] {
        [max] if limited && s.chars().count() > max as usize => Err(Invalid::TooLong { max }),
        _ => Ok(()),
    }
}

/// `8-4-4-4-12` hexadecimal digits, either case.
fn uuid(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    groups.len() == 5
        && groups.iter().zip([8, 4, 4, 4, 12]).all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests;
