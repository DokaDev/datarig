use super::*;

fn style(date: &str, interval: &str, zone: &str) -> TextStyle {
    TextStyle { date_style: date.into(), interval_style: interval.into(), time_zone: zone.into() }
}

#[test]
fn the_same_settings_read_back_the_same() {
    let sql = style("SQL, DMY", "postgres", "Asia/Seoul");
    assert!(reads_same(Some(&sql), Some(&sql.clone())));
    let iso = style("ISO, MDY", "sql_standard", "UTC");
    assert!(reads_same(Some(&iso), Some(&iso.clone())));
}

#[test]
fn another_date_style_does_not_read_text_written_in_one_not_iso() {
    let from = style("SQL, DMY", "postgres", "UTC");
    for to in ["SQL, MDY", "ISO, MDY", "ISO, DMY", "Postgres, DMY", "German, DMY"] {
        assert!(!reads_same(Some(&from), Some(&style(to, "postgres", "UTC"))), "{to}");
    }
    // Zone abbreviations of a style that is not ISO read in the session's own zone.
    assert!(!reads_same(Some(&from), Some(&style("SQL, DMY", "postgres", "Asia/Seoul"))));
    // An interval style that does not sign every field reads only in itself.
    let standard = style("ISO, MDY", "sql_standard", "UTC");
    assert!(!reads_same(Some(&standard), Some(&style("ISO, MDY", "postgres", "UTC"))));
}

#[test]
fn iso_text_reads_back_anywhere_even_where_the_settings_are_not_known() {
    for (date, interval) in [("ISO, MDY", "postgres"), ("ISO, DMY", "iso_8601"), ("iso", "postgres_verbose")] {
        let from = style(date, interval, "Asia/Seoul");
        assert!(from.reads_anywhere(), "{date} {interval}");
        assert!(reads_same(Some(&from), None), "{date} {interval}");
        assert!(reads_same(Some(&from), Some(&style("SQL, DMY", "sql_standard", "UTC"))), "{date} {interval}");
    }
}

#[test]
fn text_of_settings_not_known_reads_nowhere() {
    let iso = style("ISO, MDY", "postgres", "UTC");
    assert!(!reads_same(None, Some(&iso)));
    assert!(!reads_same(None, None));
    // Not known now, written in a style that is not ISO: refused.
    assert!(!reads_same(Some(&style("SQL, DMY", "postgres", "UTC")), None));
    // A date style only like ISO is not it.
    assert!(!style("ISOish, MDY", "postgres", "UTC").reads_anywhere());
    assert!(!style("", "postgres", "UTC").reads_anywhere());
}
