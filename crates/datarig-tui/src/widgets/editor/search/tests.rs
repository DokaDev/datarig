use super::super::tests::{Case, at, check, typ};
use super::super::{EdEvent, Editor, Mode};
use super::*;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// `/`, `?`, `n`, `N`, `*`, `#`, `g*`, `g#` with counts, around the ends of the text, at the
/// ends of lines and on Hangul, as Neovim moves (`dev/vim-cases.py`; a pattern Vim writes
/// otherwise, `\<b\>` for `\bb\b`, was given to Neovim in its own notation).
#[test]
fn search_motions_move_as_vim() {
    const CASES: &[Case] = &[
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "/from<CR>",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 12),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 7),
            "/a<CR>",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 6),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "2/a<CR>",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 6),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "/b<CR>n",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 16),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "/b<CR>nN",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 10),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (2, 0),
            "?b<CR>",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 16),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (2, 0),
            "?b<CR>n",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 10),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (2, 0),
            "?b<CR>N",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (2, 6),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (4, 0),
            "/where<CR>",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 0),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "?select<CR>",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (4, 0),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "/a<CR>3n",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (2, 9),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "/where<CR>5n",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 0),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 0),
            "/where<CR>",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 0),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "/a<CR>99n",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 6),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "/a<CR>2N",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (3, 15),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 7),
            "*",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 6),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 6),
            "#",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 7),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 7),
            "2*",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (2, 9),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 7),
            "*n",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (2, 9),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 7),
            "#n",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (3, 5),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 7),
            "#N",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 7),
            None,
        ),
        ("foo foobar barfoo foo\nfoo_x foo", (0, 0), "*", "foo foobar barfoo foo\nfoo_x foo", (0, 18), None),
        ("foo foobar barfoo foo\nfoo_x foo", (0, 0), "g*", "foo foobar barfoo foo\nfoo_x foo", (0, 4), None),
        ("foo foobar barfoo foo\nfoo_x foo", (0, 0), "#", "foo foobar barfoo foo\nfoo_x foo", (1, 6), None),
        ("foo foobar barfoo foo\nfoo_x foo", (0, 0), "g#", "foo foobar barfoo foo\nfoo_x foo", (1, 6), None),
        ("foo foobar barfoo foo\nfoo_x foo", (0, 18), "g#", "foo foobar barfoo foo\nfoo_x foo", (0, 14), None),
        ("foo foobar barfoo foo\nfoo_x foo", (0, 5), "*", "foo foobar barfoo foo\nfoo_x foo", (0, 4), None),
        ("foo foobar barfoo foo\nfoo_x foo", (0, 3), "*", "foo foobar barfoo foo\nfoo_x foo", (0, 4), None),
        ("foo foobar barfoo foo\nfoo_x foo", (1, 3), "*", "foo foobar barfoo foo\nfoo_x foo", (1, 0), None),
        ("x = a + b;\ny = b + a;", (0, 1), "*", "x = a + b;\ny = b + a;", (1, 8), None),
        ("a := ;\nb := ;", (0, 2), "*", "a := ;\nb := ;", (1, 2), None),
        ("a := ;\nb := ;", (0, 2), "#", "a := ;\nb := ;", (1, 2), None),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "/\\bb\\b<CR>",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 10),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "/\\ba<CR>n",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 6),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "/b<CR>/<CR>",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 16),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "/b<CR>?<CR>",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (4, 7),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "/b<CR>?<CR>n",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (3, 16),
            None,
        ),
        ("one\n\ntwo end\nthree", (0, 0), "/$<CR>", "one\n\ntwo end\nthree", (0, 2), None),
        ("one\n\ntwo end\nthree", (0, 0), "/$<CR>n", "one\n\ntwo end\nthree", (1, 0), None),
        ("one\n\ntwo end\nthree", (0, 0), "/$<CR>nn", "one\n\ntwo end\nthree", (2, 6), None),
        ("one\n\ntwo end\nthree", (2, 4), "?^<CR>", "one\n\ntwo end\nthree", (2, 0), None),
        ("one\n\ntwo end\nthree", (0, 0), "/^<CR>", "one\n\ntwo end\nthree", (1, 0), None),
        ("one\n\ntwo end\nthree", (1, 0), "/^<CR>", "one\n\ntwo end\nthree", (2, 0), None),
        ("one\n\ntwo end\nthree", (2, 3), "?e<CR>", "one\n\ntwo end\nthree", (0, 2), None),
        ("one\n\ntwo end\nthree", (2, 4), "?e<CR>", "one\n\ntwo end\nthree", (0, 2), None),
        ("one\n\ntwo end\nthree", (2, 4), "/e<CR>", "one\n\ntwo end\nthree", (3, 3), None),
        (
            "\u{AC00}\u{B098} \u{B2E4}\u{B77C}\n\u{B2E4}\u{B77C} \u{AC00}\u{B098}",
            (0, 0),
            "/\u{B2E4}\u{B77C}<CR>",
            "\u{AC00}\u{B098} \u{B2E4}\u{B77C}\n\u{B2E4}\u{B77C} \u{AC00}\u{B098}",
            (0, 3),
            None,
        ),
        (
            "\u{AC00}\u{B098} \u{B2E4}\u{B77C}\n\u{B2E4}\u{B77C} \u{AC00}\u{B098}",
            (0, 0),
            "*",
            "\u{AC00}\u{B098} \u{B2E4}\u{B77C}\n\u{B2E4}\u{B77C} \u{AC00}\u{B098}",
            (1, 3),
            None,
        ),
        (
            "\u{AC00}\u{B098} \u{B2E4}\u{B77C}\n\u{B2E4}\u{B77C} \u{AC00}\u{B098}",
            (1, 3),
            "#",
            "\u{AC00}\u{B098} \u{B2E4}\u{B77C}\n\u{B2E4}\u{B77C} \u{AC00}\u{B098}",
            (0, 0),
            None,
        ),
        (
            "\u{AC00}\u{B098} \u{B2E4}\u{B77C}\n\u{B2E4}\u{B77C} \u{AC00}\u{B098}",
            (0, 1),
            "/\u{B098}<CR>n",
            "\u{AC00}\u{B098} \u{B2E4}\u{B77C}\n\u{B2E4}\u{B77C} \u{AC00}\u{B098}",
            (0, 1),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "/zzz<CR>",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 3),
            "/a<CR>/zzz<CR>",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 7),
            None,
        ),
    ];
    check(CASES);
}

