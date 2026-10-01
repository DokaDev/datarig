use super::super::tests::{at, typ};
use super::*;
use crate::widgets::editor::{Editor, Mode};

/// `(text, cursor, registers set before as (name, text, kind), keys, text after, cursor after,
/// registers after as (name, (text, kind)))`; a kind is `'v'` (by character), `'V'` (by line)
/// or `'b'` (a block).
type RegCase = (
    &'static str,
    (usize, usize),
    &'static [(char, &'static str, char)],
    &'static str,
    &'static str,
    (usize, usize),
    &'static [(char, Option<(&'static str, char)>)],
);

fn kind_of(k: char) -> RegKind {
    match k {
        'V' => RegKind::Linewise,
        'b' => RegKind::Blockwise,
        _ => RegKind::Charwise,
    }
}

fn letter(k: RegKind) -> char {
    match k {
        RegKind::Charwise => 'v',
        RegKind::Linewise => 'V',
        RegKind::Blockwise => 'b',
    }
}

/// Run each case from a fresh editor and compare the text, the cursor and the registers; the
/// failures are listed together.
fn check(cases: &[RegCase]) {
    let mut failed = Vec::new();
    for &(text, cursor, set, keys, want, want_cursor, want_regs) in cases {
        let mut e = at(text, cursor);
        // setreg() leaves the unnamed register alone.
        let last = e.regs.last;
        for &(name, text, kind) in set {
            e.regs.write(name, Register::new(text, kind_of(kind)));
        }
        e.regs.last = last;
        typ(&mut e, keys);
        let regs: Vec<_> = want_regs
            .iter()
            .map(|&(name, _)| (name, e.register(name).map(|r| (r.text.as_str(), letter(r.kind)))))
            .collect();
        let have = (e.text(), (e.row, e.col), regs, e.mode);
        if have != (want.to_string(), want_cursor, want_regs.to_vec(), Mode::Normal) {
            failed.push(format!(
                "{keys:?} on {text:?} at {cursor:?}: {have:?}, not {:?}",
                (want, want_cursor, want_regs)
            ));
        }
        assert_eq!(e.len_bytes(), e.text().len());
    }
    assert!(failed.is_empty(), "{} of {} cases:\n{}", failed.len(), cases.len(), failed.join("\n"));
}

