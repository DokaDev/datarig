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
    assert_eq!((s.kind, s.stats()), (RelationKind::Table, None), "the listing has the estimates");
    let names: Vec<&str> = s.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["id", "pa", "name", "qty", "price", "created", "twice", "low", "body"]);
    let indexes: Vec<&str> = s.indexes.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(indexes, ["PRIMARY", "ft_body", "ix_desc", "ix_fn", "ix_prefix", "uq_name"]);
    let checks: Vec<&str> = s.checks.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(checks, ["ck_loose", "ck_qty"]);
    let triggers: Vec<&str> = s.triggers.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(triggers, ["items_bi", "items_au"], "in firing order");
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
        ["NO ACTION", "RESTRICT", "CASCADE", "SET NULL", "SET DEFAULT"].map(|r| fk_action("fk", r).unwrap()),
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
    let t = &s.triggers[0];
    assert_eq!(
        (t.timing, t.events.as_slice(), t.for_each_row, t.function.as_str(), t.enabled),
        (TriggerTiming::Before, [TriggerEvent::Insert].as_slice(), true, "", true)
    );
    assert_eq!(
        t.definition,
        "CREATE TRIGGER items_bi BEFORE INSERT ON shop.items FOR EACH ROW SET NEW.qty = NEW.qty + 0"
    );
    assert_eq!(
        s.triggers[1].definition,
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
    assert_eq!(parse_my(&doc("BASE TABLE")).unwrap().kind, RelationKind::Table);
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
    assert_eq!(super::super::read_error(&server(1969)), DbError::Server("ERROR 1969 (HY000): m".into()));
    assert!(matches!(super::super::read_error(&server(1142)), DbError::Server(m) if m.starts_with("ERROR 1142")));
}

/// A document of one table with `parts` (`"columns": …` and so on) besides its kind.
fn doc(parts: serde_json::Value) -> String {
    let mut d = serde_json::json!({"kind": "BASE TABLE", "rows": 7, "bytes": 16384, "trigger_privilege": true});
    if let (Some(d), Some(p)) = (d.as_object_mut(), parts.as_object()) {
        d.extend(p.clone());
    }
    d.to_string()
}

fn col(
    position: u32,
    name: &str,
    data_type: &str,
    default: Option<&str>,
    extra: &str,
    generation: &str,
) -> serde_json::Value {
    serde_json::json!({"position": position, "name": name, "type": data_type, "data_type": data_type,
        "nullable": "YES", "default": default, "extra": extra, "generation": generation})
}

fn key(
    name: &str,
    seq: u32,
    column: Option<&str>,
    expression: Option<&str>,
    sub_part: Option<u32>,
    unique: bool,
    method: &str,
) -> serde_json::Value {
    serde_json::json!({"name": name, "seq": seq, "column": column, "expression": expression, "sub_part": sub_part,
        "collation": "A", "non_unique": if unique { 0 } else { 1 }, "type": method})
}

