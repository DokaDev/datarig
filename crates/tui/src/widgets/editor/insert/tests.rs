use super::super::tests::{Case, at, check, typ};
use super::*;

/// `Ctrl+W`, `Ctrl+U` (they did nothing before), autoindent on Enter, `o` and `O`, as Vim
/// with 'autoindent' does them.
#[test]
fn insert_keys() {
    const CASES: &[Case] = &[
        ("foo\n    bar baz.qux  ", (1, 0), "A<C-w><Esc>", "foo\n    bar baz.", (1, 11), None),
        ("foo\n    bar baz.qux  ", (1, 0), "A<C-w><C-w><Esc>", "foo\n    bar baz", (1, 10), None),
        ("foo\n    bar baz.qux  ", (1, 0), "A<C-w><C-w><C-w><Esc>", "foo\n    bar ", (1, 7), None),
        ("foo\n    bar baz.qux  ", (1, 0), "A<C-w><C-w><C-w><C-w><C-w><Esc>", "foo\n", (1, 0), None),
        ("foo\n    bar baz.qux  ", (1, 0), "A<C-w><C-w><C-w><C-w><C-w><C-w><Esc>", "foo", (0, 2), None),
        ("foo\n    bar baz.qux  ", (1, 0), "A<C-u><Esc>", "foo\n    ", (1, 3), None),
        ("foo\n    bar baz.qux  ", (1, 0), "A<C-u><C-u><Esc>", "foo\n", (1, 0), None),
        ("foo\n    bar baz.qux  ", (1, 0), "Axy<C-u><Esc>", "foo\n    bar baz.qux  ", (1, 16), None),
        ("foo\n    bar baz.qux  ", (1, 0), "Axy<C-u><C-u><Esc>", "foo\n    ", (1, 3), None),
        ("foo\n    bar baz.qux  ", (1, 0), "Axy z<C-w><C-w><C-w><Esc>", "foo\n    bar baz.", (1, 11), None),
        ("foo\n    bar baz.qux  ", (1, 0), "I<C-u><Esc>", "foo\nbar baz.qux  ", (1, 0), None),
        ("foo\n    bar baz.qux  ", (1, 0), "I<C-w><Esc>", "foo\nbar baz.qux  ", (1, 0), None),
        ("foo\n    bar baz.qux  ", (1, 10), "i<C-w><Esc>", "foo\n    bar z.qux  ", (1, 7), None),
        ("foo\n    bar baz.qux  ", (1, 10), "a<C-w><Esc>", "foo\n    bar .qux  ", (1, 7), None),
        ("foo\nbar", (1, 0), "i<C-w><Esc>", "foobar", (0, 2), None),
        ("foo\nbar", (1, 0), "i<C-u><Esc>", "foobar", (0, 2), None),
        ("foo\nbar", (0, 0), "i<C-u><Esc>", "foo\nbar", (0, 0), None),
        ("foo\n    bar", (1, 0), "A<CR>x<Esc>", "foo\n    bar\n    x", (2, 4), None),
        ("foo\n    bar", (1, 0), "A<CR><Esc>", "foo\n    bar\n", (2, 0), None),
        ("foo\n    bar", (1, 0), "A<CR><CR>x<Esc>", "foo\n    bar\n\n    x", (3, 4), None),
        ("foo\n    bar", (1, 0), "ox<Esc>", "foo\n    bar\n    x", (2, 4), None),
        ("foo\n    bar", (1, 0), "Ox<Esc>", "foo\n    x\n    bar", (1, 4), None),
        ("foo\n    bar", (1, 0), "o<Esc>", "foo\n    bar\n", (2, 0), None),
        ("foo\n    bar", (1, 6), "i<CR><Esc>", "foo\n    ba\n    r", (2, 3), None),
        ("foo\n    bar", (1, 2), "i<CR>x<Esc>", "foo\n  \n  xbar", (2, 2), None),
        ("  foo", (0, 0), "A<CR><C-u>x<Esc>", "  foo\nx", (1, 0), None),
        ("  foo", (0, 0), "A<CR><C-w>x<Esc>", "  foo\nx", (1, 0), None),
    ];
    check(CASES);
}

/// `Ctrl+W` and `Ctrl+U` in Insert mode delete before the cursor (they used to be ignored) and
/// belong to the Insert session's undo step.
#[test]
fn ctrl_w_and_ctrl_u_delete_before_the_cursor() {
    let mut e = at("SELECT a, bcd", (0, 0));
    typ(&mut e, "A<C-w>");
    assert_eq!((e.text().as_str(), e.mode), ("SELECT a, ", Mode::Insert));
    typ(&mut e, "x<C-u>");
    assert_eq!(e.text(), "SELECT a, ", "Ctrl+U: what was typed first");
    typ(&mut e, "<C-u>");
    assert_eq!(e.text(), "");
    typ(&mut e, "<Esc>u");
    assert_eq!(e.text(), "SELECT a, bcd", "one undo step");
}

/// An arrow starts the Insert session again for `Ctrl+W` / `Ctrl+U`, and the autoindent of a
/// line left empty goes.
#[test]
fn arrows_and_autoindent() {
    let mut e = at("  abc", (0, 0));
    typ(&mut e, "A de<Left><Right><C-u>");
    assert_eq!(e.text(), "  ", "the whole line up to the indent, not only what was typed");
    let mut e = at("  abc", (0, 0));
    typ(&mut e, "A<CR><Up><Esc>");
    assert_eq!(e.text(), "  abc\n", "the indent of the line left goes");
    let mut e = at("\tabc", (0, 0));
    typ(&mut e, "A<CR>x<Esc>");
    assert_eq!(e.text(), "\tabc\n\tx", "tabs are indent too");
}
