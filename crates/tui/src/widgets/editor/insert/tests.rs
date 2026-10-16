use super::super::tests::{Case, at, check, check_with, typ};
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

/// `[editor] soft_tab_backspace`: Backspace after spaces that reach back to the tab stop
/// deletes them back to it, as Neovim with `softtabstop=4 expandtab` does, also when `.` or a
/// count types the Backspace again (rows from `dev/vim-cases.py` with `{"options": "sts=4"}`).
#[test]
fn backspace_deletes_back_to_the_tab_stop() {
    const CASES: &[Case] = &[
        ("", (0, 0), "i<Tab><Tab><BS><Esc>", "    ", (0, 3), None),
        ("", (0, 0), "i<Tab><Tab><BS><BS>x<Esc>", "x", (0, 0), None),
        ("        x", (0, 8), "i<BS><Esc>", "    x", (0, 3), None),
        ("        x", (0, 8), "i<BS><BS><Esc>", "x", (0, 0), None),
        ("      x", (0, 6), "i<BS><Esc>", "    x", (0, 3), None),
        ("      x", (0, 6), "i<BS><BS><Esc>", "x", (0, 0), None),
        ("   x", (0, 3), "i<BS><Esc>", "x", (0, 0), None),
        ("ab      ", (0, 0), "A<BS><Esc>", "ab  ", (0, 3), None),
        ("abc ", (0, 0), "A<BS><Esc>", "abc", (0, 2), None),
        ("abcd     ", (0, 0), "A<BS><Esc>", "abcd    ", (0, 7), None),
        ("SELECT\n        x", (1, 8), "i<BS><BS><Esc>", "SELECT\nx", (1, 0), None),
        ("SELECT\nx", (1, 0), "i<BS><Esc>", "SELECTx", (0, 5), None),
        ("\t  x", (0, 3), "i<BS><Esc>", "\tx", (0, 0), None),
        ("\u{4E00}\u{4E00}      x", (0, 8), "i<BS><Esc>", "\u{4E00}\u{4E00}    x", (0, 5), None),
        ("        a\n        b", (0, 8), "i<BS><Esc>j0w.", "    a\n    b", (1, 3), None),
        ("        a\n        b", (0, 8), "i<BS><Esc>j0w2.", "    a\nb", (1, 0), None),
        ("", (0, 0), "3A<Tab><Tab><BS><Esc>", "            ", (0, 11), None),
        ("            ", (0, 0), "A<BS>x<Esc>", "        x", (0, 8), None),
        ("    x", (0, 4), "i<BS><Esc>u", "    x", (0, 4), None),
    ];
    check_with(CASES, |e| e.soft_tab_backspace = true);
}

/// Where Vim's `softtabstop` would also delete back to the tab stop but the setting does not:
/// anything but spaces between the cursor and the tab stop (a letter, a tab, a wide character
/// across it), or a Visual mode block's Insert, and Backspace deletes one character, as it
/// does with the setting off.
#[test]
fn backspace_deletes_one_character_unless_only_spaces_reach_the_tab_stop() {
    for (text, cursor, keys, want) in [
        ("ab  ", (0, 0), "A<BS><Esc>", "ab "),
        ("  \t", (0, 0), "A<BS><Esc>", "  "),
        ("\t", (0, 0), "A<BS><Esc>", ""),
        ("abc\u{4E00}  x", (0, 6), "i<BS><Esc>", "abc\u{4E00} x"),
        ("  \u{301}  ", (0, 0), "A<BS><Esc>", "  \u{301} "),
        ("  \u{A0}  ", (0, 0), "A<BS><Esc>", "  \u{A0} "),
        ("x", (0, 0), "I<BS><Esc>", "x"),
        ("\nx", (1, 0), "i<BS><Esc>", "x"),
        ("a\nb", (0, 0), "<C-v>jI<Tab><Tab><BS>x<Esc>", "       xa\n       xb"),
        ("        a\n        b", (0, 8), "<C-v>jI<BS><Esc>", "       a\n        b"),
    ] {
        for on in [true, false] {
            let mut e = at(text, cursor);
            e.soft_tab_backspace = on;
            typ(&mut e, keys);
            assert_eq!(e.text(), want, "{keys:?} on {text:?}, setting {on}");
        }
    }
    let mut e = at("ab      ", (0, 0));
    e.soft_tab_backspace = true;
    typ(&mut e, "A<BS>");
    assert_eq!(e.text(), "ab  ", "spaces only back to the tab stop at column 4");
    typ(&mut e, "<BS>");
    assert_eq!(e.text(), "ab ", "`b` before the tab stop at column 0");
    let mut e = at("        x", (0, 8));
    typ(&mut e, "i<BS><Esc>");
    assert_eq!(e.text(), "       x", "off: one space");
}

/// Each Backspace is part of the Insert session's undo step and `".` loses the spaces it
/// deleted of what the session typed; `Ctrl+W` and `Ctrl+U` are as they were.
#[test]
fn backspace_to_the_tab_stop_undoes_with_its_session() {
    let mut e = at("SELECT", (0, 0));
    e.soft_tab_backspace = true;
    typ(&mut e, "A<Tab><Tab>x<BS><BS><Esc>");
    assert_eq!(e.text(), "SELECT      ", "back to the tab stop at column 12");
    assert_eq!(e.register('.').map(|r| r.text.clone()).as_deref(), Some("      "));
    typ(&mut e, "u");
    assert_eq!(e.text(), "SELECT", "one undo step");
    // Two spaces of autoindent and two typed go together: `".` loses the typed ones only.
    let mut e = at("  a", (0, 0));
    e.soft_tab_backspace = true;
    typ(&mut e, "A<CR>  <BS>x<Esc>");
    assert_eq!(e.text(), "  a\nx");
    assert_eq!(e.register('.').map(|r| r.text.clone()).as_deref(), Some("\nx"));
    let mut e = at("a        ", (0, 0));
    e.soft_tab_backspace = true;
    typ(&mut e, "A<C-w><Esc>");
    assert_eq!(e.text(), "", "Ctrl+W: the blanks and the word");
    let mut e = at("        ", (0, 0));
    e.soft_tab_backspace = true;
    typ(&mut e, "A<C-u><Esc>");
    assert_eq!(e.text(), "", "Ctrl+U: the indent");
}

/// With auto-pairs, Backspace between a pair it put in deletes the pair; the next one, after
/// spaces only, goes back to the tab stop.
#[test]
fn backspace_to_the_tab_stop_with_auto_pairs() {
    let mut e = at("", (0, 0));
    e.soft_tab_backspace = true;
    e.auto_pairs = true;
    typ(&mut e, "i<Tab><Tab>(");
    assert_eq!(e.text(), "        ()");
    typ(&mut e, "<BS>");
    assert_eq!(e.text(), "        ");
    typ(&mut e, "<BS>");
    assert_eq!(e.text(), "    ");
    typ(&mut e, "(<Esc>");
    assert_eq!(e.text(), "    ()");
    // `.` before `(`: Tab Tab, the pair, its deletion, Backspace back to column 8, the pair.
    typ(&mut e, ".");
    assert_eq!(e.text(), "        ()()");
}
