use super::*;
use crate::driver::structure::{Collation, ColumnFill, StructureColumn};
use crate::driver::{Cell, ValueKind};
use crate::edit::{Choice, Duplicates, EditBatch, EditTarget, Facts, Identity, plan};
use std::collections::BTreeMap;

const MY: MySql = MySql(MySqlEdit { nowait: true });

fn scol(name: &str, ty: &str, collation: Option<&str>) -> StructureColumn {
    StructureColumn {
        name: name.into(),
        type_name: ty.into(),
        not_null: false,
        default: None,
        fill: ColumnFill::Default,
        enum_labels: Vec::new(),
        collation: collation.map(|n| Collation {
            name: n.into(),
            charset: Some("utf8mb4".into()),
            deterministic: false,
        }),
    }
}

#[test]
fn exact_types_are_an_allowlist() {
    let exactness = |ty: &str| {
        let column = scol("c", ty, Some("utf8mb4_0900_ai_ci"));
        MY.exactness(&ColumnInfo { kind: ValueKind::Other, result_type: "", column: &column })
    };
    for ty in [
        "int",
        "bigint unsigned",
        "tinyint(1)",
        "decimal(10,2)",
        "bit(3)",
        "year",
        "date",
        "datetime(3)",
        "timestamp(6)",
        "time",
        "varchar(20)",
        "char(3)",
        "longtext",
        "enum('a','b')",
        "set('p','q')",
        "binary(16)",
        "varbinary(8)",
        "blob",
    ] {
        assert_eq!(exactness(ty), Exactness::Exact, "{ty}");
    }
    for (ty, why) in [
        ("float", Inexact::Approximate),
        ("double", Inexact::Approximate),
        ("json", Inexact::Document),
        ("point", Inexact::Geometry),
        ("geometry", Inexact::Geometry),
        ("vector(3)", Inexact::Unknown),
    ] {
        assert_eq!(exactness(ty), Exactness::Inexact(why), "{ty}");
    }
}

#[test]
fn text_is_exact_only_in_a_character_set_that_converts_one_to_one() {
    let exactness = |charset: Option<&str>| {
        let mut column = scol("c", "varchar(10)", Some("x"));
        column.collation =
            charset.map(|cs| Collation { name: format!("{cs}_bin"), charset: Some(cs.into()), deterministic: true });
        MY.exactness(&ColumnInfo { kind: ValueKind::Text, result_type: "", column: &column })
    };
    for cs in ["utf8mb4", "utf8mb3", "latin1", "ascii", "UTF16"] {
        assert_eq!(exactness(Some(cs)), Exactness::Exact, "{cs}");
    }
    for cs in [Some("sjis"), Some("cp932"), Some("ujis"), Some("gbk"), None] {
        assert_eq!(exactness(cs), Exactness::Inexact(Inexact::Unknown), "{cs:?}");
    }
}

fn tcol(c: StructureColumn, kind: ValueKind) -> Option<TableColumn> {
    let exact = MY.exactness(&ColumnInfo { kind, result_type: "", column: &c });
    Some(TableColumn { column: c, kind, exact, repeated: false })
}

/// `shop.items` as (id int, name varchar ci, st enum, raw varbinary, price decimal, w float).
fn target(identity: Identity) -> EditTarget {
    EditTarget {
        table: TableRef { schema: "shop".into(), name: "items".into() },
        columns: vec![
            tcol(scol("id", "int", None), ValueKind::Integer),
            tcol(scol("name", "varchar(20)", Some("utf8mb4_0900_ai_ci")), ValueKind::Text),
            tcol(scol("st", "enum('new','old')", Some("utf8mb4_0900_ai_ci")), ValueKind::Other),
            tcol(scol("raw", "varbinary(8)", None), ValueKind::Bytes),
            tcol(scol("price", "decimal(10,2)", None), ValueKind::Decimal),
            tcol(scol("w", "float", None), ValueKind::Float),
        ],
        identity,
        all_columns_shown: true,
    }
}

fn orig(r: usize) -> Vec<Cell> {
    vec![Some(r.to_string()), Some("Ab".into()), Some("new".into()), Some("0x0aff".into()), None, Some("1.5".into())]
}

