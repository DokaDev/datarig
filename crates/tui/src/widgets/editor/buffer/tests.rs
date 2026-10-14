use super::super::tests::{ctrl, typ};
use super::*;

#[test]
fn undo_and_redo_walk_every_step_back_and_forth() {
    let mut e = Editor::new("alpha beta\ngamma\ndelta;\nepsilon");
    let mut seen = vec![e.text()];
    for keys in [
        "x",
        "dd",
        "oNEW line\x1b",
        "yyP",
        "jjp",
        "ia\nb\x08\x08c\x1b",
        "vjd",
        "A!\x1b",
        "Otop\x1b",
        "3dw",
        "Vjd",
        "cwchanged<Esc>",
        "2x",
        "D",
        "ggVGc<Esc>",
    ] {
        typ(&mut e, keys);
        if e.text() != *seen.last().unwrap() {
            seen.push(e.text());
        }
    }
    for want in seen.iter().rev().skip(1) {
        typ(&mut e, "u");
        assert_eq!(&e.text(), want);
    }
    typ(&mut e, "u");
    assert_eq!(e.text(), seen[0], "nothing more to undo");
    for want in &seen[1..] {
        e.handle_key(ctrl('r'));
        assert_eq!(&e.text(), want);
    }
    assert_eq!(e.len_bytes(), e.text().len());
}

/// One command is one undo step: an operator with a count, a change and what was typed after
/// it, a put of several copies; a count on `u` undoes that many.
#[test]
fn one_command_is_one_undo_step() {
    let text = "a b c d e\nf g\nh i\nj";
    for keys in ["3dw", "d2w", "2d3w", "3dd", "d2j", "3x", "c2wX Y<Esc>", "2ccnew<Esc>", "3p", "Vjd", "vjc<Esc>"] {
        let mut e = Editor::new(text);
        typ(&mut e, "yl");
        typ(&mut e, keys);
        typ(&mut e, "u");
        assert_eq!(e.text(), text, "{keys}");
    }
    let mut e = Editor::new(text);
    typ(&mut e, "xxx3u");
    assert_eq!(e.text(), text);
    typ(&mut e, "2<C-r>");
    assert_eq!(e.text(), "b c d e\nf g\nh i\nj");
}

#[test]
fn lines_span_takes_a_line_break_with_the_lines() {
    let e = Editor::new("a\nbb\nc");
    assert_eq!(e.lines_span(0, 0), (0, 2), "the line break after");
    assert_eq!(e.lines_span(1, 2), (1, 6), "the last lines: the line break before");
    assert_eq!(e.lines_span(0, 2), (0, 6), "everything");
}

#[test]
fn edits_in_a_large_text_touch_only_their_lines() {
    let text: String = (0..50_000).map(|i| format!("SELECT {i} FROM t WHERE x = '{i};';\n")).collect();
    let mut e = Editor::new(&text);
    e.row = 25_000;
    let v = e.version();
    typ(&mut e, "A -- note\x1b");
    assert_ne!(e.version(), v);
    assert!(e.lines[25_000].ends_with(" -- note"));
    typ(&mut e, "ddu");
    assert_eq!(e.lines.len(), 50_001);
    assert_eq!(e.len_bytes(), e.text().len());
    let (start, _, body) = e.current_statement().unwrap();
    assert!(body.starts_with("SELECT 25000 FROM t"), "{body}");
    assert_eq!(start, e.line_start(25_000));
    // A linewise operator over many lines is one splice.
    typ(&mut e, "d10000j");
    assert_eq!(e.lines.len(), 40_000);
    typ(&mut e, "u");
    assert_eq!(e.lines.len(), 50_001);
}
