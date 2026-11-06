use super::*;

/// A real answer of `/releases/latest` (v0.38.3), trimmed to a few fields and one asset.
const LATEST: &str = include_str!("../release-latest.json");

fn answer(tag: &str, extra: &str) -> Vec<u8> {
    format!("{{\"tag_name\":\"{tag}\"{extra}}}").into_bytes()
}

#[test]
fn a_real_answer_reads() {
    let r = Release::from_json(LATEST.as_bytes()).unwrap();
    assert_eq!(r.version, Version::parse("0.38.3").unwrap());
    assert_eq!(r.date.as_deref(), Some("2026-11-06"));
    assert_eq!(
        r.summary.as_deref(),
        Some(
            "Screen text uses British spelling, and titles use a colon in place of a dash; the docs site shows new recordings of the editor, charts and plans."
        )
    );
}

#[test]
fn what_is_not_a_stable_release_is_a_bad_answer() {
    for bad in [
        answer("v0.39.0-rc.1", ""),
        answer("v0.39", ""),
        answer("latest", ""),
        answer("v0.39.0", ",\"draft\":true"),
        answer("v0.39.0", ",\"prerelease\":true"),
        b"{}".to_vec(),
        b"{\"tag_name\":39}".to_vec(),
        b"[]".to_vec(),
        b"<html>Rate limited</html>".to_vec(),
        b"".to_vec(),
        b"{\"tag_name\":\"v0.39.0\"".to_vec(),
    ] {
        assert_eq!(Release::from_json(&bad), Err(BadAnswer), "{}", String::from_utf8_lossy(&bad));
    }
    let r = Release::from_json(&answer("0.39.0", ",\"published_at\":null,\"body\":null")).unwrap();
    assert_eq!((r.date, r.summary), (None, None));
    let r = Release::from_json(&answer("v0.39.0", ",\"published_at\":\"soon\"")).unwrap();
    assert_eq!(r.date, None);
}

#[test]
fn an_answer_over_the_cap_is_bad_even_when_it_reads() {
    let pad = format!(",\"body\":\"{}\"", "a".repeat(MAX_ANSWER));
    assert_eq!(Release::from_json(&answer("v0.39.0", &pad)), Err(BadAnswer));
    let fits = format!(",\"body\":\"{}\"", "a".repeat(MAX_ANSWER - 64));
    assert!(Release::from_json(&answer("v0.39.0", &fits)).is_ok());
}

#[test]
fn the_summary_is_the_first_paragraph_as_one_plain_line() {
    assert_eq!(summary("One line.\n\n### Added\n\n- x").as_deref(), Some("One line."));
    assert_eq!(
        summary("This patch adds `:whatsnew` and **bold** text,\r\nsee [the guide](https://x.y/z).\r\n\r\nNext.")
            .as_deref(),
        Some("This patch adds :whatsnew and bold text, see the guide.")
    );
    assert_eq!(summary("\n\n  Leading blank lines.\n").as_deref(), Some("Leading blank lines."));
    assert_eq!(summary("Bell\u{7} and\u{1b}[31m escape\u{0} gone.").as_deref(), Some("Bell and [31m escape gone."));
    // Notes that open with something else have none.
    for none in ["### Changed\n\n- x", "- a list", "* a list", "> quote", "| table |", "", "   \n\n", "# Title"] {
        assert_eq!(summary(none), None, "{none:?}");
    }
    // Marks outside what the notes use stay as they are.
    assert_eq!(summary("A *star* stays.").as_deref(), Some("A *star* stays."));
}

#[test]
fn a_long_summary_is_cut() {
    let s = summary(&"word ".repeat(200)).unwrap();
    assert_eq!(s.chars().count(), MAX_SUMMARY);
    assert!(s.ends_with("word\u{2026}"), "{s}");
    let wide = summary(&"\u{d55c}".repeat(400)).unwrap();
    assert_eq!(wide.chars().count(), MAX_SUMMARY);
    let exact = "a".repeat(MAX_SUMMARY);
    assert_eq!(summary(&exact).as_deref(), Some(exact.as_str()));
}
