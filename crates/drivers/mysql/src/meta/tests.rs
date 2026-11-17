use super::*;

/// `COLUMN_TYPE` of an `ENUM` as MySQL 8.4 writes it: a quote doubled, a backslash doubled.
#[test]
fn enum_values_from_the_column_type() {
    let v = |xs: &[&str]| Some(xs.iter().map(|x| x.to_string()).collect::<Vec<_>>());
    assert_eq!(enum_values("enum('pending','paid')"), v(&["pending", "paid"]));
    assert_eq!(enum_values("enum('a','it''s','b\\\\c','d\"q','x,y','')"), v(&["a", "it's", "b\\c", "d\"q", "x,y", ""]));
    assert_eq!(enum_values("ENUM('a')"), v(&["a"]));
    for other in ["set('p','q')", "varchar(10)", "enum()", "enum('a',)", "enum('a' 'b')", "enum('a'", "enu", ""] {
        assert_eq!(enum_values(other), None, "{other}");
    }
}

/// Names of every kind a database or a table may have: braces as the statements' own places
/// are written (`{table}`), quotes, backticks, backslashes, blanks, parentheses, comment marks.
const NAMES: [&str; 20] = [
    "{table}",
    "{schema}",
    "{name}",
    "{grantee}",
    "{checks}",
    "{}",
    "{",
    "}",
    "a'b",
    "a''b",
    "a`b",
    "a\\",
    "a\\'b",
    "a b",
    "f(x)",
    "\"q\"",
    "-- x",
    "/* x */",
    "",
    "plain",
];

/// `sql` as `d` lexes it: every token but blanks and string literals (kind and text), and the
/// string literals as the server reads them.
fn shape(sql: &str, d: Dialect) -> (Vec<String>, Vec<String>) {
    let (mut code, mut literals) = (Vec::new(), Vec::new());
    for t in lex_in(sql, d) {
        match t.kind {
            Tok::Whitespace => {}
            Tok::Str => literals.push(d.unquote_literal(t.text(sql)).expect("a literal")),
            kind => code.push(format!("{kind:?} {}", t.text(sql))),
        }
    }
    (code, literals)
}

/// A statement built with a schema and a table name.
type Build = Box<dyn Fn(&str, &str) -> String>;

/// The table structure, the object lookup before a DDL and the order keys of a table put each
/// name in as one string literal that reads back as the name, whatever the name holds, in both
/// backslash modes: the statement around the names is the one built with plain names.
#[test]
fn each_name_is_one_literal_in_the_statements_built_with_it() {
    let servers = [(8, 4, 0, false), (8, 0, 12, false), (10, 11, 0, true)]
        .map(|(a, b, c, mariadb)| crate::session::Server { version: (a, b, c), mariadb });
    for no_backslash_escapes in [false, true] {
        let mode = MySqlMode { no_backslash_escapes, ..MySqlMode::default() };
        let d = Dialect::MySql(mode);
        let mut builders: Vec<(String, Build)> = servers
            .iter()
            .map(|&server| {
                let build = move |schema: &str, table: &str| structure::statement(server, mode, schema, table);
                (format!("structure {server:?}"), Box::new(build) as Build)
            })
            .collect();
        builders.push(("lookup".into(), Box::new(move |schema, name| ddl::find_named_sql(d, schema, name))));
        builders.push(("order keys".into(), Box::new(move |schema, table| order_keys::statement(mode, schema, table))));
        for (what, build) in &builders {
            let (plain_code, plain_literals) = shape(&build("zz_s", "zz_t"), d);
            assert!(plain_literals.iter().any(|l| l == "zz_s") && plain_literals.iter().any(|l| l == "zz_t"));
            for schema in NAMES {
                for table in NAMES {
                    let (code, literals) = shape(&build(schema, table), d);
                    let at = format!("{what}, NO_BACKSLASH_ESCAPES: {no_backslash_escapes}, {schema:?}.{table:?}");
                    assert_eq!(code, plain_code, "{at}");
                    let want: Vec<&str> = plain_literals
                        .iter()
                        .map(|l| match l.as_str() {
                            "zz_s" => schema,
                            "zz_t" => table,
                            other => other,
                        })
                        .collect();
                    assert_eq!(literals, want, "{at}");
                }
            }
        }
    }
}

