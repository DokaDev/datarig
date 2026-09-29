//! PostgreSQL values -> display strings. Every column is fetched as raw bytes ([`Raw`]) in the
//! result format [`format_code`] picks for its type: binary for the types formatted here
//! (numbers, booleans, uuid, bytea, json, inet, dates and times, intervals, and arrays of
//! these), PostgreSQL's own text output for every other type (ranges and multiranges,
//! geometry, bit strings, text search, `money`, `"char"`, `reg*`, enums, composites, extension
//! types and any type this code does not know). So a value is always shown, copied and
//! written back as the text PostgreSQL reads, never as raw binary.

use fallible_iterator::FallibleIterator;
use tokio_postgres::types::{FromSql, Kind, Type};

/// Accepts any type and hands back the raw wire bytes (or NULL).
pub struct Raw<'a>(pub Option<&'a [u8]>);

type BoxErr = Box<dyn std::error::Error + Sync + Send>;

impl<'a> FromSql<'a> for Raw<'a> {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, BoxErr> {
        Ok(Raw(Some(raw)))
    }
    fn from_sql_null(_: &Type) -> Result<Self, BoxErr> {
        Ok(Raw(None))
    }
    fn accepts(_: &Type) -> bool {
        true
    }
}

pub fn is_numeric(ty: &Type) -> bool {
    matches!(
        *ty,
        Type::INT2 | Type::INT4 | Type::INT8 | Type::FLOAT4 | Type::FLOAT8 | Type::NUMERIC | Type::OID | Type::MONEY
    ) || matches!(ty.kind(), Kind::Domain(inner) if is_numeric(inner))
}

pub fn is_json(ty: &Type) -> bool {
    matches!(*ty, Type::JSON | Type::JSONB)
}

/// The result format to ask for a column of type `ty`: `1` (binary) when [`format_value`]
/// decodes it, else `0` (PostgreSQL's text output, passed on as it is).
pub fn format_code(ty: &Type) -> i16 {
    i16::from(decodes_binary(ty))
}

/// Whether [`format_value`] decodes the binary format of `ty` (a domain: its base type; an
/// array: its element type).
fn decodes_binary(ty: &Type) -> bool {
    match *ty {
        Type::BOOL
        | Type::INT2
        | Type::INT4
        | Type::INT8
        | Type::OID
        | Type::FLOAT4
        | Type::FLOAT8
        | Type::NUMERIC
        | Type::JSONB
        | Type::JSON
        | Type::TEXT
        | Type::VARCHAR
        | Type::BPCHAR
        | Type::NAME
        | Type::TIMESTAMPTZ
        | Type::TIMESTAMP
        | Type::DATE
        | Type::TIME
        | Type::TIMETZ
        | Type::INTERVAL
        | Type::UUID
        | Type::BYTEA
        | Type::INET
        | Type::CIDR => true,
        _ => match ty.kind() {
            Kind::Array(elem) => decodes_binary(elem),
            Kind::Domain(inner) => decodes_binary(inner),
            _ => false,
        },
    }
}

pub fn type_display(ty: &Type) -> String {
    match ty.kind() {
        Kind::Array(elem) => format!("{}[]", elem.name()),
        _ => ty.name().to_string(),
    }
}

