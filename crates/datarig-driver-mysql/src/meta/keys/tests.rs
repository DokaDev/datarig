use super::*;
use datarig_core::driver::ColumnOrigin;
use datarig_core::driver::keys::KeyMarks;

fn row(
    kind: &str,
    schema: &str,
    table: &str,
    a: Option<&str>,
    b: Option<&str>,
    c: Option<&str>,
    d: Option<&str>,
) -> RawRow {
    let s = |v: Option<&str>| v.map(str::to_string);
    (kind.into(), schema.into(), table.into(), s(a), s(b), s(c), s(d))
}

fn col(schema: &str, table: &str, n: &str, name: &str, extra: &str, kind: &str) -> RawRow {
    row("C", schema, table, Some(n), Some(name), Some(extra), Some(kind))
}

fn lctn(v: &str) -> RawRow {
    row("L", "", "", Some(v), None, None, None)
}

/// `shop.orders` (a key, a foreign key, a unique key on whole columns, one on a prefix, one on
/// an expression, a generated and an `AUTO_INCREMENT` column) and the view `shop.summary`.
fn rows(l: &str) -> Vec<RawRow> {
    vec![
        lctn(l),
        col("shop", "orders", "1", "id", "auto_increment", "BASE TABLE"),
        col("shop", "orders", "2", "user_id", "", "BASE TABLE"),
        col("shop", "orders", "3", "Code", "", "BASE TABLE"),
        col("shop", "orders", "4", "note", "", "BASE TABLE"),
        col("shop", "orders", "5", "twice", "STORED GENERATED", "BASE TABLE"),
        col("shop", "orders", "6", "low", "VIRTUAL GENERATED", "BASE TABLE"),
        col("shop", "orders", "7", "at", "DEFAULT_GENERATED on update CURRENT_TIMESTAMP", "BASE TABLE"),
        col("shop", "summary", "1", "id", "", "VIEW"),
        row("P", "shop", "orders", None, Some("id"), None, None),
        row("F", "shop", "orders", None, Some("user_id"), None, None),
        row("U", "shop", "orders", Some("uq_code"), Some("Code"), None, None),
        row("U", "shop", "orders", Some("uq_note_prefix"), Some("note"), Some("10"), None),
        row("U", "shop", "orders", Some("uq_fn"), None, None, None),
        row("U", "shop", "orders", Some("uq_pair"), Some("user_id"), None, None),
        row("U", "shop", "orders", Some("uq_pair"), Some("note"), Some("5"), None),
    ]
}

fn marks(k: &KeyCatalog, table: &str, column: &str) -> KeyMarks {
    k.marks_by_name("shop", table, column)
}

#[test]
fn keys_mark_whole_columns_only() {
    let k = catalog(rows("0")).unwrap();
    assert_eq!(marks(&k, "orders", "id"), KeyMarks { pk: true, fk: false, unique: false });
    assert_eq!(
        marks(&k, "orders", "user_id"),
        KeyMarks { pk: false, fk: true, unique: false },
        "a key with a prefix part marks none of its columns"
    );
    assert_eq!(marks(&k, "orders", "Code"), KeyMarks { pk: false, fk: false, unique: true });
    assert!(!marks(&k, "orders", "note").any(), "a unique prefix makes only the prefix unique");
}

#[test]
fn generated_columns_take_no_value_but_auto_increment_does() {
    let k = catalog(rows("0")).unwrap();
    let t = k.find(Some("shop"), "orders").unwrap();
    let generated: Vec<(&str, Generated)> = t.columns.values().map(|c| (c.name.as_str(), c.generated)).collect();
    assert_eq!(
        generated,
        [
            ("id", Generated::No),
            ("user_id", Generated::No),
            ("Code", Generated::No),
            ("note", Generated::No),
            ("twice", Generated::Expression),
            ("low", Generated::Expression),
            ("at", Generated::No),
        ]
    );
    assert!(!t.view);
    assert!(k.find(Some("shop"), "summary").unwrap().view);
}

#[test]
fn names_compare_as_the_server_does() {
    let origin = |schema: &str, table: &str, column: &str| ColumnOrigin::Named {
        schema: schema.into(),
        table: table.into(),
        column: column.into(),
    };
    // A column's name never minds the case; a table's does with lower_case_table_names = 0.
    let exact = catalog(rows("0")).unwrap();
    assert!(exact.marks(Some(&origin("shop", "orders", "ID"))).pk);
    assert!(exact.marks(Some(&origin("shop", "orders", "code"))).unique);
    assert!(!exact.marks(Some(&origin("shop", "Orders", "id"))).pk);
    assert!(exact.find(Some("SHOP"), "orders").is_err());
    for l in ["1", "2"] {
        let k = catalog(rows(l)).unwrap();
        assert!(k.marks(Some(&origin("SHOP", "Orders", "Id"))).pk, "lower_case_table_names = {l}");
        assert!(k.find(Some("Shop"), "ORDERS").is_ok());
        assert!(k.find(None, "Orders").is_ok());
        assert_eq!(k.names(), NameRule::MySql { tables_ignore_case: true });
    }
    assert_eq!(exact.names(), NameRule::MySql { tables_ignore_case: false });
}

#[test]
fn what_the_model_cannot_read_fails_the_read() {
    let mut no_rule = rows("0");
    no_rule.remove(0);
    assert!(matches!(catalog(no_rule), Err(DbError::Server(m)) if m.contains("lower_case_table_names")));
    assert!(matches!(catalog(rows("7")), Err(DbError::Server(_))));
    let mut odd = rows("0");
    odd.push(row("X", "shop", "orders", None, None, None, None));
    assert!(matches!(catalog(odd), Err(DbError::Server(m)) if m.contains("kind X")));
    let mut bad = rows("0");
    bad.push(col("shop", "orders", "x", "y", "", "BASE TABLE"));
    assert!(catalog(bad).is_err(), "a position that is no number");
}

#[test]
fn the_statement_reads_nothing_that_opens_a_table() {
    let sql = statement();
    for opens in ["TABLE_ROWS", "DATA_LENGTH", "INDEX_LENGTH", "CARDINALITY", "SHOW "] {
        assert!(!sql.contains(opens), "{opens}: {sql}");
    }
    assert_eq!(sql.matches("NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys')").count(), 3);
}