/// MySQL writes the expressions it keeps (a check's condition, a generated column's, a
/// functional key's, an expression default) with one level of string escaping, MariaDB as they
/// are: on MySQL the text is unescaped once, so that it is SQL again.
#[test]
fn expressions_are_unescaped_once_on_mysql() {
    let json = doc(serde_json::json!({
        "columns": [
            col(1, "s", "varchar", Some(r"concat(_utf8mb4\'a\',_utf8mb4\'\\\'b\')"), "DEFAULT_GENERATED", ""),
            col(2, "g", "varchar", None, "VIRTUAL GENERATED", r"concat(`s`,_utf8mb4\'x\\\'y\')"),
            col(3, "QtY", "int", None, "", ""),
        ],
        "indexes": [key("fx", 1, None, Some(r"concat(`s`,_utf8mb4\'q\\\'r\')"), None, false, "BTREE")],
        "checks": [{"name": "ck", "clause": r"((`s` <> _utf8mb4\'a\\\'b\\\\c\') and (`qty` > 0))", "enforced": "YES"}],
    }));
    let s = parse_my(&json).unwrap();
    assert_eq!(s.columns[0].default.as_deref(), Some(r"(concat(_utf8mb4'a',_utf8mb4'\'b'))"));
    assert_eq!(s.columns[1].fill, ColumnFill::Virtual(r"concat(`s`,_utf8mb4'x\'y')".into()));
    assert_eq!(s.indexes[0].columns, [r"(concat(`s`,_utf8mb4'q\'r'))"]);
    assert_eq!(s.checks[0].expression, r"(`s` <> _utf8mb4'a\'b\\c') and (`qty` > 0)");
    // Every column the condition reads, past the literal, by the table's name for it.
    assert_eq!(s.checks[0].columns, ["s", "QtY"]);
    // MariaDB's text is SQL already.
    let plain = doc(serde_json::json!({"checks": [{"name": "ck", "clause": r"(`s` <> 'a\\b')", "enforced": "YES"}]}));
    assert_eq!(parse(&plain, true, MY, "shop", "items").unwrap().checks[0].expression, r"`s` <> 'a\\b'");
    assert_eq!(parse_my(&plain).unwrap().checks[0].expression, r"`s` <> 'a\b'");
}

/// Reading a table's estimates opens the table when the server has none cached: the structure
/// does not ask for them, and has none of its own (the schema's listing has them).
#[test]
fn the_estimates_are_not_read_with_the_structure() {
    let sql = statement(Server { version: (8, 4, 6), mariadb: false }, MY, "s", "t");
    assert!(!sql.contains("TABLE_ROWS") && !sql.contains("DATA_LENGTH") && !sql.contains("INDEX_LENGTH"), "{sql}");
    let s = parse_my(&doc(serde_json::json!({}))).unwrap();
    assert_eq!((s.estimated_rows, s.total_bytes, s.stats()), (None, None, None));
}

/// A spatial key has no prefix of its own (the server says 32): it is not written as one.
#[test]
fn a_spatial_key_has_no_prefix() {
    let s = parse_my(&doc(serde_json::json!({"indexes": [key("sp", 1, Some("g"), None, Some(32), false, "SPATIAL")]})))
        .unwrap();
    assert_eq!(s.indexes[0].columns, ["g"]);
    assert_eq!(s.indexes[0].definition, "CREATE SPATIAL INDEX sp ON shop.items (g)");
}

/// A unique key on a column's prefix makes only the prefix unique: no unique constraint on the
/// column, which is not marked unique.
#[test]
fn a_unique_prefix_key_is_an_index_only() {
    let json = doc(serde_json::json!({
        "columns": [col(1, "b", "text", None, "", "")],
        "indexes": [key("uq_pre", 1, Some("b"), None, Some(10), true, "BTREE")],
    }));
    let s = parse_my(&json).unwrap();
    assert!(s.unique_constraints.is_empty(), "{:?}", s.unique_constraints);
    let x = &s.indexes[0];
    assert!(x.unique && !x.constraint);
    assert_eq!(x.definition, "CREATE UNIQUE INDEX uq_pre ON shop.items (b(10))");
    assert!(!s.marks("b").unique);
}

/// `EXTRA` lists more than the `ON UPDATE` timestamp (`INVISIBLE`, `DEFAULT_GENERATED`): only
/// the timestamp is the column's. An invisible index says so in its definition.
#[test]
fn extra_gives_only_what_belongs_to_each_part() {
    let json = doc(serde_json::json!({
        "columns": [
            col(1, "ts", "timestamp", Some("CURRENT_TIMESTAMP"), "DEFAULT_GENERATED on update CURRENT_TIMESTAMP INVISIBLE", ""),
            col(2, "t3", "timestamp", None, "on update CURRENT_TIMESTAMP(3) INVISIBLE", ""),
            col(3, "n", "int", Some("1"), "INVISIBLE", ""),
        ],
        "indexes": [{"name": "inv", "seq": 1, "column": "n", "expression": null, "sub_part": null, "collation": "A",
            "non_unique": 1, "type": "BTREE", "visible": "NO"}],
    }));
    let s = parse_my(&json).unwrap();
    let defaults: Vec<Option<&str>> = s.columns.iter().map(|c| c.default.as_deref()).collect();
    assert_eq!(
        defaults,
        [Some("CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP"), Some("NULL ON UPDATE CURRENT_TIMESTAMP(3)"), Some("1")]
    );
    assert_eq!(s.indexes[0].definition, "CREATE INDEX inv ON shop.items (n) INVISIBLE");
}

