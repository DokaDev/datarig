use super::*;
use datarig_core::update::notes::parse;

fn text(rows: &[Row]) -> Vec<String> {
    rows.iter().map(|r| r.iter().map(|(t, _)| t.as_str()).collect()).collect()
}

#[test]
fn the_notes_this_build_carries_render_at_every_width() {
    let notes = notes::parse(NOTES).unwrap_or_else(|e| panic!("whats-new.md line {}: {}", e.line, e.what));
    assert!(!notes.sections.is_empty());
    for w in [8, 20, 40, 76, 96] {
        let (rows, heads) = layout(&notes.sections, w);
        assert_eq!(heads.len(), notes.sections.len());
        for (i, r) in rows.iter().enumerate() {
            let line: String = r.iter().map(|(t, _)| t.as_str()).collect();
            assert!(width(&line) <= w, "width {w}, row {i}: {line:?}");
            assert!(!line.ends_with(' '), "width {w}, row {i}: {line:?}");
        }
        for (h, s) in heads.iter().zip(&notes.sections) {
            assert_eq!(rows[*h][0], (s.version.to_string(), Ink::Version));
        }
    }
}

#[test]
fn a_version_lays_out_as_heading_paragraph_kinds_and_items() {
    let md = "## 1.0.0 (2026-01-01)\n\nThe summary of `this` release.\n\n### Added\n\n- One item that wraps over a second row here.\n- Two.\n\n### Fixed\n\n- **Fixed** it.\n";
    let s = parse(md).unwrap().sections;
    let (rows, heads) = layout(&s, 24);
    assert_eq!(heads, vec![0]);
    assert_eq!(
        text(&rows),
        [
            "1.0.0  2026-01-01",
            "",
            "The summary of this",
            "release.",
            "",
            "Added",
            "",
            "- One item that wraps",
            "  over a second row",
            "  here.",
            "- Two.",
            "",
            "Fixed",
            "",
            "- Fixed it.",
        ]
    );
    assert_eq!(rows[2], vec![("The summary of ".into(), Ink::Text), ("this".into(), Ink::Code)]);
    assert_eq!(rows[14], vec![("- ".into(), Ink::Text), ("Fixed".into(), Ink::Bold), (" it.".into(), Ink::Text)]);
}

#[test]
fn versions_are_apart_and_a_long_word_is_cut() {
    let md =
        "## 2.0.0 (2026-02-01)\n\n- `averyveryverylongidentifier_without_spaces`\n\n## 1.0.0 (2026-01-01)\n\n- x\n";
    let (rows, heads) = layout(&parse(md).unwrap().sections, 16);
    let t = text(&rows);
    assert_eq!(heads, vec![0, 6]);
    assert_eq!(t[2], "- averyveryveryl");
    assert!(t[3].starts_with("  "), "{t:?}");
    assert_eq!(t[5], "");
    // 17 columns: the date does not fit in 16.
    assert_eq!(t[6], "1.0.0");
}
