//! Dates and times as a chart places them: seconds on one line, read from the text a driver
//! delivers (`2026-10-07`, `2026-10-07 14:03:00.5+09`, `2026-10-07T14:03:00Z`, `14:03:00`).
//!
//! A value is placed by the wall-clock time its text shows: a zone offset is read and left
//! out, so the axis says what the cells say (a result's offsets are its session's zone). Dates
//! before the common era and `infinity` are not placed.

/// What a parsed value had: a date, a time of day or both.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parsed {
    /// Seconds since 1970-01-01 00:00 of the wall clock (a time alone: since midnight).
    pub secs: f64,
    pub date: bool,
    pub time: bool,
}

/// Seconds in a day.
pub const DAY: f64 = 86_400.0;

/// The value of `text`, if it reads as a date, a timestamp or a time of day.
pub fn parse(text: &str) -> Option<Parsed> {
    let s = text.trim();
    if s.is_empty() || s.ends_with("BC") {
        return None;
    }
    let b = s.as_bytes();
    if let Some((days, rest)) = date(b) {
        let rest = match rest.first() {
            None => return Some(Parsed { secs: days as f64 * DAY, date: true, time: false }),
            Some(b' ' | b'T' | b't') => &rest[1..],
            Some(_) => return None,
        };
        let secs = clock(rest)?;
        return Some(Parsed { secs: days as f64 * DAY + secs, date: true, time: true });
    }
    clock(b).map(|secs| Parsed { secs, date: false, time: true })
}

/// `YYYY-MM-DD` at the start of `b`: its day number and what follows.
fn date(b: &[u8]) -> Option<(i64, &[u8])> {
    let (year, n) = digits(b, 4, 6)?;
    let b = b[n..].strip_prefix(b"-")?;
    let (month, n) = digits(b, 2, 2)?;
    let b = b[n..].strip_prefix(b"-")?;
    let (day, n) = digits(b, 2, 2)?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&month) || day < 1 || day > days[(month - 1) as usize] {
        return None;
    }
    Some((days_from_civil(year, month as u32, day as u32), &b[n..]))
}

/// `HH:MM[:SS[.fff]]` and an optional zone (`Z`, `+09`, `-03:30`, `+0530`) that is all of
/// `b`: seconds since midnight.
fn clock(b: &[u8]) -> Option<f64> {
    let (h, n) = digits(b, 2, 2)?;
    let b = b[n..].strip_prefix(b":")?;
    let (m, n) = digits(b, 2, 2)?;
    let mut b = &b[n..];
    let mut secs = 0.0;
    if let Some(rest) = b.strip_prefix(b":") {
        let (s, n) = digits(rest, 2, 2)?;
        secs = s as f64;
        b = &rest[n..];
        if let Some(rest) = b.strip_prefix(b".") {
            let k = rest.iter().take_while(|c| c.is_ascii_digit()).count();
            if k == 0 {
                return None;
            }
            // Nanoseconds are as fine as a chart places anything.
            let d = k.min(9);
            let frac: f64 = std::str::from_utf8(&rest[..d]).ok()?.parse::<f64>().ok()? / 10f64.powi(d as i32);
            secs += frac;
            b = &rest[k..];
        }
    }
    // `24:00:00` is the end of a day; a leap second may be written.
    if h > 24 || m > 59 || secs >= 61.0 || (h == 24 && (m > 0 || secs > 0.0)) {
        return None;
    }
    zone(b)?;
    Some(h as f64 * 3600.0 + m as f64 * 60.0 + secs)
}

/// A zone offset that is all of `b` (or nothing).
fn zone(b: &[u8]) -> Option<()> {
    match b {
        [] | b"Z" | b"z" => Some(()),
        [b'+' | b'-', rest @ ..] => {
            let ok = rest.iter().all(|c| c.is_ascii_digit() || *c == b':') && (2..=8).contains(&rest.len());
            ok.then_some(())
        }
        _ => None,
    }
}

/// `min..=max` ASCII digits at the start of `b` (no more): their value and count.
fn digits(b: &[u8], min: usize, max: usize) -> Option<(i64, usize)> {
    let n = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if n < min || n > max {
        return None;
    }
    let v = b[..n].iter().fold(0i64, |v, c| v * 10 + i64::from(c - b'0'));
    Some((v, n))
}

/// Days since 1970-01-01 of a date of the proleptic Gregorian calendar.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of day `z` since 1970-01-01: (year, month, day).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

/// The parts of `secs` (since 1970-01-01): date and time of day.
pub fn parts(secs: f64) -> ((i64, u32, u32), (u32, u32, u32)) {
    let day = (secs / DAY).floor();
    let rest = (secs - day * DAY).round() as i64;
    let (day, rest) = if rest >= 86_400 { (day as i64 + 1, rest - 86_400) } else { (day as i64, rest) };
    let (h, m, s) = (rest / 3600, rest / 60 % 60, rest % 60);
    (civil_from_days(day), (h as u32, m as u32, s as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_timestamps_and_times_read_as_their_wall_clock() {
        assert_eq!(parse("1970-01-01"), Some(Parsed { secs: 0.0, date: true, time: false }));
        assert_eq!(parse("1970-01-02").map(|p| p.secs), Some(DAY));
        assert_eq!(parse("2026-10-07 14:03:00").map(|p| p.secs), Some(1_791_381_780.0));
        // The offset is read and left out: the cell's own clock.
        assert_eq!(parse("2026-10-07 14:03:00+09").map(|p| p.secs), Some(1_791_381_780.0));
        assert_eq!(parse("2026-10-07T14:03:00Z").map(|p| p.secs), Some(1_791_381_780.0));
        assert_eq!(parse("2026-10-07 14:03:00.25-03:30").map(|p| p.secs), Some(1_791_381_780.25));
        assert_eq!(parse("14:03").map(|p| (p.secs, p.date, p.time)), Some((50_580.0, false, true)));
        assert_eq!(parse("14:03:07.5+0530").map(|p| p.secs), Some(50_587.5));
        assert_eq!(parse("12345-01-01").map(|p| p.date), Some(true));
        assert_eq!(parse("1969-12-31").map(|p| p.secs), Some(-DAY));
        // The end of a day; days a month does not have; a fraction of any length.
        assert_eq!(parse("2026-10-07 24:00:00").map(|p| p.secs), parse("2026-10-08").map(|p| p.secs));
        assert_eq!(parse(&format!("10:00:00.{}", "1".repeat(400))).map(|p| p.secs), Some(36_000.111_111_111));
        for bad in [
            "2026-02-31",
            "2025-02-29",
            "2026-04-31",
            "24:30",
            "24:00:01",
            "",
            "infinity",
            "-infinity",
            "2026-13-01",
            "2026-1-1",
            "0044-03-15 BC",
            "today",
            "12",
            "1:2",
            "2026-10-07x",
        ] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn civil_days_round_trip() {
        for z in [-800_000i64, -1, 0, 1, 59, 60, 11_016, 20_733, 2_000_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        assert_eq!(civil_from_days(20_733), (2026, 10, 7));
        assert_eq!(parts(1_791_381_780.0), ((2026, 10, 7), (14, 3, 0)));
        assert_eq!(parts(-1.0), ((1969, 12, 31), (23, 59, 59)));
    }
}