/// Search motions under operators (exclusive by character, whole lines when the range starts
/// in the indent and ends at the start of a line; a match at the end of a line stops on its
/// last character), in Visual mode (where it takes the line break) by character and by line,
/// and repeated with `.`, as Neovim does them.
#[test]
fn search_motions_under_operators_as_vim() {
    const CASES: &[Case] = &[
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "d/b<CR>",
            "b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            Some(("select a, ", false)),
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (2, 3),
            "y?where<CR>",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 0),
            Some(("where a = 1 and b = 2\nord", false)),
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 0),
            "/and<CR>0dn",
            "select a, b from t\nand b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 0),
            Some(("where a = 1 ", false)),
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 0),
            "d/order<CR>",
            "select a, b from t\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 0),
            Some(("where a = 1 and b = 2", true)),
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "c/b<CR>X<Esc>",
            "Xb from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            Some(("select a, ", false)),
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "d/b<CR>.",
            "b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            Some(("b from t\nwhere a = 1 and ", false)),
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "/a<CR>0dn.",
            "a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            Some(("a, b from t\nwhere ", false)),
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "d2/a<CR>",
            "a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            Some(("select a, b from t\nwhere ", false)),
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "2d/a<CR>",
            "a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            Some(("select a, b from t\nwhere ", false)),
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "v/from<CR>d",
            "rom t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            Some(("select a, b f", false)),
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "d/zzz<CR>",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 8),
            "d?a<CR>",
            "select a, b from t\nwhere = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (1, 6),
            Some(("a ", false)),
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 7),
            "d*",
            "select a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 7),
            Some(("a, b from t\nwhere ", false)),
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 12),
            "d#",
            "select a, b from u",
            (0, 12),
            Some(("from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b ", false)),
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (2, 9),
            "dN",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (2, 9),
            None,
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "y/where<CR>",
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            Some(("select a, b from t", true)),
        ),
        (
            "foo foobar barfoo foo\nfoo_x foo",
            (0, 5),
            "dg*",
            "foo oobar barfoo foo\nfoo_x foo",
            (0, 4),
            Some(("f", false)),
        ),
        (
            "select a, b from t\nwhere a = 1 and b = 2\norder by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            "/b<CR>Vnd",
            "order by a, b\n  -- a comment about a\nselect b from u",
            (0, 0),
            Some(("select a, b from t\nwhere a = 1 and b = 2", true)),
        ),
        ("abc\ndef", (0, 0), "d/$<CR>", "c\ndef", (0, 0), Some(("ab", false))),
        ("abc\ndef", (0, 0), "c/$<CR>X<Esc>", "Xc\ndef", (0, 0), Some(("ab", false))),
        ("foo bar\nbaz", (0, 4), "d/$<CR>", "foo r\nbaz", (0, 4), Some(("ba", false))),
        ("ab\ncd", (0, 0), "d2/$<CR>", "d", (0, 0), Some(("ab\nc", false))),
        ("abc\ndef", (0, 0), "v/$<CR>d", "def", (0, 0), Some(("abc\n", false))),
        ("a;b c  \nx;", (0, 0), "d/;|$<CR>", ";b c  \nx;", (0, 0), Some(("a", false))),
        ("a;b c  \nx;", (0, 2), "d/;|$<CR>", "a; \nx;", (0, 2), Some(("b c ", false))),
        ("ab  \ncd", (0, 0), "d/\\s*$<CR>", "  \ncd", (0, 0), Some(("ab", false))),
        ("ab  \ncd", (0, 0), "y/\\s*$<CR>", "ab  \ncd", (0, 0), Some(("ab", false))),
        ("ab\ncd  ", (0, 0), "/\\s*$<CR>n", "ab\ncd  ", (1, 2), None),
        ("abc\ndef", (0, 0), "/$<CR>", "abc\ndef", (0, 2), None),
        ("abc\ndef", (1, 1), "?$<CR>", "abc\ndef", (0, 2), None),
        ("abc\ndef", (0, 0), "d/$<CR>.", "f", (0, 0), Some(("c\nde", false))),
        ("a1 b1 a2\nb2 a3\nb3 a4", (0, 0), "V/a3<CR>d", "b3 a4", (0, 0), Some(("a1 b1 a2\nb2 a3", true))),
        (
            "a1 b1 a2\nb2 a3\nb3 a4",
            (2, 1),
            "V?b1<CR>y",
            "a1 b1 a2\nb2 a3\nb3 a4",
            (0, 3),
            Some(("a1 b1 a2\nb2 a3\nb3 a4", true)),
        ),
        ("one two\nthree", (0, 4), "v/thr<CR><Esc>", "one two\nthree", (1, 0), None),
        ("a1 b1 a2 b2 a3 b3 a4", (0, 0), "d/b<CR>/a<CR>.", "b1 b2 a3 b3 a4", (0, 3), Some(("a2 ", false))),
    ];
    check(CASES);
}

