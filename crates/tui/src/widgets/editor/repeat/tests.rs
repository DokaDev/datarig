use super::super::tests::{Case, at, check, typ};

/// `.` repeats the last change with its count or a new one, its Insert session included, and
/// a Visual mode operator on as much text from the cursor.
#[test]
fn dot_repeats_the_last_change() {
    const CASES: &[Case] = &[
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "dw.",
            "\n  three four\nfive six\nseven",
            (0, 0),
            Some(("two", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "dw..",
            "  three four\nfive six\nseven",
            (0, 2),
            Some(("", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "x.",
            "e two\n  three four\nfive six\nseven",
            (0, 0),
            Some(("n", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "3x.",
            "o\n  three four\nfive six\nseven",
            (0, 0),
            Some((" tw", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "x3.",
            "two\n  three four\nfive six\nseven",
            (0, 0),
            Some(("ne ", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "3x2.",
            "wo\n  three four\nfive six\nseven",
            (0, 0),
            Some((" t", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "dd.",
            "five six\nseven",
            (0, 0),
            Some(("  three four", true)),
        ),
        ("one two\n  three four\nfive six\nseven", (0, 0), "2dd.", "", (0, 0), Some(("five six\nseven", true))),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "cwX<Esc>w.",
            "X X\n  three four\nfive six\nseven",
            (0, 2),
            Some(("two", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "ciwZ<Esc>j.",
            "Z two\nZthree four\nfive six\nseven",
            (1, 0),
            Some(("  ", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "A;<Esc>j.",
            "one two;\n  three four;\nfive six\nseven",
            (1, 12),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "ix<Esc>.",
            "xxone two\n  three four\nfive six\nseven",
            (0, 0),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "ix<Esc>3.",
            "xxxxone two\n  three four\nfive six\nseven",
            (0, 2),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "oline<Esc>.",
            "one two\nline\nline\n  three four\nfive six\nseven",
            (2, 3),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "Oab<Esc>2.",
            "ab\nab\nab\none two\n  three four\nfive six\nseven",
            (1, 1),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "yyp.",
            "one two\none two\none two\n  three four\nfive six\nseven",
            (2, 0),
            Some(("one two", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "p.",
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "D.",
            "\n  three four\nfive six\nseven",
            (0, 0),
            Some(("one two", false)),
        ),
        ("one two\n  three four\nfive six\nseven", (0, 0), "J.", "one two three four five six\nseven", (0, 18), None),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            ">>j.",
            "    one two\n      three four\nfive six\nseven",
            (1, 6),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "r-l.",
            "--e two\n  three four\nfive six\nseven",
            (0, 1),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "guiwj.",
            "one two\n  three four\nfive six\nseven",
            (1, 0),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "daw.",
            "\n  three four\nfive six\nseven",
            (0, 0),
            Some(("two", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "dfe.",
            " two\n  three four\nfive six\nseven",
            (0, 0),
            Some(("one", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "dt .",
            " two\n  three four\nfive six\nseven",
            (0, 0),
            Some(("one", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "xu.",
            "ne two\n  three four\nfive six\nseven",
            (0, 0),
            Some(("o", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "ccnew<Esc>j.",
            "new\n  new\nfive six\nseven",
            (1, 4),
            Some(("  three four", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "S-<Esc>j.",
            "-\n  -\nfive six\nseven",
            (1, 2),
            Some(("  three four", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "I# <Esc>j.",
            "# one two\n  # three four\nfive six\nseven",
            (1, 3),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "yiwP.",
            "ononeeone two\n  three four\nfive six\nseven",
            (0, 4),
            Some(("one", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "veld.",
            "  three four\nfive six\nseven",
            (0, 0),
            Some(("two\n", false)),
        ),
        ("one two\n  three four\nfive six\nseven", (0, 0), "Vjd.", "", (0, 0), Some(("five six\nseven", true))),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "vjd.",
            "ive six\nseven",
            (0, 0),
            Some((" three four\nf", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "vjx.",
            "ive six\nseven",
            (0, 0),
            Some((" three four\nf", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "Vd.",
            "five six\nseven",
            (0, 0),
            Some(("  three four", true)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "v~l.",
            "ONe two\n  three four\nfive six\nseven",
            (0, 1),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "vex.",
            "o\n  three four\nfive six\nseven",
            (0, 0),
            Some((" tw", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "ciwone<BS><BS>X<Esc>w.",
            "oX oX\n  three four\nfive six\nseven",
            (0, 4),
            Some(("two", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "i<CR><Esc>.",
            "\n\none two\n  three four\nfive six\nseven",
            (2, 0),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (0, 0),
            "ixyz<Left>q<Esc>.",
            "xyqqzone two\n  three four\nfive six\nseven",
            (0, 2),
            None,
        ),
        ("x y z w v u", (0, 0), "dw.2.", "v u", (0, 0), Some(("z w ", false))),
        ("x y z w v u", (0, 0), "d2w.", "v u", (0, 0), Some(("z w ", false))),
        ("x y z w v u", (0, 0), "d2w3.", "u", (0, 0), Some(("z w v ", false))),
        ("x y z w v u", (0, 0), "2dw.", "v u", (0, 0), Some(("z w ", false))),
        ("x y z w v u", (0, 0), "2d2w.", "", (0, 0), Some(("v u", false))),
        ("x y z w v u", (0, 0), "cwa<Esc>w.w.", "a a a w v u", (0, 4), Some(("z", false))),
        ("x y z w v u", (0, 0), "x5.", "w v u", (0, 0), Some((" y z ", false))),
        ("x y z w v u", (0, 0), "3.", "x y z w v u", (0, 0), None),
        (
            "\u{AC00}\u{B098} \u{B2E4}\u{B77C}\u{B9C8} \u{BC14}\n\u{D55C}\u{AE00} \u{D14D}\u{C2A4}\u{D2B8}, \u{B458}\n  \u{C14B} \u{B137}",
            (0, 0),
            "x.",
            " \u{B2E4}\u{B77C}\u{B9C8} \u{BC14}\n\u{D55C}\u{AE00} \u{D14D}\u{C2A4}\u{D2B8}, \u{B458}\n  \u{C14B} \u{B137}",
            (0, 0),
            Some(("\u{B098}", false)),
        ),
        (
            "\u{AC00}\u{B098} \u{B2E4}\u{B77C}\u{B9C8} \u{BC14}\n\u{D55C}\u{AE00} \u{D14D}\u{C2A4}\u{D2B8}, \u{B458}\n  \u{C14B} \u{B137}",
            (0, 0),
            "ciw\u{C0C8}<Esc>w.",
            "\u{C0C8} \u{C0C8} \u{BC14}\n\u{D55C}\u{AE00} \u{D14D}\u{C2A4}\u{D2B8}, \u{B458}\n  \u{C14B} \u{B137}",
            (0, 2),
            Some(("\u{B2E4}\u{B77C}\u{B9C8}", false)),
        ),
        (
            "\u{AC00}\u{B098} \u{B2E4}\u{B77C}\u{B9C8} \u{BC14}\n\u{D55C}\u{AE00} \u{D14D}\u{C2A4}\u{D2B8}, \u{B458}\n  \u{C14B} \u{B137}",
            (0, 0),
            "dw.",
            "\u{BC14}\n\u{D55C}\u{AE00} \u{D14D}\u{C2A4}\u{D2B8}, \u{B458}\n  \u{C14B} \u{B137}",
            (0, 0),
            Some(("\u{B2E4}\u{B77C}\u{B9C8} ", false)),
        ),
        (
            "\u{AC00}\u{B098} \u{B2E4}\u{B77C}\u{B9C8} \u{BC14}\n\u{D55C}\u{AE00} \u{D14D}\u{C2A4}\u{D2B8}, \u{B458}\n  \u{C14B} \u{B137}",
            (0, 0),
            "r\u{D558}l.",
            "\u{D558}\u{D558} \u{B2E4}\u{B77C}\u{B9C8} \u{BC14}\n\u{D55C}\u{AE00} \u{D14D}\u{C2A4}\u{D2B8}, \u{B458}\n  \u{C14B} \u{B137}",
            (0, 1),
            None,
        ),
    ];
    check(CASES);
}

/// A count before `i a I A o O` types the text again, on new lines for `o` and `O`.
#[test]
fn a_count_types_the_insert_again() {
    const CASES: &[Case] = &[
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "3ix<Esc>",
            "one two\n  thxxxree four\nfive six\nseven",
            (1, 6),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "3ax<Esc>",
            "one two\n  thrxxxee four\nfive six\nseven",
            (1, 7),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "2Iab<Esc>",
            "one two\n  ababthree four\nfive six\nseven",
            (1, 5),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "3A!<Esc>",
            "one two\n  three four!!!\nfive six\nseven",
            (1, 14),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "2oab<Esc>",
            "one two\n  three four\n  ab\n  ab\nfive six\nseven",
            (3, 3),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "3Oab<Esc>",
            "one two\n  ab\n  ab\n  ab\n  three four\nfive six\nseven",
            (3, 3),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "3ia<CR>b<Esc>",
            "one two\n  tha\n  ba\n  ba\n  bree four\nfive six\nseven",
            (4, 2),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "2ix<Esc>.",
            "one two\n  thxxxxree four\nfive six\nseven",
            (1, 6),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "3i<Esc>",
            "one two\n  three four\nfive six\nseven",
            (1, 3),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "2ixy<BS><Esc>",
            "one two\n  thxxree four\nfive six\nseven",
            (1, 5),
            None,
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "2cwq<Esc>",
            "one two\n  thq\nfive six\nseven",
            (1, 4),
            Some(("ree four", false)),
        ),
        (
            "one two\n  three four\nfive six\nseven",
            (1, 4),
            "3sq<Esc>",
            "one two\n  thq four\nfive six\nseven",
            (1, 4),
            Some(("ree", false)),
        ),
        ("    x", (0, 4), "2oab<Esc>", "    x\n    ab\n    ab", (2, 5), None),
        ("    x", (0, 4), "2Oab<Esc>", "    ab\n    ab\n    x", (1, 5), None),
        ("    x", (0, 4), "2o<Esc>", "    x\n\n", (2, 0), None),
    ];
    check(CASES);
}

/// A count given to `.` stays for the next `.`.
#[test]
fn a_count_of_dot_stays() {
    const CASES: &[Case] = &[
        (
            "one two three four five six seven eight nine ten",
            (0, 0),
            "x3..",
            " three four five six seven eight nine ten",
            (0, 0),
            Some(("two", false)),
        ),
        (
            "one two three four five six seven eight nine ten",
            (0, 0),
            "dw3..",
            "eight nine ten",
            (0, 0),
            Some(("five six seven ", false)),
        ),
        ("a\nb\nc\nd\ne\nf\ng\nh", (0, 0), "dd2..", "f\ng\nh", (0, 0), Some(("d\ne", true))),
        ("x", (0, 0), "3.", "x", (0, 0), None),
    ];
    check(CASES);
}

/// One `.` is one undo step, its Insert session included, and undo and redo are not what `.`
/// repeats.
#[test]
fn dot_is_one_undo_step() {
    let mut e = at("one two three", (0, 0));
    typ(&mut e, "cwX<Esc>w.");
    assert_eq!(e.text(), "X X three");
    typ(&mut e, "u");
    assert_eq!(e.text(), "X two three");
    typ(&mut e, ".");
    assert_eq!(e.text(), "X X three", "`.` after `u` repeats the change, not the undo");
    typ(&mut e, "uu<C-r>.");
    assert_eq!(e.text(), "X X three");
}

/// What a paste put in the Insert session is part of the change `.` repeats.
#[test]
fn a_paste_in_insert_mode_is_repeated() {
    let mut e = at("f(x)\ng(y)", (0, 2));
    typ(&mut e, "ci(");
    e.paste("a, b");
    typ(&mut e, "<Esc>j.");
    assert_eq!(e.text(), "f(a, b)\ng(a, b)");
    typ(&mut e, "u");
    assert_eq!(e.text(), "f(a, b)\ng(y)");
}

/// An arrow key in the Insert session starts what `.` inserts again from there (Vim), and ends
/// the repeat a count asked for.
#[test]
fn an_arrow_in_insert_mode_restarts_the_recording() {
    let mut e = at("ab", (0, 0));
    typ(&mut e, "3ix<Right>y<Esc>");
    assert_eq!(e.text(), "xaybn".replace('n', ""), "the count is dropped");
    typ(&mut e, "0.");
    assert_eq!(e.text(), "yxayb");
}
