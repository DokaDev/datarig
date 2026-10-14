use super::super::tests::{MarkCase, at, check_marks, typ};
use super::*;

/// Line numbers, `:s` with ranges, flags, Vim's replacement escapes, `:&`, `:&&`, `&`, `g&`,
/// empty matches and errors, as Neovim does them (`dev/vim-cases.py`; a pattern Vim writes
/// otherwise, `\(a\)` for `(a)`, was given to Neovim in its own notation). The context mark of
/// a jump by line number is checked apart: Vim sets it, Neovim does not.
#[test]
fn ex_commands_as_vim() {
    const CASES: &[MarkCase] = &[
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":3<CR>",
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (2, 0),
            &[],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":4<CR>",
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (3, 2),
            &[],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (2, 3),
            ":0<CR>",
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (2, 3),
            ":99<CR>",
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (4, 0),
            &[],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (2, 3),
            ":$<CR>",
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (4, 0),
            &[],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (2, 3),
            ":+<CR>",
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (3, 2),
            &[],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (2, 3),
            ":-2<CR>",
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (2, 3),
            ":.+2<CR>",
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (4, 0),
            &[],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (2, 3),
            ":$-1<CR>",
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (3, 2),
            &[],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 3),
            "jjmagg:'a<CR>",
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (2, 0),
            &[],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (2, 3),
            ":s/a/X/<CR>",
            "select a, b\n  from t\nwhere X = 1\n  and b = 2\norder by a",
            (2, 0),
            &[('\'', Some((2, 3)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (2, 3),
            ":s/a/X/g<CR>",
            "select a, b\n  from t\nwhere X = 1\n  and b = 2\norder by a",
            (2, 0),
            &[('\'', Some((2, 3)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (4, 3),
            ":%s/a/X/<CR>",
            "select X, b\n  from t\nwhere X = 1\n  Xnd b = 2\norder by X",
            (4, 0),
            &[('\'', Some((4, 3)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (4, 3),
            ":%s/a/X/g<CR>",
            "select X, b\n  from t\nwhere X = 1\n  Xnd b = 2\norder by X",
            (4, 0),
            &[('\'', Some((4, 3)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (4, 3),
            ":%s/a/X/g<CR>u",
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((4, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":2,4s/^/-- /<CR>",
            "select a, b\n--   from t\n-- where a = 1\n--   and b = 2\norder by a",
            (3, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":.,$s/$/;/<CR>",
            "select a, b;\n  from t;\nwhere a = 1;\n  and b = 2;\norder by a;",
            (4, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (1, 0),
            ":.,+2s/ /_/g<CR>",
            "select a, b\n__from_t\nwhere_a_=_1\n__and_b_=_2\norder by a",
            (3, 0),
            &[('\'', Some((1, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (1, 0),
            ":2;+1s/ /_/g<CR>",
            "select a, b\n__from_t\nwhere_a_=_1\n  and b = 2\norder by a",
            (2, 0),
            &[('\'', Some((1, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (1, 0),
            "Vj:s/t/T/g<CR>",
            "select a, b\n  from T\nwhere a = 1\n  and b = 2\norder by a",
            (1, 2),
            &[('\'', Some((1, 0))), ('<', Some((1, 0))), ('>', Some((2, usize::MAX)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (1, 0),
            "vjj<Esc>gg:'<,'>s/a/A/g<CR>",
            "select a, b\n  from t\nwhere A = 1\n  And b = 2\norder by a",
            (3, 2),
            &[('\'', Some((0, 0))), ('<', Some((1, 0))), ('>', Some((3, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            "jmajjmbgg:'a,'bs/=/==/<CR>",
            "select a, b\n  from t\nwhere a == 1\n  and b == 2\norder by a",
            (3, 2),
            &[('\'', Some((0, 0))), ('a', Some((1, 0))), ('b', Some((3, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            "3:s/ /+/<CR>",
            "select+a, b\n+ from t\nwhere+a = 1\n  and b = 2\norder by a",
            (2, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/zz/X/<CR>",
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":%s/zz/X/e<CR>",
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":%s/SELECT/S/i<CR>",
            "S a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "Select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/s/Z/gI<CR>",
            "Select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/a/[&]/g<CR>",
            "select [a], b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (2, 0),
            ":s/(a) = (1)/\\2 = \\1/<CR>",
            "select a, b\n  from t\nwhere 1 = a\n  and b = 2\norder by a",
            (2, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (2, 0),
            ":s/(a)/\\0\\1\\9/<CR>",
            "select a, b\n  from t\nwhere aa = 1\n  and b = 2\norder by a",
            (2, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (2, 0),
            ":s/a/$1/<CR>",
            "select a, b\n  from t\nwhere $1 = 1\n  and b = 2\norder by a",
            (2, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (3, 0),
            ":s/ and /\\rand /<CR>",
            "select a, b\n  from t\nwhere a = 1\n \nand b = 2\norder by a",
            (4, 0),
            &[('\'', Some((3, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/, /,\\r  /<CR>",
            "select a,\n  b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (1, 2),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/a/\\t/<CR>",
            "select \t, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/a/\\\\/<CR>",
            "select \\, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/a/\\&\\~/<CR>",
            "select &~, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/a/x/<CR>:s/b/~~/<CR>",
            "select x, xx\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/\\w+/\\u&/g<CR>",
            "Select A, B\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/\\w+/\\U&\\E!/<CR>",
            "SELECT! a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "SELECT A, B\n  from t",
            (0, 0),
            ":s/\\w+/\\L\\u&/g<CR>",
            "Select A, B\n  from t",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s#a#/#<CR>",
            "select /, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
        ("a/b/c\n  from t", (0, 0), ":s/\\//-/g<CR>", "a-b-c\n  from t", (0, 0), &[('\'', Some((0, 0)))]),
        ("a.b.c\n  from t", (0, 0), ":s.\\..-.g<CR>", "a-b-c\n  from t", (0, 0), &[('\'', Some((0, 0)))]),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/a<CR>",
            "select , b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/a/b<CR>",
            "select b, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            "/from<CR>:s//FROM/<CR>",
            "select a, b\n  FROM t\nwhere a = 1\n  and b = 2\norder by a",
            (1, 2),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/a/X/<CR>n",
            "select X, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (2, 6),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/ /_/<CR>jj&",
            "select_a, b\n  from t\nwhere_a = 1\n  and b = 2\norder by a",
            (2, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/ /_/g<CR>jj&",
            "select_a,_b\n  from t\nwhere_a_=_1\n  and b = 2\norder by a",
            (2, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/ /_/g<CR>jj:&&<CR>",
            "select_a,_b\n  from t\nwhere_a_=_1\n  and b = 2\norder by a",
            (2, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/ /_/g<CR>jj:s<CR>",
            "select_a,_b\n  from t\nwhere_a = 1\n  and b = 2\norder by a",
            (2, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/ /_/<CR>jjg&",
            "select_a,_b\n_ from t\nwhere_a = 1\n_ and b = 2\norder_by a",
            (4, 0),
            &[('\'', Some((2, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/ /_/<CR>jj&j.",
            "select_a, b\n  from t\nwhere_a = 1\n  and b = 2\norder by a",
            (3, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":s/ /_/<CR>:3,4&&<CR>",
            "select_a, b\n  from t\nwhere_a = 1\n_ and b = 2\norder by a",
            (3, 0),
            &[('\'', Some((0, 0)))],
        ),
        ("aaa\nabc\nb a", (0, 0), ":%s/a*/-/g<CR>", "-\n-b-c\n-b- -", (2, 0), &[('\'', Some((0, 0)))]),
        ("aaa\nabc\nb a", (0, 0), ":%s/x*/-/g<CR>", "-a-a-a\n-a-b-c\n-b- -a", (2, 0), &[('\'', Some((0, 0)))]),
        ("aaa\nabc\nb a", (0, 0), ":%s/$/;/g<CR>", "aaa;\nabc;\nb a;", (2, 0), &[('\'', Some((0, 0)))]),
        ("aaa\nabc\nb a", (0, 0), ":%s/^/> /<CR>", "> aaa\n> abc\n> b a", (2, 0), &[('\'', Some((0, 0)))]),
        (
            "\u{C120}\u{D0DD} a\n  \u{D55C}\u{AE00} b",
            (0, 0),
            ":%s/\u{D55C}\u{AE00}/\u{AE00}/<CR>",
            "\u{C120}\u{D0DD} a\n  \u{AE00} b",
            (1, 2),
            &[('\'', Some((0, 0)))],
        ),
        (
            "\u{C120}\u{D0DD} a\n  \u{D55C}\u{AE00} b",
            (1, 0),
            ":s/[\u{AC00}-\u{D7A3}]+/<&>/g<CR>",
            "\u{C120}\u{D0DD} a\n  <\u{D55C}\u{AE00}> b",
            (1, 2),
            &[('\'', Some((1, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (1, 0),
            "majjmb:%s/ = / /<CR>",
            "select a, b\n  from t\nwhere a 1\n  and b 2\norder by a",
            (3, 2),
            &[('\'', Some((3, 0))), ('a', Some((1, 0))), ('b', Some((3, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":1,9s/a/b/<CR>",
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":3,1s/a/b/<CR>",
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            ":'z,$s/a/b/<CR>",
            "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a",
            (0, 0),
            &[('\'', Some((0, 0)))],
        ),
    ];
    check_marks(CASES);
}

/// `:{n}` is a jump: the place it left from becomes the context mark, as in Vim 9.1 (Neovim
/// leaves it), unless the cursor did not move.
#[test]
fn a_line_number_sets_the_context_mark() {
    let text = "select a, b\n  from t\nwhere a = 1\n  and b = 2\norder by a";
    for (cmd, cursor) in [("0", (0, 0)), ("4", (3, 2)), ("$", (4, 0))] {
        let mut e = at(text, (2, 3));
        assert_eq!(e.ex(cmd), Ok(ExDone::Moved));
        assert_eq!(((e.row, e.col), e.marks.get('\'')), (cursor, Ok((2, 3))), "{cmd}");
    }
    let mut e = at(text, (2, 3));
    typ(&mut e, ":4<CR>:4<CR>");
    assert_eq!(e.marks.get('\''), Ok((2, 3)));
}

/// What each command says when it cannot run, and that nothing changed then.
#[test]
fn ex_errors() {
    let text = "select a\nfrom t";
    let cases: &[(&str, ExError)] = &[
        ("1,9s/a/b/", ExError::InvalidRange),
        ("-5", ExError::InvalidRange),
        ("2,1s/a/b/", ExError::Backwards),
        ("'z,$s/a/b/", ExError::Mark(MarkNotice::NotSet('z'))),
        ("'A", ExError::Mark(MarkNotice::Unknown('A'))),
        ("'<,'>s/a/b/", ExError::Mark(MarkNotice::NotSet('<'))),
        ("s/a/b/c", ExError::Confirm),
        ("s/a/b/gx", ExError::Trailing("x".into())),
        ("s/a/b/ 3", ExError::Trailing("3".into())),
        ("s/a/b/ g", ExError::Trailing("g".into())),
        ("s/(/b/", ExError::Pattern(SearchNotice::Invalid("unclosed group".into()))),
        ("s/zz/b/", ExError::Pattern(SearchNotice::NotFound("zz".into()))),
        ("s//b/", ExError::Pattern(SearchNotice::NoPrevious)),
        ("s", ExError::Pattern(SearchNotice::NoPrevious)),
        ("&&", ExError::Pattern(SearchNotice::NoPrevious)),
        ("d", ExError::Unsupported("d".into())),
        ("3d", ExError::Unsupported("d".into())),
        ("%normal x", ExError::Unsupported("normal x".into())),
    ];
    for (cmd, want) in cases {
        let mut e = at(text, (1, 2));
        assert_eq!(e.ex(cmd).as_ref(), Err(want), "{cmd}");
        assert_eq!((e.text(), (e.row, e.col)), (text.to_string(), (1, 2)), "{cmd}");
    }
    // `&` and `g&` before any `:s` say there is no pattern; `3&` past the end, that the range
    // is.
    let mut e = at(text, (1, 2));
    typ(&mut e, "&");
    assert_eq!(e.take_ex_notice(), Some(ExError::Pattern(SearchNotice::NoPrevious)));
    typ(&mut e, "g&");
    assert_eq!(e.take_ex_notice(), Some(ExError::Pattern(SearchNotice::NoPrevious)));
    assert_eq!(e.ex("s/a/b/e"), Ok(ExDone::Nothing));
    typ(&mut e, "3&");
    assert_eq!(e.take_ex_notice(), Some(ExError::InvalidRange));
}

/// Which text the app hands to the editor.
#[test]
fn what_is_an_editor_command() {
    for t in [
        "12",
        " 3",
        ".",
        "$",
        "%s/a/b/",
        "'<,'>s/a/b/",
        "+2",
        "-",
        ",5",
        ";",
        "&",
        "&&",
        "s",
        "s/a/b/",
        "su/a/b",
        "substitute/a/b/",
        "s#a#b#",
        "s /a/b/",
    ] {
        assert!(is_ex(t), "{t}");
    }
    for t in ["", "set", "settings", "sx", "select", "noh", "w", "conn local", "run", "q"] {
        assert!(!is_ex(t), "{t}");
    }
}

/// One `:s` over many lines is one splice and one undo step; the bytes it searched are counted
/// once per line in the range.
#[test]
fn substitute_is_one_step_and_counts_its_work() {
    let text = (0..1000).map(|i| format!("select {i} from t")).collect::<Vec<_>>().join("\n");
    let mut e = Editor::new(&text);
    e.take_search_work();
    assert_eq!(e.ex("%s/t$/u/"), Ok(ExDone::Substituted { count: 1000, lines: 1000 }));
    assert_eq!(e.last_step_changes(), 1);
    assert_eq!(e.take_search_work().bytes, text.len() + 1);
    typ(&mut e, "u");
    assert_eq!(e.text(), text);
    // A huge replacement or count never panics.
    assert_eq!(e.ex("99999999999999999999999"), Ok(ExDone::Moved));
    assert_eq!(e.row, 999);
    assert!(e.ex("s/./\\u\\U\\L\\E\\9\\/g").is_ok());
}

/// Found in review, each against Neovim: the `&` flag, an empty match next to the end of a
/// line, marks on and between lines a replacement splits, a charwise put of a line break at
/// the start of a line, `'<` `'>` after the undo of a Visual operator, `'.` after a redo,
/// after `>` and after block operators, `N&`, `a;b` moving the cursor, `,b`, and `gc` over
/// blank lines.
#[test]
fn review_regressions() {
    const CASES: &[MarkCase] = &[
        ("a a\na a", (0, 0), ":s/a/X/g<CR>j:s/a/Y/&<CR>", "X X\nY Y", (1, 0), &[('\'', Some((0, 0)))]),
        ("a a\na a", (0, 0), ":s/a/X/g<CR>j:s/a/Y/&g<CR>", "X X\nY a", (1, 0), &[('\'', Some((0, 0)))]),
        ("a a\na a", (0, 0), ":s/a/X/gi<CR>j:s/A/Y/&<CR>", "X X\nY Y", (1, 0), &[('\'', Some((0, 0)))]),
        (
            "xyz\nabc\naXbXc\na,b\nxyzw\nxéyz",
            (0, 0),
            ":%s/y*/-/g<CR>",
            "-x-z\n-a-b-c\n-a-X-b-X-c\n-a-,-b\n-x-z-w\n-x-é-z",
            (5, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "xyz\nabc\naXbXc\na,b\nxyzw",
            (0, 0),
            ":%s/[bX,z]*/-/g<CR>",
            "-x-y-\n-a-c\n-a-c\n-a-\n-x-y-w",
            (4, 0),
            &[('\'', Some((0, 0)))],
        ),
        (
            "a\nb\na\nd",
            (0, 0),
            "jmx:%s/a/1\\r2/<CR>",
            "1\n2\nb\n1\n2\nd",
            (4, 0),
            &[('x', Some((2, 0))), ('\'', Some((2, 0)))],
        ),
        (
            "a\nb\na\nd\na",
            (0, 0),
            "jmxjjmy:%s/a/[\\r]/g<CR>",
            "[\n]\nb\n[\n]\nd\n[\n]",
            (7, 0),
            &[('x', Some((2, 0))), ('y', Some((5, 0))), ('\'', Some((5, 0)))],
        ),
        ("a,\nb", (0, 0), "lmx:1s/,$/\\r/<CR>", "a\n\nb", (1, 0), &[('x', Some((0, 1))), ('\'', Some((0, 1)))]),
        (
            "l0 x\nl1 x\n  l2 x\nl3 x\nl4 x",
            (0, 0),
            "v$yjjmaPgg",
            "l0 x\nl1 x\nl0 x\n  l2 x\nl3 x\nl4 x",
            (0, 0),
            &[('a', Some((2, 0)))],
        ),
        (
            "l0 x\nl1 x\n  l2 x\nl3 x\nl4 x",
            (2, 3),
            "vj<Esc>Gvkdugg",
            "l0 x\nl1 x\n  l2 x\nl3 x\nl4 x",
            (0, 0),
            &[('<', Some((3, 0))), ('>', Some((4, 0)))],
        ),
        (
            "l0 x\nl1 x\n  l2 x\nl3 x\nl4 x",
            (2, 3),
            "maVjdugg",
            "l0 x\nl1 x\n  l2 x\nl3 x\nl4 x",
            (0, 0),
            &[('a', Some((2, 3))), ('<', Some((2, 0))), ('>', Some((3, usize::MAX)))],
        ),
        (
            "l0 x\nl1 x\n  l2 x\nl3 x\nl4 x",
            (2, 3),
            "maddu<C-r>gg",
            "l0 x\nl1 x\nl3 x\nl4 x",
            (0, 0),
            &[('a', None), ('.', Some((2, 0)))],
        ),
        (
            "l0 x\nl1 x\n  l2 x\nl3 x\nl4 x",
            (2, 3),
            "makd2ju<C-r>gg",
            "l0 x\nl4 x",
            (0, 0),
            &[('a', None), ('.', Some((1, 0)))],
        ),
        (
            "l0 a\nl1 a\n  l2 a\nl3 a\nl4 a",
            (4, 3),
            ":%s/a/X/g<CR>u<C-r>gg",
            "l0 X\nl1 X\n  l2 X\nl3 X\nl4 X",
            (0, 0),
            &[('.', Some((4, 0)))],
        ),
        ("a\nb\nc", (0, 0), ":s/$/!/<CR>3&", "a!!\nb!\nc!", (2, 0), &[('\'', Some((0, 0)))]),
        (
            "l0 x\nl1 x\n  l2 x\nl3 x\nl4 x",
            (2, 3),
            "3>>",
            "l0 x\nl1 x\n      l2 x\n    l3 x\n    l4 x",
            (2, 6),
            &[('.', Some((2, 0)))],
        ),
        (
            "l0 x\nl1 x\n  l2 x\nl3 x\nl4 x",
            (2, 3),
            ">j",
            "l0 x\nl1 x\n      l2 x\n    l3 x\nl4 x",
            (2, 6),
            &[('.', Some((2, 0)))],
        ),
        (
            "l0 x\nl1 x\n  l2 x\nl3 x\nl4 x",
            (2, 3),
            "Vj<",
            "l0 x\nl1 x\nl2 x\nl3 x\nl4 x",
            (2, 0),
            &[('.', Some((2, 0)))],
        ),
        (
            "l0 x\nl1 x\n  l2 x\nl3 x\nl4 x",
            (2, 3),
            "<C-v>jjd",
            "l0 x\nl1 x\n  l x\nl3 \nl4 ",
            (2, 3),
            &[('.', Some((2, 3)))],
        ),
        (
            "l0 x\nl1 x\n  l2 x\nl3 x\nl4 x",
            (2, 3),
            "<C-v>jjcZ<Esc>",
            "l0 x\nl1 x\n  lZ x\nl3 Z\nl4 Z",
            (2, 3),
            &[('.', Some((3, 0)))],
        ),
        (
            "l0 x\nl1 x\n  l2 x\nl3 x\nl4 x",
            (2, 3),
            "<C-v>jjrX",
            "l0 x\nl1 x\n  lX x\nl3 X\nl4 X",
            (2, 3),
            &[('.', Some((2, 3)))],
        ),
        (
            "l0 x\nl1 x\n  l2 x\nl3 x\nl4 x",
            (2, 3),
            "$a<CR><Esc>",
            "l0 x\nl1 x\n  l2 x\n\nl3 x\nl4 x",
            (3, 0),
            &[('.', Some((3, 0)))],
        ),
        (
            "l0 x\nl1 x\n  l2 x\nl3 x\nl4 x",
            (0, 0),
            ":2;+1s/$/!/<CR>",
            "l0 x\nl1 x!\n  l2 x!\nl3 x\nl4 x",
            (2, 2),
            &[('\'', Some((1, 0)))],
        ),
        (
            "l0 x\nl1 x\n  l2 x\nl3 x\nl4 x",
            (1, 0),
            ":,3s/x/y/<CR>",
            "l0 x\nl1 y\n  l2 y\nl3 x\nl4 x",
            (2, 2),
            &[('\'', Some((1, 0)))],
        ),
        ("  \n\t", (0, 0), "gcG", "  \n\t", (0, 0), &[('.', Some((0, 0)))]),
        (
            "l0 x\nl1 x\n  l2 x\nl3 x\nl4 x",
            (1, 0),
            "$a<CR><Esc>",
            "l0 x\nl1 x\n\n  l2 x\nl3 x\nl4 x",
            (2, 0),
            &[('.', Some((1, 4)))],
        ),
    ];
    check_marks(CASES);
}
