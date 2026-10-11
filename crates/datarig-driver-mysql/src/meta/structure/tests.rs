use super::*;

const MY: MySqlMode = MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: false };

fn parse_my(json: &str) -> Result<TableStructure, DbError> {
    parse(json, false, MY, "shop", "items")
}

/// A table as MySQL 8.4 describes it: the document's lists in the server's own (unsorted)
/// order.
const ITEMS: &str = r#"{"kind":"BASE TABLE","rows":42,"bytes":131072,"trigger_privilege":true,
  "columns":[
    {"position":3,"name":"name","type":"varchar(100)","data_type":"varchar","nullable":"NO","default":"it's","extra":"","generation":""},
    {"position":1,"name":"id","type":"bigint unsigned","data_type":"bigint","nullable":"NO","default":null,"extra":"auto_increment","generation":""},
    {"position":2,"name":"pa","type":"int","data_type":"int","nullable":"YES","default":null,"extra":"","generation":""},
    {"position":4,"name":"qty","type":"int","data_type":"int","nullable":"NO","default":"0","extra":"","generation":""},
    {"position":5,"name":"price","type":"decimal(10,2)","data_type":"decimal","nullable":"YES","default":"1.5","extra":"DEFAULT_GENERATED","generation":""},
    {"position":6,"name":"created","type":"timestamp(3)","data_type":"timestamp","nullable":"NO","default":"CURRENT_TIMESTAMP(3)","extra":"DEFAULT_GENERATED on update CURRENT_TIMESTAMP(3)","generation":""},
    {"position":7,"name":"twice","type":"int","data_type":"int","nullable":"YES","default":null,"extra":"STORED GENERATED","generation":"(`qty` * 2)"},
    {"position":8,"name":"low","type":"varchar(100)","data_type":"varchar","nullable":"YES","default":null,"extra":"VIRTUAL GENERATED","generation":"lower(`name`)"},
    {"position":9,"name":"body","type":"text","data_type":"text","nullable":"YES","default":null,"extra":"","generation":""}],
  "indexes":[
    {"name":"ix_desc","seq":2,"column":"name","expression":null,"sub_part":null,"collation":"A","non_unique":1,"type":"BTREE"},
    {"name":"PRIMARY","seq":1,"column":"id","expression":null,"sub_part":null,"collation":"A","non_unique":0,"type":"BTREE"},
    {"name":"ix_desc","seq":1,"column":"qty","expression":null,"sub_part":null,"collation":"D","non_unique":1,"type":"BTREE"},
    {"name":"ix_fn","seq":1,"column":null,"expression":"upper(`name`)","sub_part":null,"collation":"A","non_unique":0,"type":"BTREE"},
    {"name":"ix_prefix","seq":1,"column":"body","expression":null,"sub_part":10,"collation":"A","non_unique":1,"type":"BTREE"},
    {"name":"ft_body","seq":1,"column":"body","expression":null,"sub_part":null,"collation":null,"non_unique":1,"type":"FULLTEXT"},
    {"name":"uq_name","seq":1,"column":"name","expression":null,"sub_part":null,"collation":"A","non_unique":0,"type":"BTREE"}],
  "foreign_keys":[
    {"name":"fk_parent","position":2,"column":"name","ref_schema":"datarig","ref_table":"parent","ref_column":"b","on_delete":"CASCADE","on_update":"NO ACTION"},
    {"name":"fk_parent","position":1,"column":"pa","ref_schema":"datarig","ref_table":"parent","ref_column":"a","on_delete":"CASCADE","on_update":"NO ACTION"}],
  "checks":[
    {"name":"ck_qty","clause":"(`qty` >= 0)","enforced":"YES"},
    {"name":"ck_loose","clause":"((`price` < 1000) and (`name` <> _utf8mb4'`qty`'))","enforced":"NO"}],
  "triggers":[
    {"name":"items_bi","timing":"BEFORE","event":"INSERT","orientation":"ROW","statement":"SET NEW.qty = NEW.qty + 0"},
    {"name":"items_au","timing":"AFTER","event":"UPDATE","orientation":"ROW","statement":"BEGIN SET @x = 1; END"}]}"#;

#[test]
fn a_table_reads_as_the_model_in_the_servers_order() {
    let s = parse_my(ITEMS).unwrap();
    assert_eq!((s.kind, s.estimated_rows, s.total_bytes), (RelationKind::Table, Some(42), Some(131072)));
    let names: Vec<&str> = s.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["id", "pa", "name", "qty", "price", "created", "twice", "low", "body"]);
    let indexes: Vec<&str> = s.indexes.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(indexes, ["PRIMARY", "ft_body", "ix_desc", "ix_fn", "ix_prefix", "uq_name"]);
    let checks: Vec<&str> = s.checks.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(checks, ["ck_loose", "ck_qty"]);
    let triggers: Vec<&str> = s.triggers.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(triggers, ["items_au", "items_bi"]);
    assert!(s.hidden.is_empty());
}