/// Named registers (`"A` appends by Vim's rules for lines and characters), `"0`, the delete
/// ring `"1`–`"9` (whole lines, more than one line, or `%` `{` `}`), `"-`, `"_`, registers
/// Vim fills itself (a yank or delete into them fails), counts on both sides of `"x`, puts
/// by character, by line and as a block, and `Ctrl+R` in Insert mode, as Vim does them.
#[test]
fn registers_as_vim() {
    const CASES: &[RegCase] = &[
        (
            "one\ntwo\nthree",
            (0, 0),
            &[],
            "\"add",
            "two\nthree",
            (0, 0),
            &[
                ('"', Some(("one", 'V'))),
                ('a', Some(("one", 'V'))),
                ('0', None),
                ('1', Some(("one", 'V'))),
                ('-', None),
            ],
        ),
        (
            "one two\nx",
            (0, 0),
            &[],
            "\"adw",
            "two\nx",
            (0, 0),
            &[('"', Some(("one ", 'v'))), ('a', Some(("one ", 'v'))), ('1', None), ('-', None)],
        ),
        (
            "one two\nx",
            (0, 0),
            &[],
            "dw",
            "two\nx",
            (0, 0),
            &[('"', Some(("one ", 'v'))), ('0', None), ('1', None), ('-', Some(("one ", 'v')))],
        ),
        (
            "a (b) c\nx",
            (0, 0),
            &[],
            "f(d%",
            "a  c\nx",
            (0, 2),
            &[('"', Some(("(b)", 'v'))), ('1', Some(("(b)", 'v'))), ('-', Some(("(b)", 'v')))],
        ),
        (
            "one\ntwo\n\nx",
            (0, 0),
            &[],
            "d}",
            "\nx",
            (0, 0),
            &[('"', Some(("one\ntwo", 'V'))), ('1', Some(("one\ntwo", 'V'))), ('-', None)],
        ),
        (
            "one\ntwo\n\nx",
            (0, 0),
            &[],
            "c}X<Esc>",
            "X\n\nx",
            (0, 0),
            &[('"', Some(("one\ntwo", 'V'))), ('1', Some(("one\ntwo", 'V'))), ('-', None)],
        ),
        (
            "one two\nx",
            (0, 0),
            &[],
            "\"ayw\"Ayy",
            "one two\nx",
            (0, 0),
            &[('"', Some(("one \none two", 'V'))), ('a', Some(("one \none two", 'V')))],
        ),
        (
            "one\ntwo",
            (0, 0),
            &[],
            "\"ayy\"Ayw",
            "one\ntwo",
            (0, 0),
            &[('"', Some(("one\none", 'V'))), ('a', Some(("one\none", 'V')))],
        ),
        (
            "one two",
            (0, 0),
            &[],
            "\"ayw\"Ayw",
            "one two",
            (0, 0),
            &[('"', Some(("one one ", 'v'))), ('a', Some(("one one ", 'v')))],
        ),
        ("one two", (0, 0), &[], "\"Ayw", "one two", (0, 0), &[('"', Some(("one ", 'v'))), ('a', Some(("one ", 'v')))]),
        (
            "one\ntwo",
            (0, 0),
            &[],
            "yy\"_dd",
            "two",
            (0, 0),
            &[('"', Some(("one", 'V'))), ('0', Some(("one", 'V'))), ('1', None), ('-', None)],
        ),
        (
            "one\ntwo",
            (0, 0),
            &[],
            "yy\"_ddp",
            "two\none",
            (1, 0),
            &[('"', Some(("one", 'V'))), ('0', Some(("one", 'V'))), ('1', None), ('-', None)],
        ),
        (
            "one two\nx",
            (0, 0),
            &[],
            "3x",
            " two\nx",
            (0, 0),
            &[('"', Some(("one", 'v'))), ('1', None), ('-', Some(("one", 'v')))],
        ),
        (
            "one\ntwo\nthree",
            (0, 0),
            &[],
            "vjd",
            "wo\nthree",
            (0, 0),
            &[('"', Some(("one\nt", 'v'))), ('1', Some(("one\nt", 'v'))), ('-', None)],
        ),
        (
            "one\ntwo",
            (0, 0),
            &[],
            "v$d",
            "two",
            (0, 0),
            &[('"', Some(("one\n", 'v'))), ('1', Some(("one\n", 'v'))), ('-', None)],
        ),
        (
            "a\nb\nc\nd",
            (0, 0),
            &[],
            "dddddd",
            "d",
            (0, 0),
            &[('"', Some(("c", 'V'))), ('1', Some(("c", 'V'))), ('2', Some(("b", 'V'))), ('3', Some(("a", 'V')))],
        ),
        (
            "a\nb\nc\nd",
            (0, 0),
            &[],
            "dddddd\"2p",
            "d\nb",
            (1, 0),
            &[('"', Some(("c", 'V'))), ('1', Some(("c", 'V'))), ('2', Some(("b", 'V'))), ('3', Some(("a", 'V')))],
        ),
        (
            "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11",
            (0, 0),
            &[],
            "dddddddddddddddddddd",
            "11",
            (0, 0),
            &[
                ('"', Some(("10", 'V'))),
                ('1', Some(("10", 'V'))),
                ('2', Some(("9", 'V'))),
                ('3', Some(("8", 'V'))),
                ('4', Some(("7", 'V'))),
                ('5', Some(("6", 'V'))),
                ('6', Some(("5", 'V'))),
                ('7', Some(("4", 'V'))),
                ('8', Some(("3", 'V'))),
                ('9', Some(("2", 'V'))),
            ],
        ),
        (
            "one\ntwo",
            (0, 0),
            &[],
            "\"-yy",
            "one\ntwo",
            (0, 0),
            &[('"', Some(("one", 'V'))), ('-', Some(("one", 'V'))), ('0', None)],
        ),
        (
            "one\ntwo",
            (0, 0),
            &[],
            "\"1yy",
            "one\ntwo",
            (0, 0),
            &[('"', Some(("one", 'V'))), ('0', None), ('1', Some(("one", 'V')))],
        ),
        (
            "one\ntwo",
            (0, 0),
            &[],
            "\"0dd",
            "two",
            (0, 0),
            &[('"', Some(("one", 'V'))), ('0', Some(("one", 'V'))), ('1', Some(("one", 'V')))],
        ),
        (
            "a\nb\nc",
            (0, 0),
            &[],
            "\"ayyj\"Add",
            "a\nc",
            (1, 0),
            &[('"', Some(("a\nb", 'V'))), ('a', Some(("a\nb", 'V'))), ('1', Some(("b", 'V')))],
        ),
        (
            "a\nb\nc\nd",
            (0, 0),
            &[],
            "\"add.",
            "c\nd",
            (0, 0),
            &[('"', Some(("b", 'V'))), ('a', Some(("b", 'V'))), ('1', Some(("b", 'V'))), ('2', Some(("a", 'V')))],
        ),
        (
            "a\nb\nc\nd",
            (0, 0),
            &[],
            "\"Add.",
            "c\nd",
            (0, 0),
            &[('"', Some(("a\nb", 'V'))), ('a', Some(("a\nb", 'V'))), ('1', Some(("b", 'V'))), ('2', Some(("a", 'V')))],
        ),
        (
            "1\n2\n3\n4\n5\n6\n7\n8\n9",
            (0, 0),
            &[],
            "2\"a3yy",
            "1\n2\n3\n4\n5\n6\n7\n8\n9",
            (0, 0),
            &[('"', Some(("1\n2\n3\n4\n5\n6", 'V'))), ('a', Some(("1\n2\n3\n4\n5\n6", 'V'))), ('0', None)],
        ),
        (
            "1\n2\n3\n4\n5\n6\n7\n8\n9",
            (0, 0),
            &[],
            "\"a3yy",
            "1\n2\n3\n4\n5\n6\n7\n8\n9",
            (0, 0),
            &[('"', Some(("1\n2\n3", 'V'))), ('a', Some(("1\n2\n3", 'V'))), ('0', None)],
        ),
        (
            "1\n2\n3\n4\n5\n6\n7\n8\n9",
            (0, 0),
            &[],
            "\"a\"byy",
            "1\n2\n3\n4\n5\n6\n7\n8\n9",
            (0, 0),
            &[('"', Some(("1", 'V'))), ('a', None), ('b', Some(("1", 'V')))],
        ),
        (
            "1 2 3",
            (0, 0),
            &[],
            "\"a<Esc>yw",
            "1 2 3",
            (0, 0),
            &[('"', Some(("1 ", 'v'))), ('a', None), ('0', Some(("1 ", 'v')))],
        ),
        ("one two", (0, 0), &[], "\"%yw", "one two", (0, 0), &[('"', None), ('0', None)]),
        ("one two", (0, 0), &[], "\"%dw", "one two", (0, 0), &[('"', None), ('1', None), ('-', None)]),
        (
            "one two three",
            (0, 0),
            &[],
            "dwdw\"-p",
            "ttwo hree",
            (0, 4),
            &[('"', Some(("two ", 'v'))), ('-', Some(("two ", 'v')))],
        ),
        (
            "one two",
            (0, 0),
            &[],
            "cwX<Esc>",
            "X two",
            (0, 0),
            &[('"', Some(("one", 'v'))), ('1', None), ('-', Some(("one", 'v')))],
        ),
        (
            "one\ntwo",
            (0, 0),
            &[],
            "ccX<Esc>",
            "X\ntwo",
            (0, 0),
            &[('"', Some(("one", 'V'))), ('1', Some(("one", 'V'))), ('-', None)],
        ),
        (
            "one two",
            (0, 3),
            &[],
            "D",
            "one",
            (0, 2),
            &[('"', Some((" two", 'v'))), ('1', None), ('-', Some((" two", 'v')))],
        ),
        ("\nx", (0, 0), &[], "x", "\nx", (0, 0), &[('"', None), ('1', None), ('-', None)]),
        (
            "one two",
            (0, 0),
            &[],
            "\"ayw$\"ap",
            "one twoone ",
            (0, 10),
            &[('"', Some(("one ", 'v'))), ('a', Some(("one ", 'v')))],
        ),
        (
            "one two",
            (0, 0),
            &[],
            "\"ayw\"a3p",
            "oone one one ne two",
            (0, 12),
            &[('"', Some(("one ", 'v'))), ('a', Some(("one ", 'v')))],
        ),
        (
            "one two",
            (0, 0),
            &[],
            "\"ayw2\"ap",
            "oone one ne two",
            (0, 8),
            &[('"', Some(("one ", 'v'))), ('a', Some(("one ", 'v')))],
        ),
        (
            "one\ntwo",
            (0, 0),
            &[],
            "\"ayyj\"a2P",
            "one\none\none\ntwo",
            (1, 0),
            &[('"', Some(("one", 'V'))), ('a', Some(("one", 'V')))],
        ),
        (
            "one",
            (0, 0),
            &[],
            "yy\"ayw\"Ap",
            "oonene",
            (0, 3),
            &[('"', Some(("one", 'v'))), ('a', Some(("one", 'v'))), ('0', Some(("one", 'V')))],
        ),
        ("one two", (0, 0), &[], "\"bp", "one two", (0, 0), &[('"', None), ('b', None)]),
        (
            "one two\nthree",
            (0, 0),
            &[],
            "ve\"ay",
            "one two\nthree",
            (0, 0),
            &[('"', Some(("one", 'v'))), ('a', Some(("one", 'v'))), ('0', None)],
        ),
        (
            "one\ntwo\nthree",
            (0, 0),
            &[],
            "Vj\"ad",
            "three",
            (0, 0),
            &[
                ('"', Some(("one\ntwo", 'V'))),
                ('a', Some(("one\ntwo", 'V'))),
                ('1', Some(("one\ntwo", 'V'))),
                ('-', None),
            ],
        ),
        ("one two", (0, 0), &[], "ve\"_d", " two", (0, 0), &[('"', None), ('0', None), ('-', None), ('1', None)]),
        (
            "one two",
            (0, 0),
            &[],
            "v\"a3ly",
            "one two",
            (0, 0),
            &[('"', Some(("one ", 'v'))), ('a', None), ('0', Some(("one ", 'v')))],
        ),
        ("a b", (0, 0), &[('a', "x\ny", 'b')], "\"ap", "ax b\n y", (0, 1), &[('a', Some(("x\ny", 'b')))]),
        (
            "abc\nd\nefgh",
            (0, 1),
            &[('a', "12\n3", 'b')],
            "\"aP",
            "a12bc\nd3\nefgh",
            (0, 1),
            &[('a', Some(("12\n3", 'b')))],
        ),
        (
            "abc\nd\nefgh",
            (0, 1),
            &[('a', "12\n3", 'b')],
            "\"ap",
            "ab12c\nd 3\nefgh",
            (0, 2),
            &[('a', Some(("12\n3", 'b')))],
        ),
        (
            "abc\nd\nefgh",
            (0, 1),
            &[('a', "12\n3", 'b')],
            "\"a2p",
            "ab1212c\nd 3 3\nefgh",
            (0, 2),
            &[('a', Some(("12\n3", 'b')))],
        ),
        ("ab", (0, 1), &[('a', "x\ny\nz", 'b')], "\"ap", "abx\n  y\n  z", (0, 2), &[('a', Some(("x\ny\nz", 'b')))]),
        ("aéb\ncd", (0, 0), &[('a', "一\nx", 'b')], "\"ap", "a一éb\ncx d", (0, 1), &[('a', Some(("一\nx", 'b')))]),
        ("one\ntwo", (0, 0), &[], "yyA<C-r>\"<Esc>", "oneone\n\ntwo", (1, 0), &[('"', Some(("one", 'V')))]),
        (
            "one two",
            (0, 0),
            &[],
            "\"ayiwo<C-r>a<Esc>",
            "one two\none",
            (1, 2),
            &[('"', Some(("one", 'v'))), ('a', Some(("one", 'v')))],
        ),
        (
            "one two",
            (0, 0),
            &[],
            "yiwA <C-r>0<C-r>-<Esc>",
            "one two one",
            (0, 10),
            &[('"', Some(("one", 'v'))), ('0', Some(("one", 'v'))), ('-', None)],
        ),
        ("one", (0, 0), &[], "A<C-r>_x<Esc>", "onex", (0, 3), &[('"', None)]),
        (
            "one",
            (0, 0),
            &[],
            "\"ayiwA<C-r>a<Esc>\"byiw.",
            "oneoneone",
            (0, 8),
            &[('"', Some(("oneone", 'v'))), ('a', Some(("one", 'v'))), ('b', Some(("oneone", 'v')))],
        ),
        (
            "x",
            (0, 0),
            &[],
            "\"ayl\"_x3i<C-r>a<Esc>.",
            "xxxxxx",
            (0, 4),
            &[('"', Some(("x", 'v'))), ('a', Some(("x", 'v')))],
        ),
        (
            "q",
            (0, 0),
            &[],
            "\"ayl\"_x2i<C-r>a-<C-r>a<Esc>.",
            "q-qq-q-qq-qq",
            (0, 10),
            &[('"', Some(("q", 'v'))), ('a', Some(("q", 'v')))],
        ),
        (
            "foo",
            (0, 0),
            &[],
            "\"ayiwA<C-r><C-r>a<Esc>",
            "foofoo",
            (0, 5),
            &[('"', Some(("foo", 'v'))), ('a', Some(("foo", 'v')))],
        ),
        (
            "foo",
            (0, 0),
            &[],
            "\"ayiwA<C-r><C-o>a<Esc>",
            "foofoo",
            (0, 5),
            &[('"', Some(("foo", 'v'))), ('a', Some(("foo", 'v')))],
        ),
        (
            "foo",
            (0, 0),
            &[],
            "\"ayiwA<C-r><C-p>a<Esc>",
            "foofoo",
            (0, 5),
            &[('"', Some(("foo", 'v'))), ('a', Some(("foo", 'v')))],
        ),
        ("ab\n一c", (0, 0), &[('a', "x\ny", 'b')], "\"ap", "axb\n y一c", (0, 1), &[('a', Some(("x\ny", 'b')))]),
        ("one two", (0, 0), &[], "\"+x", "ne two", (0, 0), &[('"', Some(("o", 'v'))), ('-', None)]),
        (
            "one two",
            (0, 0),
            &[],
            "iab<Esc>\".p",
            "ababone two",
            (0, 3),
            &[('"', None), ('.', Some(("ab", 'v'))), ('-', None)],
        ),
        (
            "one two",
            (0, 0),
            &[],
            "iab<Esc>A<C-r>.<Esc>",
            "abone twoab",
            (0, 10),
            &[('"', None), ('.', Some(("ab", 'v')))],
        ),
        ("one two", (0, 0), &[], "\".p", "one two", (0, 0), &[('"', None), ('.', Some(("", 'v')))]),
        (
            "one two",
            (0, 0),
            &[],
            "yiwwviwp",
            "one one",
            (0, 6),
            &[('"', Some(("two", 'v'))), ('0', Some(("one", 'v'))), ('-', Some(("two", 'v'))), ('1', None)],
        ),
        (
            "one two",
            (0, 0),
            &[],
            "yiwwviwP",
            "one one",
            (0, 6),
            &[('"', Some(("one", 'v'))), ('0', Some(("one", 'v'))), ('-', None), ('1', None)],
        ),
        (
            "one two",
            (0, 0),
            &[],
            "\"ayiwwviw\"ap",
            "one one",
            (0, 6),
            &[
                ('"', Some(("two", 'v'))),
                ('a', Some(("one", 'v'))),
                ('0', None),
                ('-', Some(("two", 'v'))),
                ('1', None),
            ],
        ),
        (
            "one\ntwo\nthree",
            (0, 0),
            &[],
            "yyjviwp",
            "one\n\none\n\nthree",
            (2, 0),
            &[('"', Some(("two", 'v'))), ('0', Some(("one", 'V'))), ('-', Some(("two", 'v'))), ('1', None)],
        ),
        (
            "one\ntwo\nthree",
            (0, 0),
            &[],
            "yiwjVp",
            "one\none\nthree",
            (1, 0),
            &[('"', Some(("two", 'V'))), ('0', Some(("one", 'v'))), ('-', None), ('1', Some(("two", 'V')))],
        ),
        (
            "one\ntwo\nthree",
            (0, 0),
            &[],
            "yyjVp",
            "one\none\nthree",
            (1, 0),
            &[('"', Some(("two", 'V'))), ('0', Some(("one", 'V'))), ('-', None), ('1', Some(("two", 'V')))],
        ),
        (
            "one\ntwo\nthree",
            (0, 0),
            &[],
            "yyjVjp",
            "one\none",
            (1, 0),
            &[
                ('"', Some(("two\nthree", 'V'))),
                ('0', Some(("one", 'V'))),
                ('-', None),
                ('1', Some(("two\nthree", 'V'))),
            ],
        ),
        (
            "one two three",
            (0, 0),
            &[],
            "yiwwviwp",
            "one one three",
            (0, 6),
            &[('"', Some(("two", 'v'))), ('0', Some(("one", 'v'))), ('-', Some(("two", 'v'))), ('1', None)],
        ),
        (
            "one two three",
            (0, 0),
            &[],
            "yiwwviw2p",
            "one oneone three",
            (0, 9),
            &[('"', Some(("two", 'v'))), ('0', Some(("one", 'v'))), ('-', Some(("two", 'v'))), ('1', None)],
        ),
        (
            "one two three",
            (0, 0),
            &[],
            "yiwwvep..",
            "one one",
            (0, 6),
            &[('"', Some(("hre", 'v'))), ('0', Some(("one", 'v'))), ('-', Some(("hre", 'v'))), ('1', None)],
        ),
        (
            "one two",
            (0, 0),
            &[],
            "wviw\"_p",
            "one ",
            (0, 3),
            &[('"', Some(("two", 'v'))), ('0', None), ('-', Some(("two", 'v'))), ('1', None)],
        ),
        (
            "one two",
            (0, 0),
            &[],
            "wviw\"bp",
            "one ",
            (0, 3),
            &[('"', Some(("two", 'v'))), ('0', None), ('-', Some(("two", 'v'))), ('1', None), ('b', None)],
        ),
        (
            "ab cd",
            (0, 0),
            &[('a', "x\ny", 'b')],
            "wvlp",
            "ab ",
            (0, 2),
            &[('"', Some(("cd", 'v'))), ('-', Some(("cd", 'v')))],
        ),
        (
            "ab cd\nef",
            (0, 0),
            &[('a', "x\ny", 'b')],
            "wvl\"ap",
            "ab x\nef y",
            (0, 3),
            &[('"', Some(("cd", 'v'))), ('a', Some(("x\ny", 'b'))), ('-', Some(("cd", 'v')))],
        ),
        (
            "one two\nx",
            (0, 0),
            &[],
            "yiwwvjp",
            "one one",
            (0, 6),
            &[('"', Some(("two\nx", 'v'))), ('0', Some(("one", 'v'))), ('-', None), ('1', Some(("two\nx", 'v')))],
        ),
        ("one", (0, 0), &[], "iab<CR>c<Esc>", "ab\ncone", (1, 0), &[('.', Some(("ab\nc", 'v')))]),
        ("  one", (0, 2), &[], "oab<Esc>", "  one\n  ab", (1, 3), &[('.', Some(("ab", 'v')))]),
        ("one", (0, 0), &[], "\":p", "one", (0, 0), &[('"', None)]),
        (
            "one two",
            (0, 0),
            &[],
            "yiw\".yw",
            "one two",
            (0, 0),
            &[('"', Some(("one", 'v'))), ('0', Some(("one", 'v')))],
        ),
        ("one", (0, 0), &[], "cwxy<Esc>u\".P", "xyone", (0, 1), &[('.', Some(("xy", 'v')))]),
        ("one", (0, 0), &[], "iab<Esc>\".3p", "ababababone", (0, 7), &[('.', Some(("ababab", 'v')))]),
        ("one", (0, 0), &[], "iab<Esc>i<Esc>\".p", "abone", (0, 0), &[('.', Some(("", 'v')))]),
        ("one", (0, 0), &[], "3iab<Esc>", "abababone", (0, 5), &[('.', Some(("ab", 'v')))]),
        (
            "one two three",
            (0, 0),
            &[],
            "yiwwviwpw.",
            "one one ee",
            (0, 8),
            &[('"', Some(("thr", 'v'))), ('0', Some(("one", 'v'))), ('-', Some(("thr", 'v')))],
        ),
        (
            "one two three four",
            (0, 0),
            &[],
            "yiwwviwPw.",
            "one one ee four",
            (0, 8),
            &[('"', Some(("one", 'v'))), ('0', Some(("one", 'v'))), ('-', None)],
        ),
        (
            "aa bb cc dd",
            (0, 0),
            &[],
            "yiwwviwpww.",
            "aa aa cc ",
            (0, 8),
            &[('"', Some(("dd", 'v'))), ('0', Some(("aa", 'v'))), ('-', Some(("dd", 'v')))],
        ),
        // Neovim's `Ctrl+R` types a block yanked with `Ctrl+V` as lines by character.
        (
            "12\n34\nab\ncd",
            (2, 1),
            &[('a', "1\n3", 'b')],
            "i<C-r>a<Esc>",
            "12\n34\na1\n3b\ncd",
            (3, 0),
            &[('a', Some(("1\n3", 'b')))],
        ),
    ];
    check(CASES);
}

