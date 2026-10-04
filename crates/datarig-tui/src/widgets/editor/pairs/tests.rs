use super::super::tests::{at, typ};
use super::*;

fn on(text: &str, cursor: (usize, usize)) -> Editor {
    let mut e = at(text, cursor);
    e.auto_pairs = true;
    e
}

/// `(text, cursor, keys, text after, cursor after)`.
type PairCase<'a> = (&'a str, (usize, usize), &'a str, &'a str, (usize, usize));

/// Each case with auto-pairs on.
fn check(cases: &[PairCase]) {
    for &(text, cursor, keys, want, want_cursor) in cases {
        let mut e = on(text, cursor);
        typ(&mut e, keys);
        assert_eq!((e.text().as_str(), (e.row, e.col)), (want, want_cursor), "{text:?} {keys:?}");
    }
}

#[test]
fn brackets_and_quotes_pair_and_step_over() {
    check(&[
        ("", (0, 0), "i(", "()", (0, 1)),
        ("", (0, 0), "icount(*)", "count(*)", (0, 8)),
        ("", (0, 0), "i[1]{2}", "[1]{2}", (0, 6)),
        ("", (0, 0), "i((x))", "((x))", (0, 5)),
        ("", (0, 0), "i'it'", "'it'", (0, 4)),
        ("", (0, 0), "i\"Mixed\"", "\"Mixed\"", (0, 7)),
        ("", (0, 0), "i`x`", "`x`", (0, 3)),
        // Esc and Normal mode: the closing character stays, nothing pending.
        ("", (0, 0), "i(x<Esc>", "(x)", (0, 1)),
        // A line break inside: the closing one goes to the new line, and is still stepped over.
        ("", (0, 0), "i(<CR>x)", "(\nx)", (1, 2)),
    ]);
}

/// Only where a pair helps: not before a word, not after a letter for quotes (`E'`), not
/// inside strings, quoted names, comments or dollar bodies; a closing character typed by
/// hand is never stepped over.
#[test]
fn no_pair_where_it_would_get_in_the_way() {
    check(&[
        ("x", (0, 0), "i(", "(x", (0, 1)),
        ("", (0, 0), "iE'a", "E'a", (0, 3)),
        ("", (0, 0), "iit''", "it''", (0, 4)),
        ("'abc'", (0, 3), "a(", "'abc('", (0, 5)),
        ("\"abc\"", (0, 3), "a'", "\"abc'\"", (0, 5)),
        ("-- note", (0, 6), "a(", "-- note(", (0, 8)),
        ("/* note */", (0, 2), "a[", "/* [note */", (0, 4)),
        ("select $$ body $$", (0, 13), "a(", "select $$ body( $$", (0, 15)),
        // At the end of an open string: still inside it.
        ("'abc", (0, 3), "a(", "'abc(", (0, 5)),
        // After a closed string: outside.
        ("'abc'", (0, 4), "a ", "'abc' ", (0, 6)),
        ("f()", (0, 1), "a)", "f())", (0, 3)),
        ("", (0, 0), "i(<Left><Right>)", "())", (0, 2)),
    ]);
}

#[test]
fn backspace_deletes_an_empty_pair_it_put_in() {
    check(&[
        ("", (0, 0), "i(<BS>", "", (0, 0)),
        ("", (0, 0), "i'<BS>", "", (0, 0)),
        ("", (0, 0), "i((<BS>", "()", (0, 1)),
        ("", (0, 0), "i(x<BS><BS>", "", (0, 0)),
        // Not one typed by hand.
        ("()", (0, 0), "a<BS>", ")", (0, 0)),
    ]);
}

/// `.` and a count do what auto-pairs did, not the keys again: the same text with the
/// setting off, or in a string.
#[test]
fn repeats_do_what_happened() {
    let mut e = on("x;\ny", (0, 0));
    typ(&mut e, "icount(*)<Esc>");
    assert_eq!(e.text(), "count(*)x;\ny");
    e.auto_pairs = false;
    typ(&mut e, "j0.");
    assert_eq!(e.text(), "count(*)x;\ncount(*)y");
    let mut e = on("'a b'", (0, 2));
    e.auto_pairs = false;
    typ(&mut e, "a(<Esc>");
    e.auto_pairs = true;
    typ(&mut e, "$.");
    assert_eq!(e.text(), "'a (b'(", "a key typed without a pair repeats without one");
    let mut e = on("", (0, 0));
    typ(&mut e, "3if(x)<Esc>");
    assert_eq!(e.text(), "f(x)f(x)f(x)");
    typ(&mut e, "u");
    assert_eq!(e.text(), "", "one undo step");
    // `".` holds what was typed.
    let mut e = on("", (0, 0));
    typ(&mut e, "i(x<Esc>");
    assert_eq!(e.register('.').map(|r| r.text.as_str()), Some("(x"));
}

/// Off (the default): every key types itself.
#[test]
fn off_by_default() {
    let mut e = at("", (0, 0));
    typ(&mut e, "i(\"'");
    assert_eq!(e.text(), "(\"'");
}

/// Lines put in or taken out before the closing character (a register of several lines, a
/// paste, Backspace at a line's start) keep it pending: it is still stepped over.
#[test]
fn the_closing_character_is_followed_across_lines() {
    check(&[
        ("a\nb", (0, 0), "yjo(<C-r>\"x)<Esc>", "a\n(a\nb\nx)\nb", (3, 1)),
        ("", (0, 0), "i(<CR><BS>x)", "(x)", (0, 3)),
        ("", (0, 0), "i(<CR><CR><C-w><C-w>x)", "(x)", (0, 3)),
    ]);
    let mut e = on("", (0, 0));
    typ(&mut e, "i(");
    e.paste("1,\n2");
    typ(&mut e, ")");
    assert_eq!(e.text(), "(1,\n2)");
}

/// A count types what the session put in that many times, its closing characters included:
/// `3i(x<Esc>` is `(x)(x)(x)`.
#[test]
fn a_count_repeats_the_whole_pair() {
    check(&[
        ("", (0, 0), "3i(x<Esc>", "(x)(x)(x)", (0, 7)),
        ("", (0, 0), "2if('a<Esc>", "f('a')f('a')", (0, 9)),
        ("", (0, 0), "2o(<Esc>", "\n()\n()", (2, 0)),
    ]);
}