fn notice_of(text: &str, keys: &str) -> (Editor, Option<SearchNotice>) {
    let mut e = Editor::new(text);
    typ(&mut e, keys);
    let n = e.take_search_notice();
    (e, n)
}

#[test]
fn an_invalid_pattern_is_said_and_moves_nothing() {
    let (e, n) = notice_of("one (two)\nthree", "w/(tw<CR>");
    assert_eq!(n, Some(SearchNotice::Invalid("unclosed group".into())));
    assert_eq!((e.row, e.col, e.searching()), (0, 4, false));
    assert_eq!(e.search_pattern(), None);
    // An operator waiting for it is dropped with it.
    let (e, n) = notice_of("one (two)\nthree", "d/[<CR>");
    assert!(matches!(n, Some(SearchNotice::Invalid(_))));
    assert_eq!((e.text().as_str(), e.cmd.is_empty()), ("one (two)\nthree", true));
    // A pattern too large to compile is an error too, not a panic.
    let (_, n) = notice_of("x", r"/\w{1000}{1000}<CR>");
    assert!(matches!(&n, Some(SearchNotice::Invalid(m)) if m.contains("size limit")), "{n:?}");
}

#[test]
fn an_empty_pattern_takes_the_last_one() {
    let (e, n) = notice_of("a b a b", "/<CR>");
    assert_eq!((n, e.col), (Some(SearchNotice::NoPrevious), 0));
    let (e, n) = notice_of("a b a b", "n");
    assert_eq!((n, e.col), (Some(SearchNotice::NoPrevious), 0));
    let (e, _) = notice_of("a b a b", "/b<CR>/<CR>");
    assert_eq!((e.col, e.search_pattern()), (6, Some("b")));
    // `?` with an empty pattern turns the direction around: `n` goes back.
    let (e, _) = notice_of("a b a b", "$?<CR>");
    assert_eq!(e.col, 6);
    let (e, _) = notice_of("a b a b", "/b<CR>/<CR>?<CR>");
    assert_eq!(e.col, 2);
    let (e, _) = notice_of("a b a b", "/b<CR>/<CR>?<CR>n");
    assert_eq!(e.col, 6);
}