/// `.` after a put from a numbered register puts the next one (`"1p...` puts `"1` to `"4`),
/// also after an undo (`"1pu.u.`), and stops at `"9`.
#[test]
fn repeating_a_numbered_put_takes_the_next_register() {
    let mut e = at("a\nb\nc\nd\ne", (0, 0));
    typ(&mut e, "dddddddd");
    assert_eq!(e.text(), "e");
    typ(&mut e, "\"1p...");
    assert_eq!(e.text(), "e\nd\nc\nb\na");
    let mut e = at("a\nb\nc\nd", (0, 0));
    typ(&mut e, "dddddd\"1pu.u.");
    assert_eq!(e.text(), "d\na", "each . after an undo goes one register further");
    let mut e = at("x", (0, 0));
    for c in '1'..='9' {
        e.regs.write(c, Register::new(c.to_string(), RegKind::Linewise));
    }
    typ(&mut e, "\"8p..");
    assert_eq!(e.text(), "x\n8\n9\n9", "no register after \"9");
    // A named register stays the same.
    let mut e = at("x", (0, 0));
    typ(&mut e, "\"ayy\"ap.");
    assert_eq!(e.text(), "x\nx\nx");
}

/// `"x` waits for the register's name (the keymap hands it over) and then for the command;
/// a name that is no register cancels the command.
#[test]
fn a_register_prefix_waits_for_its_name_and_command() {
    let mut e = Editor::new("one two");
    typ(&mut e, "\"");
    assert!(e.awaiting_key() && !e.awaiting_char());
    typ(&mut e, "a");
    assert!(e.awaiting_key(), "the command comes next");
    typ(&mut e, "yw");
    assert!(!e.awaiting_key());
    assert_eq!(e.register('a').map(|r| r.text.as_str()), Some("one "));
    typ(&mut e, "\"!x");
    assert_eq!(e.text(), "ne two", "`\"!` cancelled, `x` ran alone");
    assert_eq!(e.register('-').map(|r| r.text.as_str()), Some("o"));
    typ(&mut e, "i<C-r>");
    assert!(e.awaiting_key(), "Ctrl+R waits for the register");
    typ(&mut e, "<Esc>");
    assert_eq!((e.mode, e.text().as_str()), (Mode::Insert, "ne two"), "Esc cancels Ctrl+R only");
    typ(&mut e, "<C-r><Left>z");
    assert_eq!(e.text(), "zne two", "a move cancels Ctrl+R too");
}