fn hex(raw: &[u8]) -> String {
    let mut s = String::with_capacity(2 + raw.len() * 2);
    s.push_str("\\x");
    for b in raw {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn fallback(raw: &[u8]) -> String {
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_string(),
        Err(_) => hex(raw),
    }
}

fn get<'a, T: FromSql<'a>>(ty: &Type, raw: &'a [u8]) -> Option<T> {
    T::from_sql(ty, raw).ok()
}

/// A UTC offset as PostgreSQL writes it: `+09`, `-03:30`, `+08:27:52` (local mean time).
fn fmt_offset(secs_east: i32) -> String {
    let sign = if secs_east < 0 { '-' } else { '+' };
    let a = secs_east.abs();
    let (h, m, s) = (a / 3600, (a % 3600) / 60, a % 60);
    match (m, s) {
        (0, 0) => format!("{sign}{h:02}"),
        (_, 0) => format!("{sign}{h:02}:{m:02}"),
        _ => format!("{sign}{h:02}:{m:02}:{s:02}"),
    }
}

/// Days since 2000-01-01 (PostgreSQL's epoch) -> proleptic Gregorian (year, month, day), for
/// any day PostgreSQL stores (H. Hinnant's `civil_from_days`, no range limit of its own).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 10_957 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// A date as PostgreSQL's ISO style writes it; year 0 and before are `BC` (year 0 is 1 BC).
/// Returns the text and whether it is BC (the suffix goes after a time and an offset).
fn fmt_date(days: i64) -> (String, bool) {
    let (y, m, d) = civil(days);
    if y <= 0 { (format!("{:04}-{m:02}-{d:02}", 1 - y), true) } else { (format!("{y:04}-{m:02}-{d:02}"), false) }
}

/// `timestamp` (microseconds since 2000-01-01), with `offset` seconds east for `timestamptz`
/// (already added to `micros`): `2024-03-15 12:00:00.5`, `0044-03-15 12:00:00+00 BC`,
/// `infinity`.
fn fmt_timestamp(micros: i64, offset: Option<i32>) -> String {
    let (date, bc) = fmt_date(micros.div_euclid(86_400_000_000));
    let time = fmt_micros_time(micros.rem_euclid(86_400_000_000));
    let off = offset.map(fmt_offset).unwrap_or_default();
    format!("{date} {time}{off}{}", if bc { " BC" } else { "" })
}

/// `timestamptz` in the local time zone (the offset PostgreSQL would use there, local mean
/// time included); outside chrono's range, in UTC.
fn fmt_timestamptz(micros: i64) -> String {
    use chrono::{Offset, TimeZone};
    let offset = chrono::DateTime::from_timestamp_micros(micros.saturating_add(946_684_800_000_000))
        .map(|utc| chrono::Local.offset_from_utc_datetime(&utc.naive_utc()).fix().local_minus_utc())
        .unwrap_or(0);
    fmt_timestamp(micros + i64::from(offset) * 1_000_000, Some(offset))
}

/// A float as PostgreSQL names its special values (`Infinity`, `-Infinity`, `NaN`); finite
/// values in Rust's shortest form that reads back to the same value.
/// A float as the server writes it with the default `extra_float_digits = 1` (PostgreSQL 12
/// and later): the shortest digits that read back to the same value, in fixed notation when
/// the decimal exponent is at least -4 and below `fixed_below` (15 for float8, 6 for float4),
/// else in exponent notation with a sign and at least two digits (`1e+15`, `5e-324`,
/// `3.4028235e+38`). `sci` is Rust's `{:e}` text of the value, which has the same shortest
/// digits.
fn fmt_float(v: f64, sci: String, fixed_below: i32) -> String {
    match v {
        f64::INFINITY => return "Infinity".into(),
        f64::NEG_INFINITY => return "-Infinity".into(),
        _ if v.is_nan() => return "NaN".into(),
        _ => {}
    }
    let Some((mantissa, exp)) = sci.split_once('e') else { return sci };
    let Ok(exp) = exp.parse::<i32>() else { return sci };
    let (sign, mantissa) = mantissa.strip_prefix('-').map_or(("", mantissa), |m| ("-", m));
    if !(-4..fixed_below).contains(&exp) {
        let exp_sign = if exp < 0 { '-' } else { '+' };
        return format!("{sign}{mantissa}e{exp_sign}{:02}", exp.abs());
    }
    let digits = mantissa.replace('.', "");
    let point = exp + 1;
    let fixed = if point <= 0 {
        format!("0.{}{digits}", "0".repeat(point.unsigned_abs() as usize))
    } else if (point as usize) < digits.len() {
        format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
    } else {
        format!("{digits}{}", "0".repeat(point as usize - digits.len()))
    };
    format!("{sign}{fixed}")
}

fn fmt_micros_time(micros: i64) -> String {
    let secs = micros / 1_000_000;
    let frac = micros % 1_000_000;
    let base = format!("{:02}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60);
    if frac == 0 { base } else { format!("{base}.{}", format!("{frac:06}").trim_end_matches('0')) }
}

/// The display text of a value of type `ty` fetched in the format [`format_code`] picked:
/// decoded from binary, or PostgreSQL's text output as it is (`text`).
pub fn format_value(ty: &Type, raw: &[u8], text: bool) -> String {
    if text {
        return String::from_utf8_lossy(raw).into_owned();
    }
    format_binary(ty, raw)
}

fn format_binary(ty: &Type, raw: &[u8]) -> String {
    match *ty {
        Type::BOOL => get::<bool>(ty, raw).map(|b| b.to_string()),
        Type::INT2 => get::<i16>(ty, raw).map(|v| v.to_string()),
        Type::INT4 => get::<i32>(ty, raw).map(|v| v.to_string()),
        Type::INT8 => get::<i64>(ty, raw).map(|v| v.to_string()),
        Type::OID => get::<u32>(ty, raw).map(|v| v.to_string()),
        Type::FLOAT4 => get::<f32>(ty, raw).map(|v| fmt_float(f64::from(v), format!("{v:e}"), 6)),
        Type::FLOAT8 => get::<f64>(ty, raw).map(|v| fmt_float(v, format!("{v:e}"), 15)),
        Type::NUMERIC => numeric_to_string(raw),
        Type::JSONB => raw.split_first().map(|(_, rest)| fallback(rest)),
        Type::TIMESTAMPTZ | Type::TIMESTAMP => get::<i64>(&Type::INT8, raw).map(|micros| match micros {
            i64::MAX => "infinity".into(),
            i64::MIN => "-infinity".into(),
            _ if *ty == Type::TIMESTAMPTZ => fmt_timestamptz(micros),
            _ => fmt_timestamp(micros, None),
        }),
        Type::DATE => get::<i32>(&Type::INT4, raw).map(|days| match days {
            i32::MAX => "infinity".into(),
            i32::MIN => "-infinity".into(),
            _ => match fmt_date(i64::from(days)) {
                (d, true) => format!("{d} BC"),
                (d, false) => d,
            },
        }),
        Type::TIME => get::<i64>(&Type::INT8, raw).map(fmt_micros_time),
        Type::TIMETZ if raw.len() == 12 => {
            let micros = i64::from_be_bytes(raw[..8].try_into().unwrap_or_default());
            let west = i32::from_be_bytes(raw[8..].try_into().unwrap_or_default());
            Some(format!("{}{}", fmt_micros_time(micros), fmt_offset(-west)))
        }
        Type::INTERVAL if raw.len() == 16 => {
            let micros = i64::from_be_bytes(raw[..8].try_into().unwrap_or_default());
            let days = i32::from_be_bytes(raw[8..12].try_into().unwrap_or_default());
            let months = i32::from_be_bytes(raw[12..].try_into().unwrap_or_default());
            Some(interval_to_string(months, days, micros))
        }
        Type::UUID if raw.len() == 16 => {
            let h: String = raw.iter().map(|b| format!("{b:02x}")).collect();
            Some(format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]))
        }
        Type::BYTEA => Some(hex(raw)),
        Type::INET | Type::CIDR => postgres_protocol::types::inet_from_sql(raw).ok().map(|i| {
            let full = if i.addr().is_ipv4() { 32 } else { 128 };
            if i.netmask() == full && *ty == Type::INET {
                i.addr().to_string()
            } else {
                format!("{}/{}", i.addr(), i.netmask())
            }
        }),
        _ => match ty.kind() {
            Kind::Array(elem) => array_to_string(elem, raw),
            Kind::Domain(inner) => Some(format_binary(inner, raw)),
            _ => None,
        },
    }
    .unwrap_or_else(|| fallback(raw))
}

