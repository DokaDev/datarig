use super::*;

fn style(date: &str, interval: &str, zone: &str) -> TextStyle {
    TextStyle { date_style: date.into(), interval_style: interval.into(), time_zone: zone.into() }
}

#[test]
fn the_same_settings_read_back_the_same() {
    let sql = style("SQL, DMY", "postgres", "Asia/Seoul");
    assert!(reads_same(Some(&sql), Some(&sql.clone()), false));
    let iso = style("ISO, MDY", "sql_standard", "UTC");
    assert!(reads_same(Some(&iso), Some(&iso.clone()), false));
}

#[test]
fn another_date_style_does_not_read_text_written_in_one_not_iso() {
    let from = style("SQL, DMY", "postgres", "UTC");
    for to in ["SQL, MDY", "ISO, MDY", "ISO, DMY", "Postgres, DMY", "German, DMY"] {
        assert!(!reads_same(Some(&from), Some(&style(to, "postgres", "UTC")), false), "{to}");
    }
    // Zone abbreviations of a style that is not ISO read in the session's own zone.
    assert!(!reads_same(Some(&from), Some(&style("SQL, DMY", "postgres", "Asia/Seoul")), false));
    // An interval style that does not sign every field reads only in itself.
    let standard = style("ISO, MDY", "sql_standard", "UTC");
    assert!(!reads_same(Some(&standard), Some(&style("ISO, MDY", "postgres", "UTC")), false));
}

#[test]
fn iso_text_reads_back_anywhere_even_where_the_settings_are_not_known() {
    for (date, interval) in [("ISO, MDY", "postgres"), ("ISO, DMY", "iso_8601"), ("iso", "postgres_verbose")] {
        let from = style(date, interval, "Asia/Seoul");
        assert!(from.reads_anywhere(), "{date} {interval}");
        assert!(reads_same(Some(&from), None, false), "{date} {interval}");
        assert!(reads_same(Some(&from), Some(&style("SQL, DMY", "sql_standard", "UTC")), false), "{date} {interval}");
    }
}

#[test]
fn text_of_settings_not_known_reads_nowhere() {
    let iso = style("ISO, MDY", "postgres", "UTC");
    assert!(!reads_same(None, Some(&iso), false));
    assert!(!reads_same(None, None, false));
    // Not known now, written in a style that is not ISO: refused.
    assert!(!reads_same(Some(&style("SQL, DMY", "postgres", "UTC")), None, false));
    // A date style only like ISO is not it.
    assert!(!style("ISOish, MDY", "postgres", "UTC").reads_anywhere());
    assert!(!style("", "postgres", "UTC").reads_anywhere());
}

/// Outside ISO a time with a zone is written with the zone's abbreviation, which reads in the
/// reading session's `timezone_abbreviations` (never reported): such text never reads the same
/// by the settings alone. In ISO its offset is a number, the same instant anywhere.
#[test]
fn a_time_with_a_zone_written_outside_iso_never_reads_the_same_by_its_settings() {
    let sql = style("SQL, DMY", "postgres", "America/Chicago");
    assert!(!reads_same(Some(&sql), Some(&sql.clone()), true));
    assert!(reads_same(Some(&sql), Some(&sql.clone()), false), "dates and times without a zone do");
    let iso = style("ISO, MDY", "postgres", "America/Chicago");
    assert!(reads_same(Some(&iso), Some(&style("ISO, DMY", "postgres", "Asia/Seoul")), true));
    assert!(reads_same(Some(&iso), None, true));
}

/// Text reads the same whatever the session's settings only when every date and time in it is
/// in ISO 8601 form, every time with a zone has a numeric offset, and every interval is in ISO
/// 8601 form; any other type is not known to.
#[test]
fn only_iso_dates_offsets_and_iso_intervals_read_the_same_anywhere() {
    use Dated::{Of, Unknown as U};
    const D: Dated = Of(DatePart::Date);
    const TS: Dated = Of(DatePart::Timestamp);
    const TZ: Dated = Of(DatePart::TimestampTz);
    const TTZ: Dated = Of(DatePart::TimeTz);
    const IV: Dated = Of(DatePart::Interval);
    assert!(reads_same_anywhere(Dated::No, "02/01/2026"), "no dates in it");
    for (ty, text) in [
        (D, "[2026-01-02,2026-01-05)"),
        (D, "empty"),
        (D, "[2026-01-02,)"),
        (D, "{[2026-01-02,2026-01-05),[2026-02-01,2026-02-03)}"),
        (TS, "[\"2026-01-02 10:00:00\",\"2026-01-02 11:00:00.5\")"),
        (TZ, "[\"2026-01-02 10:00:00+09\",\"2026-01-02 11:00:00-03:30\")"),
        (TZ, "[\"2026-01-02 10:00:00Z\",infinity)"),
        (D, "{\"[2026-01-02,2026-01-05)\",NULL}"),
        (D, "0044-03-15 BC"),
        (TTZ, "10:00:00+09"),
        (IV, "P1Y2M3DT4H5M6.5S"),
        (IV, "P-1D"),
    ] {
        assert!(reads_same_anywhere(ty, text), "{ty:?} {text}");
    }
    for (ty, text) in [
        (D, "[02/01/2026,05/01/2026)"),
        (D, "[02.01.2026,05.01.2026)"),
        (TZ, "[\"2026-01-02 10:00:00\",)"),
        (TZ, "[\"02/01/2026 10:00:00 CST\",)"),
        (TS, "[\"Fri Jan 02 10:00:00 2026\",)"),
        (TTZ, "10:00:00"),
        (IV, "1 day 02:00:00"),
        (IV, "@ 1 day ago"),
        (IV, "-1 2:00:00"),
        (U, "happy"),
        (U, "(guest,2026-01-02)"),
        (U, "(1,2026-01-02)"),
        (D, ""),
    ] {
        assert!(!reads_same_anywhere(ty, text), "{ty:?} {text}");
    }
}

/// An interval the driver wrote from its binary value reads back exactly as ISO 8601, signs per
/// field as PostgreSQL's `iso_8601` style writes them.
#[test]
fn an_interval_read_in_binary_is_written_again_in_iso_8601() {
    for (shown, iso) in [
        ("1 year 2 mons 3 days 04:05:06.789", "P1Y2M3DT4H5M6.789S"),
        ("-1 years -2 mons +3 days -04:05:06", "P-1Y-2M3DT-4H-5M-6S"),
        ("1 day -00:00:00.5", "P1DT-0.5S"),
        ("00:00:00", "PT0S"),
        ("25:00:00", "PT25H"),
        ("-3 days", "P-3D"),
        ("1 mon", "P1M"),
        ("infinity", "infinity"),
    ] {
        assert_eq!(interval_as_iso(shown).as_deref(), Some(iso), "{shown}");
        assert!(reads_same_anywhere(Dated::Of(DatePart::Interval), iso), "{iso}");
    }
    for text in ["P1D", "1 day 2 hours", "@ 1 day", "-1 2:00:00", "1 fortnight", ""] {
        assert_eq!(interval_as_iso(text), None, "{text}");
    }
}
