use super::*;

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
        sql_insert(&target, &cols(), &rows()),
        "INSERT INTO \"shop\".\"order items\" (\"id\", \"name\", \"note\", \"ok\", \"doc\") VALUES (1, '陳大文 🐘', NULL, true, '{\"a\": [1, 2]}');\n\
         INSERT INTO \"shop\".\"order items\" (\"id\", \"name\", \"note\", \"ok\", \"doc\") VALUES (-2.5, '', 'tab\there\nnew \"line\"', false, NULL);\n\
         INSERT INTO \"shop\".\"order items\" (\"id\", \"name\", \"note\", \"ok\", \"doc\") VALUES ('NaN', 'O''Reilly, Inc.', 'a|b\\c', NULL, 'not json');"
    );
    let aliased = Target::Table { schema: "s", name: "t", columns: vec!["real_name"], overriding: false };
    assert_eq!(
        sql_insert(&aliased, &[col("alias", Kind::Text)], &[vec![Some("x")]]),
        "INSERT INTO \"s\".\"t\" (\"real_name\") VALUES ('x');",
        "the table's column names, not the aliases"
    );
    assert_eq!(
        sql_insert(&Target::Unknown, &[col("n", Kind::Text)], &[vec![Some("\\x0102")]]),
        "INSERT INTO <table> (\"n\") VALUES ('\\x0102');",
        "bytea as its hex input form"
    );
    assert_eq!(sql_insert(&Target::Unknown, &[col("n", Kind::Text)], &[]), "");
    assert_eq!(
        sql_insert(
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
        sql_insert(&identity, &[col("id", Kind::Number)], &[vec![Some("7")]]),
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
    assert_eq!(sql_insert(&Target::Unknown, &c, &r), "INSERT INTO <table> (\"a\") VALUES ('{{1,2},{3,4}}');");
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
        (Format::Sql(target.clone()), sql_insert(&target, &c, &all), sql_insert(&target, &c, &[])),
        (Format::JsonPretty, json_pretty(&c, &all), json_pretty(&c, &[])),
        (Format::List, comma_list(&all), comma_list(&[])),
        (Format::Html, html(&c, &all), html(&c, &[])),
        (Format::Xml, xml(&c, &all), xml(&c, &[])),
        (Format::Update(update.clone()), sql_update(&update, &c, &all), sql_update(&update, &c, &[])),
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
    assert_eq!(sql_in(&text, &rows), Ok("('O''Reilly', '中文 🐘', '')".to_string()));
    let num = [col("n", Kind::Number)];
    assert_eq!(sql_in(&num, &[vec![Some("1")], vec![Some("-2.5")], vec![Some("NaN")]]), Ok("(1, -2.5, 'NaN')".into()));
    assert_eq!(sql_in(&[col("a", Kind::Text), col("b", Kind::Text)], &[]), Err(NotInList::SeveralColumns));
    assert_eq!(sql_in(&text, &[vec![None]]), Err(NotInList::NoValues));
    assert_eq!(sql_in(&text, &[]), Err(NotInList::NoValues));
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
        sql_update(&target, &c, &rows),
        "UPDATE \"my s\".\"t\"\"x\" SET \"name\" = '陳 ''q''', \"note\" = NULL WHERE \"id\" = 5 AND \"k\" = 'a\nb';\n\
         UPDATE \"my s\".\"t\"\"x\" SET \"name\" = '', \"note\" = 'x' WHERE \"id\" = 6 AND \"k\" = 'z';"
    );
    assert!(!sql_update(&target, &c, &rows).contains("IS NULL"));
}
