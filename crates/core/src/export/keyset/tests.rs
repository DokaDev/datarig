use super::*;
use crate::driver::structure::{ColumnFill, Index, KeyConstraint, RelationKind, StructureColumn};

const PG: Dialect = Dialect::Postgres;

fn my() -> Dialect {
    Dialect::MySql(Default::default())
}

fn meta(name: &str, ty: &str, kind: ValueKind) -> ColumnMeta {
    ColumnMeta::new(name.into(), ty.into(), kind, None)
}

fn structure(columns: &[(&str, bool)], pk: Option<&[&str]>, unique: &[&[&str]]) -> TableStructure {
    let key = |cols: &[&str]| KeyConstraint {
        name: "k".into(),
        columns: cols.iter().map(|c| c.to_string()).collect(),
        definition: String::new(),
    };
    TableStructure {
        kind: RelationKind::Table,
        estimated_rows: None,
        total_bytes: None,
        columns: columns
            .iter()
            .map(|(n, not_null)| StructureColumn {
                name: n.to_string(),
                type_name: "int".into(),
                not_null: *not_null,
                default: None,
                fill: ColumnFill::Default,
            })
            .collect(),
        primary_key: pk.map(key),
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        unique_constraints: unique.iter().map(|u| key(u)).collect(),
        checks: Vec::new(),
        triggers: Vec::new(),
        hidden: Vec::new(),
        estimates: false,
        inherited: false,
    }
}

fn index(cols: &[&str], options: &[&str]) -> Index {
    Index {
        name: "i".into(),
        columns: cols.iter().map(|c| c.to_string()).collect(),
        options: options.iter().map(|o| o.to_string()).collect(),
        include: Vec::new(),
        key_columns: cols.iter().map(|c| Some(c.to_string())).collect(),
        include_columns: Vec::new(),
        unique: true,
        method: "btree".into(),
        predicate: None,
        primary: false,
        constraint: true,
        definition: String::new(),
    }
}

#[test]
fn the_primary_key_comes_first_then_a_unique_key_of_not_null_columns() {
    let s = structure(&[("a", true), ("b", true), ("c", false)], Some(&["b", "a"]), &[&["c"]]);
    assert_eq!(table_key(&s), Ok(vec!["b".to_string(), "a".to_string()]), "in the constraint's order");
    let s = structure(&[("a", true), ("b", false)], None, &[&["b"], &["a"]]);
    assert_eq!(table_key(&s), Ok(vec!["a".to_string()]), "a nullable unique column never reaches its NULLs");
    let s = structure(&[("a", false)], None, &[&["a"]]);
    assert_eq!(table_key(&s), Err(NotKeyset::NoKey));
    assert_eq!(table_key(&structure(&[("a", true)], None, &[])), Err(NotKeyset::NoKey));
}

#[test]
fn a_key_whose_index_orders_otherwise_is_not_used() {
    let mut s = structure(&[("a", true), ("b", true)], Some(&["a", "b"]), &[]);
    s.indexes.push(index(&["a", "b"], &["", "DESC"]));
    assert_eq!(table_key(&s), Err(NotKeyset::Ordered("b".into())));
    s.indexes[0].options = vec![String::new(), String::new()];
    assert_eq!(table_key(&s), Ok(vec!["a".to_string(), "b".to_string()]));
}

fn cols() -> Vec<ColumnMeta> {
    vec![
        meta("id", "int8", ValueKind::Integer),
        meta("name", "text", ValueKind::Text),
        meta("price", "float8", ValueKind::Float),
        meta("data", "bytea", ValueKind::Bytes),
    ]
}

fn names(c: &[ColumnMeta]) -> Vec<String> {
    c.iter().map(|c| c.name.clone()).collect()
}

#[test]
fn postgres_chunks_compare_the_key_as_a_row_in_index_order() {
    let c = cols();
    let key = ["name".to_string(), "id".to_string()];
    let k = Keyset::new(PG, "s", "t", &key, &c, &names(&c), true).unwrap();
    assert_eq!(k.first(), "SELECT * FROM \"s\".\"t\" ORDER BY \"name\", \"id\" LIMIT 10000");
    let last = [Some("9007199254740993".into()), Some("O'Neil \\ x".into()), None, None];
    assert_eq!(
        k.after(&last).unwrap(),
        "SELECT * FROM \"s\".\"t\" WHERE (\"name\", \"id\") > ('O''Neil \\ x', 9007199254740993) ORDER BY \"name\", \"id\" LIMIT 10000",
        "numbers bare (every digit), text a literal"
    );
    let one = Keyset::new(PG, "s", "t", &["id".to_string()], &c, &names(&c), true).unwrap();
    assert_eq!(
        one.after(&[Some("5".into()), None, None, None]).unwrap(),
        "SELECT * FROM \"s\".\"t\" WHERE \"id\" > 5 ORDER BY \"id\" LIMIT 10000"
    );
    let bytes = Keyset::new(PG, "s", "t", &["data".to_string()], &c, &names(&c), true).unwrap();
    assert!(bytes.after(&[None, None, None, Some("\\x00ff".into())]).unwrap().contains("\"data\" > '\\x00ff'"));
}