#[test]
fn fill_reads_each_place_once() {
    assert_eq!(fill("{a}-{b}-{c}", &[("a", "{b}"), ("b", "x")]), "{b}-x-{c}");
    assert_eq!(fill("{{a}}", &[("a", "1")]), "{1}");
    assert_eq!(fill("{a", &[("a", "1")]), "{a");
    assert_eq!(fill("{ab} {a}", &[("a", "1")]), "{ab} 1");
    assert_eq!(fill("", &[("a", "1")]), "");
}

/// MariaDB's system-versioned tables are tables (`SYSTEM VERSIONED`), never views.
#[test]
fn a_system_versioned_table_is_a_table() {
    assert!(super::is_table("BASE TABLE") && super::is_table("SYSTEM VERSIONED"));
    assert!(!super::is_table("VIEW") && !super::is_table("SYSTEM VIEW"));
}

/// A MariaDB sequence (`TABLE_TYPE` `SEQUENCE`) is listed with the tables, as its DDL is read
/// (`SHOW CREATE TABLE`), never with the views.
#[test]
fn a_mariadb_sequence_is_a_table() {
    assert!(super::is_table("SEQUENCE"));
}

/// `EXTRA` as MariaDB writes it for invisible columns (a comma before `INVISIBLE`, read from
/// MariaDB 11.4): the key metadata still sees the generated and `AUTO_INCREMENT` columns, the
/// order keys the invisible ones, as the structure does.
#[test]
fn extra_with_commas_marks_invisible_in_keys_and_order_keys() {
    use datarig_core::driver::keys::Generated;
    let extras = [
        ("id", "auto_increment, INVISIBLE"),
        ("a", ""),
        ("g", "VIRTUAL GENERATED, INVISIBLE"),
        ("s", "STORED GENERATED, INVISIBLE"),
        ("u", "on update current_timestamp(), INVISIBLE"),
    ];
    let s = |v: &str| Some(v.to_string());
    let mut rows = vec![("L".to_string(), String::new(), String::new(), s("0"), None, None, None)];
    for (n, (name, extra)) in (1..).zip(extras) {
        rows.push(("C".into(), "shop".into(), "t".into(), s(&n.to_string()), s(name), s(extra), s("BASE TABLE")));
    }
    let k = super::keys::catalog(rows).unwrap();
    let generated: Vec<(&str, Generated)> =
        k.find(Some("shop"), "t").unwrap().columns.values().map(|c| (c.name.as_str(), c.generated)).collect();
    assert_eq!(
        generated,
        [
            ("id", Generated::AutoIncrement),
            ("a", Generated::No),
            ("g", Generated::Expression),
            ("s", Generated::Expression),
            ("u", Generated::No),
        ]
    );

    let mut rows = vec![("T".to_string(), None, None, None, None, None)];
    for (n, (name, extra)) in (1..).zip(extras) {
        rows.push(("C".into(), s(name), s(&n.to_string()), s("int"), s("YES"), s(extra)));
    }
    let columns = super::order_keys::keys_of(rows).unwrap().columns.unwrap();
    let invisible: Vec<(&str, bool)> = columns.iter().map(|c| (c.name.as_str(), c.invisible)).collect();
    assert_eq!(invisible, [("id", true), ("a", false), ("g", true), ("s", true), ("u", true)]);
}

/// The words of `EXTRA`, MySQL's (spaces) and MariaDB's (commas too).
#[test]
fn extra_words_split_on_spaces_and_commas() {
    fn words(e: &str) -> Vec<&str> {
        super::extra_words(e).collect()
    }
    assert_eq!(
        words("DEFAULT_GENERATED on update CURRENT_TIMESTAMP"),
        ["DEFAULT_GENERATED", "on", "update", "CURRENT_TIMESTAMP"]
    );
    assert_eq!(words("auto_increment, INVISIBLE"), ["auto_increment", "INVISIBLE"]);
    assert_eq!(words("VIRTUAL GENERATED,INVISIBLE"), ["VIRTUAL", "GENERATED", "INVISIBLE"]);
    assert!(words("").is_empty() && words(" , ").is_empty());
}
