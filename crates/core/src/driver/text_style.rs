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
/// session with the very same settings.
pub fn reads_same(from: Option<&TextStyle>, to: Option<&TextStyle>) -> bool {
    match from {
        None => false,
        Some(f) => f.reads_anywhere() || to == Some(f),
    }
}

#[cfg(test)]
mod tests;