/// `"+p` and `"*P` put what the app read from the system clipboard (whole lines when it ends
/// with a line break); the app is asked to read it only for them, `.` repeating one, and
/// `Ctrl+R +`, never for other puts; nothing read, nothing is put.
#[test]
fn clipboard_puts_use_what_the_app_read() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let k = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    let mut e = at("one", (0, 0));
    assert!(!e.reads_clipboard(&k('p')), "the unnamed register is the editor's own");
    typ(&mut e, "\"+");
    assert!(e.reads_clipboard(&k('p')) && e.reads_clipboard(&k('P')) && !e.reads_clipboard(&k('y')));
    e.set_clipboard_text(Some("two\r\n"));
    typ(&mut e, "p");
    assert_eq!(e.text(), "one\ntwo");
    assert!(e.reads_clipboard(&k('.')), "`.` puts the clipboard again");
    assert!(!e.reads_clipboard(&k('u')));
    e.set_clipboard_text(Some("x"));
    typ(&mut e, ".");
    assert_eq!(e.text(), "one\ntxwo");
    typ(&mut e, "\"*3");
    assert!(e.reads_clipboard(&k('P')), "a count between");
    e.set_clipboard_text(None);
    typ(&mut e, "P");
    assert_eq!(e.text(), "one\ntxwo", "nothing read, nothing put");
    typ(&mut e, "yy");
    assert!(e.reads_clipboard(&k('.')), "a yank changes nothing: `.` still repeats the put");
    typ(&mut e, "x");
    assert!(!e.reads_clipboard(&k('.')), "another change");
    typ(&mut e, "A");
    assert!(!e.reads_clipboard(&k('+')));
    typ(&mut e, "<C-r>");
    assert!(e.reads_clipboard(&k('+')) && e.reads_clipboard(&k('*')) && !e.reads_clipboard(&k('a')));
    e.set_clipboard_text(Some("!"));
    typ(&mut e, "+<Esc>");
    assert_eq!(e.text(), "one\ntwo!");
    typ(&mut e, "v");
    assert!(!e.reads_clipboard(&k('p')));
}

