use super::super::tests::{Case, check};
use super::*;

/// [`super::toggle`] in PostgreSQL.
fn toggle(lines: &[String]) -> Vec<String> {
    super::toggle(lines, Dialect::Postgres)
}

/// `gcc`, `gc{motion}`, `gc` in Visual mode (any kind), counts, `.` and undo, as Neovim's
/// built-in commenting does with `commentstring` `-- %s` (`dev/vim-cases.py`): blank lines,
/// mixed indents (the smallest one, tabs or spaces as written), lines that are comments
/// already, Hangul.
#[test]
fn gc_comments_as_neovim() {
    const CASES: &[Case] = &[
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (1, 4),
            "gcc",
            "select 1;\n  -- select 2;\n\n    select 3;",
            (1, 2),
            None,
        ),
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (1, 1),
            "gcc",
            "select 1;\n  -- select 2;\n\n    select 3;",
            (1, 1),
            None,
        ),
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (0, 4),
            "gcG",
            "-- select 1;\n--   select 2;\n--\n--     select 3;",
            (0, 4),
            None,
        ),
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (0, 4),
            "gcGu",
            "select 1;\n  select 2;\n\n    select 3;",
            (0, 4),
            None,
        ),
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (0, 4),
            "gcG<C-r>",
            "-- select 1;\n--   select 2;\n--\n--     select 3;",
            (0, 4),
            None,
        ),
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (3, 6),
            "gcgg",
            "-- select 1;\n--   select 2;\n--\n--     select 3;",
            (0, 0),
            None,
        ),
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (1, 3),
            "gcj",
            "select 1;\n  -- select 2;\n  --\n    select 3;",
            (1, 3),
            None,
        ),
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (1, 3),
            "gck",
            "-- select 1;\n--   select 2;\n\n    select 3;",
            (0, 3),
            None,
        ),
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (1, 3),
            "2gcc",
            "select 1;\n  -- select 2;\n  --\n    select 3;",
            (1, 3),
            None,
        ),
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (0, 3),
            "3gcc",
            "-- select 1;\n--   select 2;\n--\n    select 3;",
            (0, 3),
            None,
        ),
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (0, 3),
            "9gcc",
            "-- select 1;\n--   select 2;\n--\n--     select 3;",
            (0, 3),
            None,
        ),
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (3, 3),
            "2gcc",
            "select 1;\n  select 2;\n\n    select 3;",
            (3, 3),
            None,
        ),
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (1, 3),
            "gc2j",
            "select 1;\n  -- select 2;\n  --\n  --   select 3;",
            (1, 3),
            None,
        ),
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (0, 3),
            "gcip",
            "-- select 1;\n--   select 2;\n\n    select 3;",
            (0, 0),
            None,
        ),
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (0, 3),
            "gcap",
            "-- select 1;\n--   select 2;\n--\n    select 3;",
            (0, 0),
            None,
        ),
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (0, 3),
            "gcw",
            "-- select 1;\n  select 2;\n\n    select 3;",
            (0, 3),
            None,
        ),
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (0, 3),
            "gc}",
            "-- select 1;\n--   select 2;\n\n    select 3;",
            (0, 3),
            None,
        ),
        (
            "select 1;\n  select 2;\n\n    select 3;",
            (0, 3),
            "gc/2<CR>",
            "-- select 1;\n--   select 2;\n\n    select 3;",
            (0, 3),
            None,
        ),
        (
            "-- select 1;\n  -- select 2;\n\n    select 3;",
            (0, 4),
            "gcj",
            "select 1;\n  select 2;\n\n    select 3;",
            (0, 4),
            None,
        ),
        (
            "-- select 1;\n  -- select 2;\n\n    select 3;",
            (0, 4),
            "gcG",
            "-- -- select 1;\n--   -- select 2;\n--\n--     select 3;",
            (0, 4),
            None,
        ),
        ("-- select 1;\n  --select 2;\n--\n  --   ", (0, 4), "gcG", "select 1;\n  select 2;\n\n  ", (0, 4), None),
        ("--\n  -- \n  --x", (0, 0), "gcG", "\n\n  x", (0, 0), None),
        ("  \n\n  ", (0, 0), "gcG", "  \n\n  ", (0, 0), None),
        ("  a\n\n  b", (0, 0), "gcG", "  -- a\n  --\n  -- b", (0, 0), None),
        ("  a\n   \n  b", (2, 1), "gcgg", "  -- a\n  --\n  -- b", (0, 2), None),
        ("\ta\n  b\n    c", (0, 0), "gcG", "\t-- a\n\t--  b\n\t--    c", (0, 0), None),
        ("    a\n\tb", (0, 0), "gcj", "\t--    a\n\t-- b", (0, 0), None),
        ("a\nb\nc\nd", (1, 0), "2gccj.", "a\n-- b\n-- -- c\n-- d", (2, 0), None),
        ("a\nb\nc\nd", (0, 0), "gcjjj.", "-- a\n-- b\n-- c\n-- d", (2, 0), None),
        ("a\nb\nc\nd", (0, 0), "gcc..", "-- a\nb\nc\nd", (0, 0), None),
        ("a\nb\nc\nd", (3, 0), "vkkgc", "a\n-- b\n-- c\n-- d", (1, 0), None),
        ("a\nb\nc\nd", (1, 0), "Vjgc", "a\n-- b\n-- c\nd", (1, 0), None),
        ("abc\nbcd\ncde\nd", (1, 2), "Vjgc", "abc\n-- bcd\n-- cde\nd", (1, 0), None),
        ("abc\nbcd\ncde\nd", (2, 2), "Vkgc", "abc\n-- bcd\n-- cde\nd", (1, 2), None),
        ("abc\nbcd\ncde\nd", (0, 1), "vjlgc", "-- abc\n-- bcd\ncde\nd", (0, 1), None),
        ("abc\nbcd\ncde\nd", (0, 1), "<C-v>jjlgc", "-- abc\n-- bcd\n-- cde\nd", (0, 1), None),
        ("abc\nbcd\ncde\nd", (0, 1), "Vjgcj.", "-- abc\n-- -- bcd\n-- cde\nd", (1, 0), None),
        ("abc\nbcd\ncde\nd", (0, 1), "Vjgcu", "abc\nbcd\ncde\nd", (0, 0), None),
        (
            "\u{C120}\u{D0DD} 1;\n  \u{D55C}\u{AE00} b",
            (1, 3),
            "gcc",
            "\u{C120}\u{D0DD} 1;\n  -- \u{D55C}\u{AE00} b",
            (1, 2),
            None,
        ),
        (
            "\u{C120}\u{D0DD} 1;\n  \u{D55C}\u{AE00} b",
            (0, 1),
            "gcj",
            "-- \u{C120}\u{D0DD} 1;\n--   \u{D55C}\u{AE00} b",
            (0, 3),
            None,
        ),
        (
            "  -- \u{C120}\u{D0DD} 1;\n  --\u{D55C}\u{AE00} b",
            (0, 3),
            "gcj",
            "  \u{C120}\u{D0DD} 1;\n  \u{D55C}\u{AE00} b",
            (0, 2),
            None,
        ),
    ];
    check(CASES);
}