fn run(f: &MySql, t: &EditTarget, b: &EditBatch, matches: &[(usize, u64)], choices: &[(usize, Choice)]) -> Vec<String> {
    let matches: BTreeMap<usize, u64> = matches.iter().copied().collect();
    let choices: BTreeMap<usize, Choice> = choices.iter().copied().collect();
    let whole = |_: usize, _: usize| true;
    let p = plan(t, b, f, &Facts { whole: &whole, matches: &matches, choices: &choices });
    p.unwrap_or_else(|e| panic!("{e:?}")).steps.into_iter().map(|s| s.sql).collect()
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

/// The condition of row `r` found by its values (every column but the float).
fn values(r: usize) -> String {
    format!(
        "`id` <=> {r} AND `name` = _utf8mb4 X'4162' AND CONVERT(`name` USING utf8mb4) COLLATE utf8mb4_0900_bin \
         <=> _utf8mb4 X'4162' AND `st` = _utf8mb4 X'6E6577' AND CONVERT(`st` USING utf8mb4) COLLATE utf8mb4_0900_bin <=> _utf8mb4 X'6E6577' AND `raw` <=> X'0AFF' \
         AND `price` <=> NULL"
    )
}

#[test]
fn text_compares_byte_for_byte_and_every_compare_is_null_safe() {
    let t = target(Identity::Key { primary: true, columns: vec![0] });
    let mut b = EditBatch::new();
    b.set(&[(4, 1)], &text("it's \\ x"), orig);
    b.set(&[(4, 4)], &text("2.50"), orig);
    let w = "`id` <=> 4 AND `name` = _utf8mb4 X'4162' AND CONVERT(`name` USING utf8mb4) COLLATE utf8mb4_0900_bin <=> _utf8mb4 X'4162' AND `price` <=> NULL";
    assert_eq!(
        run(&MY, &t, &b, &[], &[]),
        [
            format!("SELECT 1 FROM `shop`.`items` WHERE {w} FOR UPDATE NOWAIT"),
            format!("UPDATE `shop`.`items` SET `name` = _utf8mb4 X'69742773205C2078', `price` = 2.50 WHERE {w}"),
        ]
    );
}

#[test]
fn without_nowait_the_check_waits_as_the_session_allows() {
    let t = target(Identity::Key { primary: true, columns: vec![0] });
    let mut b = EditBatch::new();
    b.set(&[(1, 1)], &text("x"), orig);
    let f = MySql(MySqlEdit { nowait: false });
    assert!(run(&f, &t, &b, &[], &[])[0].ends_with(" FOR UPDATE"));
}

#[test]
fn a_delete_inserts_and_one_of_several_rows() {
    let t = target(Identity::Values { columns: vec![0, 1, 2, 3, 4], duplicates: Duplicates::Choose });
    let mut b = EditBatch::new();
    b.delete(&[2], orig);
    b.set(&[(3, 1)], &text("z"), orig);
    b.add(3, None, vec![Value::Default; 6]);
    b.add(3, None, vec![text("7"), text("q"), text("old"), text("0x00"), Value::Null, Value::Default]);
    let steps = run(&MY, &t, &b, &[(2, 3), (3, 2)], &[(2, Choice::One), (3, Choice::All)]);
    assert_eq!(
        steps[2..],
        [
            format!("DELETE FROM `shop`.`items` WHERE {} LIMIT 1", values(2)),
            format!("UPDATE `shop`.`items` SET `name` = 'z' WHERE {}", values(3)),
            "INSERT INTO `shop`.`items` () VALUES ()".to_string(),
            "INSERT INTO `shop`.`items` (`id`, `name`, `st`, `raw`, `price`) VALUES (7, 'q', 'old', X'00', NULL)"
                .to_string(),
        ]
    );
    assert!(steps[0].ends_with(&format!("WHERE {} FOR UPDATE NOWAIT", values(2))));
}

#[test]
fn the_format_is_chosen_by_the_drivers_word() {
    use crate::driver::EditFormat;
    let t = TableRef { schema: "s".into(), name: "t".into() };
    assert_eq!(crate::edit::format(EditFormat::Postgres).table(&t), "\"s\".\"t\"");
    assert_eq!(crate::edit::format(EditFormat::MySql(MySqlEdit { nowait: true })).table(&t), "`s`.`t`");
}

#[test]
fn the_counts_of_rows_found_by_values_are_on_the_allowlist_of_a_count() {
    let t = target(Identity::Values { columns: vec![0, 1, 2, 3, 4], duplicates: Duplicates::Refuse });
    let mut b = EditBatch::new();
    b.set(&[(1, 1)], &text("x"), orig);
    b.delete(&[2], orig);
    let whole = |_: usize, _: usize| true;
    let counts = crate::edit::count_statements(&t, &b, &MY, &whole).unwrap();
    assert_eq!(counts.len(), 2);
    for (_, sql) in counts {
        assert_eq!(crate::sql::risk::mysql::repeatable(&sql, MySqlMode::default()), Ok(()), "{sql}");
    }
}