/// A yank into `"+` is kept for a later `"+p` like any register, and the app gets it for the
/// system clipboard with its kind.
#[test]
fn clipboard_registers_keep_their_text() {
    let mut e = at("one\ntwo", (0, 0));
    typ(&mut e, "\"+yy");
    assert_eq!(e.register('*'), Some(&Register::new("one", RegKind::Linewise)));
    assert_eq!(e.register('"'), Some(&Register::new("one", RegKind::Linewise)));
    assert_eq!(e.take_yank().map(|y| y.reg.clipboard_text()), Some("one\n".into()));
}

#[test]
fn appending_follows_the_kinds() {
    let (c, l, b) = (RegKind::Charwise, RegKind::Linewise, RegKind::Blockwise);
    for (old, new, want) in [
        ((c, "a"), (c, "b"), (c, "ab")),
        ((c, "a"), (l, "b"), (l, "a\nb")),
        ((l, "a"), (c, "b"), (l, "a\nb")),
        ((l, "a"), (l, "b"), (l, "a\nb")),
        ((b, "a"), (c, "b"), (b, "a\nb")),
        ((c, "a"), (b, "b"), (c, "ab")),
        ((b, "a"), (l, "b"), (l, "a\nb")),
    ] {
        let mut r = Registers::default();
        r.write('a', Register::new(old.1, old.0));
        r.write('A', Register::new(new.1, new.0));
        assert_eq!(r.get('a'), Some(&Register::new(want.1, want.0)), "{old:?} + {new:?}");
        assert_eq!(r.get('"'), r.get('a'), "the unnamed register is the whole register");
    }
}

