use super::super::tests::{at, typ};
use super::*;

#[test]
fn the_statement_under_the_cursor_or_the_selection() {
    let mut e = at("select 1;\n  select a,\n  b from t;\nselect 3", (1, 4));
    let (a, b) = e.sql_target().unwrap();
    assert_eq!(e.text_between(a, b), "select a,\n  b from t;");
    assert_eq!((e.indent_before(a), e.indent_before(a + 1)), ("  ".to_string(), String::new()));
    // By character: what is selected, Visual mode ends.
    let mut e = at("select 1, 2 from t", (0, 7));
    typ(&mut e, "vee");
    let (a, b) = e.sql_target().unwrap();
    assert_eq!(e.text_between(a, b), "1, 2");
    assert_eq!(e.mode, Mode::Normal);
    // By line and by block: whole lines.
    let mut e = at("select 1;\n  select 2;\nselect 3", (1, 4));
    typ(&mut e, "Vj");
    let (a, b) = e.sql_target().unwrap();
    assert_eq!(e.text_between(a, b), "  select 2;\nselect 3");
    let mut e = at("select 1;\nselect 2", (0, 2));
    typ(&mut e, "<C-v>jl");
    let (a, b) = e.sql_target().unwrap();
    assert_eq!(e.text_between(a, b), "select 1;\nselect 2");
    assert_eq!(e.lines_target(0, 1), (0, 18));
    // Nothing but blanks: no statement.
    assert!(at("  \n ", (0, 0)).sql_target().is_none());
}

/// One undo step; the cursor at its start, and back there after `u`.
#[test]
fn a_replaced_span_is_one_undo_step() {
    let mut e = at("select 1;\nselect a,b from t;", (1, 9));
    e.replace_with(10, 29, "select\n  a,\n  b\nfrom\n  t;");
    assert_eq!(e.text(), "select 1;\nselect\n  a,\n  b\nfrom\n  t;");
    assert_eq!((e.row, e.col), (1, 0));
    typ(&mut e, "u");
    assert_eq!(e.text(), "select 1;\nselect a,b from t;");
    typ(&mut e, "<C-r>");
    assert_eq!(e.text(), "select 1;\nselect\n  a,\n  b\nfrom\n  t;");
}

/// The same as `gcc` / Visual `gc`, and `.` repeats it.
#[test]
fn the_comment_toggle_is_gc() {
    let mut e = at("select 1;\nselect 2;\nselect 3;", (0, 3));
    e.comment_toggle();
    assert_eq!(e.text(), "-- select 1;\nselect 2;\nselect 3;");
    typ(&mut e, "j.");
    assert_eq!(e.text(), "-- select 1;\n-- select 2;\nselect 3;");
    e.comment_toggle();
    assert_eq!(e.text(), "-- select 1;\nselect 2;\nselect 3;");
    typ(&mut e, "ggVG");
    e.comment_toggle();
    assert_eq!(e.text(), "-- -- select 1;\n-- select 2;\n-- select 3;");
    assert_eq!(e.mode, Mode::Normal);
    // From Insert mode: as after Esc.
    let mut e = at("select 1;", (0, 0));
    typ(&mut e, "Ax");
    e.comment_toggle();
    assert_eq!((e.text().as_str(), e.mode), ("-- select 1;x", Mode::Normal));
    typ(&mut e, "u");
    assert_eq!(e.text(), "select 1;x", "the typing and the toggle are two undo steps");
}
