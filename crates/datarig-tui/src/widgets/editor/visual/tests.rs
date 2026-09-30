use super::super::tests::{Case, at, check, typ};
use super::*;

/// Visual mode by character and by line: `y d x c` and `Y D X C S s`, `o`, switching between
/// `v` and `V`, motions with counts, as Vim does them.
#[test]
fn visual_operators() {
    const CASES: &[Case] = &[
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "Vy",
            "one two\n  three four\nfive six\nseven",
            (1, 0),
            Some(("  three four", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "Vjy",
            "one two\n  three four\nfive six\nseven",
            (1, 0),
            Some(("  three four\nfive six", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "Vky",
            "one two\n  three four\nfive six\nseven",
            (0, 4),
            Some(("one two\n  three four", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "Vjd",
            "one two\nseven",
            (1, 0),
            Some(("  three four\nfive six", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "Vjx",
            "one two\nseven",
            (1, 0),
            Some(("  three four\nfive six", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "Vjc_<Esc>",
            "one two\n  _\nseven",
            (1, 2),
            Some(("  three four\nfive six", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "VGd",
            "one two",
            (0, 0),
            Some(("  three four\nfive six\nseven", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "Vggd",
            "five six\nseven",
            (0, 0),
            Some(("one two\n  three four", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "V2jd",
            "one two",
            (0, 0),
            Some(("  three four\nfive six\nseven", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "Vjoky",
            "one two\n  three four\nfive six\nseven",
            (0, 4),
            Some(("one two\n  three four\nfive six", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "Vjokd",
            "seven",
            (0, 0),
            Some(("one two\n  three four\nfive six", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "vey",
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            Some(("ree", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "vjd",
            "one two\n  thsix\nseven",
            (1, 4),
            Some(("ree four\nfive ", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "vjx",
            "one two\n  thsix\nseven",
            (1, 4),
            Some(("ree four\nfive ", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "vec_<Esc>",
            "one two\n  th_ four\nfive six\nseven",
            (1, 4),
            Some(("ree", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "vjVd",
            "one two\nseven",
            (1, 0),
            Some(("  three four\nfive six", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "Vjvd",
            "one two\n  thsix\nseven",
            (1, 4),
            Some(("ree four\nfive ", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "vlohd",
            "one two\n  te four\nfive six\nseven",
            (1, 3),
            Some(("hre", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "vjY",
            "one two\n  three four\nfive six\nseven",
            (1, 0),
            Some(("  three four\nfive six", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "vlD",
            "one two\nfive six\nseven",
            (1, 0),
            Some(("  three four", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "vlX",
            "one two\nfive six\nseven",
            (1, 0),
            Some(("  three four", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "vlC_<Esc>",
            "one two\n  _\nfive six\nseven",
            (1, 2),
            Some(("  three four", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "vlS_<Esc>",
            "one two\n  _\nfive six\nseven",
            (1, 2),
            Some(("  three four", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "vls_<Esc>",
            "one two\n  th_e four\nfive six\nseven",
            (1, 4),
            Some(("re", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "vj$d",
            "one two\n  thseven",
            (1, 4),
            Some(("ree four\nfive six\n", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (3, 1),
            "Vkd",
            "one two\n  three four",
            (1, 2),
            Some(("five six\nseven", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "VV",
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "vv",
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "V3jy",
            "one two\n  three four\nfive six\nseven",
            (1, 0),
            Some(("  three four\nfive six\nseven", true)),
        ),
        ("a\n\nb", (1, 0), "vd", "a\nb", (1, 0), Some(("\n", false))),
        ("a\n\nb", (0, 0), "vjd", "b", (0, 0), Some(("a\n\n", false))),
        ("a\n\nb", (1, 0), "Vd", "a\nb", (1, 0), Some(("", true))),
    ];
    check(CASES);
}

/// `V` selects whole lines: Ctrl+E runs them (the selection), they are drawn selected to the
/// line break, and `V` with `j` then `d` deletes them. It used to do nothing.
#[test]
fn v_selects_lines() {
    let mut e = at("SELECT 1;\nSELECT 2;\nSELECT 3;", (0, 3));
    typ(&mut e, "V");
    assert_eq!((e.mode, e.visual_lines()), (Mode::Visual, true));
    assert_eq!(e.selection().as_deref(), Some("SELECT 1;"));
    typ(&mut e, "j");
    assert_eq!(e.selection().as_deref(), Some("SELECT 1;\nSELECT 2;"));
    assert_eq!(e.visual_bounds(), (0, 20), "both lines and the line break after them");
    typ(&mut e, "o");
    assert_eq!((e.row, e.col, e.anchor), (0, 3, (1, 3)), "o swaps the ends");
    typ(&mut e, "d");
    assert_eq!((e.text().as_str(), e.mode), ("SELECT 3;", Mode::Normal));
    typ(&mut e, "u");
    assert_eq!(e.text(), "SELECT 1;\nSELECT 2;\nSELECT 3;");
    e.exit_visual();
    assert_eq!(e.selection(), None);
}

#[test]
fn visual_selection_by_character() {
    let mut e = Editor::new("SELECT 1;\nSELECT 2;");
    typ(&mut e, "vj$");
    assert_eq!(e.selection().unwrap(), "SELECT 1;\nSELECT 2;");
    typ(&mut e, "\x1bgg0vey");
    assert_eq!(e.mode, Mode::Normal);
    typ(&mut e, "$p");
    assert_eq!(e.lines[0], "SELECT 1;SELECT");
    typ(&mut e, "0vld");
    assert_eq!(e.lines[0], "LECT 1;SELECT");
    // `$` selects the line break of the cursor's line.
    let mut e = at("ab\ncd", (0, 1));
    typ(&mut e, "v$");
    assert_eq!(e.selection().as_deref(), Some("b\n"));
    typ(&mut e, "h");
    assert_eq!(e.selection().as_deref(), Some("ab"), "another motion leaves the line end");
}

/// A click leaves Visual mode; a drag or a double click selects by character, also after `V`.
#[test]
fn the_mouse_selects_by_character() {
    let mut e = at("one two\nthree", (0, 0));
    typ(&mut e, "Vj");
    e.select_word((0, 5));
    assert_eq!((e.visual_lines(), e.selection().as_deref()), (false, Some("two")));
    typ(&mut e, "V");
    e.drag_select((0, 1), (1, 1));
    assert_eq!(e.selection().as_deref(), Some("ne two\nth"));
    e.click(0, 0);
    assert_eq!(e.mode, Mode::Normal);
}