#[test]
fn the_delete_ring_shifts_and_small_deletes_go_to_the_minus_register() {
    let mut r = Registers::default();
    let line = |t: &str| Register::new(t, RegKind::Linewise);
    for i in 1..=10 {
        assert!(r.delete(None, line(&i.to_string()), true).is_some());
    }
    let ring: Vec<_> = ('1'..='9').map(|c| r.get(c).unwrap().text.clone()).collect();
    assert_eq!(ring, ["10", "9", "8", "7", "6", "5", "4", "3", "2"]);
    assert_eq!(r.get('-'), None);
    r.delete(None, Register::new("x", RegKind::Charwise), false);
    assert_eq!((r.get('-').unwrap().text.as_str(), r.get('1').unwrap().text.as_str()), ("x", "10"));
    assert_eq!(r.get('"').unwrap().text, "x");
    // `"_` touches nothing and gives the app nothing.
    assert_eq!(r.delete(Some('_'), line("gone"), true), None);
    assert_eq!(r.yank(Some('_'), line("gone")), None);
    assert_eq!((r.get('"').unwrap().text.as_str(), r.get('1').unwrap().text.as_str()), ("x", "10"));
    // A named delete goes to the ring as well, but not to `"-`, and not to the app.
    assert_eq!(r.delete(Some('b'), Register::new("y", RegKind::Charwise), false), None);
    assert_eq!(r.get('-').unwrap().text, "x");
    // A yank without a register goes to `"0` and to the app; into `"+` to the app only.
    assert_eq!(r.yank(None, line("z")).map(|y| y.register), Some('"'));
    assert_eq!(r.get('0'), Some(&line("z")));
    assert_eq!(r.yank(Some('+'), line("w")).map(|y| y.register), Some('+'));
    assert_eq!((r.get('0'), r.get('*')), (Some(&line("z")), Some(&line("w"))));
    // Registers Vim fills itself take nothing.
    assert!(!Registers::can_write(Some('%')) && Registers::can_write(Some('A')) && Registers::can_write(None));
}