#[test]
fn mysql_chunks_write_the_comparison_out_with_a_lock_wait_hint() {
    let c = cols();
    let key = ["id".to_string(), "name".to_string(), "data".to_string()];
    let k = Keyset::new(my(), "s", "t", &key, &c, &names(&c), true).unwrap();
    assert_eq!(
        k.first(),
        "SELECT /*+ SET_VAR(lock_wait_timeout=5) */ * FROM `s`.`t` ORDER BY `id`, `name`, `data` LIMIT 10000"
    );
    let last = [Some("18446744073709551615".into()), Some("a\\b".into()), None, Some("0x00FF".into())];
    assert_eq!(
        k.after(&last).unwrap(),
        "SELECT /*+ SET_VAR(lock_wait_timeout=5) */ * FROM `s`.`t` WHERE `id` > 18446744073709551615 OR \
         (`id` = 18446744073709551615 AND (`name` > _utf8mb4 X'615C62' OR (`name` = _utf8mb4 X'615C62' AND \
         (`data` > X'00FF')))) ORDER BY `id`, `name`, `data` LIMIT 10000",
        "a backslash is read the same whatever NO_BACKSLASH_ESCAPES says"
    );
    // Bytes not in the driver's form cannot be compared back.
    assert_eq!(k.after(&[Some("1".into()), Some("a".into()), None, Some("raw".into())]), None);
    // A NULL key value (never in a key, but not trusted) ends the chunks.
    assert_eq!(k.after(&[None, Some("a".into()), None, Some("0x00".into())]), None);
}

#[test]
fn key_kinds_are_an_allowlist() {
    let pg = |m: ColumnMeta| key_kind(PG, &m, true);
    assert_eq!(pg(meta("f", "float8", ValueKind::Float)), Err(NotKeyset::Kind("f".into())), "floats are rounded");
    assert_eq!(pg(meta("m", "money", ValueKind::Decimal)), Err(NotKeyset::Kind("m".into())));
    assert_eq!(pg(meta("u", "uuid", ValueKind::Other)), Ok(Kind::Text));
    assert_eq!(pg(meta("t", "timestamptz", ValueKind::TimestampTz)), Ok(Kind::Text));
    assert_eq!(pg(meta("j", "jsonb", ValueKind::Json)), Err(NotKeyset::Kind("j".into())));
    assert_eq!(pg(meta("e", "mood", ValueKind::Other)), Err(NotKeyset::Kind("e".into())));
    let mysql = |m: ColumnMeta, text_ok| key_kind(my(), &m, text_ok);
    assert_eq!(mysql(meta("t", "TIMESTAMP", ValueKind::TimestampTz), true), Err(NotKeyset::Kind("t".into())));
    assert_eq!(mysql(meta("v", "VARCHAR", ValueKind::Text), false), Err(NotKeyset::Kind("v".into())));
    assert_eq!(mysql(meta("v", "VARCHAR", ValueKind::Text), true), Ok(Kind::Text));
    assert_eq!(mysql(meta("d", "DATETIME", ValueKind::Timestamp), true), Ok(Kind::Text));
    assert_eq!(mysql(meta("u", "uuid", ValueKind::Other), true), Err(NotKeyset::Kind("u".into())));
    assert_eq!(mysql(meta("e", "enum", ValueKind::Text), true), Err(NotKeyset::Kind("e".into())), "sorted by number");
    assert_eq!(mysql(meta("s", "set", ValueKind::Text), true), Err(NotKeyset::Kind("s".into())));
}

#[test]
fn the_result_must_be_the_table_itself() {
    let c = cols();
    let mut other = names(&c);
    other.swap(0, 1);
    assert_eq!(Keyset::new(PG, "s", "t", &["id".to_string()], &c, &other, true), Err(NotKeyset::NotTheTable));
    assert_eq!(
        Keyset::new(PG, "s", "t", &["price".to_string()], &c, &names(&c), true),
        Err(NotKeyset::Kind("price".into()))
    );
}

#[test]
fn a_key_read_with_replaced_characters_is_not_written_back() {
    let c = cols();
    let k = Keyset::new(PG, "s", "t", &["name".to_string()], &c, &names(&c), true).unwrap();
    assert_eq!(k.after(&[None, Some("caf\u{FFFD}".into()), None, None]), None);
    assert!(k.after(&[None, Some("caf\u{e9}".into()), None, None]).is_some());
}

#[test]
fn a_table_others_inherit_from_is_not_read_by_its_key() {
    let mut s = structure(&[("a", true)], Some(&["a"]), &[]);
    s.inherited = true;
    assert_eq!(table_key(&s), Err(NotKeyset::NoKey));
}

#[test]
fn a_prefix_key_is_not_used() {
    let mut s = structure(&[("a", true)], None, &[&["a"]]);
    let mut i = index(&["a"], &[""]);
    i.columns = vec!["`a`(10)".into()];
    s.indexes.push(i);
    assert_eq!(table_key(&s), Err(NotKeyset::Ordered("a".into())));
}