#[test]
fn columns_say_how_they_are_filled() {
    let s = parse_my(ITEMS).unwrap();
    let fills: Vec<(&str, &ColumnFill, Option<&str>, bool)> =
        s.columns.iter().map(|c| (c.name.as_str(), &c.fill, c.default.as_deref(), c.not_null)).collect();
    assert_eq!(
        fills,
        [
            ("id", &ColumnFill::AutoIncrement, None, true),
            ("pa", &ColumnFill::Default, None, false),
            ("name", &ColumnFill::Default, Some("'it''s'"), true),
            ("qty", &ColumnFill::Default, Some("0"), true),
            ("price", &ColumnFill::Default, Some("(1.5)"), false),
            ("created", &ColumnFill::Default, Some("CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3)"), true),
            ("twice", &ColumnFill::Stored("`qty` * 2".into()), None, false),
            ("low", &ColumnFill::Virtual("lower(`name`)".into()), None, false),
            ("body", &ColumnFill::Default, None, false),
        ]
    );
    assert_eq!(s.columns[0].type_name, "bigint unsigned");
}

#[test]
fn defaults_are_written_as_sql() {
    let my = Dialect::MySql(MY);
    let plain = Dialect::MySql(MySqlMode { no_backslash_escapes: true, ..MY });
    let def = |v: Option<&str>, ty: &str, extra: &str, mariadb: bool, d: Dialect| {
        default_text(v.map(str::to_string), ty, extra, mariadb, d)
    };
    assert_eq!(def(Some(r"a\b"), "varchar", "", false, my).as_deref(), Some(r"'a\\b'"));
    assert_eq!(def(Some(r"a\b"), "varchar", "", false, plain).as_deref(), Some(r"'a\b'"));
    assert_eq!(def(Some("b'101'"), "bit", "", false, my).as_deref(), Some("b'101'"));
    assert_eq!(def(Some("0x6162"), "varbinary", "", false, my).as_deref(), Some("0x6162"));
    assert_eq!(def(Some("0x6162"), "varchar", "", false, my).as_deref(), Some("'0x6162'"), "a string");
    assert_eq!(def(Some("2024"), "year", "", false, my).as_deref(), Some("'2024'"));
    assert_eq!(def(Some("rand()"), "double", "DEFAULT_GENERATED", false, my).as_deref(), Some("(rand())"));
    // Before MySQL 8.0.13 `CURRENT_TIMESTAMP` is not marked.
    assert_eq!(def(Some("CURRENT_TIMESTAMP"), "datetime", "", false, my).as_deref(), Some("CURRENT_TIMESTAMP"));
    assert_eq!(def(Some("CURRENT_TIMESTAMP"), "varchar", "", false, my).as_deref(), Some("'CURRENT_TIMESTAMP'"));
    // No default but an `ON UPDATE`.
    assert_eq!(
        def(None, "timestamp", "on update CURRENT_TIMESTAMP", false, my).as_deref(),
        Some("NULL ON UPDATE CURRENT_TIMESTAMP")
    );
    // MariaDB writes its defaults as SQL already, `NULL` for none.
    assert_eq!(def(Some("'x'"), "varchar", "", true, my).as_deref(), Some("'x'"));
    assert_eq!(def(Some("NULL"), "varchar", "", true, my), None);
    assert_eq!(
        def(Some("current_timestamp()"), "timestamp", "on update current_timestamp()", true, my).as_deref(),
        Some("current_timestamp() ON UPDATE current_timestamp()")
    );
}

