use super::*;

const PG: Dialect = Dialect::Postgres;

fn col(name: &str, kind: Kind) -> Column<'_> {
    Column { name, kind }
}

/// id (number), name (text), note (text), ok (bool), doc (json).
fn cols() -> Vec<Column<'static>> {
    vec![
        col("id", Kind::Number),
        col("name", Kind::Text),
        col("note", Kind::Text),
        col("ok", Kind::Bool),
        col("doc", Kind::Json),
    ]
}

fn rows() -> Vec<Row<'static>> {
    vec![
        vec![Some("1"), Some("陳大文 🐘"), None, Some("true"), Some(r#"{"a": [1, 2]}"#)],
        vec![Some("-2.5"), Some(""), Some("tab\there\nnew \"line\""), Some("false"), None],
        vec![Some("NaN"), Some("O'Reilly, Inc."), Some("a|b\\c"), None, Some("not json")],
    ]
}

#[test]
fn kinds_come_from_the_column_type() {
    assert_eq!(Kind::of("int8", true, false), Kind::Number);
    assert_eq!(Kind::of("jsonb", false, true), Kind::Json);
    assert_eq!(Kind::of("bool", false, false), Kind::Bool);
    assert_eq!(Kind::of("text", false, false), Kind::Text);
    assert_eq!(Kind::of("bytea", false, false), Kind::Text);
}

#[test]
fn tsv_quotes_tabs_newlines_and_quotes_and_leaves_null_empty() {
    assert_eq!(
        tsv(&cols(), &rows(), true),
        "id\tname\tnote\tok\tdoc\n\
         1\t陳大文 🐘\t\ttrue\t{\"a\": [1, 2]}\n\
         -2.5\t\t\"tab\there\nnew \"\"line\"\"\"\tfalse\t\n\
         NaN\tO'Reilly, Inc.\ta|b\\c\t\tnot json"
    );
    assert_eq!(tsv(&cols()[1..2], &[vec![Some("\"quoted\" start")]], false), "\"\"\"quoted\"\" start\"");
    assert_eq!(tsv(&cols()[..1], &[vec![Some("7")]], false), "7", "one cell, no header");
}

#[test]
fn csv_follows_rfc_4180_and_tells_null_from_empty() {
    assert_eq!(
        csv(&cols(), &rows(), true),
        "id,name,note,ok,doc\n\
         1,陳大文 🐘,,true,\"{\"\"a\"\": [1, 2]}\"\n\
         -2.5,\"\",\"tab\there\nnew \"\"line\"\"\",false,\n\
         NaN,\"O'Reilly, Inc.\",a|b\\c,,not json"
    );
}

#[test]
fn json_keeps_types_embeds_json_and_names_duplicates() {
    assert_eq!(
        json(&cols(), &rows()),
        "[\n  {\"id\": 1, \"name\": \"陳大文 🐘\", \"note\": null, \"ok\": true, \"doc\": {\"a\": [1, 2]}},\n  \
         {\"id\": -2.5, \"name\": \"\", \"note\": \"tab\\there\\nnew \\\"line\\\"\", \"ok\": false, \"doc\": null},\n  \
         {\"id\": \"NaN\", \"name\": \"O'Reilly, Inc.\", \"note\": \"a|b\\\\c\", \"ok\": null, \"doc\": \"not json\"}\n]"
    );
    let dup = [col("id", Kind::Number), col("id", Kind::Number), col("id", Kind::Text)];
    assert_eq!(
        json(&dup, &[vec![Some("1"), Some("2"), Some("x")]]),
        "[\n  {\"id\": 1, \"id_2\": 2, \"id_3\": \"x\"}\n]"
    );
    assert_eq!(json(&dup, &[]), "[]");
    // Every output parses, with the values it had.
    let parsed: serde_json::Value = serde_json::from_str(&json(&cols(), &rows())).unwrap();
    assert_eq!(parsed[1]["note"], "tab\there\nnew \"line\"");
    assert_eq!(parsed[0]["name"], "陳大文 🐘");
}

#[test]
fn markdown_escapes_pipes_and_line_breaks() {
    assert_eq!(
        markdown(&cols(), &rows()),
        "| id | name | note | ok | doc |\n\
         | ---: | --- | --- | --- | --- |\n\
         | 1 | 陳大文 🐘 | NULL | true | {\"a\": [1, 2]} |\n\
         | -2.5 |  | tab\there<br>new \"line\" | false | NULL |\n\
         | NaN | O'Reilly, Inc. | a\\|b\\\\c | NULL | not json |"
    );
}

#[test]
fn sql_quotes_identifiers_and_literals() {
    assert_eq!(quote_ident("Order \"Items\""), "\"Order \"\"Items\"\"\"");
    assert_eq!(quote_literal("it's"), "'it''s'");
    assert_eq!(quote_literal("back\\slash"), "'back\\slash'", "standard conforming strings");
    for n in ["0", "12", "-3", "1.5", ".5", "5.", "1e10", "-2.5E-3"] {
        assert!(sql_number(n), "{n}");
    }
    for n in ["", "-", ".", "NaN", "Infinity", "1,000", "1e", "0x1F", "+1", "1 2"] {
        assert!(!sql_number(n), "{n}");
    }
}

#[test]
fn sql_insert_names_the_table_or_a_placeholder() {
    let target = Target::Table {
        schema: "shop",
        name: "order items",
        columns: vec!["id", "name", "note", "ok", "doc"],
        overriding: false,
    };
    assert_eq!(
        sql_insert(PG, &target, &cols(), &rows()),
        "INSERT INTO \"shop\".\"order items\" (\"id\", \"name\", \"note\", \"ok\", \"doc\") VALUES (1, '陳大文 🐘', NULL, true, '{\"a\": [1, 2]}');\n\
         INSERT INTO \"shop\".\"order items\" (\"id\", \"name\", \"note\", \"ok\", \"doc\") VALUES (-2.5, '', 'tab\there\nnew \"line\"', false, NULL);\n\
         INSERT INTO \"shop\".\"order items\" (\"id\", \"name\", \"note\", \"ok\", \"doc\") VALUES ('NaN', 'O''Reilly, Inc.', 'a|b\\c', NULL, 'not json');"
    );
    let aliased = Target::Table { schema: "s", name: "t", columns: vec!["real_name"], overriding: false };
    assert_eq!(
        sql_insert(PG, &aliased, &[col("alias", Kind::Text)], &[vec![Some("x")]]),
        "INSERT INTO \"s\".\"t\" (\"real_name\") VALUES ('x');",
        "the table's column names, not the aliases"
    );
    assert_eq!(
        sql_insert(PG, &Target::Unknown, &[col("n", Kind::Text)], &[vec![Some("\\x0102")]]),
        "INSERT INTO <table> (\"n\") VALUES ('\\x0102');",
        "bytea as its hex input form"
    );
    assert_eq!(sql_insert(PG, &Target::Unknown, &[col("n", Kind::Text)], &[]), "");
    assert_eq!(
        sql_insert(
            PG,
            &Target::Unknown,
            &[col("f", Kind::Number)],
            &[vec![Some("-0")], vec![Some("-0.0e0")], vec![Some("0")]]
        ),
        "INSERT INTO <table> (\"f\") VALUES ('-0');\nINSERT INTO <table> (\"f\") VALUES ('-0.0e0');\nINSERT INTO <table> (\"f\") VALUES (0);",
        "a float's negative zero keeps its sign"
    );
    // An identity column that is GENERATED ALWAYS takes a value only with OVERRIDING SYSTEM VALUE.
    let identity = Target::Table { schema: "s", name: "t", columns: vec!["id"], overriding: true };
    assert_eq!(
        sql_insert(PG, &identity, &[col("id", Kind::Number)], &[vec![Some("7")]]),
        "INSERT INTO \"s\".\"t\" (\"id\") OVERRIDING SYSTEM VALUE VALUES (7);"
    );
}

#[test]
fn json_numbers_keep_every_digit_and_special_values_are_strings() {
    let n = [col("n", Kind::Number)];
    let one = |v: &str| json(&n, &[vec![Some(v)]]);
    for exact in [
        "1234567890123456789012345678901234567890.123456789",
        "9007199254740993",
        "-9223372036854775808",
        "18446744073709551616",
        "0.10000000000000000000000000000000000001",
        "1.7976931348623157e+308",
        "1e-300",
        "0",
        "-0.5",
        "1234.50",
    ] {
        assert_eq!(one(exact), format!("[\n  {{\"n\": {exact}}}\n]"), "{exact}");
    }
    for special in ["NaN", "Infinity", "-Infinity", "$1,234.00", "+1", "01", "1.", ".5", "1e", "0x10", ""] {
        assert_eq!(one(special), format!("[\n  {{\"n\": \"{special}\"}}\n]"), "{special}");
    }
    let doc = [col("doc", Kind::Json)];
    assert_eq!(
        json(&doc, &[vec![Some(r#"{"big": 123456789012345678901234567890.5, "i": 9007199254740993}"#)]]),
        "[\n  {\"doc\": {\"big\": 123456789012345678901234567890.5, \"i\": 9007199254740993}}\n]",
        "numbers inside json keep their digits too"
    );
    // The output still parses as JSON.
    let v: serde_json::Value =
        serde_json::from_str(&one("1234567890123456789012345678901234567890.123456789")).unwrap();
    assert!(v[0]["n"].is_number());
}

#[test]
fn arrays_nest_in_json_and_stay_literals_elsewhere() {
    assert_eq!(Kind::of("int4[]", false, false), Kind::Array(Element::Number));
    assert_eq!(Kind::of("text[]", false, false), Kind::Array(Element::Text));
    assert_eq!(Kind::of("bool[]", false, false), Kind::Array(Element::Bool));
    assert_eq!(Kind::of("jsonb[]", false, false), Kind::Array(Element::Json));
    assert_eq!(Kind::of("box[]", false, false), Kind::Text);
    let one = |kind: Kind, v: &str| json(&[col("a", kind)], &[vec![Some(v)]]);
    let n = Kind::Array(Element::Number);
    assert_eq!(one(n, "{{1,2},{3,NULL}}"), "[\n  {\"a\": [[1,2],[3,null]]}\n]");
    assert_eq!(one(n, "{{{1},{2}},{{3},{4}}}"), "[\n  {\"a\": [[[1],[2]],[[3],[4]]]}\n]");
    assert_eq!(one(n, "{}"), "[\n  {\"a\": []}\n]");
    assert_eq!(
        one(n, "{1.5,NaN,123456789012345678901234567890}"),
        "[\n  {\"a\": [1.5,\"NaN\",123456789012345678901234567890]}\n]"
    );
    assert_eq!(one(n, "[0:1]={7,8}"), "[\n  {\"a\": \"[0:1]={7,8}\"}\n]", "bounds: the literal as a string");
    let t = Kind::Array(Element::Text);
    assert_eq!(
        one(t, r#"{{"a b","q\"uote"},{"back\\slash",NULL},{"NULL",""}}"#),
        "[\n  {\"a\": [[\"a b\",\"q\\\"uote\"],[\"back\\\\slash\",null],[\"NULL\",\"\"]]}\n]"
    );
    assert_eq!(one(Kind::Array(Element::Bool), "{t,f,true}"), "[\n  {\"a\": [true,false,true]}\n]");
    assert_eq!(one(Kind::Array(Element::Json), r#"{"{\"k\": 1}",NULL}"#), "[\n  {\"a\": [{\"k\": 1},null]}\n]");
    assert_eq!(one(n, "{1,2"), "[\n  {\"a\": \"{1,2\"}\n]", "not an array literal: a string");
    // Every other format writes the literal, which PostgreSQL reads back.
    let c = [col("a", n)];
    let r = [vec![Some("{{1,2},{3,4}}")]];
    assert_eq!(csv(&c, &r, false), "\"{{1,2},{3,4}}\"");
    assert_eq!(tsv(&c, &r, false), "{{1,2},{3,4}}");
    assert_eq!(sql_insert(PG, &Target::Unknown, &c, &r), "INSERT INTO <table> (\"a\") VALUES ('{{1,2},{3,4}}');");
}

#[test]
fn a_writer_fed_in_chunks_writes_the_same_text() {
    let c = cols();
    let all = rows();
    let target =
        Target::Table { schema: "s", name: "t", columns: c.iter().map(|c| c.name).collect(), overriding: false };
    let update = UpdateTarget { schema: "s", name: "t", set: vec![(1, "name"), (2, "note")], keys: vec![(0, "id")] };
    let formats = [
        (Format::Tsv { header: true }, tsv(&c, &all, true), tsv(&c, &[], true)),
        (Format::Tsv { header: false }, tsv(&c, &all, false), tsv(&c, &[], false)),
        (Format::Csv { header: true }, csv(&c, &all, true), csv(&c, &[], true)),
        (Format::Csv { header: false }, csv(&c, &all, false), csv(&c, &[], false)),
        (Format::Json, json(&c, &all), json(&c, &[])),
        (Format::Markdown, markdown(&c, &all), markdown(&c, &[])),
        (Format::Sql(PG, target.clone()), sql_insert(PG, &target, &c, &all), sql_insert(PG, &target, &c, &[])),
        (Format::JsonPretty, json_pretty(&c, &all), json_pretty(&c, &[])),
        (Format::List, comma_list(&all), comma_list(&[])),
        (Format::Html, html(&c, &all), html(&c, &[])),
        (Format::Xml, xml(&c, &all), xml(&c, &[])),
        (Format::Update(PG, update.clone()), sql_update(PG, &update, &c, &all), sql_update(PG, &update, &c, &[])),
    ];
    for (format, whole, empty) in formats {
        for chunk in 1..=all.len() + 1 {
            let mut w = Writer::new(format.clone(), &c);
            for part in all.chunks(chunk) {
                w.rows(part);
                w.rows(&[]);
            }
            assert_eq!(w.count(), all.len());
            assert_eq!(w.finish(), whole, "{format:?} in chunks of {chunk}");
        }
        assert_eq!(Writer::new(format.clone(), &c).finish(), empty, "{format:?} without rows");
    }
}

// ── ─────────────────────────────────────────────────────────

#[test]
fn a_comma_list_flattens_rows_left_to_right() {
    assert_eq!(comma_list(&[vec![Some("a")], vec![Some("b")], vec![None]]), "a, b, NULL");
    assert_eq!(comma_list(&[vec![Some("1"), Some("x")], vec![Some("2"), Some("中文 🐘")]]), "1, x, 2, 中文 🐘");
    assert_eq!(comma_list(&[]), "");
}

#[test]
fn pretty_json_indents_and_keeps_every_token() {
    let c = [col("n", Kind::Number), col("doc", Kind::Json), col("s", Kind::Text)];
    let one = [vec![Some("12345678901234567890.5"), Some(r#"{"a":[1, {}], "b": "x, y: {z}"}"#), Some("改\n行")]];
    assert_eq!(
        json_pretty(&c, &one),
        "[\n  {\n    \"n\": 12345678901234567890.5,\n    \"doc\": {\n      \"a\": [\n        1,\n        {}\n      ],\n      \
         \"b\": \"x, y: {z}\"\n    },\n    \"s\": \"改\\n行\"\n  }\n]"
    );
    assert_eq!(json_pretty(&c, &[]), "[]");
    // Still the same JSON.
    let a: serde_json::Value = serde_json::from_str(&json_pretty(&cols(), &rows())).unwrap();
    let b: serde_json::Value = serde_json::from_str(&json(&cols(), &rows())).unwrap();
    assert_eq!(a, b);
}

#[test]
fn html_escapes_everything_and_marks_null() {
    assert_eq!(html_text(r#"<a href="x">&'</a>"#), "&lt;a href=&quot;x&quot;&gt;&amp;&#39;&lt;/a&gt;");
    assert_eq!(html_text("a\r\nb\nc\rd"), "a<br>b<br>c<br>d");
    // Control characters HTML does not allow (they were left raw).
    assert_eq!(
        html_text("a\u{0}b\u{1}c\u{1b}[31md\u{7f}e\u{85}f\u{FFFF}g\th"),
        "a\u{FFFD}b\u{FFFD}c\u{FFFD}[31md\u{FFFD}e\u{FFFD}f\u{FFFD}g\th"
    );
    let c = [col("名前 <x>", Kind::Text), col("n", Kind::Number)];
    let rows = [vec![Some("陳大文 🐘 & co"), None], vec![Some(""), Some("1")]];
    assert_eq!(
        html(&c, &rows),
        "<table>\n  <thead>\n    <tr><th>名前 &lt;x&gt;</th><th>n</th></tr>\n  </thead>\n  <tbody>\n    \
         <tr><td>陳大文 🐘 &amp; co</td><td>NULL</td></tr>\n    <tr><td></td><td>1</td></tr>\n  </tbody>\n</table>"
    );
}

#[test]
fn xml_escapes_everything_and_tells_null_from_empty() {
    assert_eq!(xml_text("<&>\"'"), "&lt;&amp;&gt;&quot;&apos;");
    assert_eq!(xml_text("a\tb\nc\rd"), "a\tb\nc&#13;d", "tab and newline kept, CR as a reference");
    assert_eq!(xml_text("bell\u{7}nul\u{0}"), "bell\u{FFFD}nul\u{FFFD}", "no XML 1.0 form: U+FFFD");
    let c = [col("a\"b", Kind::Text), col("note", Kind::Text)];
    let rows = [vec![Some("中文 🐘"), None], vec![Some("x<y"), Some("")]];
    assert_eq!(
        xml(&c, &rows),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rows>\n  <row>\n    <column name=\"a&quot;b\">中文 🐘</column>\n    \
         <column name=\"note\" null=\"true\"/>\n  </row>\n  <row>\n    <column name=\"a&quot;b\">x&lt;y</column>\n    \
         <column name=\"note\"></column>\n  </row>\n</rows>"
    );
    assert_eq!(xml(&c, &[]), "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rows>\n</rows>");
}

#[test]
fn an_in_list_quotes_literals_skips_null_and_repeats_and_takes_one_column() {
    let text = [col("t", Kind::Text)];
    let rows = [vec![Some("O'Reilly")], vec![None], vec![Some("中文 🐘")], vec![Some("O'Reilly")], vec![Some("")]];
    assert_eq!(sql_in(PG, &text, &rows), Ok("('O''Reilly', '中文 🐘', '')".to_string()));
    let num = [col("n", Kind::Number)];
    assert_eq!(
        sql_in(PG, &num, &[vec![Some("1")], vec![Some("-2.5")], vec![Some("NaN")]]),
        Ok("(1, -2.5, 'NaN')".into())
    );
    assert_eq!(sql_in(PG, &[col("a", Kind::Text), col("b", Kind::Text)], &[]), Err(NotInList::SeveralColumns));
    assert_eq!(sql_in(PG, &text, &[vec![None]]), Err(NotInList::NoValues));
    assert_eq!(sql_in(PG, &text, &[]), Err(NotInList::NoValues));
}

#[test]
fn updates_set_the_other_columns_where_the_key_matches() {
    let c = [col("id", Kind::Number), col("name", Kind::Text), col("note", Kind::Text), col("k", Kind::Text)];
    let target = UpdateTarget {
        schema: "my s",
        name: "t\"x",
        set: vec![(1, "name"), (2, "note")],
        keys: vec![(0, "id"), (3, "k")],
    };
    let rows = [vec![Some("5"), Some("陳 'q'"), None, Some("a\nb")], vec![Some("6"), Some(""), Some("x"), Some("z")]];
    assert_eq!(
        sql_update(PG, &target, &c, &rows),
        "UPDATE \"my s\".\"t\"\"x\" SET \"name\" = '陳 ''q''', \"note\" = NULL WHERE \"id\" = 5 AND \"k\" = 'a\nb';\n\
         UPDATE \"my s\".\"t\"\"x\" SET \"name\" = '', \"note\" = 'x' WHERE \"id\" = 6 AND \"k\" = 'z';"
    );
    assert!(!sql_update(PG, &target, &c, &rows).contains("IS NULL"));
}

mod golden;

/// Bytes and bits are written as text is, in every format.
#[test]
fn bytes_and_bits_are_written_as_text() {
    let rows = [vec![Some("\\x0102ff")], vec![Some("0101")], vec![None], vec![Some("it's")]];
    let whole = |k: Kind| {
        let c = [col("b", k)];
        let target = Target::Table { schema: "s", name: "t", columns: vec!["b"], overriding: false };
        let update = UpdateTarget { schema: "s", name: "t", set: vec![(0, "b")], keys: vec![(0, "b")] };
        let formats = [
            Format::Tsv { header: true },
            Format::Csv { header: true },
            Format::Json,
            Format::JsonPretty,
            Format::Markdown,
            Format::Sql(PG, target),
            Format::List,
            Format::Html,
            Format::Xml,
            Format::Update(PG, update),
        ];
        let mut out: Vec<String> = formats
            .into_iter()
            .map(|f| {
                let mut w = Writer::new(f, &c);
                w.rows(&rows);
                w.finish()
            })
            .collect();
        out.push(format!("{:?}", sql_in(PG, &c, &rows)));
        out.extend(rows.iter().map(|r| sql_value(PG, k, r[0])));
        out
    };
    assert_eq!(whole(Kind::Bytes), whole(Kind::Text));
    assert_eq!(whole(Kind::Bit), whole(Kind::Text));
    assert_eq!(Kind::from(ValueKind::Bytes), Kind::Bytes);
    assert_eq!(Kind::from(ValueKind::Bit), Kind::Bit);
}

const MY: Dialect = Dialect::MySql(crate::sql::dialect::MySqlMode {
    ansi_quotes: false,
    no_backslash_escapes: false,
    dollar_quotes: false,
});

/// MySQL's bytes (`0x…`) and bits (`b'…'`) are written as hexadecimal and bit literals;
/// strings read the same in every sql mode: as they are, or as their UTF-8 bytes when they hold
/// a backslash or a NUL. PostgreSQL's stay string literals.
#[test]
fn mysql_values_are_mysql_literals() {
    let plain = Dialect::MySql(crate::sql::dialect::MySqlMode { no_backslash_escapes: true, ..Default::default() });
    for d in [MY, plain] {
        assert_eq!(sql_value(d, Kind::Bytes, Some("0x00FF27")), "X'00FF27'");
        assert_eq!(sql_value(d, Kind::Bytes, Some("0x")), "X''");
        assert_eq!(sql_value(d, Kind::Bit, Some("b'0101'")), "b'0101'");
        assert_eq!(sql_value(d, Kind::Text, Some("it's")), "'it''s'");
        assert_eq!(sql_value(d, Kind::Text, Some("a\\b")), "_utf8mb4 X'615C62'");
        assert_eq!(sql_value(d, Kind::Text, Some("n\0")), "_utf8mb4 X'6E00'");
        assert_eq!(sql_value(d, Kind::Json, Some(r#"{"a": "b\\c"}"#)), "_utf8mb4 X'7B2261223A2022625C5C63227D'");
        assert_eq!(sql_value(d, Kind::Text, Some("é😀")), "'é😀'");
    }
    assert_eq!(sql_value(MY, Kind::Number, Some("18446744073709551615")), "18446744073709551615");
    assert_eq!(sql_value(PG, Kind::Bytes, Some("\\x00ff")), r"'\x00ff'");
    assert_eq!(sql_value(PG, Kind::Text, Some("a\\b")), r"'a\b'");
    let insert = sql_insert(
        MY,
        &Target::Table { schema: "shop", name: "t", columns: vec!["b", "n"], overriding: true },
        &[col("b", Kind::Bytes), col("n", Kind::Number)],
        &[vec![Some("0x41"), None]],
    );
    assert_eq!(insert, "INSERT INTO `shop`.`t` (`b`, `n`) VALUES (X'41', NULL);", "no OVERRIDING on MySQL");
}

fn sqlc<'a>(name: &'a str, type_name: &'a str, kind: crate::driver::ValueKind) -> SqlColumn<'a> {
    SqlColumn { name, type_name, kind, auto_increment: false }
}

fn session(zone: &str, charset: &str) -> crate::driver::ReadIn {
    crate::driver::ReadIn::Values(crate::driver::SessionValues { time_zone: zone.into(), charset: charset.into() })
}

/// MySQL's `FLOAT` is sent rounded (`16777215` shows as `16777200`): its column is refused, by
/// name; `DOUBLE` is sent whole.
#[test]
fn a_mysql_float_is_not_copied_as_sql() {
    use crate::driver::ValueKind as V;
    let utf8 = session("+00:00", "utf8mb4");
    let cols = [sqlc("id", "int", V::Integer), sqlc("ratio", "float", V::Float)];
    assert_eq!(sql_cells(MY, &cols, &utf8, None), Err(Refused { column: "ratio".into(), why: NotWhole::Float }));
    let unsigned = [sqlc("r", "float unsigned", V::Float)];
    assert!(sql_cells(MY, &unsigned, &utf8, None).is_err());
    let double = [sqlc("d", "double", V::Float)];
    assert_eq!(sql_cells(MY, &double, &utf8, None).unwrap().cell(0, "1.2345678").unwrap(), "1.2345678");
    assert!(
        sql_cells(PG, &cols, &crate::driver::ReadIn::Unknown, None).is_ok(),
        "PostgreSQL's float4 is text the driver writes whole"
    );
}

/// A `TIMESTAMP` is copied with the offset of the session's zone when that is fixed, and is
/// refused, naming the zone, when it is not (a time in the hour a clock goes back is two
/// instants).
#[test]
fn a_mysql_timestamp_is_copied_with_its_offset_or_refused() {
    use crate::driver::ValueKind as V;
    let ts = [sqlc("at", "timestamp", V::TimestampTz)];
    let cells = |zone: &str, server: Option<&str>| sql_cells(MY, &ts, &session(zone, "utf8mb4"), server);
    let utc = cells("+00:00", None).unwrap();
    assert_eq!(utc.cell(0, "2024-10-27 00:30:00").unwrap(), "2024-10-27 00:30:00+00:00");
    assert_eq!(utc.cell(0, "2024-10-27 00:30:00.500").unwrap(), "2024-10-27 00:30:00.500+00:00");
    assert_eq!(utc.cell(0, "0000-00-00 00:00:00").unwrap(), "0000-00-00 00:00:00", "no instant, no offset");
    assert_eq!(cells("-03:30", None).unwrap().cell(0, "2024-01-01 00:00:00").unwrap(), "2024-01-01 00:00:00-03:30");
    assert_eq!(cells("+5:30", None).unwrap().cell(0, "2024-01-01 00:00:00").unwrap(), "2024-01-01 00:00:00+05:30");
    assert!(cells("SYSTEM", Some("UTC")).is_ok());
    for (zone, server) in [("Europe/Berlin", None), ("SYSTEM", Some("CET")), ("SYSTEM", None)] {
        assert_eq!(
            cells(zone, server),
            Err(Refused { column: "at".into(), why: NotWhole::TimeZone(zone.into()) }),
            "{zone} {server:?}"
        );
    }
    let unknown = sql_cells(MY, &ts, &crate::driver::ReadIn::Unknown, None);
    assert_eq!(unknown, Err(Refused { column: "at".into(), why: NotWhole::SessionUnknown }));
    // A DATETIME has no zone.
    let dt = [sqlc("dt", "datetime", V::Timestamp)];
    assert_eq!(
        sql_cells(MY, &dt, &crate::driver::ReadIn::Unknown, None).unwrap().cell(0, "2024-10-27 02:30:00").unwrap(),
        "2024-10-27 02:30:00"
    );
}

/// Text read in a character set that is not UTF-8 is not the text: refused. With UTF-8, a text
/// that looks like bytes (`0xE9`) is text. Bytes and bits must be in the driver's form; an
/// `AUTO_INCREMENT` 0 is refused.
#[test]
fn only_whole_values_are_written_as_sql() {
    use crate::driver::ValueKind as V;
    let cols = [sqlc("t", "varchar", V::Text), sqlc("b", "varbinary", V::Bytes), sqlc("n", "int", V::Integer)];
    let latin1 = sql_cells(MY, &cols, &session("+00:00", "latin1"), None).unwrap();
    assert_eq!(latin1.cell(0, "abc"), Err(Refused { column: "t".into(), why: NotWhole::Charset("latin1".into()) }));
    assert_eq!(latin1.cell(2, "7").unwrap(), "7", "numbers are numbers in any character set");
    let utf8 = sql_cells(MY, &cols, &session("+00:00", "utf8mb4"), None).unwrap();
    assert_eq!(utf8.cell(0, "0xE9").unwrap(), "0xE9");
    assert_eq!(utf8.cell(1, "0x00FF").unwrap(), "0x00FF");
    assert_eq!(utf8.cell(1, "0x0"), Err(Refused { column: "b".into(), why: NotWhole::NotShownWhole }));
    let auto = [SqlColumn { auto_increment: true, ..sqlc("id", "int", V::Integer) }];
    let auto = sql_cells(MY, &auto, &session("+00:00", "utf8mb4"), None).unwrap();
    assert_eq!(auto.cell(0, "0"), Err(Refused { column: "id".into(), why: NotWhole::AutoIncrementZero }));
    assert_eq!(auto.cell(0, "7").unwrap(), "7");
    let pg = sql_cells(PG, &cols, &crate::driver::ReadIn::Unknown, None).unwrap();
    assert_eq!(pg.cell(1, "anything").unwrap(), "anything");
}

#[test]
fn fixed_offsets_are_read_from_zone_names() {
    assert_eq!(fixed_offset("UTC", None).as_deref(), Some("+00:00"));
    assert_eq!(fixed_offset("SYSTEM", Some("GMT")).as_deref(), Some("+00:00"));
    assert_eq!(fixed_offset("+09:00", None).as_deref(), Some("+09:00"));
    // `Z` is no zone MySQL reports.
    for zone in ["Europe/Berlin", "+9", "+09:0", "KST", "+09:00:00", "", "Z"] {
        assert_eq!(fixed_offset(zone, None), None, "{zone}");
    }
}

/// A result is judged by what its pages were read in: a `TIMESTAMP` or a text read before the
/// session said, or across a change of its time zone or character set, is refused; numbers are
/// not.
#[test]
fn values_read_in_an_unknown_or_changed_session_are_refused() {
    use crate::driver::{ReadIn, SessionValues, ValueKind as V};
    let cols = [sqlc("at", "timestamp", V::TimestampTz)];
    let text = [sqlc("n", "int", V::Integer), sqlc("t", "varchar", V::Text)];
    for (read_in, why) in [(ReadIn::Unknown, NotWhole::SessionUnknown), (ReadIn::Changed, NotWhole::SessionChanged)] {
        assert_eq!(sql_cells(MY, &cols, &read_in, None), Err(Refused { column: "at".into(), why: why.clone() }));
        let cells = sql_cells(MY, &text, &read_in, None).unwrap();
        assert_eq!(cells.cell(0, "7").unwrap(), "7");
        assert_eq!(cells.cell(1, "x"), Err(Refused { column: "t".into(), why }));
    }
    let utc = SessionValues { time_zone: "+00:00".into(), charset: "utf8mb4".into() };
    let mut read = ReadIn::first(Some(&utc));
    read.page(Some(&utc));
    assert_eq!(read, ReadIn::Values(utc.clone()));
    read.page(Some(&SessionValues { time_zone: "Europe/Berlin".into(), ..utc.clone() }));
    assert_eq!(read, ReadIn::Changed);
    assert_eq!(ReadIn::first(None), ReadIn::Unknown);
}

/// Only `utf8mb4` holds every character: text read in `utf8mb3` (an emoji arrives as `?`) is
/// refused like any other character set.
#[test]
fn text_read_in_utf8mb3_is_refused() {
    use crate::driver::ValueKind as V;
    let cols = [sqlc("t", "varchar", V::Text)];
    let cells = sql_cells(MY, &cols, &session("+00:00", "utf8mb3"), None).unwrap();
    assert_eq!(cells.cell(0, "a?"), Err(Refused { column: "t".into(), why: NotWhole::Charset("utf8mb3".into()) }));
}
