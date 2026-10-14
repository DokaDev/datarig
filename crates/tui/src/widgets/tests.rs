use super::*;
use crate::text::width;

#[test]
fn a_status_keeps_its_place_and_the_title_gives_way() {
    let (full, short) = ("(paging · 23s)", "(23s)");
    // Room for both.
    assert_eq!(
        title_and_status("Results · 500 rows", &[full, short], 60),
        ("Results · 500 rows".into(), Some(full.into()))
    );
    // The title is cut first, whole characters only.
    let (t, s) = title_and_status("結果 · 1,234,567行", &["(分頁中 · 23s)", "(23s)"], 30);
    assert_eq!(s.as_deref(), Some("(分頁中 · 23s)"));
    assert!(t.ends_with('…'), "{t}");
    assert!(width(&t) + 2 + 1 + width("(分頁中 · 23s)") + 2 <= 28, "{t}");
    // Too narrow for the full state: the seconds only; then nothing.
    let (t, s) = title_and_status("Results · 500 rows", &[full, short], 22);
    assert_eq!(s.as_deref(), Some(short));
    assert!(width(&t) >= 8, "{t}");
    assert_eq!(title_and_status("Results · 500 rows", &[full, short], 16).1, None);
}

#[test]
fn a_clipped_title_never_splits_a_wide_character() {
    for w in 14..40 {
        let (t, s) = title_and_status("結果 · 500行", &["(分頁中 · 23s)", "(23s)"], w);
        let used = width(&t) + 2 + s.as_ref().map_or(0, |s| width(s) + 3);
        assert!(used <= usize::from(w) - 2, "{w}: {t} {s:?}");
        assert!(t.chars().all(|c| c != '\u{FFFD}'));
    }
}
