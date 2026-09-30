use super::super::tests::typ;
use super::*;

/// Forty lines, every seventh indented.
const TEXT: &str = "l1\nl2\nl3\nl4\nl5\nl6\n  x7\nl8\nl9\nl10\nl11\nl12\nl13\n  x14\nl15\nl16\nl17\nl18\nl19\nl20\n  x21\nl22\nl23\nl24\nl25\nl26\nl27\n  x28\nl29\nl30\nl31\nl32\nl33\nl34\n  x35\nl36\nl37\nl38\nl39\nl40";

/// `(text, cursor, lines on screen, top line, keys, cursor after, top line after)`.
type ViewCase = (&'static str, (usize, usize), usize, usize, &'static str, (usize, usize), usize);

/// `Ctrl+D` `Ctrl+U` `Ctrl+F` `Ctrl+B` (with counts, at both ends of the text, on small screens),
/// `zz` `zt` `zb` and `H` `M` `L`, as Vim scrolls and moves.
#[test]
fn scrolling_as_vim() {
    const CASES: &[ViewCase] = &[
        (TEXT, (5, 1), 10, 2, "<C-d>", (10, 0), 7),
        (TEXT, (5, 1), 10, 2, "<C-u>", (0, 0), 0),
        (TEXT, (5, 1), 10, 2, "<C-f>", (10, 0), 10),
        (TEXT, (5, 1), 10, 2, "<C-b>", (9, 0), 0),
        (TEXT, (5, 1), 10, 2, "3<C-d>", (8, 0), 5),
        (TEXT, (5, 1), 10, 2, "<C-d><C-d>", (15, 0), 12),
        (TEXT, (5, 1), 10, 2, "zz", (5, 1), 1),
        (TEXT, (5, 1), 10, 2, "zt", (5, 1), 5),
        (TEXT, (5, 1), 10, 2, "zb", (5, 1), 0),
        (TEXT, (5, 1), 10, 2, "H", (2, 0), 2),
        (TEXT, (5, 1), 10, 2, "M", (6, 2), 2),
        (TEXT, (5, 1), 10, 2, "L", (11, 0), 2),
        (TEXT, (5, 1), 10, 2, "3H", (4, 0), 2),
        (TEXT, (5, 1), 10, 2, "3L", (9, 0), 2),
        (TEXT, (5, 1), 10, 2, "5zt", (4, 1), 4),
        (TEXT, (5, 1), 10, 2, "dH", (2, 2), 2),
        (TEXT, (5, 1), 10, 2, "dL", (5, 0), 2),
        (TEXT, (5, 1), 10, 2, "dM", (5, 0), 2),
        (TEXT, (5, 1), 10, 2, "<C-d><C-d><C-d><C-d>", (25, 0), 22),
        (TEXT, (5, 1), 10, 2, "<C-f><C-f><C-f><C-f>", (34, 2), 34),
        (TEXT, (5, 1), 10, 2, "G<C-u>", (34, 2), 25),
        (TEXT, (5, 1), 10, 2, "G<C-b>", (31, 0), 22),
        (TEXT, (5, 1), 10, 2, "Gzz", (39, 0), 35),
        (TEXT, (5, 1), 10, 2, "ggzb", (0, 0), 0),
        (TEXT, (5, 1), 10, 2, "3<C-d><C-d>", (11, 0), 8),
        (TEXT, (38, 1), 10, 30, "<C-d>", (39, 0), 30),
        (TEXT, (39, 1), 10, 30, "<C-d>", (39, 1), 30),
        (TEXT, (35, 1), 10, 30, "<C-f>", (39, 0), 39),
        (TEXT, (1, 1), 10, 0, "<C-u>", (0, 0), 0),
        (TEXT, (1, 1), 10, 0, "<C-b>", (1, 1), 0),
        (TEXT, (3, 1), 10, 2, "<C-b>", (9, 0), 0),
        ("a\nb\nc", (1, 0), 10, 0, "M", (1, 0), 0),
        ("a\nb\nc", (1, 0), 10, 0, "L", (2, 0), 0),
        ("a\nb\nc", (1, 0), 10, 0, "<C-d>", (2, 0), 0),
        ("a\nb\nc", (1, 0), 10, 0, "<C-f>", (2, 0), 2),
        (TEXT, (30, 1), 10, 28, "<C-d>", (35, 0), 30),
        (TEXT, (7, 1), 10, 5, "<C-b>", (9, 0), 0),
        (TEXT, (12, 1), 10, 5, "<C-b>", (9, 0), 0),
        (TEXT, (20, 1), 10, 15, "<C-b>", (16, 0), 7),
        (TEXT, (20, 1), 9, 15, "zz", (20, 1), 16),
        (TEXT, (20, 1), 10, 15, "zz", (20, 1), 16),
        (TEXT, (20, 1), 10, 15, "zb", (20, 1), 11),
        (TEXT, (2, 1), 10, 0, "zz", (2, 1), 0),
        (TEXT, (20, 1), 9, 15, "M", (19, 0), 15),
        (TEXT, (35, 1), 10, 33, "M", (36, 0), 33),
        (TEXT, (35, 1), 10, 33, "L", (39, 0), 33),
        (TEXT, (35, 1), 10, 33, "99H", (39, 0), 33),
        (TEXT, (35, 1), 10, 33, "99L", (33, 0), 33),
        (TEXT, (20, 1), 10, 18, "<C-u>", (15, 0), 13),
        (TEXT, (20, 1), 10, 18, "2<C-f>", (34, 2), 34),
        (TEXT, (20, 1), 10, 18, "2<C-b>", (11, 0), 2),
        (TEXT, (39, 1), 10, 39, "<C-f>", (39, 1), 39),
        (TEXT, (32, 1), 10, 32, "<C-f>", (39, 0), 39),
        (TEXT, (5, 3), 10, 2, "<C-d>", (10, 0), 7),
        (TEXT, (5, 1), 10, 2, "0<C-d>", (10, 0), 7),
        (TEXT, (20, 1), 10, 15, "zt", (20, 1), 20),
        (TEXT, (20, 1), 10, 15, "z<CR>", (20, 2), 20),
        (TEXT, (5, 1), 1, 5, "<C-d>", (6, 2), 6),
        (TEXT, (5, 1), 1, 5, "<C-f>", (6, 2), 6),
        (TEXT, (5, 1), 2, 5, "<C-f>", (7, 0), 7),
        (TEXT, (5, 1), 2, 5, "<C-b>", (4, 0), 3),
        (TEXT, (5, 1), 1, 5, "<C-b>", (4, 0), 4),
        (TEXT, (5, 1), 3, 5, "<C-d>", (6, 2), 6),
        (TEXT, (5, 1), 3, 5, "<C-u>", (4, 0), 4),
    ];
    for &(text, (row, col), h, top, keys, want, want_top) in CASES {
        let mut e = Editor::new(text);
        (e.row, e.col, e.view_h, e.top) = (row, col, h, top);
        typ(&mut e, keys);
        assert_eq!(((e.row, e.col), e.top), (want, want_top), "{keys:?} at {:?}, {h} lines from {top}", (row, col));
    }
}