/// An array as `array_out` writes it: nested braces per dimension (`{{1,2},{3,4}}`), the
/// bounds first when a lower bound is not 1 (`[0:1]={7,8}`), `{}` when empty, and elements
/// quoted when they must be.
fn array_to_string(elem: &Type, raw: &[u8]) -> Option<String> {
    let arr = postgres_protocol::types::array_from_sql(raw).ok()?;
    let dims: Vec<_> = arr.dimensions().collect().ok()?;
    let mut parts = Vec::new();
    let mut it = arr.values();
    while let Some(v) = it.next().ok()? {
        parts.push(match v {
            None => "NULL".to_string(),
            Some(b) => {
                // `array_out` writes booleans as `t` / `f` (a lone bool is shown as a word).
                let s = match (elem, format_binary(elem, b)) {
                    (&Type::BOOL, s) => s[..1].to_string(),
                    (_, s) => s,
                };
                // Quoted like `array_out`: empty, `NULL`, a delimiter, a brace, a quote, a
                // backslash or white space (`array_isspace`).
                let special = [',', '"', '{', '}', '\\', ' ', '\t', '\n', '\r', '\u{b}', '\u{c}'];
                if s.is_empty() || s.contains(special) || s.eq_ignore_ascii_case("null") {
                    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
                } else {
                    s
                }
            }
        });
    }
    let lens: Vec<usize> = dims.iter().map(|d| usize::try_from(d.len).unwrap_or(0)).collect();
    if dims.is_empty() || parts.len() != lens.iter().product::<usize>() {
        return (parts.is_empty() && dims.is_empty()).then(|| "{}".to_string());
    }
    let bounds: String = if dims.iter().all(|d| d.lower_bound == 1) {
        String::new()
    } else {
        let b: String = dims
            .iter()
            .map(|d| format!("[{}:{}]", d.lower_bound, i64::from(d.lower_bound) + i64::from(d.len) - 1))
            .collect();
        format!("{b}=")
    };
    Some(format!("{bounds}{}", nest(&parts, &lens)))
}