/// A check's condition names a column in its own case (MySQL's column names are not case
/// sensitive): it is still that column.
#[test]
fn check_columns_match_names_in_any_case() {
    let json = doc(serde_json::json!({
        "columns": [col(1, "QtY", "int", None, "", ""), col(2, "Price", "int", None, "", "")],
        "checks": [{"name": "ck", "clause": "((`qty` > 0) and (`PRICE` > 0))", "enforced": "YES"}],
    }));
    assert_eq!(parse_my(&json).unwrap().checks[0].columns, ["QtY", "Price"]);
}

/// Triggers are in the order they fire: by timing and event, then their order among those.
#[test]
fn triggers_are_in_firing_order() {
    let t = |name: &str, timing: &str, event: &str, order: u32| {
        serde_json::json!({"name": name, "timing": timing, "event": event, "orientation": "ROW",
            "statement": "SET @x = 1", "order": order})
    };
    let json = doc(serde_json::json!({"triggers": [
        t("a_after_delete", "AFTER", "DELETE", 1),
        t("b_before_update_2", "BEFORE", "UPDATE", 2),
        t("c_before_insert", "BEFORE", "INSERT", 1),
        t("d_before_update_1", "BEFORE", "UPDATE", 1),
        t("e_after_insert", "AFTER", "INSERT", 1),
    ]}));
    let names: Vec<String> = parse_my(&json).unwrap().triggers.into_iter().map(|t| t.name).collect();
    assert_eq!(
        names,
        ["c_before_insert", "d_before_update_1", "b_before_update_2", "e_after_insert", "a_after_delete"]
    );
}

/// A foreign key rule the model has no words for fails the read: it is not taken for
/// `NO ACTION`.
#[test]
fn an_unknown_foreign_key_rule_fails_the_read() {
    let fk = |rule: &str| {
        doc(serde_json::json!({"foreign_keys": [{"name": "fk", "position": 1, "column": "a", "ref_schema": "s",
            "ref_table": "p", "ref_column": "a", "on_delete": rule, "on_update": "NO ACTION"}]}))
    };
    assert!(parse_my(&fk("CASCADE")).is_ok());
    assert!(matches!(parse_my(&fk("SET SOMETHING")), Err(DbError::Server(m)) if m.contains("SET SOMETHING")));
}

/// The privilege tables name a grantee `'user'@'host'`, and a user name may hold an `@`: the
/// host is after the last one.
#[test]
fn the_grantee_is_split_at_the_last_at_sign() {
    let sql = statement(Server { version: (8, 4, 6), mariadb: false }, MY, "s", "t");
    assert!(!sql.contains("SUBSTRING_INDEX(CURRENT_USER(), '@', 1)"), "{sql}");
    assert!(sql.contains("SUBSTRING_INDEX(CURRENT_USER(), '@', -1)"), "{sql}");
}

/// A read the server stops at the session's time limit keeps the server's own words, which say
/// what happened.
#[test]
fn a_read_past_the_time_limit_keeps_the_servers_message() {
    let e = mysql_async::Error::Server(mysql_async::ServerError {
        code: 3024,
        message: "Query execution was interrupted, maximum statement execution time exceeded".into(),
        state: "HY000".into(),
    });
    assert_eq!(
        super::super::read_error(&e),
        DbError::Server(
            "ERROR 3024 (HY000): Query execution was interrupted, maximum statement execution time exceeded".into()
        )
    );
}