#[test]
fn going_around_the_end_and_not_finding_are_said() {
    let (e, n) = notice_of("x\ny\nx", "G/y<CR>");
    assert_eq!((n, e.row), (Some(SearchNotice::Wrapped { bottom: true }), 1));
    let (e, n) = notice_of("x\ny\nx", "?y<CR>");
    assert_eq!((n, e.row), (Some(SearchNotice::Wrapped { bottom: false }), 1));
    let (e, n) = notice_of("x\ny\nx", "/x<CR>");
    assert_eq!((n, e.row), (None, 2));
    let (e, n) = notice_of("x\ny\nx", "j/zz<CR>");
    assert_eq!((n, e.row), (Some(SearchNotice::NotFound("zz".into())), 1));
    let (_, n) = notice_of("   \nx", "*");
    assert_eq!(n, Some(SearchNotice::NoWord));
}

#[test]
fn patterns_are_case_sensitive_unless_they_say_otherwise() {
    let (e, n) = notice_of("select x\nSELECT y", "/SELECT<CR>");
    assert_eq!((e.row, n), (1, None));
    let (e, _) = notice_of("select x\nSELECT y", "j/select<CR>");
    assert_eq!(e.row, 0);
    let (e, _) = notice_of("one\nSeLeCt y", "/(?i)select<CR>");
    assert_eq!((e.row, e.col), (1, 0));
}

/// Columns are graphemes, whatever their width or bytes; a Hangul pattern finds Hangul.
#[test]
fn unicode_text_and_patterns() {
    let (e, _) = notice_of("\u{8868}\u{8868} x \u{1F418} x", "/x<CR>n");
    assert_eq!(e.col, 7);
    let (e, _) = notice_of("e\u{301}t\u{e9} \u{AC00}\u{B098}\u{B2E4}", "/\u{B098}<CR>");
    assert_eq!(e.col, 5);
    let (e, n) = notice_of("\u{AC00}\u{B098} x \u{AC00}\u{B098}\u{B2E4}", "/\\b\u{AC00}\u{B098}\\b<CR>");
    assert_eq!((e.col, n), (0, Some(SearchNotice::Wrapped { bottom: true })));
    // `\w` and `\b` know Hangul as letters: the second one is part of a longer word.
    let (e, _) = notice_of("\u{AC00} \u{AC00}\u{B098}", r"/\w+<CR>");
    assert_eq!(e.col, 2);
}

