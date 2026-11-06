use super::*;

/// The notes the app shows: What's new of the docs site.
const WHATS_NEW: &str = include_str!("../../../../../docs/site/src/whats-new.md");

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

fn text(s: &str) -> Span {
    Span::Text(s.to_string())
}

#[test]
fn the_notes_of_the_docs_site_read_whole() {
    let notes = parse(WHATS_NEW).unwrap_or_else(|e| panic!("whats-new.md line {}: {}", e.line, e.what));
    assert!(notes.sections.len() >= 5, "{}", notes.sections.len());
    // Every version heading is read: none is skipped as text.
    let headings = WHATS_NEW.lines().filter(|l| l.starts_with("## ")).count();
    assert_eq!(notes.sections.len(), headings);
    for s in &notes.sections {
        assert!(!s.blocks.is_empty(), "{} has no notes", s.version);
    }
}

#[test]
fn the_running_version_has_its_notes() {
    // The release pull request adds the version's section with the version, so every release
    // shows its own notes. A release candidate's notes come with its release.
    let running = Version::running();
    let notes = parse(WHATS_NEW).unwrap();
    if !running.is_rc() {
        assert!(notes.get(&running).is_some(), "whats-new.md has no section for {running}");
        assert_eq!(notes.sections[0].version, running, "the running version is the newest");
    } else {
        assert!(notes.sections[0].version < running);
    }
}

#[test]
fn a_version_reads_into_blocks() {
    let md = "# What's new\n\nThe introduction, [with a link](https://x.y).\n\n## 0.39.0 (2026-11-10)\n\nThe summary,\non two lines.\n\n### Added\n\n- An item with `code`, **bold**\n  and a second line; see <https://a.b/c>.\n- Another.\n\n### Fixed\n\n- A fix.\n\n## 0.38.3 (2026-11-06)\n\n### Changed\n\n- Old.\n";
    let notes = parse(md).unwrap();
    assert_eq!(notes.sections.len(), 2);
    let s = &notes.sections[0];
    assert_eq!((s.version, s.date.as_str()), (v("0.39.0"), "2026-11-10"));
    assert_eq!(
        s.blocks,
        vec![
            Block::Paragraph(vec![text("The summary, on two lines.")]),
            Block::Heading("Added".into()),
            Block::Item(vec![
                text("An item with "),
                Span::Code("code".into()),
                text(", "),
                Span::Bold("bold".into()),
                text(" and a second line; see https://a.b/c."),
            ]),
            Block::Item(vec![text("Another.")]),
            Block::Heading("Fixed".into()),
            Block::Item(vec![text("A fix.")]),
        ]
    );
    assert_eq!(notes.sections[1].blocks, vec![Block::Heading("Changed".into()), Block::Item(vec![text("Old.")])]);
    assert_eq!(plain(&[text("a "), Span::Code("b".into()), Span::Bold(" c".into())]), "a b c");
}

#[test]
fn text_inside_code_is_kept_as_it_is() {
    let notes = parse("## 1.0.0 (2026-01-01)\n\n- `*` and `_x_` and `<b>` and `\\N` and `[a](b)`.\n").unwrap();
    assert_eq!(
        notes.sections[0].blocks[0],
        Block::Item(vec![
            Span::Code("*".into()),
            text(" and "),
            Span::Code("_x_".into()),
            text(" and "),
            Span::Code("<b>".into()),
            text(" and "),
            Span::Code("\\N".into()),
            text(" and "),
            Span::Code("[a](b)".into()),
            text("."),
        ])
    );
    let ok = parse("## 1.0.0 (2026-01-01)\n\nsnake_case, a < b, 5 * 3 is not, `x`.\n");
    // A future note with "5 * 3" fails at its line, saying how to write it.
    let e = ok.unwrap_err();
    assert_eq!(e.line, 3);
    assert!(e.what.contains("write it in code (`5 * 3`)"), "{}", e.what);
    assert!(parse("## 1.0.0 (2026-01-01)\n\nsnake_case and a < b & c.\n").is_ok());
}

#[test]
fn anything_outside_the_subset_names_its_line() {
    let head = "# What's new\n\n## 1.0.0 (2026-01-01)\n\n";
    for (body, what) in [
        ("#### Deep\n", "a heading other than `## X.Y.Z (YYYY-MM-DD)` and `### <kind>`"),
        ("### Notes\n", "a `###` heading other than Added, Changed, Fixed, Removed or Security"),
        ("1. one\n", "a list other than `- ` items"),
        ("* star\n", "a list other than `- ` items"),
        ("+ plus\n", "a list other than `- ` items"),
        ("- a\n  - nested\n", "a nested list"),
        ("- a\n  1. nested\n", "a nested list"),
        ("- a\nlazy\n", "a list item's line that is not indented"),
        ("> quote\n", "a quote"),
        ("```sql\nselect\n```\n", "a code block"),
        ("    indented code\n", "an indented line"),
        ("| a | b |\n", "a table"),
        ("---\n", "a rule or an underlined heading"),
        ("Title\n===\n", "a rule or an underlined heading"),
        ("<details>open</details>\n", "HTML"),
        ("A <b>bold</b> word.\n", "HTML"),
        ("An ![image](x.png).\n", "an image"),
        ("Some *emphasis*.\n", "a `*` outside code (emphasis on the site): write it in code (`5 * 3`) or as ×"),
        ("Some _emphasis_.\n", "a `_` at a word's edge outside code (emphasis on the site): write it in code"),
        ("An \\* escape.\n", "a backslash escape"),
        ("An &amp; entity.\n", "an HTML entity"),
        ("A `code span.\n", "a backtick without its closing one"),
        ("A ``double`` span.\n", "code marked with more than one backtick"),
        ("A **bold.\n", "`**` without its closing one"),
        ("A **`code` in bold**.\n", "marks inside bold text"),
        ("A [bracket] alone.\n", "a `[` that is not a link"),
        ("A [`code`](x) link.\n", "marks inside a link's text"),
        ("A\ttab.\n", "a tab"),
    ] {
        let e = parse(&format!("{head}{body}")).expect_err(body);
        assert_eq!(
            (e.line, e.what),
            (
                5 + usize::from(what == "a nested list" || what.contains("not indented") || body.starts_with("Title")),
                what
            ),
            "{body:?}"
        );
    }
}

