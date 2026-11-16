//! How a PostgreSQL session writes dates, times and intervals as text: its `DateStyle`,
//! `IntervalStyle` and `TimeZone`, as the server reports them (all three are reported to the
//! client at startup and whenever they change: a `SET`, a `RESET`, a `SET LOCAL`, a rolled back
//! `SET`).
//!
//! The values a driver reads in the server's text output (a range, a composite, a type it does
//! not decode itself) are written in these settings, so the same text can be another value in a
//! session with others: `[02/01/2026,05/01/2026)` is January in `SQL, DMY` and February in
//! `SQL, MDY`. A result records the settings it was read in ([`TextStyle`]), and text copied
//! from it goes into another result only where it reads back the same ([`reads_same`]).

/// The settings a PostgreSQL session writes dates, times and intervals in, as the server
/// reported them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextStyle {
    /// `DateStyle` (`ISO, MDY`, `SQL, DMY`, `German, DMY`).
    pub date_style: String,
    /// `IntervalStyle` (`postgres`, `postgres_verbose`, `sql_standard`, `iso_8601`).
    pub interval_style: String,
    /// `TimeZone` (`UTC`, `Asia/Seoul`, `<+09>-09`).
    pub time_zone: String,
}

/// The interval styles whose text gives every field its own sign or unit, so it reads back as
/// the same interval whatever the reading session's `IntervalStyle` (`sql_standard` writes
/// `-1 2:00:00` for minus one day and two hours, which `postgres` reads as minus one day plus
/// two hours).
const SELF_SIGNED_INTERVALS: [&str; 3] = ["postgres", "postgres_verbose", "iso_8601"];

impl TextStyle {
    /// Text written in this style reads back as the same values in a session with any
    /// settings: dates in ISO 8601 (`DateStyle` `ISO`, which every session reads whatever its
    /// field order, and whose times with a zone carry a numeric offset, never a zone's
    /// abbreviation), and intervals in a style that signs each field
    /// ([`SELF_SIGNED_INTERVALS`]).
    pub fn reads_anywhere(&self) -> bool {
        let output = self.date_style.split(',').next().unwrap_or_default().trim();
        output.eq_ignore_ascii_case("ISO")
            && SELF_SIGNED_INTERVALS.iter().any(|s| self.interval_style.trim().eq_ignore_ascii_case(s))
    }
}

/// Whether text a session wrote in `from` reads back as the same values in a session in `to`
/// (`None`: not known). Text written in a style not known is never taken to: it may be in any.
/// Text in a style that [reads anywhere](TextStyle::reads_anywhere) does; other text only in a
/// session with the very same settings, and only when it holds no time with a time zone
/// (`zoned`, [`ColumnMeta::zoned`](super::ColumnMeta)): outside ISO such a time is written with
/// the zone's abbreviation, read in the reading session's `timezone_abbreviations`, which the
/// server never reports (`CST` is UTC-06:00 in `Default` and UTC+09:30 in `Australia`).
pub fn reads_same(from: Option<&TextStyle>, to: Option<&TextStyle>, zoned: bool) -> bool {
    match from {
        None => false,
        Some(f) => f.reads_anywhere() || (!zoned && to == Some(f)),
    }
}

/// What dates, times or intervals a column's values hold, as the server reads them from text
/// (whatever format a result reads them in).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Dated {
    /// None: its text is read the same whatever the session's settings (numbers, text, JSON,
    /// geometry, enums, ranges of numbers, …), or the driver does not say (MySQL).
    #[default]
    No,
    /// Values of one kind of date or time: alone, or in a range, a multirange, an array or a
    /// domain (`date`, `daterange`, `tstzrange[]`, a domain over `interval`).
    Of(DatePart),
    /// The driver cannot tell what of them it holds: a composite with such a field, a
    /// pseudo-type, a type of an extension or of the database's own.
    Unknown,
    /// A type of the database's own (`oid`) that the connection reports as a base type, which
    /// may also be a multirange: not known until the metadata session says its kind
    /// ([`super::DbCommand::LoadTypeKinds`]); a base type or an enum holds none, any other
    /// kind is not known.
    Lookup(u32),
}

/// A kind of date or time a value holds ([`Dated::Of`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DatePart {
    Date,
    Timestamp,
    TimestampTz,
    Time,
    TimeTz,
    Interval,
}