/// A match far into a very long line is found by searching that line once.
#[test]
fn very_long_lines() {
    let line = format!("{}needle{}", "a".repeat(1 << 20), "b".repeat(1000));
    let mut e = Editor::new(&format!("x\n{line}\ny"));
    typ(&mut e, "/needle");
    e.take_search_work();
    typ(&mut e, "<CR>");
    assert_eq!((e.row, e.col), (1, 1 << 20));
    assert_eq!(e.take_search_work().bytes, 2 + line.len() + 1);
    typ(&mut e, "/b$<CR>");
    assert_eq!((e.row, e.col), (1, (1 << 20) + 6 + 999));
}

/// The bytes a search searched: (each line with its line break).
fn text_bytes(e: &Editor) -> usize {
    e.len_bytes() + 1
}

/// A miss searches the text once; `n` up to the next match; any count fewer than three times.
#[test]
fn search_work_is_bounded() {
    let text = (0..1000).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
    let mut e = Editor::new(&text);
    e.row = 500;
    typ(&mut e, "/nothing");
    e.take_search_work();
    typ(&mut e, "<CR>");
    assert_eq!(e.take_search_work().bytes, text_bytes(&e));
    typ(&mut e, "/line 7");
    e.take_search_work();
    typ(&mut e, "<CR>");
    assert_eq!(e.row, 700);
    assert_eq!(e.take_search_work().bytes, e.lines[500..=700].iter().map(|l| l.len() + 1).sum::<usize>());
    typ(&mut e, "/line 999<CR>");
    e.take_search_work();
    typ(&mut e, "99999n");
    assert_eq!(e.row, 999);
    assert!(e.take_search_work().bytes <= 3 * text_bytes(&e));
    // While typing, each key searches the text once at most.
    e.row = 0;
    for c in "/nothing".chars() {
        typ(&mut e, &c.to_string());
        assert!(e.take_search_work().bytes <= text_bytes(&e));
    }
    typ(&mut e, "<Esc>");
}

/// One long line of many matches: a huge count, forward and back, searches it a few times, not
/// once per match (that was minutes).
#[test]
fn many_matches_on_one_line_with_a_huge_count() {
    let line = "a, ".repeat(20_000);
    let mut e = Editor::new(&format!("x\n{line}\ny"));
    let bound = 3 * text_bytes(&e);
    typ(&mut e, "/,<CR>");
    e.take_search_work();
    typ(&mut e, "999999n");
    // 20,000 commas; the first `/` took the first: 999,999 more lands on the 999,999 % 20,000th
    // after it (the same comma 20,000 later).
    assert_eq!((e.row, e.col), (1, (999_999 % 20_000) * 3 + 1));
    assert!(e.take_search_work().bytes <= bound);
    typ(&mut e, "999999N");
    assert_eq!((e.row, e.col), (1, 1));
    assert!(e.take_search_work().bytes <= bound);
    typ(&mut e, "N");
    assert_eq!((e.row, e.col), (1, line.len() - 2), "around to the last comma");
}

#[test]
fn ctrl_w_in_the_prompt_deletes_a_word() {
    let mut e = Editor::new("foo bar\nfoo");
    typ(&mut e, "/foo bar<C-w><CR>");
    assert_eq!(e.search_pattern(), Some("foo "));
    assert_eq!((e.row, e.col), (0, 0), "around to the only `foo `");
}

