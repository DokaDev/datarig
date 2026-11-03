use super::*;
use crate::driver::keys::{KeyKind, NameRule, NotInsertable};
use crate::driver::structure::{
    ColumnFill, Index, KeyConstraint, RelationKind, StructureColumn, StructureGroup, TableStructure,
};
use crate::driver::{ColumnMeta, ColumnOrigin, KeyCatalog, ValueKind};
use crate::sql::dialect::{Dialect, MySqlMode};

use super::fake::{ALL, Fake};

const PG: Dialect = Dialect::Postgres;

/// Table 7 `app.t` with columns 1.. named `names`.
fn catalog(names: &[&str]) -> KeyCatalog {
    let mut k = KeyCatalog::default();
    k.add_table(7, "app", "t", names.iter().enumerate().map(|(i, n)| (i as i16 + 1, n.to_string())));
    k.add_table(8, "app", "other", [(1, "id".to_string())]);
    k
}

/// A result column `name` of kind `kind`, from column `n` of table 7 (none: computed).
fn col(name: &str, kind: ValueKind, n: Option<i16>) -> ColumnMeta {
    ColumnMeta::new(name.into(), "t".into(), kind, n.map(|column| ColumnOrigin::Pg { table: 7, column }))
}

fn int(name: &str, n: i16) -> ColumnMeta {
    col(name, ValueKind::Integer, Some(n))
}

fn scol(name: &str, not_null: bool) -> StructureColumn {
    StructureColumn {
        name: name.into(),
        type_name: "integer".into(),
        not_null,
        default: None,
        fill: ColumnFill::Default,
        enum_labels: Vec::new(),
        collation: None,
    }
}

fn key(columns: &[&str]) -> KeyConstraint {
    KeyConstraint {
        name: "k".into(),
        columns: columns.iter().map(|c| c.to_string()).collect(),
        definition: String::new(),
    }
}

/// A valid unique index on plain columns `cols` (the primary key's when `primary`).
fn index(cols: &[&str], primary: bool) -> Index {
    Index {
        name: cols.join("_"),
        columns: cols.iter().map(|c| c.to_string()).collect(),
        options: Vec::new(),
        include: Vec::new(),
        key_columns: cols.iter().map(|c| Some(c.to_string())).collect(),
        include_columns: Vec::new(),
        unique: true,
        method: "btree".into(),
        predicate: None,
        primary,
        constraint: true,
        valid: true,
        definition: String::new(),
    }
}

/// A table of columns `cols` (name, `NOT NULL`) with primary key `pk` (and its index).
fn table(cols: &[(&str, bool)], pk: Option<&[&str]>) -> TableStructure {
    let mut s = TableStructure::new(RelationKind::Table);
    s.columns = cols.iter().map(|(n, nn)| scol(n, *nn)).collect();
    s.primary_key = pk.map(key);
    s.indexes = pk.map(|k| index(k, true)).into_iter().collect();
    s
}

/// `s` with a unique constraint on `cols` (and its index).
fn unique(mut s: TableStructure, cols: &[&str]) -> TableStructure {
    s.unique_constraints.push(key(cols));
    s.indexes.push(index(cols, false));
    s
}

fn resolve(names: &[&str], cols: &[ColumnMeta], s: &TableStructure) -> Result<EditTarget, ReadOnly> {
    let k = catalog(names);
    source(PG, Some(&k), cols, "SELECT * FROM app.t")?.target(s, &ALL)
}

#[test]
fn a_result_holding_the_primary_key_finds_rows_by_it_even_under_an_alias() {
    let s = table(&[("id", true), ("name", false)], Some(&["id"]));
    let cols = [col("name", ValueKind::Text, Some(2)), int("x", 1)];
    let t = resolve(&["id", "name"], &cols, &s).unwrap();
    assert_eq!(t.table, TableRef { schema: "app".into(), name: "t".into() });
    assert_eq!(t.identity, Identity::Key { primary: true, columns: vec![1] });
    assert!(t.all_columns_shown);
    assert_eq!(t.columns[1].as_ref().map(|c| c.column.name.as_str()), Some("id"), "the table's name, not the alias");
}

