use super::*;

/// shop.users (id PK, email UQ), shop.orders (id PK, user_id FK), shop.order_items (order_id,
/// product_id: composite PK; order_id FK, product_id FK), shop.pairs (a, b: composite UQ).
fn shop() -> KeyCatalog {
    let cols =
        |names: &[&str]| names.iter().enumerate().map(|(i, n)| (i as i16 + 1, n.to_string())).collect::<Vec<_>>();
    let mut k = KeyCatalog::default();
    k.add_table(10, "shop", "users", cols(&["id", "email", "name"]));
    k.add_table(11, "shop", "orders", cols(&["id", "user_id", "status"]));
    k.add_table(12, "shop", "order_items", cols(&["order_id", "product_id", "qty"]));
    k.add_table(13, "shop", "pairs", cols(&["a", "b", "c"]));
    k.mark(10, &[1], KeyKind::Primary);
    k.mark(10, &[2], KeyKind::Unique);
    k.mark(11, &[1], KeyKind::Primary);
    k.mark(11, &[2], KeyKind::Foreign);
    k.mark(12, &[1, 2], KeyKind::Primary);
    k.mark(12, &[1], KeyKind::Foreign);
    k.mark(12, &[2], KeyKind::Foreign);
    k.mark(13, &[1, 2], KeyKind::Unique);
    // A key of a table the catalog does not know, a column number that is not there.
    k.mark(99, &[1], KeyKind::Primary);
    k.mark(10, &[42], KeyKind::Primary);
    k
}

fn o(table: u32, column: i16) -> Option<ColumnOrigin> {
    Some(ColumnOrigin::Pg { table, column })
}

const PG: Dialect = Dialect::Postgres;

const PK: KeyMarks = KeyMarks { pk: true, fk: false, unique: false };
const FK: KeyMarks = KeyMarks { pk: false, fk: true, unique: false };
const UQ: KeyMarks = KeyMarks { pk: false, fk: false, unique: true };
const NONE: KeyMarks = KeyMarks { pk: false, fk: false, unique: false };

#[test]
fn single_column_keys() {
    let k = shop();
    assert_eq!(k.marks(o(10, 1).as_ref()), PK);
    assert_eq!(k.marks(o(10, 2).as_ref()), UQ);
    assert_eq!(k.marks(o(10, 3).as_ref()), NONE);
    assert_eq!(k.marks(o(11, 2).as_ref()), FK);
}

#[test]
fn every_member_of_a_composite_key_is_marked() {
    let k = shop();
    let both = KeyMarks { pk: true, fk: true, unique: false };
    assert_eq!(
        (k.marks(o(12, 1).as_ref()), k.marks(o(12, 2).as_ref()), k.marks(o(12, 3).as_ref())),
        (both, both, NONE)
    );
    assert_eq!((k.marks(o(13, 1).as_ref()), k.marks(o(13, 2).as_ref()), k.marks(o(13, 3).as_ref())), (UQ, UQ, NONE));
    assert!(both.any() && !NONE.any());
}

#[test]
fn computed_and_unknown_columns_have_no_marks() {
    let k = shop();
    assert_eq!(k.marks(None), NONE, "an expression has no origin");
    assert_eq!(k.marks(o(99, 1).as_ref()), NONE, "a table the catalog does not know");
    assert_eq!(k.marks(o(10, 42).as_ref()), NONE, "a column that is not there");
    assert!(k.column(&ColumnOrigin::Pg { table: 10, column: 42 }).is_none());
}

#[test]
fn lookups_by_name() {
    let k = shop();
    assert_eq!(k.marks_by_name("shop", "order_items", "product_id"), KeyMarks { pk: true, fk: true, unique: false });
    assert_eq!(k.marks_by_name("shop", "users", "email"), UQ);
    assert_eq!(k.marks_by_name("shop", "users", "nope"), NONE);
    assert_eq!(k.marks_by_name("public", "users", "id"), NONE);
    assert_eq!(k.len(), 4);
}

