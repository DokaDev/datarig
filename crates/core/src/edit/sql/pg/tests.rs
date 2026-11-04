use super::*;
use crate::driver::structure::{Collation, ColumnFill, StructureColumn};
use crate::driver::{Cell, ValueKind};
use crate::edit::{Choice, Duplicates, EditBatch, EditTarget, Facts, Identity, PlanError, count_statements, plan};
use std::collections::{BTreeMap, BTreeSet};

fn scol(name: &str, ty: &str) -> StructureColumn {
    StructureColumn {
        name: name.into(),
        type_name: ty.into(),
        not_null: false,
        default: None,
        fill: ColumnFill::Default,
        enum_labels: Vec::new(),
        collation: None,
    }
}

fn exactness(column: &StructureColumn) -> Exactness {
    Postgres.exactness(&ColumnInfo { kind: ValueKind::Other, result_type: "", column })
}

#[test]
fn exact_types_are_an_allowlist() {
    for ty in [
        "integer",
        "bigint",
        "smallint",
        "boolean",
        "text",
        "character varying(20)",
        "character(3)",
        "uuid",
        "date",
        "timestamp with time zone",
        "timestamp(3) without time zone",
        "time without time zone",
        "bytea",
        "numeric(10,2)",
        "bit(3)",
        "bit varying(5)",
    ] {
        assert_eq!(exactness(&scol("c", ty)), Exactness::Exact, "{ty}");
    }
    let inexact = [
        ("double precision", Inexact::Approximate),
        ("real", Inexact::Approximate),
        ("jsonb", Inexact::Document),
        ("xml", Inexact::Document),
        ("point", Inexact::Geometry),
        ("numeric", Inexact::Unknown),
        ("interval", Inexact::Unknown),
        ("time with time zone", Inexact::Unknown),
        ("integer[]", Inexact::Unknown),
        ("inet", Inexact::Unknown),
        ("citext", Inexact::Unknown),
        ("money", Inexact::Unknown),
        ("my_domain", Inexact::Unknown),
    ];
    for (ty, why) in inexact {
        assert_eq!(exactness(&scol("c", ty)), Exactness::Inexact(why), "{ty}");
    }
    let mood = StructureColumn { enum_labels: vec!["sad".into()], ..scol("m", "mood") };
    assert_eq!(exactness(&mood), Exactness::Exact, "an enum");
}

fn tcol(c: StructureColumn, kind: ValueKind) -> Option<TableColumn> {
    let exact = Postgres.exactness(&ColumnInfo { kind, result_type: "", column: &c });
    Some(TableColumn { column: c, kind, exact, repeated: false })
}

/// `public.Users` as (id identity, name text, note text ci, score float8).
fn target(identity: Identity) -> EditTarget {
    let ci = Collation { name: "und-ci".into(), charset: None, deterministic: false };
    EditTarget {
        table: TableRef { schema: "public".into(), name: "Users".into() },
        columns: vec![
            tcol(StructureColumn { fill: ColumnFill::IdentityAlways, ..scol("id", "bigint") }, ValueKind::Integer),
            tcol(scol("name", "text"), ValueKind::Text),
            tcol(StructureColumn { collation: Some(ci), ..scol("note", "text") }, ValueKind::Text),
            tcol(scol("score", "double precision"), ValueKind::Float),
        ],
        identity,
        all_columns_shown: true,
        unshown_required: Vec::new(),
    }
}

fn orig(r: usize) -> Vec<Cell> {
    vec![Some(r.to_string()), Some(format!("it's {r}")), None, Some("1.5".into())]
}