#[test]
fn a_computed_column_is_read_only_and_the_others_stay_editable() {
    let s = table(&[("id", true), ("name", false)], Some(&["id"]));
    let cols = [int("id", 1), col("upper", ValueKind::Text, None), col("name", ValueKind::Text, Some(2))];
    let k = catalog(&["id", "name"]);
    let sql = "SELECT id, upper(name), name FROM app.t";
    let t = source(PG, Some(&k), &cols, sql).unwrap().target(&s, &ALL).unwrap();
    assert_eq!(t.allows(1, Write::Update), Err(ColumnReason::Computed));
    assert_eq!(t.allows(1, Write::Insert), Err(ColumnReason::Computed));
    assert!(t.allows(2, Write::Update).is_ok());
    assert_eq!(t.identity, Identity::Key { primary: true, columns: vec![0] });
}

#[test]
fn rows_not_of_one_table_are_read_only_with_the_reason() {
    let k = catalog(&["id"]);
    let cols = [int("id", 1)];
    let src = |sql: &str| source(PG, Some(&k), &cols, sql).map(|_| ());
    let refused = |r| Err(ReadOnly::Source(r));
    assert_eq!(src("SELECT t.id FROM app.t JOIN app.other o ON o.id = t.id"), refused(NotInsertable::SeveralTables));
    assert_eq!(src("SELECT id FROM app.t GROUP BY id"), refused(NotInsertable::Grouping));
    assert_eq!(src("WITH x AS (SELECT 1) SELECT id FROM app.t"), refused(NotInsertable::With));
    assert_eq!(src("SELECT id FROM app.other"), refused(NotInsertable::OtherTable));
    assert_eq!(src("SELECT DISTINCT id FROM app.t"), refused(NotInsertable::Grouping));
    assert_eq!(src("select distinctrow id from app.t"), refused(NotInsertable::Grouping));
    assert_eq!(src("SELECT id, (SELECT 1) FROM app.t"), refused(NotInsertable::Subquery));
    assert!(src("SELECT id FROM app.t WHERE id IN (SELECT DISTINCT id FROM app.t)").is_ok(), "a filter");
    assert!(src("SELECT id, count(DISTINCT id) OVER () FROM app.t").is_err());
    // Columns of two tables though `FROM` names one.
    let two = [
        int("id", 1),
        ColumnMeta::new("x".into(), "int4".into(), ValueKind::Integer, Some(ColumnOrigin::Pg { table: 8, column: 1 })),
    ];
    assert_eq!(source(PG, Some(&k), &two, "SELECT * FROM app.t").map(|_| ()), refused(NotInsertable::SeveralTables));
    assert_eq!(source(PG, None, &cols, "SELECT id FROM app.t").map(|_| ()), refused(NotInsertable::NoCatalog));
    let unknown = [ColumnMeta::new(
        "id".into(),
        "int4".into(),
        ValueKind::Integer,
        Some(ColumnOrigin::Named { schema: "app".into(), table: "gone".into(), column: "id".into() }),
    )];
    assert_eq!(
        source(PG, Some(&k), &unknown, "SELECT id FROM app.t").map(|_| ()),
        refused(NotInsertable::UnknownTable)
    );
    let computed = [col("one", ValueKind::Integer, None)];
    assert_eq!(source(PG, Some(&k), &computed, "SELECT 1 FROM app.t").map(|_| ()), Err(ReadOnly::NoTableColumn));
    // A column of another table than the one `FROM` names.
    let other = [ColumnMeta::new(
        "id".into(),
        "int4".into(),
        ValueKind::Integer,
        Some(ColumnOrigin::Pg { table: 8, column: 1 }),
    )];
    assert_eq!(source(PG, Some(&k), &other, "SELECT id FROM app.t").map(|_| ()), refused(NotInsertable::OtherTable));
}

#[test]
fn a_view_is_read_only() {
    let mut k = catalog(&["id"]);
    k.set_view(7);
    let cols = [int("id", 1)];
    assert_eq!(source(PG, Some(&k), &cols, "SELECT id FROM app.t"), Err(ReadOnly::Source(NotInsertable::View)));
    // As the structure says it, when the key cache did not.
    let k = catalog(&["id"]);
    let src = source(PG, Some(&k), &cols, "SELECT id FROM app.t").unwrap();
    for kind in [RelationKind::View, RelationKind::MaterializedView, RelationKind::ForeignTable] {
        let s = TableStructure { columns: vec![scol("id", true)], ..TableStructure::new(kind) };
        assert_eq!(src.target(&s, &ALL), Err(ReadOnly::NotATable(kind)));
    }
    let s = TableStructure { columns: vec![scol("id", true)], ..TableStructure::new(RelationKind::PartitionedTable) };
    assert!(src.target(&s, &ALL).is_ok(), "a partitioned table is a table");
}

