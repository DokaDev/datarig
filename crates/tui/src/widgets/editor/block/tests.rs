use super::super::registers::tests::{RegCase, check};
use super::super::tests::{at, ctrl, typ};
use super::*;
use datarig_core::i18n::Label;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// Visual mode by block: `y d x X D Y r ~ u U gu gU g~ > < J gJ S R`, short and empty lines,
/// `$`, motions, counts, `o` and `O`, and switching between `v`, `V` and `Ctrl+V`, as
/// Neovim does them (rows from `dev/vim-cases.py`).
#[test]
fn block_operators_as_vim() {
    const CASES: &[RegCase] = &[
        ("abcd\nab\nabcdef", (0, 1), &[], "<C-v>jjly", "abcd\nab\nabcdef", (0, 1), &[('"', Some(("bc\nb\nbc", 'b')))]),
        ("abcd\n\nabcdef", (0, 2), &[], "<C-v>jjly", "abcd\n\nabcdef", (0, 2), &[('"', Some(("cd\n  \ncd", 'b')))]),
        (
            "abcdef\nabc\nabcdef",
            (0, 3),
            &[],
            "<C-v>jj$y",
            "abcdef\nabc\nabcdef",
            (0, 3),
            &[('"', Some(("def\n\ndef", 'b')))],
        ),
        ("abcdef\nab", (0, 3), &[], "<C-v>j$y", "abcdef\nab", (0, 2), &[('"', Some(("cdef\n", 'b')))]),
        (
            "abcd\nab\nabcdef",
            (0, 1),
            &[],
            "<C-v>jj$y",
            "abcd\nab\nabcdef",
            (0, 1),
            &[('"', Some(("bcd\nb\nbcdef", 'b')))],
        ),
        ("abcdef\nab", (0, 3), &[], "<C-v>j$d", "ab\nab", (0, 1), &[('"', Some(("cdef\n", 'b')))]),
        ("abcd\nabcd\nabcdef", (0, 1), &[], "<C-v>jjld", "ad\nad\nadef", (0, 1), &[('"', Some(("bc\nbc\nbc", 'b')))]),
        ("abcd\nabcd\nabcdef", (0, 1), &[], "<C-v>jjlD", "a\na\na", (0, 0), &[('"', Some(("bcd\nbcd\nbcdef", 'b')))]),
        ("abcd\nabcd\nabcdef", (0, 1), &[], "<C-v>jjlX", "ad\nad\nadef", (0, 1), &[('"', Some(("bc\nbc\nbc", 'b')))]),
        (
            "abcd\nabcd\nabcdef",
            (0, 1),
            &[],
            "<C-v>jjlY",
            "abcd\nabcd\nabcdef",
            (0, 1),
            &[('"', Some(("bc\nbc\nbc", 'b')))],
        ),
        (
            "abcd\nabcd\nabcdef\nab",
            (0, 1),
            &[],
            "<C-v>3jlx",
            "ad\nad\nadef\na",
            (0, 1),
            &[('"', Some(("bc\nbc\nbc\nb", 'b')))],
        ),
        ("abcdef\n\nabcdef", (0, 3), &[], "<C-v>jjd", "abcef\n\nabcef", (0, 3), &[('"', Some(("d\n \nd", 'b')))]),
        ("abcdef\nabcdef", (0, 4), &[], "<C-v>j0d", "f\nf", (0, 0), &[('"', Some(("abcde\nabcde", 'b')))]),
        ("abcdef\nabcdef", (0, 4), &[], "<C-v>jbd", "f\nf", (0, 0), &[('"', Some(("abcde\nabcde", 'b')))]),
        ("abcdef\nabcdef", (0, 4), &[], "<C-v>jwd", "abcd\nabcd", (0, 3), &[('"', Some(("ef\nef", 'b')))]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>j2d", "acdef\nacdef", (0, 1), &[('"', Some(("b\nb", 'b')))]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>j2y", "abcdef\nabcdef", (0, 1), &[('"', Some(("b\nb", 'b')))]),
        ("abcd\nab\nabcdef", (0, 1), &[], "<C-v>jjlrX", "aXXd\naX\naXXdef", (0, 1), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>j2rX", "aXcdef\naXcdef", (0, 1), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jlr<CR>", "a\ndef\na\ndef", (0, 0), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jr<Tab>", "a\tcdef\na\tcdef", (0, 1), &[('"', None)]),
        ("abcd\nab\nabcdef", (0, 1), &[], "<C-v>jjl~", "aBCd\naB\naBCdef", (0, 1), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>j2~", "aBcdef\naBcdef", (0, 1), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jlu", "abcdef\nabcdef", (0, 1), &[('"', None)]),
        ("ABCDEF\nABCDEF", (0, 1), &[], "<C-v>jlu", "AbcDEF\nAbcDEF", (0, 1), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jlU", "aBCdef\naBCdef", (0, 1), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jlgU", "aBCdef\naBCdef", (0, 1), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jlg~", "aBCdef\naBCdef", (0, 1), &[('"', None)]),
        ("ABCDEF\nABCDEF", (0, 1), &[], "<C-v>jlgu", "AbcDEF\nAbcDEF", (0, 1), &[('"', None)]),
        ("abcd\nab\nabcdef", (0, 1), &[], "<C-v>jjl>", "a    bcd\na    b\na    bcdef", (0, 1), &[('"', None)]),
        (
            "abcd\nab\nabcdef",
            (0, 1),
            &[],
            "<C-v>jjl2>",
            "a        bcd\na        b\na        bcdef",
            (0, 1),
            &[('"', None)],
        ),
        ("abcdef\n\nabcdef", (0, 3), &[], "<C-v>jj>", "abc    def\n\nabc    def", (0, 3), &[('"', None)]),
        ("abcdef\nabc\nabcdef", (0, 3), &[], "<C-v>jj>", "abc    def\nabc    \nabc    def", (0, 3), &[('"', None)]),
        (
            "abcdef\nabc\nabcdef",
            (0, 3),
            &[],
            "<C-v>jj3>",
            "abc            def\nabc            \nabc            def",
            (0, 3),
            &[('"', None)],
        ),
        (
            "x  abcd\nx    ab\nx      abcdef",
            (0, 3),
            &[],
            "<C-v>jj<",
            "x  abcd\nx  ab\nx  abcdef",
            (0, 3),
            &[('"', None)],
        ),
        ("    abc\n    abc\n    abc", (0, 2), &[], "<C-v>jj<", "  abc\n  abc\n  abc", (0, 2), &[('"', None)]),
        ("    abc\n    abc\n    abc", (0, 2), &[], "<C-v>jj2<", "  abc\n  abc\n  abc", (0, 2), &[('"', None)]),
        ("abcd\nabcd\nabcdef", (0, 1), &[], "<C-v>jjlJ", "abcd abcd abcdef", (0, 9), &[('"', None)]),
        ("abcdef\nab\nabcdef", (0, 4), &[], "<C-v>jjhJ", "abcdef ab abcdef", (0, 9), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jlgJ", "abcdefabcdef", (0, 6), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jlS<Esc>", "", (0, 0), &[('"', Some(("abcdef\nabcdef", 'V')))]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jlR<Esc>", "", (0, 0), &[('"', Some(("abcdef\nabcdef", 'V')))]),
        ("abcd\nabcd\nabcdef", (0, 1), &[], "<C-v>jlOd", "ad\nad\nabcdef", (0, 1), &[('"', Some(("bc\nbc", 'b')))]),
        ("abcd\nabcd\nabcdef", (0, 1), &[], "<C-v>jlOOd", "ad\nad\nabcdef", (0, 1), &[('"', Some(("bc\nbc", 'b')))]),
        ("abcd\nabcd\nabcdef", (0, 1), &[], "<C-v>jlod", "ad\nad\nabcdef", (0, 1), &[('"', Some(("bc\nbc", 'b')))]),
        (
            "abcdef\nabcdef\nabcdef",
            (0, 3),
            &[],
            "<C-v>jhOjd",
            "abef\nabef\nabef",
            (0, 2),
            &[('"', Some(("cd\ncd\ncd", 'b')))],
        ),
        (
            "abcdef\nabcdef\nabcdef",
            (0, 3),
            &[],
            "<C-v>jhojd",
            "abcdef\nabef\nabcdef",
            (1, 2),
            &[('"', Some(("cd", 'b')))],
        ),
        (
            "abcdef\nabcdef\nabcdef",
            (0, 3),
            &[],
            "<C-v>jhOOjd",
            "abef\nabef\nabef",
            (0, 2),
            &[('"', Some(("cd\ncd\ncd", 'b')))],
        ),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jlv<Esc>", "abcdef\nabcdef", (1, 2), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jlvd", "adef", (0, 1), &[('"', Some(("bcdef\nabc", 'v')))]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jlVd", "", (0, 0), &[('"', Some(("abcdef\nabcdef", 'V')))]),
        ("abcdef\nabcdef", (0, 1), &[], "vj<C-v>d", "acdef\nacdef", (0, 1), &[('"', Some(("b\nb", 'b')))]),
        ("abcdef\nabcdef", (0, 1), &[], "Vj<C-v>d", "acdef\nacdef", (0, 1), &[('"', Some(("b\nb", 'b')))]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>j<C-v>", "abcdef\nabcdef", (1, 1), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jl<C-v>x", "abcdef\nabdef", (1, 2), &[('"', Some(("c", 'v')))]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jl<Esc>x", "abcdef\nabdef", (1, 2), &[('"', Some(("c", 'v')))]),
        ("abcdef\nabcdef", (0, 4), &[], "<C-v>jh<Esc>x", "abcdef\nabcef", (1, 3), &[('"', Some(("d", 'v')))]),
        ("abcdef\nabcdef", (0, 4), &[], "<C-v>j$<Esc>x", "abcdef\nabcde", (1, 4), &[('"', Some(("f", 'v')))]),
        ("abc def\nabcdef", (0, 1), &[], "<C-v>jiwd", "af\na", (0, 1), &[('"', Some(("bc de\nbcdef", 'b')))]),
    ];
    check(CASES);
}

/// Blocks over wide characters (CJK, Hangul) and tabs: a character partly inside is blanks for
/// `y`, goes whole with blanks left for `d` `c` `r`, and `I` `A` `>` put their text and
/// blanks around it as Neovim does. (A tab here is always 4 columns wide, so the rows keep
/// tabs where Vim's tab stops agree: at the start of a line or after 4 columns.)
#[test]
fn blocks_over_wide_characters_and_tabs() {
    const CASES: &[RegCase] = &[
        ("ab漢cd\nabcdef", (0, 3), &[], "<C-v>jy", "ab漢cd\nabcdef", (0, 3), &[('"', Some(("c\ne", 'b')))]),
        ("ab漢cd\nabcdef", (1, 3), &[], "<C-v>ky", "ab漢cd\nabcdef", (0, 2), &[('"', Some(("漢\ncd", 'b')))]),
        ("ab漢cd\nabcdef", (1, 3), &[], "<C-v>kd", "abcd\nabef", (0, 2), &[('"', Some(("漢\ncd", 'b')))]),
        ("ab漢cd\nabcdef", (1, 2), &[], "<C-v>kd", "abcd\nabef", (0, 2), &[('"', Some(("漢\ncd", 'b')))]),
        ("ab漢cd\nabcdef", (1, 3), &[], "<C-v>kIX<Esc>", "abX漢cd\nabXcdef", (0, 2), &[('"', None)]),
        ("ab漢cd\nabcdef", (1, 3), &[], "<C-v>kAX<Esc>", "ab漢Xcd\nabcdXef", (0, 2), &[('"', None)]),
        ("ab漢cd\nabcdef", (1, 2), &[], "<C-v>kAX<Esc>", "ab漢Xcd\nabcdXef", (0, 2), &[('"', None)]),
        ("ab漢cd\nabcdef", (1, 3), &[], "<C-v>krx", "abxxcd\nabxxef", (0, 2), &[('"', None)]),
        ("ab漢cd\nabcdef", (1, 3), &[], "<C-v>kcX<Esc>", "abXcd\nabXef", (0, 2), &[('"', Some(("漢\ncd", 'b')))]),
        (
            "abcdef\nab漢cd\nabcdef",
            (0, 3),
            &[],
            "<C-v>jjy",
            "abcdef\nab漢cd\nabcdef",
            (0, 3),
            &[('"', Some(("d\n \nd", 'b')))],
        ),
        (
            "abcdef\nab漢cd\nabcdef",
            (0, 3),
            &[],
            "<C-v>jjd",
            "abcef\nab cd\nabcef",
            (0, 3),
            &[('"', Some(("d\n \nd", 'b')))],
        ),
        (
            "abcdef\nab漢cd\nabcdef",
            (0, 3),
            &[],
            "<C-v>jjld",
            "abcf\nab d\nabcf",
            (0, 3),
            &[('"', Some(("de\n c\nde", 'b')))],
        ),
        (
            "abcdef\nab漢cd\nabcdef",
            (0, 2),
            &[],
            "<C-v>jjd",
            "abdef\nab cd\nabdef",
            (0, 2),
            &[('"', Some(("c\n \nc", 'b')))],
        ),
        ("abcdef\nab漢cd\nabcdef", (0, 3), &[], "<C-v>jjIX<Esc>", "abcXdef\nab X漢cd\nabcXdef", (0, 3), &[('"', None)]),
        ("abcdef\nab漢cd\nabcdef", (0, 2), &[], "<C-v>jjAX<Esc>", "abcXdef\nab X漢cd\nabcXdef", (0, 2), &[('"', None)]),
        ("abcdef\nab漢cd\nabcdef", (0, 3), &[], "<C-v>jjAX<Esc>", "abcdXef\nab漢Xcd\nabcdXef", (0, 3), &[('"', None)]),
        (
            "abcdef\nab漢cd\nabcdef",
            (0, 3),
            &[],
            "<C-v>jjcX<Esc>",
            "abcXef\nab Xcd\nabcXef",
            (0, 3),
            &[('"', Some(("d\n \nd", 'b')))],
        ),
        ("abcdef\nab漢cd\nabcdef", (0, 3), &[], "<C-v>jjrX", "abcXef\nab Xcd\nabcXef", (0, 3), &[('"', None)]),
        ("abcdef\nab漢cd\nabcdef", (0, 2), &[], "<C-v>jjrX", "abXdef\nabX cd\nabXdef", (0, 2), &[('"', None)]),
        ("abcdef\nab漢cd\nabcdef", (0, 1), &[], "<C-v>jjlr一", "a一def\na一 cd\na一def", (0, 1), &[('"', None)]),
        ("abcdef\nab漢cd\nabcdef", (0, 1), &[], "<C-v>jjllr一", "a一 ef\na一 cd\na一 ef", (0, 1), &[('"', None)]),
        ("abcdef\nab漢cd\nabcdef", (0, 3), &[], "<C-v>jj~", "abcDef\nab漢cd\nabcDef", (0, 3), &[('"', None)]),
        ("abcdef\nab漢cd\nabcdef", (0, 3), &[], "<C-v>jjU", "abcDef\nab漢cd\nabcDef", (0, 3), &[('"', None)]),
        (
            "abcdef\nab漢cd\nabcdef",
            (0, 3),
            &[],
            "<C-v>jj>",
            "abc    def\nab    漢cd\nabc    def",
            (0, 3),
            &[('"', None)],
        ),
        (
            "abcdef\nab漢cd\nabcdef",
            (0, 3),
            &[],
            "<C-v>jj$d",
            "abc\nab \nabc",
            (0, 2),
            &[('"', Some(("def\n cd\ndef", 'b')))],
        ),
        (
            "abcdef\nab漢cd\nabcdef",
            (0, 3),
            &[],
            "<C-v>jj$IX<Esc>",
            "abcXdef\nab X漢cd\nabcXdef",
            (0, 3),
            &[('"', None)],
        ),
        (
            "\u{AC00}\u{B098}\u{B2E4}\nabcdef\n\u{AC00}\u{B098}\u{B2E4}",
            (0, 1),
            &[],
            "<C-v>jjy",
            "\u{AC00}\u{B098}\u{B2E4}\nabcdef\n\u{AC00}\u{B098}\u{B2E4}",
            (0, 1),
            &[('"', Some(("\u{B098}\ncd\n\u{B098}", 'b')))],
        ),
        (
            "\u{AC00}\u{B098}\u{B2E4}\nabcdef\n\u{AC00}\u{B098}\u{B2E4}",
            (1, 1),
            &[],
            "<C-v>jly",
            "\u{AC00}\u{B098}\u{B2E4}\nabcdef\n\u{AC00}\u{B098}\u{B2E4}",
            (1, 1),
            &[('"', Some(("bcd\n \u{B098}", 'b')))],
        ),
        (
            "\u{AC00}\u{B098}\u{B2E4}\nabcdef\n\u{AC00}\u{B098}\u{B2E4}",
            (1, 1),
            &[],
            "<C-v>jld",
            "\u{AC00}\u{B098}\u{B2E4}\naef\n \u{B2E4}",
            (1, 1),
            &[('"', Some(("bcd\n \u{B098}", 'b')))],
        ),
        (
            "\u{AC00}\u{B098}\u{B2E4}\nabcdef\n\u{AC00}\u{B098}\u{B2E4}",
            (1, 1),
            &[],
            "<C-v>kjjd",
            "\u{AC00}\u{B098}\u{B2E4}\ncdef\n\u{B098}\u{B2E4}",
            (1, 0),
            &[('"', Some(("ab\n\u{AC00}", 'b')))],
        ),
        (
            "\u{AC00}\u{B098}\u{B2E4}\nabcdef\n\u{AC00}\u{B098}\u{B2E4}",
            (1, 1),
            &[],
            "<C-v>kjjy",
            "\u{AC00}\u{B098}\u{B2E4}\nabcdef\n\u{AC00}\u{B098}\u{B2E4}",
            (1, 0),
            &[('"', Some(("ab\n\u{AC00}", 'b')))],
        ),
        (
            "\u{AC00}\u{B098}\u{B2E4}\nabcdef\n\u{AC00}\u{B098}\u{B2E4}",
            (1, 1),
            &[],
            "<C-v>kjjcX<Esc>",
            "\u{AC00}\u{B098}\u{B2E4}\nXcdef\nX\u{B098}\u{B2E4}",
            (1, 0),
            &[('"', Some(("ab\n\u{AC00}", 'b')))],
        ),
        (
            "\u{AC00}\u{B098}\u{B2E4}\nabcdef\n\u{AC00}\u{B098}\u{B2E4}",
            (1, 1),
            &[],
            "<C-v>kjjIX<Esc>",
            "\u{AC00}\u{B098}\u{B2E4}\nXabcdef\nX\u{AC00}\u{B098}\u{B2E4}",
            (1, 0),
            &[('"', None)],
        ),
        (
            "\u{AC00}\u{B098}\u{B2E4}\nabcdef\n\u{AC00}\u{B098}\u{B2E4}",
            (1, 1),
            &[],
            "<C-v>kjjAX<Esc>",
            "\u{AC00}\u{B098}\u{B2E4}\nabXcdef\n\u{AC00}X\u{B098}\u{B2E4}",
            (1, 0),
            &[('"', None)],
        ),
        (
            "\u{AC00}\u{B098}\u{B2E4}\nabcdef\n\u{AC00}\u{B098}\u{B2E4}",
            (1, 1),
            &[],
            "<C-v>kjjrx",
            "\u{AC00}\u{B098}\u{B2E4}\nxxcdef\nxx\u{B098}\u{B2E4}",
            (1, 0),
            &[('"', None)],
        ),
        (
            "\u{AC00}\u{B098}\u{B2E4}\nabcdef\n\u{AC00}\u{B098}\u{B2E4}",
            (1, 1),
            &[],
            "<C-v>kjj>",
            "\u{AC00}\u{B098}\u{B2E4}\n    abcdef\n    \u{AC00}\u{B098}\u{B2E4}",
            (1, 0),
            &[('"', None)],
        ),
        (
            "\u{AC00}\u{B098}\u{B2E4}\nabcdef\n\u{AC00}\u{B098}\u{B2E4}",
            (0, 1),
            &[],
            "<C-v>jj$y",
            "\u{AC00}\u{B098}\u{B2E4}\nabcdef\n\u{AC00}\u{B098}\u{B2E4}",
            (0, 1),
            &[('"', Some(("\u{B098}\u{B2E4}\ncdef\n\u{B098}\u{B2E4}", 'b')))],
        ),
        (
            "\u{AC00}\u{B098}\u{B2E4}\nabcdef\n\u{AC00}\u{B098}\u{B2E4}",
            (0, 1),
            &[],
            "<C-v>jj$AX<Esc>",
            "\u{AC00}\u{B098}\u{B2E4}X\nabcdefX\n\u{AC00}\u{B098}\u{B2E4}X",
            (0, 1),
            &[('"', None)],
        ),
        (
            "\tabc\n\tabc\n\tabc",
            (0, 1),
            &[],
            "<C-v>jjly",
            "\tabc\n\tabc\n\tabc",
            (0, 1),
            &[('"', Some(("ab\nab\nab", 'b')))],
        ),
        ("\tabc\n\tabc\n\tabc", (0, 0), &[], "<C-v>jjd", "abc\nabc\nabc", (0, 0), &[('"', Some(("\t\n\t\n\t", 'b')))]),
        (
            "\tabc\n\tabc\n\tabc",
            (0, 0),
            &[],
            "<C-v>jjly",
            "\tabc\n\tabc\n\tabc",
            (0, 0),
            &[('"', Some(("\ta\n\ta\n\ta", 'b')))],
        ),
        ("\tabc\n\tabc\n\tabc", (0, 1), &[], "<C-v>jjIX<Esc>", "\tXabc\n\tXabc\n\tXabc", (0, 1), &[('"', None)]),
        ("\tabc\n\tabc\n\tabc", (0, 1), &[], "<C-v>jj<", "\tabc\n\tabc\n\tabc", (0, 1), &[('"', None)]),
        ("\tabc\n\tabc\n\tabc", (0, 0), &[], "<C-v>jjAX<Esc>", "\tXabc\n\tXabc\n\tXabc", (0, 0), &[('"', None)]),
        ("abcd\tcd\nabcdefgh", (0, 4), &[], "<C-v>jy", "abcd\tcd\nabcdefgh", (0, 4), &[('"', Some(("\t\nefgh", 'b')))]),
        ("abcd\tcd\nabcdefgh", (1, 5), &[], "<C-v>ky", "abcd\tcd\nabcdefgh", (0, 4), &[('"', Some(("\t\nefgh", 'b')))]),
        ("abcd\tcd\nabcdefgh", (1, 5), &[], "<C-v>kd", "abcdcd\nabcd", (0, 4), &[('"', Some(("\t\nefgh", 'b')))]),
    ];
    check(CASES);
}

/// `I` `A` `c` `s` `C`: what Insert mode types goes on every line of the block when it ends
/// (`I` and `c` leave out lines that end before the block, `A` fills them with blanks, `$A`
/// appends at each line's end); nothing when the cursor left the line or nothing was added;
/// counts; one undo step.
#[test]
fn block_insert_append_and_change_as_vim() {
    const CASES: &[RegCase] = &[
        ("abcd\nab\nabcdef", (0, 1), &[], "<C-v>jj$AX<Esc>", "abcdX\nabX\nabcdefX", (0, 1), &[('"', None)]),
        ("abcd\nab\nabcdef", (0, 1), &[], "<C-v>jjlAX<Esc>", "abcXd\nab X\nabcXdef", (0, 1), &[('"', None)]),
        ("abcd\nab\nabcdef", (0, 2), &[], "<C-v>jjlIX<Esc>", "abXcd\nabX\nabXcdef", (0, 2), &[('"', None)]),
        (
            "abcd\nab\nabcdef",
            (0, 2),
            &[],
            "<C-v>jjlcX<Esc>",
            "abX\nabX\nabXef",
            (0, 2),
            &[('"', Some(("cd\n\ncd", 'b')))],
        ),
        ("abcdef\nab\nabcdef", (0, 3), &[], "<C-v>jjIX<Esc>", "abcXdef\nab\nabcXdef", (0, 3), &[('"', None)]),
        ("abcdef\nab\nabcdef", (0, 3), &[], "<C-v>jj$IX<Esc>", "abcXdef\nab\nabcXdef", (0, 3), &[('"', None)]),
        (
            "abcdef\nab\nabcdef",
            (0, 3),
            &[],
            "<C-v>jjcX<Esc>",
            "abcXef\nab\nabcXef",
            (0, 3),
            &[('"', Some(("d\n \nd", 'b')))],
        ),
        ("abcdef\nab\nabcdef", (0, 3), &[], "<C-v>jjAX<Esc>", "abcdXef\nab  X\nabcdXef", (0, 3), &[('"', None)]),
        ("abcdef\n\nabcdef", (0, 3), &[], "<C-v>jjAX<Esc>", "abcdXef\n    X\nabcdXef", (0, 3), &[('"', None)]),
        ("abcdef\n\nabcdef", (0, 3), &[], "<C-v>jj$AX<Esc>", "abcdefX\nX\nabcdefX", (0, 3), &[('"', None)]),
        (
            "abcd\nabcd\nabcdef",
            (0, 1),
            &[],
            "<C-v>jjlC<Esc>",
            "a\na\na",
            (0, 0),
            &[('"', Some(("bcd\nbcd\nbcdef", 'b')))],
        ),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jlCX<Esc>", "aX\naX", (0, 1), &[('"', Some(("bcdef\nbcdef", 'b')))]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jls<Esc>", "adef\nadef", (0, 0), &[('"', Some(("bc\nbc", 'b')))]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>j3IX<Esc>", "aXXXbcdef\naXXXbcdef", (0, 1), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>j2AXY<Esc>", "abXYXYcdef\nabXYXYcdef", (0, 1), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>j2cX<Esc>", "aXcdef\naXcdef", (0, 1), &[('"', Some(("b\nb", 'b')))]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jIX<CR>Y<Esc>", "aX\nYbcdef\nabcdef", (1, 0), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jIXY<BS><Esc>", "aXbcdef\naXbcdef", (0, 1), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jI<BS><Esc>", "bcdef\nabcdef", (0, 0), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jIX<Left>Y<Esc>", "aYXbcdef\naYXbcdef", (0, 1), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jAX<Left>Y<Esc>", "abYXcdef\nabYXcdef", (0, 1), &[('"', None)]),
        (
            "abcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jcX<Left>Y<Esc>",
            "aYXcdef\naYXcdef",
            (0, 1),
            &[('"', Some(("b\nb", 'b')))],
        ),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jIX<C-w>Y<Esc>", "aYbcdef\naYbcdef", (0, 1), &[('"', None)]),
        ("  abcdef\n  abcdef", (0, 2), &[], "<C-v>jIX<Esc>", "  Xabcdef\n  Xabcdef", (0, 2), &[('"', None)]),
        ("  abcdef\n  abcdef", (0, 0), &[], "<C-v>jIX<Esc>", "X  abcdef\nX  abcdef", (0, 0), &[('"', None)]),
        (
            "  abcdef\n  abcdef",
            (0, 0),
            &[],
            "<C-v>jcX<Esc>",
            "X abcdef\nX abcdef",
            (0, 0),
            &[('"', Some((" \n ", 'b')))],
        ),
        (
            "  abcdef\n  abcdef",
            (0, 0),
            &[],
            "<C-v>jlcX<Esc>",
            "Xabcdef\nXabcdef",
            (0, 0),
            &[('"', Some(("  \n  ", 'b')))],
        ),
        ("  abcdef\n  abcdef", (0, 0), &[], "<C-v>jlc<Esc>", "abcdef\nabcdef", (0, 0), &[('"', Some(("  \n  ", 'b')))]),
        (
            "  abcdef\n  abcdef",
            (0, 0),
            &[],
            "<C-v>jlcX<CR>Y<Esc>",
            "X\nYabcdef\nabcdef",
            (1, 0),
            &[('"', Some(("  \n  ", 'b')))],
        ),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jlIX<Esc>u", "abcdef\nabcdef", (0, 1), &[('"', None)]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jlcX<Esc>u", "abcdef\nabcdef", (0, 1), &[('"', Some(("bc\nbc", 'b')))]),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jlXu", "abcdef\nabcdef", (0, 1), &[('"', Some(("bc\nbc", 'b')))]),
    ];
    check(CASES);
}

/// Puts of a block in Normal mode, `p` / `P` over a block (a block register, lines, text of
/// one line on each line, other text), and the registers block yanks and deletes write.
#[test]
fn block_puts_and_registers_as_vim() {
    const CASES: &[RegCase] = &[
        ("abcd\nabcd\nabcd", (0, 1), &[], "<C-v>jlyP", "abcbcd\nabcbcd\nabcd", (0, 1), &[('"', Some(("bc\nbc", 'b')))]),
        ("abcd\nabcd\nabcd", (0, 1), &[], "<C-v>jlyp", "abbccd\nabbccd\nabcd", (0, 2), &[('"', Some(("bc\nbc", 'b')))]),
        (
            "abcdef\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jlyjp",
            "abcdef\nabbccdef\nabbccdef",
            (1, 2),
            &[('"', Some(("bc\nbc", 'b')))],
        ),
        (
            "abcdef\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jlyGp",
            "abcdef\nabcdef\nabcbcdef\n bc",
            (2, 1),
            &[('"', Some(("bc\nbc", 'b')))],
        ),
        (
            "abcdef\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jly$p",
            "abcdefbc\nabcdefbc\nabcdef",
            (0, 6),
            &[('"', Some(("bc\nbc", 'b')))],
        ),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jlYP", "abcbcdef\nabcbcdef", (0, 1), &[('"', Some(("bc\nbc", 'b')))]),
        (
            "abcd\nab\nabcdef",
            (0, 1),
            &[('a', "XY", 'v')],
            "<C-v>jjl\"ap",
            "aXYd\naXY\naXYdef",
            (0, 2),
            &[('"', Some(("bc\nb\nbc", 'b'))), ('a', Some(("XY", 'v')))],
        ),
        (
            "abcd\nabcd\nabcdef",
            (0, 1),
            &[('a', "XY", 'v')],
            "<C-v>jjl\"ap",
            "aXYd\naXYd\naXYdef",
            (0, 2),
            &[('"', Some(("bc\nbc\nbc", 'b'))), ('a', Some(("XY", 'v')))],
        ),
        (
            "abcd\nabcd\nabcdef",
            (0, 1),
            &[('a', "XY", 'V')],
            "<C-v>jjl\"ap",
            "ad\nad\nadef\nXY",
            (3, 0),
            &[('"', Some(("bc\nbc\nbc", 'b'))), ('a', Some(("XY", 'V')))],
        ),
        (
            "abcd\nabcd\nabcdef",
            (0, 1),
            &[('a', "X\nY", 'b')],
            "<C-v>jjl\"ap",
            "aXd\naYd\nadef",
            (0, 1),
            &[('"', Some(("bc\nbc\nbc", 'b'))), ('a', Some(("X\nY", 'b')))],
        ),
        (
            "abcd\nabcd\nabcd",
            (0, 1),
            &[('a', "XY", 'v')],
            "<C-v>jl\"aP",
            "aXYd\naXYd\nabcd",
            (0, 2),
            &[('"', None), ('a', Some(("XY", 'v')))],
        ),
        (
            "abcd\nabcd\nabcd",
            (0, 2),
            &[('a', "XY", 'v')],
            "<C-v>jl\"ap",
            "abXY\nabXY\nabcd",
            (0, 3),
            &[('"', Some(("cd\ncd", 'b'))), ('a', Some(("XY", 'v')))],
        ),
        (
            "abcd\nabcd\nabcd",
            (0, 2),
            &[('a', "XY", 'v')],
            "<C-v>jl2\"ap",
            "abXYXY\nabXYXY\nabcd",
            (0, 5),
            &[('"', Some(("cd\ncd", 'b'))), ('a', Some(("XY", 'v')))],
        ),
        (
            "abcd\nabcd\nabcd",
            (0, 2),
            &[('a', "X\nY\nZ", 'v')],
            "<C-v>jl\"ap",
            "abX\nY\nZ\nab\nabcd",
            (0, 2),
            &[('"', Some(("cd\ncd", 'b'))), ('a', Some(("X\nY\nZ", 'v')))],
        ),
        (
            "abcd\nabcd\nabcd",
            (0, 1),
            &[('a', "XY", 'V')],
            "<C-v>jl\"aP",
            "XY\nad\nad\nabcd",
            (0, 0),
            &[('"', None), ('a', Some(("XY", 'V')))],
        ),
        (
            "abcd\nabcd\nabcd",
            (1, 1),
            &[('a', "XY", 'V')],
            "<C-v>kl\"ap",
            "ad\nXY\nad\nabcd",
            (1, 0),
            &[('"', Some(("bc\nbc", 'b'))), ('a', Some(("XY", 'V')))],
        ),
        (
            "abcd\na\nabcd",
            (0, 1),
            &[('a', "XY", 'v')],
            "<C-v>jjl\"ap",
            "aXYd\naXY\naXYd",
            (0, 2),
            &[('"', Some(("bc\n\nbc", 'b'))), ('a', Some(("XY", 'v')))],
        ),
        (
            "abcd\nabcd\nabcd",
            (0, 1),
            &[('a', "", 'v')],
            "<C-v>jjl\"ap",
            "ad\nad\nad",
            (0, 1),
            &[('"', Some(("bc\nbc\nbc", 'b'))), ('a', Some(("", 'v')))],
        ),
        (
            "abcd\nabcd\nabcd",
            (0, 1),
            &[('a', "X\nYY", 'b')],
            "<C-v>jl\"ap",
            "aX d\naYYd\nabcd",
            (0, 1),
            &[('"', Some(("bc\nbc", 'b'))), ('a', Some(("X\nYY", 'b')))],
        ),
        (
            "ab漢cd\nabcdef\nabcdef",
            (1, 3),
            &[('a', "XY", 'v')],
            "<C-v>j\"ap",
            "ab漢cd\nabcXYef\nabcXYef",
            (1, 4),
            &[('"', Some(("d\nd", 'b'))), ('a', Some(("XY", 'v')))],
        ),
        (
            "abcd\nabcd\nabcd",
            (0, 1),
            &[],
            "<C-v>jl\"ay",
            "abcd\nabcd\nabcd",
            (0, 1),
            &[('a', Some(("bc\nbc", 'b'))), ('0', None), ('"', Some(("bc\nbc", 'b'))), ('-', None), ('1', None)],
        ),
        (
            "abcd\nabcd\nabcd",
            (0, 1),
            &[],
            "<C-v>ld",
            "ad\nabcd\nabcd",
            (0, 1),
            &[('0', None), ('"', Some(("bc", 'b'))), ('-', Some(("bc", 'b'))), ('1', None)],
        ),
        (
            "abcd\nabcd\nabcd",
            (0, 1),
            &[],
            "<C-v>jld",
            "ad\nad\nabcd",
            (0, 1),
            &[('0', None), ('"', Some(("bc\nbc", 'b'))), ('-', None), ('1', Some(("bc\nbc", 'b')))],
        ),
        (
            "abcd\nabcd\nabcd",
            (0, 1),
            &[],
            "<C-v>jlc<Esc>",
            "ad\nad\nabcd",
            (0, 0),
            &[('0', None), ('"', Some(("bc\nbc", 'b'))), ('-', None), ('1', Some(("bc\nbc", 'b')))],
        ),
        (
            "abcd\nabcd\nabcd",
            (0, 1),
            &[],
            "<C-v>jlcX<Esc>",
            "aXd\naXd\nabcd",
            (0, 1),
            &[
                ('0', None),
                ('"', Some(("bc\nbc", 'b'))),
                ('-', None),
                ('1', Some(("bc\nbc", 'b'))),
                ('.', Some(("X", 'v'))),
            ],
        ),
        (
            "abcd\nabcd\nabcd",
            (0, 1),
            &[],
            "<C-v>j$y",
            "abcd\nabcd\nabcd",
            (0, 1),
            &[('0', Some(("bcd\nbcd", 'b'))), ('"', Some(("bcd\nbcd", 'b')))],
        ),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jl\"_d", "adef\nadef", (0, 1), &[('"', None), ('-', None)]),
        (
            "abcdef\nabcdef",
            (0, 1),
            &[('a', "xx", 'v')],
            "<C-v>jl\"Ay",
            "abcdef\nabcdef",
            (0, 1),
            &[('a', Some(("xxbc\nbc", 'v')))],
        ),
        (
            "abcdef\nabcdef",
            (0, 1),
            &[('a', "xx", 'V')],
            "<C-v>jl\"Ay",
            "abcdef\nabcdef",
            (0, 1),
            &[('a', Some(("xx\nbc\nbc", 'V')))],
        ),
        (
            "abcdef\nabcdef",
            (0, 1),
            &[('a', "xx\nyy", 'b')],
            "<C-v>jl\"Ay",
            "abcdef\nabcdef",
            (0, 1),
            &[('a', Some(("xx\nyy\nbc\nbc", 'b')))],
        ),
        ("abcdef\nabcdef", (0, 1), &[], "<C-v>jlD", "a\na", (0, 0), &[('-', None), ('1', Some(("bcdef\nbcdef", 'b')))]),
        ("abcdef", (0, 1), &[], "<C-v>lD", "a", (0, 0), &[('-', Some(("bcdef", 'b'))), ('1', None)]),
        (
            "abcdef\nabcdef",
            (0, 1),
            &[('b', "XY", 'v')],
            "<C-v>jl\"bp",
            "aXYdef\naXYdef",
            (0, 2),
            &[('"', Some(("bc\nbc", 'b'))), ('b', Some(("XY", 'v')))],
        ),
    ];
    check(CASES);
}

/// `.` repeats a block operator on a block as large from the cursor (to the line ends after
/// `$`), Insert text included; a count typed with `.` does not change a Visual operator's.
#[test]
fn block_repeat_as_vim() {
    const CASES: &[RegCase] = &[
        (
            "abcd\nabcd\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jlxj.",
            "ad\na\nadef\nabcdef",
            (1, 0),
            &[('"', Some(("d\nbc", 'b')))],
        ),
        (
            "abcd\nabcd\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jlIZ<Esc>jj.",
            "aZbcd\naZbcd\naZbcdef\naZbcdef",
            (2, 1),
            &[('"', None)],
        ),
        (
            "abcd\nabcd\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>j$xjj.",
            "a\na\n\n",
            (2, 0),
            &[('"', Some(("abcdef\nabcdef", 'b')))],
        ),
        (
            "abcdef\nabcdef\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jlxjj.",
            "adef\nadef\nadef\nadef",
            (2, 1),
            &[('"', Some(("bc\nbc", 'b')))],
        ),
        (
            "abcdef\nabcdef\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jlrxjj.",
            "axxdef\naxxdef\naxxdef\naxxdef",
            (2, 1),
            &[('"', None)],
        ),
        (
            "abcdef\nabcdef\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jl>jj.",
            "a    bcdef\na    bcdef\na    bcdef\na    bcdef",
            (2, 1),
            &[('"', None)],
        ),
        (
            "abcdef\nabcdef\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jl~jj.",
            "aBCdef\naBCdef\naBCdef\naBCdef",
            (2, 1),
            &[('"', None)],
        ),
        (
            "abcdef\nabcdef\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jlcX<Esc>jj.",
            "aXdef\naXdef\naXdef\naXdef",
            (2, 1),
            &[('"', Some(("bc\nbc", 'b')))],
        ),
        (
            "abcdef\nabcdef\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jlAX<Esc>jj.",
            "abcXdef\nabcXdef\nabcXdef\nabcXdef",
            (2, 1),
            &[('"', None)],
        ),
        (
            "abcdef\nabcdef\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>j$AX<Esc>jj.",
            "abcdefX\nabcdefX\nabcdefX\nabcdefX",
            (2, 1),
            &[('"', None)],
        ),
        (
            "abcdef\nabcdef\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jl2>jj.",
            "a        bcdef\na        bcdef\na        bcdef\na        bcdef",
            (2, 1),
            &[('"', None)],
        ),
        (
            "abcdef\nabcdef\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jl>jj3.",
            "a    bcdef\na    bcdef\na    bcdef\na    bcdef",
            (2, 1),
            &[('"', None)],
        ),
        (
            "abcdef\nabcdef\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jlxjj2.",
            "adef\nadef\nadef\nadef",
            (2, 1),
            &[('"', Some(("bc\nbc", 'b')))],
        ),
        (
            "abcdef\nabcdef\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jlDjj.",
            "a\na\n\n",
            (2, 0),
            &[('"', Some(("abcdef\nabcdef", 'b')))],
        ),
        ("abcdef\nabcdef\nabcdef", (0, 1), &[], "vl>j3.", "    abcdef\n    abcdef\nabcdef", (1, 4), &[('"', None)]),
        ("abcdef\nabcdef\nabcdef", (0, 1), &[], "vlxj3.", "adef\nadef\nabcdef", (1, 1), &[('"', Some(("bc", 'v')))]),
    ];
    check(CASES);
}

/// Lines shorter than the block at its corners and its first line: `c` `s` `C` type at the
/// first line's end, `O` swaps screen columns, the cursor after `I` `A` `>` stays past
/// such a line's text when that is where the block starts; undo and redo after `A`; text
/// objects (a word keeps the block, a paragraph takes lines, a bracket characters); a register
/// Vim fills itself; a block of one empty line (no register written). As Neovim does them.
#[test]
fn blocks_at_short_lines_and_edges_as_vim() {
    const CASES: &[RegCase] = &[
        ("ab\nabcdef", (1, 4), &[], "<C-v>kcX<Esc>", "abX\nabXf", (0, 2), &[('"', Some(("\ncde", 'b')))]),
        ("ab\nabcdef", (1, 4), &[], "<C-v>kCX<Esc>", "abX\nabX", (0, 2), &[('"', Some(("\ncdef", 'b')))]),
        ("ab\nabcdef", (1, 4), &[], "<C-v>ksX<Esc>", "abX\nabXf", (0, 2), &[('"', Some(("\ncde", 'b')))]),
        (
            "a\nabcdef\nabcdef",
            (2, 4),
            &[],
            "<C-v>kkcX<Esc>",
            "aX\naXf\naXf",
            (0, 1),
            &[('"', Some(("\nbcde\nbcde", 'b')))],
        ),
        ("漢\nabcdef", (1, 4), &[], "<C-v>kcX<Esc>", "漢X\nabXf", (0, 1), &[('"', Some(("\ncde", 'b')))]),
        ("abcdef\nab\nabcd", (0, 1), &[], "<C-v>jlOy", "abcdef\nab\nabcd", (0, 1), &[('"', Some(("bc\nb", 'b')))]),
        ("ab\nabcdef", (1, 4), &[], "<C-v>kOy", "ab\nabcdef", (0, 1), &[('"', Some(("\ncde", 'b')))]),
        ("ab\nabcdef", (1, 4), &[], "<C-v>kOd", "ab\nabf", (0, 1), &[('"', Some(("\ncde", 'b')))]),
        ("abcdef\nab\nabc", (0, 4), &[], "<C-v>jOd", "abf\nab\nabc", (0, 2), &[('"', Some(("cde\n", 'b')))]),
        ("ab\nabcdef", (0, 1), &[], "<C-v>jlllOy", "ab\nabcdef", (0, 1), &[('"', Some(("b\nbc", 'b')))]),
        ("abcdef\nab\nabc", (0, 4), &[], "<C-v>jhOy", "abcdef\nab\nabc", (0, 1), &[('"', Some(("bc\nb", 'b')))]),
        ("abcdef\nab\nabc", (0, 4), &[], "<C-v>jOOy", "abcdef\nab\nabc", (0, 2), &[('"', Some(("cde\n", 'b')))]),
        (
            "abc def\nabc def\n\nx",
            (0, 1),
            &[],
            "<C-v>ipy",
            "abc def\nabc def\n\nx",
            (0, 0),
            &[('"', Some(("abc def\nabc def", 'V')))],
        ),
        ("f(abc)\nf(abc)", (0, 3), &[], "<C-v>jibd", "f()\nf(abc)", (0, 2), &[('"', Some(("abc", 'v')))]),
        ("abc def\nabc def", (0, 1), &[], "<C-v>jiWd", "a def\na def", (0, 1), &[('"', Some(("bc\nbc", 'b')))]),
        ("abc def\nabc def", (0, 1), &[], "<C-v>jawd", "adef\nadef", (0, 1), &[('"', Some(("bc \nbc ", 'b')))]),
        ("ab\nabcdef", (1, 4), &[], "<C-v>kIX<Esc>", "abX\nabXcdef", (0, 2), &[('"', None)]),
        ("ab\nabcdef", (1, 4), &[], "<C-v>kAX<Esc>", "ab   X\nabcdeXf", (0, 2), &[('"', None)]),
        ("ab\nabcdef", (1, 4), &[], "<C-v>k>", "ab    \nab    cdef", (0, 2), &[('"', None)]),
        ("漢\nabcdef", (1, 4), &[], "<C-v>kIX<Esc>", "漢X\nabXcdef", (0, 1), &[('"', None)]),
        ("ab\nabcdef", (1, 4), &[], "<C-v>kIX<Home>Y<Esc>", "YabX\nabbXcdef", (0, 0), &[('"', None)]),
        (
            "abcdef\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jlAX<Esc>u<C-r>",
            "abcXdef\nabcXdef\nabcdef",
            (0, 3),
            &[('"', None)],
        ),
        (
            "abcdef\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>j$AX<Esc>u<C-r>",
            "abcdefX\nabcdefX\nabcdef",
            (0, 6),
            &[('"', None)],
        ),
        ("abcdef\na漢cdef\nabcdef", (0, 2), &[], "<C-v>jjAX<Esc>u", "abcdef\na漢cdef\nabcdef", (0, 3), &[('"', None)]),
        ("abcdef\nabcdef\nabcdef", (0, 1), &[], "<C-v>jlAX<Esc>u", "abcdef\nabcdef\nabcdef", (0, 3), &[('"', None)]),
        (
            "abcdef\nabcdef\nabcdef",
            (0, 1),
            &[],
            "<C-v>jlIX<Esc>u<C-r>",
            "aXbcdef\naXbcdef\nabcdef",
            (0, 1),
            &[('"', None)],
        ),
        ("abcdef\nabcdef", (1, 3), &[], "<C-v>kl\".y", "abcdef\nabcdef", (0, 3), &[('"', None)]),
        ("abcdef\nabcdef", (1, 3), &[], "<C-v>kl\".d", "abcdef\nabcdef", (0, 3), &[('"', None)]),
        ("", (0, 0), &[], "<C-v>d", "", (0, 0), &[('"', None), ('-', None), ('0', None), ('1', None)]),
        ("", (0, 0), &[], "<C-v>x", "", (0, 0), &[('"', None), ('-', None), ('0', None), ('1', None)]),
        ("", (0, 0), &[], "<C-v>D", "", (0, 0), &[('"', None), ('-', None), ('0', None), ('1', None)]),
        ("", (0, 0), &[], "<C-v>C<Esc>", "", (0, 0), &[('"', None), ('-', None), ('0', None), ('1', None)]),
        ("", (0, 0), &[], "<C-v>cX<Esc>", "X", (0, 0), &[('"', None), ('-', None), ('0', None), ('1', None)]),
        (
            "",
            (0, 0),
            &[],
            "<C-v>y",
            "",
            (0, 0),
            &[('"', Some(("", 'b'))), ('-', None), ('0', Some(("", 'b'))), ('1', None)],
        ),
        (
            "",
            (0, 0),
            &[('a', "x", 'v')],
            "<C-v>\"ap",
            "x",
            (0, 0),
            &[('"', None), ('-', None), ('0', None), ('1', None), ('a', Some(("x", 'v')))],
        ),
        (
            "",
            (0, 0),
            &[('a', "x\ny", 'b')],
            "<C-v>\"ap",
            "x\ny",
            (0, 0),
            &[('"', None), ('-', None), ('0', None), ('1', None), ('a', Some(("x\ny", 'b')))],
        ),
        ("ab\n\ncd", (1, 0), &[], "<C-v>d", "ab\n\ncd", (1, 0), &[('"', None), ('-', None), ('0', None), ('1', None)]),
        ("ab\n\ncd", (1, 0), &[], "<C-v>r-", "ab\n\ncd", (1, 0), &[('"', None), ('-', None), ('0', None), ('1', None)]),
        ("", (0, 0), &[], "<C-v>IX<Esc>", "X", (0, 0), &[('"', None), ('-', None), ('0', None), ('1', None)]),
        ("", (0, 0), &[], "<C-v>AX<Esc>", " X", (0, 0), &[('"', None), ('-', None), ('0', None), ('1', None)]),
        (
            "abcd\nabcd\nabcd",
            (0, 1),
            &[],
            "<C-v>jjIXY<Left>Z<Esc>",
            "aXZYbcd\naXZYbcd\naXZYbcd",
            (0, 1),
            &[('"', None)],
        ),
        ("abcd\nabcd\nabcd", (0, 1), &[], "<C-v>jjIX<Up>Y<Esc>", "aXYbcd\naXYbcd\naXYbcd", (0, 1), &[('"', None)]),
    ];
    check(CASES);
}

/// `Ctrl+V` starts a block (`V-BLOCK` in the status bar); `v`, `V` and `Ctrl+V` switch between
/// characters, lines and a block, the same key again ends Visual mode. Ctrl+E runs the block's
/// text, as `y` takes it.
#[test]
fn ctrl_v_starts_and_switches_a_block() {
    let mut e = at("SELECT 1;\nSELECT 22;", (0, 7));
    e.handle_key(ctrl('v'));
    assert_eq!((e.mode, e.mode_label(), e.visual_block()), (Mode::Visual, Label::StatusModeVisualBlock, true));
    typ(&mut e, "jl");
    assert_eq!(e.selection().as_deref(), Some("1;\n22"));
    typ(&mut e, "v");
    assert_eq!((e.mode_label(), e.visual_block()), (Label::StatusModeVisual, false));
    assert_eq!(e.selection().as_deref(), Some("1;\nSELECT 22"));
    typ(&mut e, "<C-v>");
    assert_eq!(e.mode_label(), Label::StatusModeVisualBlock);
    typ(&mut e, "V");
    assert_eq!((e.mode_label(), e.visual_lines()), (Label::StatusModeVisualLine, true));
    typ(&mut e, "<C-v><C-v>");
    assert_eq!((e.mode, e.mode_label(), e.visual_block()), (Mode::Normal, Label::StatusModeNormal, false));
    assert_eq!(e.selection(), None);
    // After `$` the block reaches every line's end.
    typ(&mut e, "gg0<C-v>j$");
    assert_eq!(e.selection().as_deref(), Some("SELECT 1;\nSELECT 22;"));
}

/// Which cells of `line` (from screen column `from` to `to`) are drawn selected.
fn selected(buf: &Buffer, y: u16, from: u16, to: u16) -> Vec<bool> {
    let bg = crate::theme::cur().selection.bg;
    (from..to).map(|x| buf[(x, y)].bg == bg.unwrap()).collect()
}

/// The block is drawn as the characters with a column in it: a wide character partly inside
/// is marked whole, a short line only up to its end, after `$` each line to its end; scrolled
/// sideways, the marks are where the characters are drawn.
#[test]
fn the_block_is_drawn_by_columns() {
    let mut e = at("abcdef\nab\u{6f22}cd\nab\nabcdefgh", (0, 3));
    let area = Rect::new(0, 0, 20, 5);
    let mut buf = Buffer::empty(area);
    typ(&mut e, "<C-v>jjj");
    e.render(area, &mut buf, None);
    // "  1 " is the gutter.
    let g = 4;
    assert_eq!(selected(&buf, 0, g, g + 7), [false, false, false, true, false, false, false]);
    // A wide character is drawn from its first cell.
    assert_eq!(selected(&buf, 1, g, g + 3), [false, false, true], "the wide character, whole");
    assert_eq!(selected(&buf, 1, g + 4, g + 7), [false; 3]);
    assert_eq!(selected(&buf, 2, g, g + 7), [false; 7], "a line that ends before the block");
    assert_eq!(selected(&buf, 3, g, g + 7), [false, false, false, true, false, false, false]);
    typ(&mut e, "$");
    e.render(area, &mut buf, None);
    assert_eq!(selected(&buf, 0, g, g + 8), [false, false, false, true, true, true, false, false]);
    assert_eq!(selected(&buf, 1, g, g + 3), [false, false, true]);
    assert_eq!(selected(&buf, 1, g + 4, g + 8), [true, true, false, false]);
    assert_eq!(selected(&buf, 3, g, g + 9), [false, false, false, true, true, true, true, true, false]);

    // 30 wide characters (two columns each), then "abc": scrolled to the block at the end.
    let mut e = at(&format!("{}abc\n{}abc", "\u{8868}".repeat(30), "x".repeat(60)), (0, 30));
    typ(&mut e, "<C-v>j");
    let mut buf = Buffer::empty(area);
    e.render(area, &mut buf, None);
    assert!(e.left > 0, "scrolled sideways");
    let x = g + (60 - e.left) as u16;
    assert_eq!((buf[(x, 0)].symbol(), buf[(x, 1)].symbol()), ("a", "a"));
    assert_eq!(selected(&buf, 0, x - 2, x + 1), [false, false, true]);
    assert_eq!(selected(&buf, 1, x - 2, x + 1), [false, false, true]);
    assert_eq!(e.render(area, &mut buf, None), (x, 1), "the cursor on the block's corner");
}

/// Every block operator is one undo step made of a splice per part (the block's text, the
/// text typed, the copies of it, a put: its work does not grow with the square of its lines),
/// and `u` brings the text back.
#[test]
fn block_operators_are_one_splice_and_one_undo_step() {
    let text = (0..2000).map(|i| format!("line {i} \u{6f22}x")).collect::<Vec<_>>().join("\n");
    for (yank, keys) in [
        (false, "<C-v>G$d"),
        (false, "<C-v>Gld"),
        (false, "<C-v>G$IX<Esc>"),
        (false, "<C-v>GlAX<Esc>"),
        (false, "<C-v>GlcX<Esc>"),
        (false, "<C-v>Gl~"),
        (false, "<C-v>GlrX"),
        (false, "<C-v>Gl>"),
        (false, "<C-v>Gl<"),
        (true, "<C-v>G$p"),
    ] {
        let mut e = at(&text, (0, 1));
        if yank {
            typ(&mut e, "<C-v>Gly");
        }
        e.take_block_work();
        typ(&mut e, keys);
        let changed = e.text() != text;
        assert!(changed || keys.ends_with('<'), "{keys}");
        assert!(
            e.last_step_changes() <= 3 && (e.last_step_changes() > 0) == changed,
            "{keys}: {}",
            e.last_step_changes()
        );
        assert!(e.take_block_work() <= 8 * (text.len() + 2000), "{keys}: bytes walked");
        typ(&mut e, "u");
        assert_eq!(e.text(), text, "{keys}");
        // `A` comes back where its text went, the others to the block's corner (Vim).
        let col = if keys.contains('A') { 2 } else { 1 };
        assert_eq!((e.row, e.col, e.mode), (0, col, Mode::Normal), "{keys}");
    }
}

/// The Insert session of `I`, `A` or `c` copies its text to the block's other lines only when
/// it ends on the first line: a click on another line, or a line break typed, leaves them.
#[test]
fn block_insert_copies_only_from_the_first_line() {
    let mut e = at("abc\nabc\nabc", (0, 1));
    typ(&mut e, "<C-v>jjIX");
    e.click(2, 2);
    typ(&mut e, "Y<Esc>");
    assert_eq!(e.text(), "aXbc\nabc\nabYc");
    let mut e = at("abc\nabc\nabc", (0, 1));
    typ(&mut e, "<C-v>jjAX<Esc>");
    assert_eq!(e.text(), "abXc\nabXc\nabXc");
    typ(&mut e, "u");
    assert_eq!(e.text(), "abc\nabc\nabc", "one undo step");
}

/// A paste from the terminal over a block takes the block's place as one undo step, and keeps
/// the registers.
#[test]
fn a_paste_over_a_block_replaces_it() {
    let mut e = at("abcd\nabcd", (0, 1));
    typ(&mut e, "<C-v>jl");
    e.paste("XY");
    assert_eq!((e.text().as_str(), e.mode), ("aXYd\nad", Mode::Normal));
    assert_eq!(e.register('"'), None);
    typ(&mut e, "u");
    assert_eq!(e.text(), "abcd\nabcd");
}

/// A block yank is offered to the app for the system clipboard: its pieces, each line ending
/// with a line break (as Vim writes a block there).
#[test]
fn a_block_yank_goes_to_the_clipboard() {
    let mut e = at("ab\u{6f22}cd\nabcdef", (0, 1));
    typ(&mut e, "<C-v>jly");
    let y = e.take_yank().expect("a yank for the app");
    assert_eq!((y.register, y.reg.kind), ('"', RegKind::Blockwise));
    assert_eq!(y.reg.clipboard_text(), "b \nbc\n");
}

/// A count that would put more text into the lines of a block than an undo step may hold
/// (`99999>` over hundreds of lines, `99999I`) does nothing, as a put that large does; a
/// smaller one works.
#[test]
fn huge_counts_on_a_block_do_nothing() {
    let text = vec!["abc"; 1000].join("\n");
    let mut e = at(&text, (0, 1));
    typ(&mut e, "<C-v>199j99999>");
    assert_eq!((e.text() == text, e.mode, e.last_step_changes()), (true, Mode::Normal, 0));
    let mut e = at(&text, (0, 1));
    typ(&mut e, "<C-v>G99999I");
    assert_eq!((e.text() == text, e.mode), (true, Mode::Normal), "no Insert mode");
    let mut e = at(&text, (0, 1));
    typ(&mut e, "<C-v>j3>");
    assert!(e.text().starts_with("a            bc\na            bc\nabc"), "{:?}", &e.text()[..40]);
}