#[test]
fn indexes_keep_prefixes_order_and_expressions() {
    let s = parse_my(ITEMS).unwrap();
    let x = |name: &str| s.indexes.iter().find(|x| x.name == name).unwrap();
    let desc = x("ix_desc");
    assert_eq!(desc.keys(), ["qty DESC", "name"]);
    assert_eq!(desc.key_columns, [Some("qty".to_string()), Some("name".to_string())]);
    assert_eq!(desc.definition, "CREATE INDEX ix_desc ON shop.items (qty DESC, name)");
    let prefix = x("ix_prefix");
    assert_eq!(
        (prefix.columns.as_slice(), prefix.key_columns.as_slice()),
        (["body(10)".to_string()].as_slice(), [Some("body".to_string())].as_slice())
    );
    let f = x("ix_fn");
    assert_eq!(
        (f.columns.as_slice(), f.key_columns.as_slice()),
        (["(upper(`name`))".to_string()].as_slice(), [None].as_slice())
    );
    assert!(f.unique && !f.constraint, "a unique key on an expression is an index only");
    assert_eq!(f.definition, "CREATE UNIQUE INDEX ix_fn ON shop.items ((upper(`name`)))");
    let ft = x("ft_body");
    assert_eq!((ft.method.as_str(), ft.unique), ("FULLTEXT", false));
    assert_eq!(ft.definition, "CREATE FULLTEXT INDEX ft_body ON shop.items (body)");
    let pk = x("PRIMARY");
    assert!(pk.primary && pk.unique && pk.constraint);
    assert_eq!(pk.definition, "ALTER TABLE shop.items ADD PRIMARY KEY (id)");
    assert_eq!(
        s.primary_key,
        Some(KeyConstraint {
            name: "PRIMARY".into(),
            columns: vec!["id".into()],
            definition: "PRIMARY KEY (id)".into()
        })
    );
    assert_eq!(
        s.unique_constraints,
        [KeyConstraint {
            name: "uq_name".into(),
            columns: vec!["name".into()],
            definition: "UNIQUE KEY uq_name (name)".into()
        }]
    );
    assert!(x("uq_name").constraint);
}

#[test]
fn a_foreign_key_names_the_database_it_references() {
    let s = parse_my(ITEMS).unwrap();
    assert_eq!(
        s.foreign_keys,
        [ForeignKey {
            name: "fk_parent".into(),
            columns: vec!["pa".into(), "name".into()],
            ref_schema: "datarig".into(),
            ref_table: "parent".into(),
            ref_columns: vec!["a".into(), "b".into()],
            on_delete: FkAction::Cascade,
            on_update: FkAction::NoAction,
            definition:
                "CONSTRAINT fk_parent FOREIGN KEY (pa, name) REFERENCES datarig.parent (a, b) ON DELETE CASCADE".into(),
        }]
    );
    assert_eq!(
        ["NO ACTION", "RESTRICT", "CASCADE", "SET NULL", "SET DEFAULT"].map(fk_action),
        [FkAction::NoAction, FkAction::Restrict, FkAction::Cascade, FkAction::SetNull, FkAction::SetDefault]
    );
}

#[test]
fn checks_read_their_columns_and_say_when_not_enforced() {
    let s = parse_my(ITEMS).unwrap();
    let qty = &s.checks[1];
    assert_eq!((qty.expression.as_str(), qty.columns.as_slice()), ("`qty` >= 0", ["qty".to_string()].as_slice()));
    assert_eq!(qty.definition, "CONSTRAINT ck_qty CHECK (`qty` >= 0)");
    assert!(qty.enforced());
    let loose = &s.checks[0];
    // A name inside a string literal is no column it reads; the order is the table's.
    assert_eq!(loose.columns, ["name", "price"]);
    assert_eq!(loose.expression, "(`price` < 1000) and (`name` <> _utf8mb4'`qty`')");
    assert!(loose.definition.ends_with(" /*!80016 NOT ENFORCED */"), "{}", loose.definition);
    assert!(!loose.enforced());
    assert_eq!(unwrapped("(a) or (b)"), "(a) or (b)");
    assert_eq!(unwrapped("(`a)` > 0)"), "`a)` > 0");
    assert_eq!(quoted_names("`a``b` + `c`"), ["a`b", "c"]);
}