#[test]
fn version_headings_are_exact_and_newest_first() {
    for bad in [
        "## 0.39.0",
        "## v0.39.0 (2026-11-10)",
        "## 0.39.0 (2026-11-1)",
        "## 0.39.0-rc.1 (2026-11-10)",
        "## 0.39.0 (10 November)",
        "## 0.39.0  (2026-11-10)",
        "## Unreleased",
    ] {
        let e = parse(&format!("{bad}\n")).expect_err(bad);
        assert_eq!(e.what, "a version heading that is not `## X.Y.Z (YYYY-MM-DD)`", "{bad}");
    }
    let order = |a: &str, b: &str| parse(&format!("## {a} (2026-01-01)\n\n- x\n\n## {b} (2026-01-01)\n\n- y\n"));
    assert!(order("0.39.0", "0.38.3").is_ok());
    assert_eq!(
        order("0.38.3", "0.39.0").unwrap_err(),
        Unsupported { line: 5, what: "a version not older than the one before it" }
    );
    assert_eq!(order("0.38.3", "0.38.3").unwrap_err().line, 5);
}

#[test]
fn nothing_but_the_introduction_comes_before_the_first_version() {
    assert!(parse("# What's new\n\nIntro.\n").unwrap().sections.is_empty());
    assert_eq!(parse("Intro.\n\n- a list\n").unwrap_err().what, "a list before the first version");
    assert_eq!(
        parse("### Added\n").unwrap_err().what,
        "a `###` heading other than Added, Changed, Fixed, Removed or Security"
    );
    assert_eq!(parse("Intro.\n\n# Second title\n").unwrap_err().line, 3);
    assert_eq!(
        parse("Intro with `unclosed.\n").unwrap_err(),
        Unsupported { line: 1, what: "a backtick without its closing one" }
    );
}

#[test]
fn versions_between_two() {
    let md = "## 0.40.0 (2026-12-01)\n\n- d\n\n## 0.39.1 (2026-11-12)\n\n- c\n\n## 0.39.0 (2026-11-10)\n\n- b\n\n## 0.38.3 (2026-11-06)\n\n- a\n";
    let notes = parse(md).unwrap();
    let between =
        |a: &str, b: &str| notes.between(&v(a), &v(b)).iter().map(|s| s.version.to_string()).collect::<Vec<_>>();
    assert_eq!(between("0.38.3", "0.39.1"), ["0.39.1", "0.39.0"]);
    assert_eq!(between("0.38.0", "0.40.0"), ["0.40.0", "0.39.1", "0.39.0", "0.38.3"]);
    assert_eq!(between("0.39.1", "0.39.1"), Vec::<String>::new());
    // A release candidate of the next version has no notes of its own yet.
    assert_eq!(between("0.39.1", "0.40.0-rc.1"), Vec::<String>::new());
    assert_eq!(notes.get(&v("0.39.0")).map(|s| s.date.as_str()), Some("2026-11-10"));
    assert_eq!(notes.get(&v("0.1.0")), None);
}

#[test]
fn crlf_and_trailing_spaces_read_the_same() {
    let lf = parse("## 1.0.0 (2026-01-01)\n\n### Added\n\n- a\n  b\n").unwrap();
    assert_eq!(parse("## 1.0.0 (2026-01-01)  \r\n\r\n### Added\r\n\r\n- a \r\n  b\r\n").unwrap(), lf);
}

#[test]
fn no_text_makes_the_reader_panic() {
    // Every sequence of up to four of these pieces, as a line and as a version's item.
    let pieces = [
        "`", "**", "*", "_", "[", "](", ")", "<", ">", "<h", "\\", "\\*", "&", ";", "#", "- ", "  ", "\u{d55c}",
        "\u{2026}", "a", " ", "\n", "![",
    ];
    let mut seq = [0usize; 4];
    loop {
        let s: String = seq.iter().map(|i| pieces[*i]).collect();
        let _ = parse(&s);
        let _ = parse(&format!("## 1.0.0 (2026-01-01)\n\n- {s}\n"));
        let _ = crate::update::release::summary(&s);
        let mut i = 0;
        loop {
            seq[i] += 1;
            if seq[i] < pieces.len() {
                break;
            }
            seq[i] = 0;
            i += 1;
            if i == seq.len() {
                return;
            }
        }
    }
}