#[test]
fn a_not_null_unique_key_finds_rows_when_the_primary_key_is_not_shown() {
    let s = table(&[("id", true), ("email", true), ("nick", false), ("name", false)], Some(&["id"]));
    let s = unique(unique(s, &["nick"]), &["email"]);
    let cols = [col("nick", ValueKind::Text, Some(3)), col("email", ValueKind::Text, Some(2))];
    let t = resolve(&["id", "email", "nick", "name"], &cols, &s).unwrap();
    assert_eq!(t.identity, Identity::Key { primary: false, columns: vec![1] }, "nick may be NULL");
    // A unique index on plain columns counts without a constraint; an expression or partial one
    // does not.
    let mut s = table(&[("id", true), ("code", true)], None);
    s.indexes = vec![
        Index { key_columns: vec![None], ..index(&["id"], false) },
        Index { predicate: Some("id > 0".into()), ..index(&["id"], false) },
        index(&["code"], false),
    ];
    let cols = [int("id", 1), col("code", ValueKind::Text, Some(2))];
    let t = resolve(&["id", "code"], &cols, &s).unwrap();
    assert_eq!(t.identity, Identity::Key { primary: false, columns: vec![1] });
}

#[test]
fn a_key_counts_only_through_a_valid_index() {
    // `CREATE UNIQUE INDEX CONCURRENTLY` failed on duplicates: the index is left invalid.
    let mut s = unique(table(&[("email", true), ("name", false)], None), &["email"]);
    s.indexes[0].valid = false;
    let cols = [col("email", ValueKind::Text, Some(1)), col("name", ValueKind::Text, Some(2))];
    let t = resolve(&["email", "name"], &cols, &s).unwrap();
    assert_eq!(
        t.identity,
        Identity::Values { columns: vec![0, 1], duplicates: Duplicates::Refuse },
        "not a key, but the table is keyed: never duplicates"
    );
    // A partitioned table's primary key while a partition lacks its index; a constraint without
    // an index the structure lists.
    let mut s = table(&[("id", true), ("name", false)], Some(&["id"]));
    s.indexes[0].valid = false;
    let cols = [int("id", 1), col("name", ValueKind::Text, Some(2))];
    let t = resolve(&["id", "name"], &cols, &s).unwrap();
    assert_eq!(t.identity, Identity::Values { columns: vec![0, 1], duplicates: Duplicates::Refuse });
    s.indexes.clear();
    let t = resolve(&["id", "name"], &cols, &s).unwrap();
    assert_eq!(t.identity, Identity::Values { columns: vec![0, 1], duplicates: Duplicates::Refuse });
}

#[test]
fn a_repeated_key_column_finds_rows_by_its_first_copy() {
    let s = table(&[("id", true), ("name", false)], Some(&["id"]));
    let cols = [col("name", ValueKind::Text, Some(2)), int("id", 1), int("id2", 1)];
    let t = resolve(&["id", "name"], &cols, &s).unwrap();
    assert_eq!(t.identity, Identity::Key { primary: true, columns: vec![1] });
}

#[test]
fn without_the_key_rows_are_found_by_their_values_and_never_as_duplicates() {
    let s = table(&[("id", true), ("name", false), ("score", false)], Some(&["id"]));
    let cols = [col("name", ValueKind::Text, Some(2)), col("score", ValueKind::Float, Some(3))];
    let t = resolve(&["id", "name", "score"], &cols, &s).unwrap();
    assert_eq!(t.identity, Identity::Values { columns: vec![0], duplicates: Duplicates::Refuse }, "score is inexact");
    assert!(!t.all_columns_shown);
}