#[test]
fn triggers_have_no_function_and_keep_their_statement() {
    let s = parse_my(ITEMS).unwrap();
    let t = &s.triggers[1];
    assert_eq!(
        (t.timing, t.events.as_slice(), t.for_each_row, t.function.as_str(), t.enabled),
        (TriggerTiming::Before, [TriggerEvent::Insert].as_slice(), true, "", true)
    );
    assert_eq!(
        t.definition,
        "CREATE TRIGGER items_bi BEFORE INSERT ON shop.items FOR EACH ROW SET NEW.qty = NEW.qty + 0"
    );
    assert_eq!(
        s.triggers[0].definition,
        "CREATE TRIGGER items_au AFTER UPDATE ON shop.items FOR EACH ROW BEGIN SET @x = 1; END"
    );
    // A timing or event the model has no words for fails the read, it is not left out.
    let odd = ITEMS.replace(r#""timing":"BEFORE""#, r#""timing":"INSTEAD""#);
    assert!(matches!(parse_my(&odd), Err(DbError::Server(m)) if m.contains("items_bi")));
}

#[test]
fn triggers_the_user_may_not_see_are_unknown_not_none() {
    let none = r#"{"kind":"BASE TABLE","rows":0,"bytes":16384,"trigger_privilege":0}"#;
    let s = parse_my(none).unwrap();
    assert_eq!(s.hidden, [StructureGroup::Triggers]);
    assert!(s.triggers.is_empty() && s.columns.is_empty());
    let granted = parse_my(&none.replace(r#""trigger_privilege":0"#, r#""trigger_privilege":true"#)).unwrap();
    assert!(granted.hidden.is_empty(), "with the privilege an empty list is none");
    // Triggers listed are known, whatever the grants say.
    let listed = ITEMS.replace(r#""trigger_privilege":true"#, r#""trigger_privilege":false"#);
    assert!(parse_my(&listed).unwrap().hidden.is_empty());
    // A view has no triggers on MySQL: nothing to hide.
    let view = r#"{"kind":"VIEW","rows":null,"bytes":null,"trigger_privilege":false}"#;
    let v = parse_my(view).unwrap();
    assert_eq!(
        (v.kind, v.estimated_rows, v.total_bytes, v.hidden.as_slice()),
        (RelationKind::View, None, None, [].as_slice())
    );
}

#[test]
fn kinds_of_tables_are_an_allowlist() {
    let doc = |kind: &str| format!(r#"{{"kind":"{kind}","rows":5,"bytes":0,"trigger_privilege":true}}"#);
    assert_eq!(parse_my(&doc("BASE TABLE")).unwrap().estimated_rows, Some(5));
    assert_eq!(parse_my(&doc("SYSTEM VERSIONED")).unwrap().kind, RelationKind::Table);
    assert_eq!(parse_my(&doc("SYSTEM VIEW")), Err(DbError::NotSupported));
    assert_eq!(parse_my(&doc("SEQUENCE")), Err(DbError::NotSupported));
    assert!(matches!(parse_my("{}"), Err(DbError::Server(m)) if m.starts_with("table structure:")));
}

#[test]
fn names_are_quoted_as_mysql_reads_them() {
    let s = parse(
        r#"{"kind":"BASE TABLE","rows":0,"bytes":0,"trigger_privilege":true,
        "indexes":[{"name":"by key","seq":1,"column":"select","expression":null,"sub_part":null,"collation":"A","non_unique":1,"type":"HASH"}]}"#,
        false,
        MY,
        "my db",
        "t`1",
    )
    .unwrap();
    assert_eq!(s.indexes[0].definition, "CREATE INDEX `by key` ON `my db`.`t``1` (`select`) USING HASH");
}

#[test]
fn the_statement_asks_only_for_what_the_server_has() {
    let my = |v| Server { version: v, mariadb: false };
    let sql = statement(my((8, 4, 6)), MY, "it's", "t");
    assert!(sql.contains("x.EXPRESSION") && sql.contains("tc.ENFORCED"));
    assert!(sql.contains(r"t.TABLE_SCHEMA = 'it''s' AND t.TABLE_NAME = 't'"), "{sql}");
    assert!(!sql.contains("CARDINALITY"), "it can open the table");
    // MariaDB and MySQL before 8.0.14 cannot name the outer table inside a derived table.
    assert!(!sql.contains("FROM ("), "no derived table: {sql}");
    let old = statement(my((8, 0, 12)), MY, "s", "t");
    assert!(!old.contains("x.EXPRESSION") && !old.contains("CHECK_CONSTRAINTS"));
    let mid = statement(my((8, 0, 15)), MY, "s", "t");
    assert!(mid.contains("x.EXPRESSION") && !mid.contains("CHECK_CONSTRAINTS"));
    let maria = statement(Server { version: (11, 4, 2), mariadb: true }, MY, "s", "t");
    assert!(!maria.contains("x.EXPRESSION") && maria.contains("cc.TABLE_NAME = t.TABLE_NAME"));
    let plain = statement(my((8, 4, 6)), MySqlMode { no_backslash_escapes: true, ..MY }, r"a\b", "t");
    assert!(plain.contains(r"t.TABLE_SCHEMA = 'a\b'"), "{plain}");
}

#[test]
fn a_read_the_server_stopped_or_gave_up_says_so() {
    let server = |code| {
        mysql_async::Error::Server(mysql_async::ServerError { code, message: "m".into(), state: "HY000".into() })
    };
    assert_eq!(super::super::read_error(&server(1205)), DbError::Locked);
    assert_eq!(super::super::read_error(&server(3024)), DbError::NoAnswer(std::time::Duration::from_secs(10)));
    assert_eq!(super::super::read_error(&server(1969)), DbError::NoAnswer(std::time::Duration::from_secs(10)));
    assert!(matches!(super::super::read_error(&server(1142)), DbError::Server(m) if m.starts_with("ERROR 1142")));
}
