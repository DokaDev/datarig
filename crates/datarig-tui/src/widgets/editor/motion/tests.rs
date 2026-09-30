use super::super::Editor;
use super::super::tests::{Case, at, check, typ};
use super::*;

#[test]
fn word_motions() {
    let mut e = Editor::new("SELECT u.name, x\n\nFROM t");
    typ(&mut e, "w");
    assert_eq!((e.row, e.col), (0, 7));
    typ(&mut e, "w");
    assert_eq!((e.row, e.col), (0, 8));
    typ(&mut e, "ww");
    assert_eq!((e.row, e.col), (0, 13));
    typ(&mut e, "ww");
    assert_eq!((e.row, e.col), (1, 0));
    typ(&mut e, "w");
    assert_eq!((e.row, e.col), (2, 0));
    typ(&mut e, "b");
    assert_eq!((e.row, e.col), (1, 0));
    typ(&mut e, "b");
    assert_eq!((e.row, e.col), (0, 15));
    typ(&mut e, "0e");
    assert_eq!((e.row, e.col), (0, 5));
    typ(&mut e, "G");
    assert_eq!(e.row, 2);
    typ(&mut e, "gg");
    assert_eq!(e.row, 0);
}

/// Motions with counts, at the ends of lines and of the text, as Vim moves.
#[test]
fn motions_with_counts_move_as_vim() {
    const CASES: &[Case] = &[
        (
            "one two\n  three four\nfive six\nseven",
            (0, 4),
            "5j",
            "one two\n  three four\nfive six\nseven",
            (3, 4),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (3, 2),
            "2k",
            "one two\n  three four\nfive six\nseven",
            (1, 2),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "3w",
            "one two\n  three four\nfive six\nseven",
            (1, 8),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "3e",
            "one two\n  three four\nfive six\nseven",
            (1, 6),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (3, 0),
            "4b",
            "one two\n  three four\nfive six\nseven",
            (1, 2),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "3G",
            "one two\n  three four\nfive six\nseven",
            (2, 0),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "10G",
            "one two\n  three four\nfive six\nseven",
            (3, 0),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (3, 0),
            "2gg",
            "one two\n  three four\nfive six\nseven",
            (1, 2),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "2$",
            "one two\n  three four\nfive six\nseven",
            (2, 7),
            None,
        ),
        ("one two\n  three four\nfive six\nseven", (3, 4), "w", "one two\n  three four\nfive six\nseven", (3, 4), None),
        ("one two\n  three four\nfive six\nseven", (3, 4), "e", "one two\n  three four\nfive six\nseven", (3, 4), None),
        ("one two\n  three four\nfive six\nseven", (0, 0), "b", "one two\n  three four\nfive six\nseven", (0, 0), None),
        ("a\n\nb", (0, 0), "w", "a\n\nb", (1, 0), None),
        ("a\n\nb", (2, 0), "b", "a\n\nb", (1, 0), None),
        ("a\n\nb", (0, 0), "e", "a\n\nb", (2, 0), None),
        ("foo(bar, baz)", (0, 0), "3w", "foo(bar, baz)", (0, 7), None),
        ("foo(bar, baz)", (0, 12), "2b", "foo(bar, baz)", (0, 7), None),
        ("foo(bar, baz)", (0, 0), "3e", "foo(bar, baz)", (0, 6), None),
        ("one two\n  three four", (0, 5), "2l", "one two\n  three four", (0, 6), None),
        ("one two\n  three four", (0, 5), "9h", "one two\n  three four", (0, 0), None),
        ("one two\n  three four", (1, 9), "0", "one two\n  three four", (1, 0), None),
        ("one two\n  three four", (1, 9), "^", "one two\n  three four", (1, 2), None),
        ("one two\n  three four", (1, 9), "3<Left><Down>", "one two\n  three four", (1, 6), None),
        ("one two\n  three four", (1, 9), "<Home><Up><End>", "one two\n  three four", (0, 6), None),
    ];
    check(CASES);
}

/// `$` keeps the cursor at the ends of lines going up and down; `j`/`k` keep the column.
#[test]
fn vertical_moves_keep_the_wanted_column() {
    let mut e = at("long line here\nab\nanother long one", (0, 9));
    typ(&mut e, "j");
    assert_eq!((e.row, e.col), (1, 1));
    typ(&mut e, "j");
    assert_eq!((e.row, e.col), (2, 9));
    typ(&mut e, "$2k");
    assert_eq!((e.row, e.col), (0, 13));
}

#[test]
fn operator_ranges_have_their_kind() {
    let e = at("one two\n  three four\nfive", (1, 4));
    let r = |m, n| e.op_range(m, n, false).map(|r| (r.start, r.end, r.kind));
    use RangeKind::*;
    assert_eq!(r(Motion::WordForward, 1), Some(((1, 4), (1, 8), Exclusive)));
    assert_eq!(r(Motion::WordEnd, 1), Some(((1, 4), (1, 6), Inclusive)));
    assert_eq!(r(Motion::LineEnd, 1), Some(((1, 4), (1, 11), Inclusive)));
    assert_eq!(r(Motion::WordBack, 1), Some(((1, 2), (1, 4), Exclusive)));
    assert_eq!(r(Motion::Down, 1), Some(((1, 4), (2, 4), Linewise)));
    assert_eq!(r(Motion::Right, 99), Some(((1, 4), (1, 12), Exclusive)), "to the end of the line");
    // `w` over the last word of a line ends there; ending at the start of a line from the
    // indent takes the lines whole.
    assert_eq!(r(Motion::WordForward, 2), Some(((1, 4), (1, 12), Exclusive)));
    let e = at("a\n\nb", (1, 0));
    let w = e.op_range(Motion::WordForward, 1, false).map(|r| (r.start, r.end, r.kind));
    assert_eq!(w, Some(((1, 0), (1, 0), Linewise)));
    let e = at("a", (0, 0));
    assert_eq!(e.op_range(Motion::Left, 1, false), None, "h in the first column");
    assert_eq!(e.op_range(Motion::Down, 1, false), None, "j on the last line");
}