/// Text pasted into the prompt is part of the change `.` repeats.
#[test]
fn a_pasted_pattern_is_repeated_by_dot() {
    let mut e = Editor::new("a1 b1 a2 b2 a3 b3 a4");
    typ(&mut e, "d/");
    e.paste("b");
    typ(&mut e, "<CR>/a<CR>.");
    // As Neovim does `d/b<CR>/a<CR>.`.
    assert_eq!((e.text().as_str(), (e.row, e.col)), ("b1 b2 a3 b3 a4", (0, 3)));
}

/// `Esc` in Visual mode leaves the prompt only: the selection and its cursor stay.
#[test]
fn esc_in_the_prompt_keeps_visual_mode() {
    let mut e = Editor::new("one two\nthree");
    typ(&mut e, "vl/thr");
    assert_eq!(e.row, 1);
    typ(&mut e, "<Esc>");
    assert_eq!((e.mode, e.row, e.col, e.searching()), (Mode::Visual, 0, 1, false));
}

/// An editor of one line draws the prompt over it.
#[test]
fn a_one_line_editor_draws_the_prompt() {
    let mut e = Editor::new("abc");
    let area = Rect::new(0, 0, 20, 1);
    let mut buf = Buffer::empty(area);
    typ(&mut e, "/b");
    let cursor = e.render(area, &mut buf, None);
    let row: String = (0..area.width).map(|x| buf[(x, 0)].symbol().to_string()).collect();
    assert_eq!((row.trim_end(), cursor), ("/b", (2, 0)));
}

#[test]
fn the_prompt_previews_and_esc_goes_back() {
    let text = (0..100).map(|i| format!("row {i}")).collect::<Vec<_>>().join("\n");
    let mut e = Editor::new(&text);
    let area = Rect::new(0, 0, 30, 10);
    let mut buf = Buffer::empty(area);
    typ(&mut e, "5j");
    e.render(area, &mut buf, None);
    typ(&mut e, "/row 8");
    assert!(e.searching());
    assert_eq!((e.row, e.col), (8, 0), "the cursor previews the match");
    typ(&mut e, "0");
    assert_eq!(e.row, 80);
    let cursor = e.render(area, &mut buf, None);
    assert!(e.top > 70, "the view follows the preview");
    let last: String = (0..area.width).map(|x| buf[(x, 9)].symbol().to_string()).collect();
    assert_eq!(last.trim_end(), "/row 80");
    assert_eq!(cursor, (7, 9), "the cursor is in the prompt");
    typ(&mut e, "<Esc>");
    assert_eq!((e.row, e.col, e.top, e.searching()), (5, 0, 0, false));
    assert_eq!(e.search_pattern(), None);
    // Backspace on an empty pattern closes the prompt too.
    typ(&mut e, "?x<BS>");
    assert!(e.searching());
    typ(&mut e, "<BS>");
    assert!(!e.searching());
    // A pattern that does not compile yet previews nothing.
    typ(&mut e, "/row (9");
    assert_eq!(e.row, 5);
    typ(&mut e, ")<CR>");
    assert_eq!(e.row, 9);
}

#[test]
fn a_paste_goes_into_the_prompt() {
    let mut e = Editor::new("a\nb c\nd");
    typ(&mut e, "/");
    assert_eq!(e.paste("b\nc"), EdEvent::Moved);
    assert_eq!(e.text(), "a\nb c\nd");
    typ(&mut e, "<CR>");
    assert_eq!((e.row, e.col), (1, 0));
    assert_eq!(e.search_pattern(), Some("b c"));
}

