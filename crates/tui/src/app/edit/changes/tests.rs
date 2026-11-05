use super::*;
use datarig_core::driver::structure::StructureColumn;
use datarig_core::edit::{Duplicates, Exactness, TableRef};

fn tcol(name: &str) -> Option<TableColumn> {
    let column = StructureColumn {
        name: name.into(),
        type_name: "text".into(),
        not_null: false,
        default: None,
        fill: ColumnFill::Default,
        enum_labels: Vec::new(),
        collation: None,
    };
    Some(TableColumn { column, kind: ValueKind::Text, exact: Exactness::Exact, repeated: false })
}

/// `app.t` shown as (id, name, note, upper(name) computed), found by `identity`.
fn target(identity: Identity) -> EditTarget {
    EditTarget {
        table: TableRef { schema: "app".into(), name: "t".into() },
        columns: vec![tcol("id"), tcol("name"), tcol("note"), None],
        identity,
        all_columns_shown: true,
        unshown_required: Vec::new(),
    }
}

fn names() -> Vec<String> {
    ["id", "name", "note", "upper"].map(String::from).to_vec()
}

/// Row `r` as read: (r, "n<r>", NULL, "N<r>").
fn orig(r: usize) -> Vec<Cell> {
    vec![Some(r.to_string()), Some(format!("n{r}")), None, Some(format!("N{r}"))]
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn line(mark: Mark, at: RowAt, col: Option<usize>, row: Option<Pointer>, column: &str) -> ListLine {
    ListLine { mark, at, col, row, column: column.into(), old: None, new: None }
}

#[test]
fn every_changed_cell_is_a_line_grouped_by_row_deletes_then_updates_then_new_rows() {
    let t = target(Identity::Key { primary: true, columns: vec![0] });
    let mut b = EditBatch::new();
    let id = b.add(9, None, vec![Value::Default, text("Kim"), Value::Null, Value::Default]);
    b.set(&[(1, 1)], &text("x"), orig);
    b.set(&[(1, 2)], &text("it's"), orig);
    b.delete(&[4], orig);
    let got = list_lines(&t, &b, &names());
    let key = |k: &str| Some(Pointer::Key(k.into()));
    let set = |col: usize, row: Option<Pointer>, column: &str, old: &str, new: &str| ListLine {
        old: Some(old.into()),
        new: Some(new.into()),
        ..line(Mark::Changed, RowAt::Read(1), Some(col), row, column)
    };
    assert_eq!(
        got,
        [
            line(Mark::Deleted, RowAt::Read(4), None, key("id=4"), ""),
            set(1, key("id=1"), "name", "'n1'", "'x'"),
            set(2, None, "note", "NULL", "'it's'"),
            ListLine {
                new: Some("name='Kim' \u{B7} note=NULL \u{B7} DEFAULT \u{D7}1".into()),
                ..line(Mark::Inserted, RowAt::New(id), None, Some(Pointer::New(1)), "")
            },
        ]
    );
}

#[test]
fn rows_found_by_values_are_named_by_their_grid_row_and_keys_and_values_are_cut() {
    let t = target(Identity::Values { columns: vec![0, 1], duplicates: Duplicates::Refuse });
    let mut b = EditBatch::new();
    b.set(&[(30, 2)], &text("call back"), orig);
    let got = list_lines(&t, &b, &names());
    assert_eq!(got[0].row, Some(Pointer::Row(31)));
    assert_eq!((got[0].old.as_deref(), got[0].new.as_deref()), (Some("NULL"), Some("'call back'")));
    // A composite key, cut at 24 columns; a long value at 40, a line break as one character.
    let t = target(Identity::Key { primary: true, columns: vec![0, 1] });
    let mut b = EditBatch::new();
    let long = format!("a\n{}", "b".repeat(60));
    b.set(&[(7, 2)], &text(&long), |r| {
        let mut o = orig(r);
        o[1] = Some("a rather long name".into());
        o
    });
    let got = list_lines(&t, &b, &names());
    let Some(Pointer::Key(k)) = &got[0].row else { panic!("{got:?}") };
    assert!(k.starts_with("id=7,name=a rather") && crate::text::width(k) <= 24, "{k}");
    let new = got[0].new.as_deref().unwrap();
    assert!(new.starts_with("'a") && new.ends_with("\u{2026}'") && !new.contains('\n'), "{new}");
    assert!(crate::text::width(new) <= 42, "{new}");
}

#[test]
fn a_new_row_lists_the_values_set_and_counts_the_columns_left_to_their_default() {
    let t = target(Identity::Key { primary: true, columns: vec![0] });
    let new = |cells: Vec<Value>| {
        let mut b = EditBatch::new();
        b.add(9, None, cells);
        list_lines(&t, &b, &names()).remove(0).new.unwrap()
    };
    // Every column set: no count; the computed column is never one.
    assert_eq!(new(vec![text("1"), text("a"), Value::Null, Value::Default]), "id='1' \u{B7} name='a' \u{B7} note=NULL");
    assert_eq!(new(vec![Value::Default; 4]), "DEFAULT \u{D7}3");
    assert_eq!(new(vec![Value::Default, Value::Default, text("x"), Value::Default]), "note='x' \u{B7} DEFAULT \u{D7}2");
}

#[test]
fn control_characters_of_values_names_and_keys_are_shown_escaped() {
    let t = target(Identity::Key { primary: true, columns: vec![1] });
    let mut names = names();
    names[2] = "no\tte".into();
    let mut b = EditBatch::new();
    b.set(&[(0, 2)], &text("a\u{7}b\r\nc\td \\n \\q \u{1b}\u{7f}\u{85}"), |r| {
        let mut o = orig(r);
        o[1] = Some("k\ny".into());
        o
    });
    let got = list_lines(&t, &b, &names);
    assert_eq!(got[0].row, Some(Pointer::Key(r"name=k\ny".into())));
    assert_eq!(got[0].column, r"no\tte");
    assert_eq!(got[0].new.as_deref(), Some(r"'a\x07b\r\nc\td \\n \q \x1b\x7f\u{85}'"));
    // A new row's too.
    let mut b = EditBatch::new();
    b.add(9, None, vec![Value::Default, text("x\ny"), Value::Default, Value::Default]);
    assert_eq!(list_lines(&t, &b, &names)[0].new.as_deref(), Some("name='x\\ny' \u{B7} DEFAULT \u{D7}2"));
}

#[test]
fn a_row_s_problems_follow_its_lines_and_a_row_with_no_line_is_named_before_its_own() {
    let t = target(Identity::Key { primary: true, columns: vec![0] });
    let mut b = EditBatch::new();
    let id = b.add(9, None, vec![Value::Default; 4]);
    b.set(&[(1, 1), (1, 2)], &text("x"), orig);
    b.delete(&[4], orig);
    let list = list_lines(&t, &b, &names());
    let problem =
        |at: Option<(RowAt, Option<usize>)>| Problem { text: String::new(), bare: String::new(), at, choice: false };
    let problems = [
        problem(Some((RowAt::New(id), Some(1)))),
        problem(Some((RowAt::Read(7), None))),
        problem(None),
        problem(Some((RowAt::Read(1), Some(2)))),
        problem(Some((RowAt::Read(4), None))),
        problem(Some((RowAt::New(id), None))),
    ];
    let got = entries(&problems, &list, |at| row_pointer(&t, &b, &names(), at));
    use ListEntry::{Line, Problem as P, Row};
    assert_eq!(
        got,
        [P(2), Line(0), P(4), Line(1), Line(2), P(3), Line(3), P(0), P(5), Row(RowAt::Read(7), Pointer::Row(8)), P(1)]
    );
    // How a problem names its row: as the list does.
    assert_eq!(row_pointer(&t, &b, &names(), RowAt::Read(1)), Pointer::Key("id=1".into()));
    assert_eq!(row_pointer(&t, &b, &names(), RowAt::New(id)), Pointer::New(1));
}