#[test]
fn the_columns_a_new_row_must_be_given_that_the_result_does_not_show_are_kept() {
    let mut s = table(
        &[("id", true), ("name", false), ("email", true), ("code", true), ("n", true), ("g", true)],
        Some(&["id"]),
    );
    s.columns[3].default = Some("'x'".into());
    s.columns[4].fill = ColumnFill::AutoIncrement;
    s.columns[5].fill = ColumnFill::Stored("1".into());
    let names = ["id", "name", "email", "code", "n", "g"];
    let t = resolve(&names, &[int("id", 1)], &s).unwrap();
    assert_eq!(t.unshown_required, ["email"], "NOT NULL, no default, filled by nothing, not shown");
    let t = resolve(&names, &[int("id", 1), col("email", ValueKind::Text, Some(3))], &s).unwrap();
    assert!(t.unshown_required.is_empty(), "shown: the result's own check");
}

#[test]
fn identical_rows_of_a_keyless_table_are_a_choice_only_when_every_column_is_shown_exactly() {
    let s = table(&[("a", false), ("b", false)], None);
    let names = ["a", "b"];
    let both = [int("a", 1), col("b", ValueKind::Text, Some(2))];
    let t = resolve(&names, &both, &s).unwrap();
    assert_eq!(t.identity, Identity::Values { columns: vec![0, 1], duplicates: Duplicates::Choose });
    let one = [int("a", 1)];
    let t = resolve(&names, &one, &s).unwrap();
    assert_eq!(t.identity, Identity::Values { columns: vec![0], duplicates: Duplicates::Refuse }, "b is not shown");
    let inexact = [int("a", 1), col("b", ValueKind::Json, Some(2))];
    let t = resolve(&names, &inexact, &s).unwrap();
    assert_eq!(t.identity, Identity::Values { columns: vec![0], duplicates: Duplicates::Refuse }, "b is inexact");
    let none = [col("b", ValueKind::Float, Some(2))];
    assert_eq!(resolve(&names, &none, &s), Err(ReadOnly::NoExactColumn));
}

#[test]
fn a_table_keyed_only_by_a_unique_key_the_result_lacks_never_has_duplicates() {
    let s = unique(table(&[("email", true), ("name", false)], None), &["email"]);
    let cols = [col("name", ValueKind::Text, Some(2))];
    let t = resolve(&["email", "name"], &cols, &s).unwrap();
    assert_eq!(t.identity, Identity::Values { columns: vec![0], duplicates: Duplicates::Refuse });
}

#[test]
fn rows_are_never_duplicates_when_the_structure_is_not_whole() {
    let s = table(&[("a", false), ("b", false)], None);
    let cols = [int("a", 1), int("b", 2)];
    let choose = Identity::Values { columns: vec![0, 1], duplicates: Duplicates::Choose };
    let refuse = Identity::Values { columns: vec![0, 1], duplicates: Duplicates::Refuse };
    assert_eq!(resolve(&["a", "b"], &cols, &s).unwrap().identity, choose);
    // The server would not list some of it (MySQL lists only the columns a user may read).
    for g in [StructureGroup::Columns, StructureGroup::Indexes, StructureGroup::UniqueConstraints] {
        let s = TableStructure { hidden: vec![g], ..s.clone() };
        let t = resolve(&["a", "b"], &cols, &s).unwrap();
        assert_eq!(t.identity, refuse, "{g:?}");
    }
    let s = TableStructure { hidden: vec![StructureGroup::Triggers], ..s.clone() };
    assert_eq!(resolve(&["a", "b"], &cols, &s).unwrap().identity, choose, "triggers do not matter");
    // A key on a column the structure does not list: unknown whether it may be NULL.
    let mut s = unique(table(&[("a", false), ("b", false)], None), &["hidden"]);
    s.indexes.clear();
    assert_eq!(resolve(&["a", "b"], &cols, &s).unwrap().identity, refuse);
    // Nor in a table others inherit from, even without a key.
    let s = TableStructure { inherited: true, ..table(&[("a", false), ("b", false)], None) };
    assert_eq!(resolve(&["a", "b"], &cols, &s).unwrap().identity, refuse);
}

#[test]
fn a_key_column_that_does_not_compare_exactly_is_no_key() {
    let s = table(&[("k", true), ("name", false)], Some(&["k"]));
    let cols = [col("k", ValueKind::Other, Some(1)), col("name", ValueKind::Text, Some(2))];
    let t = resolve(&["k", "name"], &cols, &s).unwrap();
    assert_eq!(t.identity, Identity::Values { columns: vec![1], duplicates: Duplicates::Refuse });
}