/// `parts` (row-major) as nested braces, one level per length in `lens`.
fn nest(parts: &[String], lens: &[usize]) -> String {
    match lens {
        [] | [_] => format!("{{{}}}", parts.join(",")),
        [n, rest @ ..] => {
            let step = parts.len() / (*n).max(1);
            let inner: Vec<String> = parts.chunks(step.max(1)).map(|c| nest(c, rest)).collect();
            format!("{{{}}}", inner.join(","))
        }
    }
}

/// An interval as PostgreSQL writes it with `IntervalStyle = postgres` (the default,
/// `EncodeInterval`): `1 year 2 mons -3 days +04:05:06.5`. A field after a negative one shows
/// its `+`; `-1 years` (only `1` is singular). `infinity` and `-infinity` (PostgreSQL 17).
fn interval_to_string(months: i32, days: i32, micros: i64) -> String {
    match (months, days, micros) {
        (i32::MAX, i32::MAX, i64::MAX) => return "infinity".into(),
        (i32::MIN, i32::MIN, i64::MIN) => return "-infinity".into(),
        _ => {}
    }
    let mut out = String::new();
    let mut before = false;
    for (value, unit) in [(i64::from(months / 12), "year"), (i64::from(months % 12), "mon"), (i64::from(days), "day")] {
        if value == 0 {
            continue;
        }
        let space = if out.is_empty() { "" } else { " " };
        let plus = if before && value > 0 { "+" } else { "" };
        out.push_str(&format!("{space}{plus}{value} {unit}{}", if value == 1 { "" } else { "s" }));
        before = value < 0;
    }
    if out.is_empty() || micros != 0 {
        let space = if out.is_empty() { "" } else { " " };
        let sign = if micros < 0 {
            "-"
        } else if before {
            "+"
        } else {
            ""
        };
        out.push_str(&format!("{space}{sign}{}", fmt_micros_time(micros.checked_abs().unwrap_or(i64::MAX))));
    }
    out
}

/// Decode the NUMERIC binary format: ndigits, weight, sign, dscale, then base-10000 digits.
pub fn numeric_to_string(raw: &[u8]) -> Option<String> {
    if raw.len() < 8 {
        return None;
    }
    let rd = |i: usize| i16::from_be_bytes([raw[i], raw[i + 1]]);
    let ndigits = rd(0).max(0) as usize;
    let weight = rd(2) as i32;
    let sign = u16::from_be_bytes([raw[4], raw[5]]);
    let dscale = u16::from_be_bytes([raw[6], raw[7]]) as usize;
    if raw.len() < 8 + ndigits * 2 {
        return None;
    }
    match sign {
        0xC000 => return Some("NaN".into()),
        0xD000 => return Some("Infinity".into()),
        0xF000 => return Some("-Infinity".into()),
        _ => {}
    }
    let digit = |k: i32| -> i16 { if k >= 0 && (k as usize) < ndigits { rd(8 + 2 * k as usize) } else { 0 } };
    let mut out = String::new();
    if sign == 0x4000 {
        out.push('-');
    }
    if weight < 0 {
        out.push('0');
    } else {
        for k in 0..=weight {
            if k == 0 {
                out.push_str(&digit(k).to_string());
            } else {
                out.push_str(&format!("{:04}", digit(k)));
            }
        }
    }
    if dscale > 0 {
        let mut frac = String::new();
        let groups = dscale.div_ceil(4) as i32;
        for j in 1..=groups {
            frac.push_str(&format!("{:04}", digit(weight + j)));
        }
        frac.truncate(dscale);
        out.push('.');
        out.push_str(&frac);
    }
    if out == "-0" || (out.starts_with("-0.") && out[3..].bytes().all(|b| b == b'0')) {
        out.remove(0);
    }
    Some(out)
}

#[cfg(test)]
mod tests;
