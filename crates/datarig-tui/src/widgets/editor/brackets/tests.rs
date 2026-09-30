use super::super::Editor;
use super::super::tests::{at, typ};

/// Where `%` from `from` leads.
fn pct(e: &mut Editor, from: (usize, usize)) -> (usize, usize) {
    (e.row, e.col) = from;
    typ(e, "%");
    (e.row, e.col)
}

/// Brackets in strings, quoted identifiers, dollar bodies and comments do not count for `%`;
/// from a bracket in one of these, only the brackets in it do.
#[test]
fn brackets_in_strings_and_comments_do_not_count() {
    let mut e = Editor::new("f(')', \"(\", x) -- )");
    assert_eq!(pct(&mut e, (0, 1)), (0, 13));
    assert_eq!(pct(&mut e, (0, 13)), (0, 1));
    assert_eq!(pct(&mut e, (0, 0)), (0, 13), "the first bracket after the cursor");
    assert_eq!(pct(&mut e, (0, 3)), (0, 3), "the string holds no match");
    assert_eq!(pct(&mut e, (0, 18)), (0, 18), "nor does the comment");
    let mut e = Editor::new("'x (y) z'");
    assert_eq!(pct(&mut e, (0, 3)), (0, 5), "brackets within one string match");
    let mut e = Editor::new("(a /* ) */\n) $$ ( $$ )");
    assert_eq!(pct(&mut e, (0, 0)), (1, 0), "across lines, past a block comment");
    assert_eq!(pct(&mut e, (1, 0)), (0, 0));
    assert_eq!(pct(&mut e, (1, 10)), (1, 10), "the dollar body's bracket is not its match");
    let mut e = Editor::new("f(\")\")");
    assert_eq!(pct(&mut e, (0, 1)), (0, 5), "a quoted identifier");
}

/// The bracket objects skip the same brackets; from inside a string they take the block around
/// it.
#[test]
fn bracket_objects_skip_strings() {
    for from in [(0, 12), (0, 3), (0, 8)] {
        let mut e = at("f(')', \"(\", x) -- )", from);
        typ(&mut e, "di(");
        assert_eq!((e.text().as_str(), e.col), ("f() -- )", 2), "from {from:?}");
    }
    let mut e = at("SELECT coalesce(a, ')')", (0, 21));
    typ(&mut e, "ci(b<Esc>");
    assert_eq!(e.text(), "SELECT coalesce(b)");
}

/// A match further than the lines searched first is found (the search takes more lines), and
/// none is found in the whole text when there is none.
#[test]
fn far_brackets_are_found() {
    let text = format!("(\n{}\n)\n(", "x\n".repeat(300));
    let mut e = Editor::new(&text);
    e.region = 4;
    let last = e.lines.len() - 1;
    assert_eq!(pct(&mut e, (0, 0)), (last - 1, 0));
    assert_eq!(pct(&mut e, (last - 1, 0)), (0, 0));
    assert_eq!(pct(&mut e, (last, 0)), (last, 0), "unmatched");
    (e.row, e.col) = (150, 0);
    typ(&mut e, "di(");
    assert_eq!(e.text(), "(\n)\n(");
}
