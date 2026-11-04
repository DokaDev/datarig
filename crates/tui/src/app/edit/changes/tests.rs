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
                new: Some("id=DEFAULT \u{B7} name='Kim' \u{B7} note=NULL".into()),
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
