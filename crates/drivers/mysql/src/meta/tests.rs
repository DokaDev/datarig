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