/// `shop()` with a view `shop.recent` (over orders), a table `shop.status` with a column
/// `status`, and `other.orders` (a second table of that name).
fn more() -> KeyCatalog {
    let mut k = shop();
    k.add_table(20, "shop", "recent", [(1, "id".to_string()), (2, "status".to_string())]);
    k.set_view(20);
    k.add_table(21, "shop", "status", [(1, "status".to_string())]);
    k.add_table(22, "other", "orders", [(1, "id".to_string())]);
    k
}

fn table_of<'a>(r: Result<InsertPlan<'a>, NotInsertable>) -> (String, Vec<(usize, &'a str)>) {
    let p = r.expect("insertable");
    (format!("{}.{}", p.table.schema, p.table.name), p.columns)
}

#[test]
fn a_plain_single_table_select_is_insertable() {
    let k = more();
    let src =
        |origins: &[Option<ColumnOrigin>], names: &[&str], sql: &str| insert_source(PG, Some(&k), origins, names, sql);
    let Ok(InsertPlan { table, columns, skipped, overriding }) =
        src(&[o(11, 3), o(11, 1)], &["s", "i"], "SELECT status AS s, id AS i FROM shop.orders")
    else {
        panic!("one table")
    };
    assert_eq!(
        (table.schema.as_str(), table.name.as_str(), columns),
        ("shop", "orders", vec![(0, "status"), (1, "id")])
    );
    assert!(skipped.is_empty() && !overriding);
    let ok = [
        "SELECT o.id, o.status FROM orders o WHERE orders_x = 1",
        "select orders.id, shop.orders.status from shop.orders",
        "SELECT id, status FROM ONLY shop.orders AS o WHERE status <> 'x' ORDER BY id DESC LIMIT 5 OFFSET 1;",
        "SELECT id, status FROM \"shop\".\"orders\" FOR UPDATE",
        "SELECT DISTINCT id, status FROM orders",
        "-- a comment\nSELECT id, status /* x */ FROM orders WHERE user_id IN (SELECT id FROM users WHERE name LIKE 'a%')",
        "SELECT id, status FROM orders WHERE EXISTS (SELECT 1 FROM users u JOIN orders o2 ON true UNION SELECT 2)",
    ];
    for sql in ok {
        assert_eq!(table_of(src(&[o(11, 1), o(11, 3)], &["id", "status"], sql)).0, "shop.orders", "{sql}");
    }
    // The explorer's own statement.
    assert!(src(&[o(11, 1)], &["id"], "SELECT * FROM \"shop\".\"orders\"").is_ok());
    // A table and a column of the same name: no longer taken for a self-join.
    assert_eq!(
        table_of(src(&[o(21, 1)], &["status"], "SELECT status FROM status")),
        ("shop.status".into(), vec![(0, "status")])
    );
}

#[test]
fn a_cte_self_join_is_refused() {
    let k = more();
    // The repro: both instances of the CTE read one table, so the driver names that table for
    // every column; the rows it returns never existed.
    let sql = "WITH x AS (SELECT * FROM shop.orders) SELECT a.id, b.status FROM x a JOIN x b ON a.user_id = b.id";
    assert_eq!(insert_source(PG, Some(&k), &[o(11, 1), o(11, 3)], &["id", "status"], sql), Err(NotInsertable::With));
}

#[test]
fn anything_but_one_plain_table_is_refused() {
    use NotInsertable::*;
    let k = more();
    let cols = [o(11, 1), o(11, 3)];
    let names = ["id", "status"];
    for (sql, why) in [
        ("SELECT a.id, b.status FROM shop.orders a JOIN shop.orders b ON b.user_id = a.user_id", SeveralTables),
        ("SELECT a.id, b.status FROM orders a, orders b", SeveralTables),
        ("SELECT a.id, a.status FROM orders a CROSS JOIN users", SeveralTables),
        ("SELECT a.id, a.status FROM orders a NATURAL JOIN orders", SeveralTables),
        ("SELECT id, status FROM (SELECT * FROM orders) s", Subquery),
        ("SELECT id, (SELECT status FROM orders LIMIT 1) FROM orders", Subquery),
        ("SELECT id, status FROM orders UNION SELECT id, status FROM orders", SetOperation),
        ("SELECT id, status FROM orders EXCEPT ALL SELECT id, status FROM orders WHERE false", SetOperation),
        ("SELECT id, status FROM orders GROUP BY id, status", Grouping),
        ("SELECT id, status FROM orders HAVING true", Grouping),
        ("SELECT DISTINCT ON (status) id, status FROM orders", Grouping),
        ("SELECT id, status FROM orders ORDER BY row_number() OVER (PARTITION BY status)", Window),
        ("SELECT id, status FROM orders WINDOW w AS (ORDER BY id)", Window),
        ("SELECT id, status FROM orders o, LATERAL (SELECT 1) l", Subquery),
        ("SELECT id, status FROM LATERAL generate_series(1, 2)", NotATable),
        ("SELECT id, status FROM generate_series(1, 2) g", NotATable),
        ("SELECT id, status FROM ROWS FROM (f()) r", NotATable),
        ("SELECT id, status FROM orders TABLESAMPLE SYSTEM (10)", NotATable),
        ("SELECT id, status FROM orders AS o (a, b)", NotATable),
        ("SELECT id, status FROM db.shop.orders", NotATable),
        ("SELECT 1 AS id, 'x' AS status", NotATable),
        ("VALUES (1, 'x')", NotSelect),
        ("TABLE orders", NotSelect),
        ("(SELECT id, status FROM orders)", NotSelect),
        ("SELECT id, status INTO copy FROM orders", NotSelect),
        ("SHOW search_path", NotSelect),
        ("", NotSelect),
        ("WITH x AS (SELECT 1) SELECT id, status FROM orders", With),
    ] {
        assert_eq!(insert_source(PG, Some(&k), &cols, &names, sql), Err(why), "{sql}");
    }
}

#[test]
fn the_result_columns_must_be_that_tables_columns() {
    use NotInsertable::*;
    let k = more();
    let src =
        |origins: &[Option<ColumnOrigin>], names: &[&str], sql: &str| insert_source(PG, Some(&k), origins, names, sql);
    let sql = "SELECT id, status FROM shop.orders";
    assert_eq!(src(&[o(11, 1), None], &["id", "n"], "SELECT id, 1 AS n FROM shop.orders"), Err(Computed));
    assert_eq!(src(&[None], &["count"], "SELECT count(*) FROM orders"), Err(Computed));
    assert_eq!(src(&[o(11, 1), o(10, 1)], &["id", "id2"], sql), Err(SeveralTables));
    assert_eq!(
        src(&[o(11, 1), o(11, 1)], &["id", "again"], "SELECT id, id AS again FROM shop.orders"),
        Err(RepeatedColumn)
    );
    assert_eq!(src(&[o(10, 1)], &["id"], "SELECT id FROM shop.orders"), Err(OtherTable), "FROM names another table");
    assert_eq!(src(&[o(22, 1)], &["id"], "SELECT id FROM shop.orders"), Err(OtherTable), "another schema");
    assert_eq!(src(&[o(99, 1)], &["x"], "SELECT x FROM gone"), Err(UnknownTable));
    assert_eq!(insert_source(PG, None, &[o(11, 1)], &["id"], "SELECT id FROM orders"), Err(NoCatalog));
    assert_eq!(src(&[o(11, 1), o(11, 3)], &["a", "a"], "SELECT id AS a, status AS a FROM orders"), Err(DuplicateNames));
    // A view: its rows are not a table's rows.
    assert_eq!(src(&[o(20, 1), o(20, 2)], &["id", "status"], "SELECT * FROM shop.recent"), Err(View));
}

#[test]
fn copy_insert_into_a_named_table() {
    use NotInsertable::*;
    let mut k = more();
    k.set_generated(13, 1, Generated::IdentityAlways);
    k.set_generated(13, 3, Generated::Expression);
    // Result columns go into the columns of the same name, in result order.
    assert_eq!(
        table_of(insert_into(PG, Some(&k), "shop.orders", &["status", "id"])),
        ("shop.orders".into(), vec![(0, "status"), (1, "id")])
    );
    assert_eq!(table_of(insert_into(PG, Some(&k), " \"shop\" . \"users\" ", &["email"])).0, "shop.users");
    assert_eq!(table_of(insert_into(PG, Some(&k), "USERS", &["name"])).0, "shop.users", "folded, one schema");
    // Into a view, when the user says so.
    assert_eq!(table_of(insert_into(PG, Some(&k), "shop.recent", &["id"])).0, "shop.recent");
    let p = insert_into(PG, Some(&k), "shop.pairs", &["a", "b"]).unwrap();
    assert!(p.overriding, "an identity column keeps its value");
    assert_eq!(insert_into(PG, Some(&k), "shop.pairs", &["b", "c"]), Err(GeneratedColumn("c".into())));
    assert_eq!(insert_into(PG, Some(&k), "shop.orders", &["id", "nope"]), Err(NoSuchColumn("nope".into())));
    assert_eq!(insert_into(PG, Some(&k), "shop.nope", &["id"]), Err(NoSuchTable));
    assert_eq!(insert_into(PG, Some(&k), "a.b.c", &["id"]), Err(NoSuchTable));
    assert_eq!(insert_into(PG, Some(&k), "orders", &["id"]), Err(AmbiguousTable), "shop.orders and other.orders");
    assert_eq!(insert_into(PG, Some(&k), "shop.orders", &["id", "id"]), Err(DuplicateNames));
    assert_eq!(insert_into(PG, None, "shop.orders", &["id"]), Err(NoCatalog));
}

#[test]
fn generated_columns_are_left_out_and_identity_always_overrides() {
    let mut k = shop();
    // shop.pairs: a GENERATED ALWAYS AS IDENTITY, c GENERATED ALWAYS AS (…) STORED.
    k.set_generated(13, 1, Generated::IdentityAlways);
    k.set_generated(13, 3, Generated::Expression);
    k.set_generated(13, 42, Generated::Expression);
    let sql = "SELECT a, b, c FROM shop.pairs";
    let Ok(InsertPlan { columns, skipped, overriding, .. }) =
        insert_source(PG, Some(&k), &[o(13, 1), o(13, 2), o(13, 3)], &["a", "b", "c"], sql)
    else {
        panic!("one table")
    };
    assert_eq!((columns, skipped, overriding), (vec![(0, "a"), (1, "b")], vec!["c"], true));
    let Ok(InsertPlan { columns, overriding, .. }) =
        insert_source(PG, Some(&k), &[o(13, 2), o(13, 3)], &["b", "c"], sql)
    else {
        panic!("one table")
    };
    assert_eq!((columns, overriding), (vec![(0, "b")], false), "no identity column written");
    assert_eq!(insert_source(PG, Some(&k), &[o(13, 3)], &["c"], sql), Err(NotInsertable::OnlyGenerated));
}

/// `UPDATE` statements need the `INSERT` allowlist and every primary key
/// column; the key finds the row, the other columns are set (generated ones are not).
#[test]
fn updates_need_one_table_and_its_whole_primary_key() {
    let k = shop();
    let upd =
        |origins: &[Option<ColumnOrigin>], names: &[&str], sql: &str| update_source(PG, Some(&k), origins, names, sql);
    let Ok(UpdatePlan { table, set, keys }) =
        upd(&[o(11, 3), o(11, 1)], &["s", "i"], "SELECT status AS s, id AS i FROM shop.orders")
    else {
        panic!("updatable")
    };
    assert_eq!((table.name.as_str(), set, keys), ("orders", vec![(0, "status")], vec![(1, "id")]));
    // A composite key: both columns, in result order.
    let Ok(p) = upd(
        &[o(12, 2), o(12, 3), o(12, 1)],
        &["p", "q", "o"],
        "SELECT product_id, qty, order_id FROM shop.order_items",
    ) else {
        panic!("updatable")
    };
    assert_eq!((p.set, p.keys), (vec![(1, "qty")], vec![(0, "product_id"), (2, "order_id")]));
    // Refused, with why.
    assert_eq!(
        upd(&[o(12, 1), o(12, 3)], &["o", "q"], "SELECT order_id, qty FROM shop.order_items"),
        Err(NotUpdatable::MissingKey(vec!["product_id".into()]))
    );
    assert_eq!(upd(&[o(13, 1), o(13, 3)], &["a", "c"], "SELECT a, c FROM shop.pairs"), Err(NotUpdatable::NoPrimaryKey));
    assert_eq!(upd(&[o(11, 1)], &["id"], "SELECT id FROM shop.orders"), Err(NotUpdatable::NothingToSet));
    assert_eq!(
        upd(&[o(11, 1), o(10, 3)], &["id", "name"], "SELECT o.id, u.name FROM shop.orders o JOIN shop.users u ON true"),
        Err(NotUpdatable::NotInsertable(NotInsertable::SeveralTables))
    );
    assert_eq!(
        update_source(
            Dialect::Postgres,
            None,
            &[o(11, 1), o(11, 3)],
            &["id", "status"],
            "SELECT id, status FROM shop.orders"
        ),
        Err(NotUpdatable::NotInsertable(NotInsertable::NoCatalog))
    );
    // Generated columns are not set.
    let mut k = shop();
    k.set_generated(11, 3, Generated::Expression);
    k.set_generated(11, 2, Generated::IdentityAlways);
    let sql = "SELECT id, user_id, status FROM shop.orders";
    assert_eq!(
        update_source(PG, Some(&k), &[o(11, 1), o(11, 2), o(11, 3)], &["id", "user_id", "status"], sql),
        Err(NotUpdatable::NothingToSet)
    );
}

/// A driver that names the column a result column comes from: the catalog finds it by name,
/// and the copies treat it as one of that table's columns.
#[test]
fn a_column_named_by_schema_table_and_column_is_found() {
    let k = shop();
    let named = |table: &str, column: &str| {
        Some(ColumnOrigin::Named { schema: "shop".into(), table: table.into(), column: column.into() })
    };
    assert_eq!(k.marks(named("users", "id").as_ref()), PK);
    assert_eq!(k.marks(named("users", "email").as_ref()), UQ);
    assert_eq!(k.marks(named("users", "nope").as_ref()), NONE);
    assert_eq!(k.marks(named("nope", "id").as_ref()), NONE);
    let sql = "SELECT id, status FROM shop.orders";
    let origins = [named("orders", "id"), named("orders", "status")];
    let plan = insert_source(PG, Some(&k), &origins, &["id", "status"], sql).unwrap();
    assert_eq!((plan.table.name.as_str(), plan.columns), ("orders", vec![(0, "id"), (1, "status")]));
    let plan = update_source(PG, Some(&k), &origins, &["id", "status"], sql).unwrap();
    assert_eq!((plan.set, plan.keys), (vec![(1, "status")], vec![(0, "id")]));
    assert_eq!(insert_source(PG, None, &origins, &["id", "status"], sql), Err(NotInsertable::NoCatalog));
    let unknown = [named("orders", "id"), named("orders", "nope")];
    assert_eq!(insert_source(PG, Some(&k), &unknown, &["id", "x"], sql), Err(NotInsertable::UnknownTable));
    let mixed = [named("orders", "id"), named("users", "name")];
    assert_eq!(insert_source(PG, Some(&k), &mixed, &["id", "name"], sql), Err(NotInsertable::SeveralTables));
}