/// Whether `text`, read as a value of a column that holds `dated`, reads as the same value
/// whatever `DateStyle`, `IntervalStyle` and `TimeZone` the reading session has: every date
/// and time in it is in ISO 8601 form (`2026-01-02`, `2026-01-02 10:00:00`), every time with a
/// zone carries a numeric offset or `Z` (`+09`, `-03:30`), and every interval is in ISO 8601
/// form (`P1DT2H`); `infinity`, `-infinity`, `empty` and `NULL` read the same anywhere. Any text
/// of a column that holds no dates does; of one whose dates the driver cannot tell, none:
/// not known is never the same.
pub fn reads_same_anywhere(dated: Dated, text: &str) -> bool {
    let part: fn(&str) -> bool = match dated {
        Dated::No => return true,
        Dated::Unknown | Dated::Lookup(_) => return false,
        Dated::Of(DatePart::Date) => iso_date,
        Dated::Of(DatePart::Timestamp) => |p| iso_timestamp(p, false),
        Dated::Of(DatePart::TimestampTz) => |p| iso_timestamp(p, true),
        Dated::Of(DatePart::Time) => |p| iso_time(p, false),
        Dated::Of(DatePart::TimeTz) => |p| iso_time(p, true),
        Dated::Of(DatePart::Interval) => iso_interval,
    };
    // The parts between the punctuation of ranges, multiranges and arrays (`[`, `)`, `{`, `,`,
    // quotes); a bound left open or an empty range or array has none.
    let parts = text.split(['[', ']', '(', ')', '{', '}', ',', '"']).map(str::trim).filter(|p| !p.is_empty());
    let mut any = false;
    for p in parts {
        any = true;
        let special = ["empty", "infinity", "-infinity", "NULL"].iter().any(|w| p.eq_ignore_ascii_case(w));
        if !special && !part(p) {
            return false;
        }
    }
    any || !text.trim().is_empty()
}

fn digits(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_digit())
}

/// `2026-01-02` (a year of four digits or more), with ` BC` after it or not.
fn iso_date(p: &str) -> bool {
    let p = p.strip_suffix(" BC").unwrap_or(p);
    let mut it = p.split('-');
    let (Some(y), Some(m), Some(d), None) = (it.next(), it.next(), it.next(), it.next()) else { return false };
    y.len() >= 4 && y.bytes().all(|b| b.is_ascii_digit()) && digits(m, 2) && digits(d, 2)
}

/// `10:00:00` or `10:00:00.5`; with a numeric offset or `Z` after it (`zoned`) or without.
fn iso_time(p: &str, zoned: bool) -> bool {
    let (clock, offset) = match p.find(['+', '-', 'Z']) {
        Some(i) => (&p[..i], Some(&p[i..])),
        None => (p, None),
    };
    let ok_offset = match offset {
        None => !zoned,
        Some("Z") => zoned,
        Some(o) => {
            zoned && {
                let o = &o[1..];
                let mut it = o.split(':');
                it.next().is_some_and(|h| digits(h, 2)) && it.all(|x| digits(x, 2))
            }
        }
    };
    let (hms, frac) = clock.split_once('.').map_or((clock, None), |(a, b)| (a, Some(b)));
    let mut it = hms.split(':');
    let clock_ok = matches!((it.next(), it.next(), it.next(), it.next()), (Some(h), Some(m), Some(s), None)
        if digits(h, 2) && digits(m, 2) && digits(s, 2))
        && frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()));
    clock_ok && ok_offset
}

/// `2026-01-02 10:00:00` (`zoned`: with a numeric offset or `Z`), with ` BC` after it or not.
fn iso_timestamp(p: &str, zoned: bool) -> bool {
    let p = p.strip_suffix(" BC").unwrap_or(p);
    let Some((date, time)) = p.split_once(' ') else { return false };
    iso_date(date) && iso_time(time, zoned)
}

/// An interval in ISO 8601 form: `P1Y2M3DT4H5M6.5S`, with signs on its fields or not.
fn iso_interval(p: &str) -> bool {
    let body = p.strip_prefix('-').unwrap_or(p);
    let Some(rest) = body.strip_prefix('P') else { return false };
    !rest.is_empty()
        && rest.bytes().any(|b| b.is_ascii_digit())
        && rest.bytes().all(|b| b.is_ascii_digit() || b"YMWDTHS.-".contains(&b))
}

#[cfg(test)]
mod tests;