#[test]
fn an_inherited_tables_keys_do_not_hold() {
    let mut s = table(&[("id", true), ("name", false)], Some(&["id"]));
    s.inherited = true;
    let cols = [int("id", 1), col("name", ValueKind::Text, Some(2))];
    let t = resolve(&["id", "name"], &cols, &s).unwrap();
    assert_eq!(t.identity, Identity::Values { columns: vec![0, 1], duplicates: Duplicates::Refuse });
}

#[test]
fn the_formats_offers_are_taken_in_its_order() {
    let s = unique(table(&[("id", true), ("email", true)], Some(&["id"])), &["email"]);
    let cols = [int("id", 1), col("email", ValueKind::Text, Some(2))];
    let k = catalog(&["id", "email"]);
    let src = source(PG, Some(&k), &cols, "SELECT * FROM app.t").unwrap();
    let t = src.target(&s, &Fake::offering(&[Offer::Unique, Offer::Primary])).unwrap();
    assert_eq!(t.identity, Identity::Key { primary: false, columns: vec![1] });
    let t = src.target(&s, &Fake::offering(&[Offer::Values])).unwrap();
    assert_eq!(t.identity, Identity::Values { columns: vec![0, 1], duplicates: Duplicates::Refuse }, "a keyed table");
    let keyless = table(&[("id", true), ("email", true)], None);
    assert_eq!(src.target(&keyless, &Fake::offering(&[Offer::Primary])), Err(ReadOnly::NoIdentity));
}

#[test]
fn generated_identity_and_repeated_columns_say_why_they_are_not_set() {
    let mut s = table(&[("id", true), ("twice", false), ("name", false)], Some(&["id"]));
    s.columns[0].fill = ColumnFill::IdentityAlways;
    s.columns[1].fill = ColumnFill::Stored("n * 2".into());
    let cols =
        [int("id", 1), int("twice", 2), col("name", ValueKind::Text, Some(3)), col("again", ValueKind::Text, Some(3))];
    let t = resolve(&["id", "twice", "name"], &cols, &s).unwrap();
    assert_eq!(t.allows(0, Write::Update).map(|_| ()), Err(ColumnReason::IdentityAlways));
    assert!(t.allows(0, Write::Insert).is_ok(), "a new row may be given one");
    assert_eq!(t.allows(1, Write::Insert).map(|_| ()), Err(ColumnReason::Generated));
    assert_eq!(t.allows(2, Write::Update).map(|_| ()), Err(ColumnReason::Repeated));
    assert_eq!(t.allows(3, Write::Update).map(|_| ()), Err(ColumnReason::Repeated));
    assert_eq!(t.allows(9, Write::Update).map(|_| ()), Err(ColumnReason::Computed), "out of range");
}

#[test]
fn a_column_the_structure_does_not_have_means_the_table_changed() {
    let s = table(&[("id", true)], Some(&["id"]));
    let cols = [int("id", 1), col("name", ValueKind::Text, Some(2))];
    assert_eq!(resolve(&["id", "name"], &cols, &s), Err(ReadOnly::TableChanged));
}

#[test]
fn mysql_origins_are_found_by_name_as_the_server_compares_names() {
    let mut k = KeyCatalog::default();
    k.set_names(NameRule::MySql { tables_ignore_case: false });
    k.add_table(1, "shop", "items", [(1, "id".to_string()), (2, "Name".to_string())]);
    k.mark(1, &[1], KeyKind::Primary);
    let named = |column: &str| {
        Some(ColumnOrigin::Named { schema: "shop".into(), table: "items".into(), column: column.into() })
    };
    let cols = [
        ColumnMeta::new("id".into(), "BIGINT".into(), ValueKind::Integer, named("ID")),
        ColumnMeta::new("name".into(), "VARCHAR".into(), ValueKind::Text, named("name")),
    ];
    let d = Dialect::MySql(MySqlMode::default());
    let src = source(d, Some(&k), &cols, "SELECT * FROM items").unwrap();
    let names: Vec<Option<&str>> = src.columns.iter().map(|c| c.as_ref().map(|c| c.name.as_str())).collect();
    assert_eq!(names, [Some("id"), Some("Name")], "the catalog's names");
    let s = table(&[("id", true), ("Name", false)], Some(&["id"]));
    let t = src.target(&s, &ALL).unwrap();
    assert_eq!(t.identity, Identity::Key { primary: true, columns: vec![0] });
}