#[test]
fn the_slash_register_holds_the_last_pattern() {
    let mut e = at("foo.bar foo.bar", (0, 0));
    typ(&mut e, "*");
    assert_eq!(e.register('/').map(|r| r.text.as_str()), Some(r"\bfoo\b"));
    typ(&mut e, "g#");
    assert_eq!(e.register('/').map(|r| r.text.as_str()), Some("foo"));
    typ(&mut e, "0f.*");
    assert_eq!(e.register('/').map(|r| r.text.as_str()), Some(r"\bbar\b"));
    typ(&mut e, "\"/P");
    assert_eq!(e.text(), r"foo.bar foo.\bbar\bbar");
    // `*` on a run of other characters (with no keyword after it) takes it as it is.
    let mut e = at("x\n:= ;", (1, 1));
    typ(&mut e, "*");
    assert_eq!(e.register('/').map(|r| r.text.as_str()), Some(":="));
    assert_eq!(e.mode, Mode::Normal);
}

/// A match that starts inside a grapheme (a combining mark) marks the grapheme; with the view
/// scrolled sideways over wide characters, the matches are where their characters are drawn.
#[test]
fn highlight_inside_graphemes_and_scrolled() {
    let mut e = Editor::new("e\u{301}x");
    let area = Rect::new(0, 0, 20, 3);
    let mut buf = Buffer::empty(area);
    typ(&mut e, "/\u{301}<CR>");
    e.render(area, &mut buf, None);
    assert_eq!(styled(&buf, 0, 4, 6), [true, false]);

    // 30 wide characters (two columns each), then the match.
    let mut e = Editor::new(&format!("{}ab", "\u{8868}".repeat(30)));
    let area = Rect::new(0, 0, 20, 3);
    typ(&mut e, "/ab<CR>");
    assert_eq!(e.col, 30);
    let mut buf = Buffer::empty(area);
    e.render(area, &mut buf, None);
    assert!(e.left > 0, "scrolled sideways");
    let x = 4 + (60 - e.left) as u16;
    assert_eq!(buf[(x, 0)].symbol(), "a");
    // The cursor's column is the view's last: the wide character before it is not marked.
    assert_eq!(styled(&buf, 0, x - 2, x + 1), [false, false, true]);
    typ(&mut e, "0");
    e.render(area, &mut buf, None);
    assert!(!(4..20).any(|x| buf[(x, 0)].bg == crate::theme::cur().search_match.bg.unwrap()), "none on screen");
}

fn styled(buf: &Buffer, y: u16, from: u16, to: u16) -> Vec<bool> {
    let th = crate::theme::cur();
    (from..to).map(|x| buf[(x, y)].bg == th.search_match.bg.unwrap()).collect()
}

/// Matches are highlighted on the lines on screen until `:nohlsearch`; the next search shows
/// them again, and a frame reads only the lines it draws.
#[test]
fn matches_are_highlighted_on_screen() {
    let text = (0..200).map(|i| format!("ab x{i} ab")).collect::<Vec<_>>().join("\n");
    let mut e = Editor::new(&text);
    let area = Rect::new(0, 0, 30, 10);
    let mut buf = Buffer::empty(area);
    typ(&mut e, "/ab<CR>");
    e.take_search_work();
    e.render(area, &mut buf, None);
    assert_eq!(e.take_search_work().highlighted, 10);
    // "  1 " is the gutter; "ab x0 ab" follows.
    let g = 4;
    assert_eq!(styled(&buf, 1, g, g + 8), [true, true, false, false, false, false, true, true]);
    e.clear_highlight();
    e.render(area, &mut buf, None);
    assert_eq!(e.take_search_work().highlighted, 0);
    assert!(!styled(&buf, 1, g, g + 8).iter().any(|s| *s));
    typ(&mut e, "n");
    e.render(area, &mut buf, None);
    assert!(styled(&buf, 2, g, g + 2).iter().all(|s| *s));
    // While typing, the pattern typed so far is highlighted instead.
    typ(&mut e, "/x1");
    e.render(area, &mut buf, None);
    assert_eq!(styled(&buf, 0, g, g + 8), [false; 8]);
    assert_eq!(styled(&buf, 1, g, g + 8), [false, false, false, true, true, false, false, false]);
    typ(&mut e, "<Esc>");
    assert!(!e.searching());
}