#[test]
fn clipboard_text_marks_lines_with_a_line_break() {
    assert_eq!(Register::new("a\nb", RegKind::Linewise).clipboard_text(), "a\nb\n");
    assert_eq!(Register::new("a\nb", RegKind::Blockwise).clipboard_text(), "a\nb\n");
    assert_eq!(Register::new("a\nb", RegKind::Charwise).clipboard_text(), "a\nb");
    assert_eq!(Register::from_clipboard("a\r\nb\r\n"), Register::new("a\nb", RegKind::Linewise));
    assert_eq!(Register::from_clipboard("a\rb"), Register::new("a\nb", RegKind::Charwise));
    assert_eq!(Register::from_clipboard(""), Register::new("", RegKind::Charwise));
}

/// A clipboard the app could not read is unknown, not empty: `"+p` puts nothing then, and what
/// `"+` held (also as the unnamed register) stays for later puts.
#[test]
fn an_unread_clipboard_keeps_what_the_register_held() {
    let mut e = at("one\ntwo", (0, 0));
    typ(&mut e, "\"+yy\"+");
    // The app read nothing for the key that ends the put.
    e.set_clipboard_text(None);
    typ(&mut e, "p");
    assert_eq!(e.text(), "one\ntwo", "nothing put");
    assert_eq!(e.take_register_problem(), None, "the app said why already");
    assert_eq!(e.register('+'), Some(&Register::new("one", RegKind::Linewise)));
    typ(&mut e, "p");
    assert_eq!(e.text(), "one\none\ntwo", "the unnamed register still holds it");
    typ(&mut e, "\"+p");
    assert_eq!(e.text(), "one\none\none\ntwo", "only the one command put nothing");
}

/// A put from an empty register, and a yank into one that takes none, are said (Vim's E353
/// and E354); `"_` puts nothing silently.
#[test]
fn register_problems_are_offered_to_the_app() {
    let mut e = at("one", (0, 0));
    typ(&mut e, "\"bp");
    assert_eq!(e.take_register_problem(), Some(RegProblem::Empty('b')));
    typ(&mut e, "p");
    assert_eq!(e.take_register_problem(), Some(RegProblem::Empty('"')));
    typ(&mut e, "\"_p\":p");
    assert_eq!(e.take_register_problem(), Some(RegProblem::Empty(':')));
    typ(&mut e, "\"%yy");
    assert_eq!(e.take_register_problem(), Some(RegProblem::ReadOnly('%')));
    typ(&mut e, "i<C-r>z");
    assert_eq!(e.take_register_problem(), Some(RegProblem::Empty('z')));
    assert_eq!(e.take_register_problem(), None, "once");
}