fn run(t: &EditTarget, b: &EditBatch, matches: &[(usize, u64)], choices: &[(usize, Choice)]) -> Vec<String> {
    let matches: BTreeMap<usize, u64> = matches.iter().copied().collect();
    let choices: BTreeMap<usize, Choice> = choices.iter().copied().collect();
    let whole = |_: usize, _: usize| true;
    let p = plan(
        t,
        b,
        &Postgres,
        &Facts { whole: &whole, matches: &matches, choices: &choices, invalid: &BTreeSet::new() },
    );
    p.unwrap_or_else(|e| panic!("{e:?}")).steps.into_iter().map(|s| s.sql).collect()
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

#[test]
fn a_keyed_row_is_checked_then_changed_and_given_back() {
    let t = target(Identity::Key { primary: true, columns: vec![0] });
    let mut b = EditBatch::new();
    b.set(&[(4, 1)], &text("a\\b"), orig);
    b.set(&[(4, 2)], &text("x"), orig);
    let w = r#""id" = '4' AND "name" = 'it''s 4' AND "note" IS NULL"#;
    assert_eq!(
        run(&t, &b, &[], &[]),
        [
            format!(r#"SELECT 1 FROM "public"."Users" WHERE {w} FOR UPDATE NOWAIT"#),
            format!(r#"UPDATE "public"."Users" SET "name" = E'a\\b', "note" = 'x' WHERE {w} RETURNING *"#),
        ]
    );
}

#[test]
fn a_delete_and_inserts() {
    let t = target(Identity::Key { primary: true, columns: vec![0] });
    let mut b = EditBatch::new();
    b.delete(&[2], orig);
    b.add(2, None, vec![Value::Default; 4]);
    b.add(2, None, vec![text("10"), text("n"), Value::Null, Value::Default]);
    b.add(2, None, vec![Value::Default, text("m"), Value::Default, Value::Default]);
    let w = r#""id" = '2' AND "name" = 'it''s 2' AND "note" IS NULL"#;
    assert_eq!(
        run(&t, &b, &[], &[])[1..],
        [
            format!(r#"DELETE FROM "public"."Users" WHERE {w}"#),
            r#"INSERT INTO "public"."Users" DEFAULT VALUES RETURNING *"#.to_string(),
            r#"INSERT INTO "public"."Users" ("id", "name", "note") OVERRIDING SYSTEM VALUE VALUES ('10', 'n', NULL) RETURNING *"#.to_string(),
            r#"INSERT INTO "public"."Users" ("name") VALUES ('m') RETURNING *"#.to_string(),
        ],
        "score is inexact: not in the delete's condition"
    );
}

#[test]
fn one_of_several_identical_rows_is_picked_by_its_place() {
    let t = target(Identity::Values { columns: vec![0, 1, 2], duplicates: Duplicates::Choose });
    let mut b = EditBatch::new();
    b.set(&[(1, 1)], &text("x"), orig);
    b.delete(&[2], orig);
    let counts = count_statements(&t, &b, &Postgres, &|_, _| true).unwrap();
    assert!(counts[0].1.starts_with(r#"SELECT count(*) FROM "public"."Users" WHERE "id" = '1'"#));
    let steps = run(&t, &b, &[(1, 3), (2, 2)], &[(1, Choice::One), (2, Choice::One)]);
    let w = |r: usize| format!(r#""id" = '{r}' AND "name" = 'it''s {r}' AND "note" IS NULL"#);
    let one = |r| format!(r#"(tableoid, ctid) = (SELECT tableoid, ctid FROM "public"."Users" WHERE {} LIMIT 1)"#, w(r));
    assert_eq!(steps[2], format!(r#"DELETE FROM "public"."Users" WHERE {}"#, one(2)));
    assert_eq!(steps[3], format!(r#"UPDATE "public"."Users" SET "name" = 'x' WHERE {} RETURNING *"#, one(1)));
    let all = run(&t, &b, &[(1, 3), (2, 2)], &[(1, Choice::All), (2, Choice::All)]);
    assert_eq!(all[3], format!(r#"UPDATE "public"."Users" SET "name" = 'x' WHERE {} RETURNING *"#, w(1)));
}

#[test]
fn an_identity_column_of_an_existing_row_is_never_set() {
    let t = target(Identity::Key { primary: true, columns: vec![0] });
    let mut b = EditBatch::new();
    b.set(&[(1, 0)], &text("9"), orig);
    let whole = |_: usize, _: usize| true;
    let none = BTreeMap::new();
    let got = plan(
        &t,
        &b,
        &Postgres,
        &Facts { whole: &whole, matches: &none, choices: &BTreeMap::new(), invalid: &BTreeSet::new() },
    );
    assert!(matches!(got, Err(PlanError::Column { .. })), "{got:?}");
}

#[test]
fn a_value_finds_its_row_through_an_index_and_byte_for_byte_where_the_collation_is_not() {
    let t = target(Identity::Key { primary: true, columns: vec![0] });
    let [_, name, note, _] = &t.columns[..] else { unreachable!() };
    assert_eq!(Postgres.eq_exact(name.as_ref().unwrap(), Some("a")), r#""name" = 'a'"#);
    assert_eq!(Postgres.eq_exact(note.as_ref().unwrap(), Some("a")), r#""note" = 'a' AND "note" COLLATE "C" = 'a'"#);
    assert_eq!(Postgres.eq_exact(note.as_ref().unwrap(), None), r#""note" IS NULL"#);
}

#[test]
fn quoted_names_are_read_back_as_the_catalogs() {
    let mut t = target(Identity::Key { primary: true, columns: vec![0] });
    t.table = TableRef { schema: "My \"S\"".into(), name: "a.b".into() };
    let mut b = EditBatch::new();
    b.set(&[(1, 1)], &text("x"), orig);
    assert!(run(&t, &b, &[], &[])[1].starts_with(r#"UPDATE "My ""S"""."a.b" SET"#));
}

#[test]
fn the_counts_of_rows_found_by_values_are_on_the_allowlist_of_a_count() {
    let t = target(Identity::Values { columns: vec![0, 1, 2], duplicates: Duplicates::Refuse });
    let mut b = EditBatch::new();
    b.set(&[(1, 1)], &text("x"), orig);
    b.delete(&[2], |_| vec![Some("2".into()), None, Some("ß".into()), Some("1.5".into())]);
    let whole = |_: usize, _: usize| true;
    let counts = count_statements(&t, &b, &Postgres, &whole).unwrap();
    assert_eq!(counts.len(), 2);
    for (_, sql) in counts {
        assert_eq!(crate::sql::risk::repeat::repeatable(&sql), Ok(()), "{sql}");
    }
}