#[test]
fn toggle_comments_lines_in_and_out() {
    let lines = |s: &str| s.split('\n').map(str::to_string).collect::<Vec<_>>();
    assert_eq!(toggle(&lines("a\n  b")), lines("-- a\n--   b"));
    assert_eq!(toggle(&lines("-- a\n  --b")), lines("a\n  b"));
    // A line that is not a comment among comments: all of them get one more.
    assert_eq!(toggle(&lines("-- a\nb")), lines("-- -- a\n-- b"));
    // Only blank lines: they are left as they are.
    assert_eq!(toggle(&lines("  \n")), lines("  \n"));
    // `-- ` followed by blanks only: the blanks after it are left, without the indent.
    assert_eq!(toggle(&lines("  --  ")), lines(" "));
}

/// MySQL: lines commented with `-- ` (a blank one with `--` alone, which ends the line) come
/// back; `#` lines are comments too; `--x` (minus minus x) is code and gets commented.
#[test]
fn mysql_toggles_its_own_comments() {
    use datarig_core::sql::dialect::MySqlMode;
    let my = Dialect::MySql(MySqlMode::default());
    let lines = |s: &[&str]| s.iter().map(|l| l.to_string()).collect::<Vec<_>>();
    let code = lines(&["select 1;", "", "  select 2;"]);
    let commented = super::toggle(&code, my);
    assert_eq!(commented, lines(&["-- select 1;", "--", "--   select 2;"]));
    assert_eq!(super::toggle(&commented, my), code);
    assert_eq!(super::toggle(&lines(&["# a", "  -- b", "#c"]), my), lines(&["a", "  b", "c"]));
    assert_eq!(super::toggle(&lines(&["--x", "-- y"]), my), lines(&["-- --x", "-- -- y"]));
    // PostgreSQL strips `--x` and keeps `#`, as before.
    assert_eq!(toggle(&lines(&["--x", "-- y"])), lines(&["x", "y"]));
    assert_eq!(toggle(&lines(&["# a"])), lines(&["-- # a"]));
}
