use super::*;
use crate::sql::dialect::Language;
use crate::sql::dialect::MySqlMode;
use crate::sql::risk::{Class, Classifier, Danger};

const MY: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: false });

fn users() -> TableName {
    TableName { database: None, schema: "shop".into(), name: "users".into() }
}

fn target(d: Dialect) -> Target {
    let mut name = ColumnFacts::new("name", ColumnKind::Text);
    name.collation = Some("utf8mb4_0900_bin".into());
    let mut email = ColumnFacts::new("email", ColumnKind::Text);
    email.collation = Some("utf8mb4_0900_ai_ci".into());
    let columns = vec![
        ColumnFacts::new("id", ColumnKind::Number),
        name,
        email,
        ColumnFacts::new("Mixed Case", ColumnKind::Text),
        ColumnFacts::new("price", ColumnKind::Number),
        ColumnFacts::new("data", ColumnKind::Bytes),
        ColumnFacts::new("born", ColumnKind::Other),
    ];
    Target { primary_key: vec!["id".into()], ..Target::new(d, columns) }
}

fn query(filter: Option<&str>, order: Vec<OrderKey>) -> Query {
    Query { filter: filter.map(|t| Filter { text: t.to_string(), recognised: None }), order, ..Query::table(users()) }
}

fn check(d: Dialect, text: &str, read_only: bool) -> Result<FilterVerdict, BarRefusal> {
    check_filter(text, &users(), &target(d), &Classifier::new(Language::Sql(d)), read_only)
}

fn render_pg(c: &Cond) -> String {
    render(c, &target(Dialect::Postgres)).unwrap()
}

fn render_my(c: &Cond, utf8mb4: bool) -> String {
    render(c, &Target { utf8mb4, ..target(MY) }).unwrap()
}

fn cmp(col: &str, op: CmpOp, value: Value) -> Cond {
    Cond::Cmp { col: col.into(), op, value }
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn num(s: &str) -> Value {
    Value::Number(s.into())
}

// compile

#[test]
fn compile_quotes_the_table_and_marks_the_filter() {
    let t = target(Dialect::Postgres);
    assert_eq!(compile(&query(None, vec![]), &t).sql, r#"SELECT * FROM "shop"."users""#);
    let c = compile(&query(Some("status = 'done'"), vec![]), &t);
    assert_eq!(c.sql, r#"SELECT * FROM "shop"."users" WHERE (status = 'done')"#);
    assert_eq!(&c.sql[c.filter_span.clone().unwrap()], "status = 'done'");
    assert!(!c.total);
    let my = compile(&query(Some(" a = 1 "), vec![]), &target(MY));
    assert_eq!(my.sql, "SELECT * FROM `shop`.`users` WHERE ( a = 1 )");
    assert_eq!(&my.sql[my.filter_span.unwrap()], " a = 1 ");
    // Blank filter text filters nothing.
    assert_eq!(compile(&query(Some("  "), vec![]), &t).filter_span, None);
}

#[test]
fn compile_appends_the_primary_key_as_tie_breaker() {
    let t = target(Dialect::Postgres);
    let c = compile(&query(None, vec![OrderKey::user("name", Dir::Desc)]), &t);
    assert_eq!(c.sql, r#"SELECT * FROM "shop"."users" ORDER BY "name" DESC, "id" ASC"#);
    assert_eq!(c.order[1].origin, KeyOrigin::TieBreak);
    assert!(c.total);
    // A key already sorted on is not appended again; old tie-breakers are worked out anew.
    let stale = OrderKey { col: "price".into(), dir: Dir::Asc, nulls: None, origin: KeyOrigin::TieBreak };
    let c = compile(&query(None, vec![OrderKey::user("id", Dir::Desc), stale]), &t);
    assert_eq!(c.sql, r#"SELECT * FROM "shop"."users" ORDER BY "id" DESC"#);
    assert!(c.total);
    // No order: no tie-breaker, not total.
    let c = compile(&query(None, vec![]), &t);
    assert!(c.order.is_empty() && !c.total);
}

#[test]
fn compile_total_order_needs_a_key() {
    let mut t = Target { primary_key: vec![], ..target(Dialect::Postgres) };
    let q = query(None, vec![OrderKey::user("name", Dir::Asc)]);
    assert!(!compile(&q, &t).total);
    t.unique_keys = vec![vec!["name".into()]];
    assert!(compile(&q, &t).total);
    let t = Target { tie_break: false, ..target(Dialect::Postgres) };
    let c = compile(&q, &t);
    assert_eq!(c.sql, r#"SELECT * FROM "shop"."users" ORDER BY "name" ASC"#);
    assert!(!c.total);
}

#[test]
fn compile_nulls_order_per_dialect() {
    let key = |n| OrderKey { nulls: Some(n), ..OrderKey::user("name", Dir::Desc) };
    let pg = Target { tie_break: false, ..target(Dialect::Postgres) };
    let my = Target { tie_break: false, ..target(MY) };
    assert!(compile(&query(None, vec![key(Nulls::Last)]), &pg).sql.ends_with(r#"ORDER BY "name" DESC NULLS LAST"#));
    assert!(compile(&query(None, vec![key(Nulls::First)]), &pg).sql.ends_with(r#"ORDER BY "name" DESC NULLS FIRST"#));
    assert!(
        compile(&query(None, vec![key(Nulls::Last)]), &my).sql.ends_with("ORDER BY `name` IS NULL ASC, `name` DESC")
    );
    assert!(
        compile(&query(None, vec![key(Nulls::First)]), &my).sql.ends_with("ORDER BY `name` IS NULL DESC, `name` DESC")
    );
}

#[test]
fn only_the_capped_compile_writes_limit_and_offset() {
    let pg = target(Dialect::Postgres);
    let my = target(MY);
    let q = Query { limit: Some(100), ..query(Some("id > 1"), vec![]) };
    assert!(!compile(&q, &pg).sql.contains("LIMIT"));
    assert!(!compile(&q, &my).sql.contains("LIMIT"));
    assert!(compile_capped(&q, &pg).sql.ends_with("WHERE (id > 1) LIMIT 100"));
    assert!(compile_capped(&q, &my).sql.ends_with("WHERE (id > 1) LIMIT 100"));
    let q = Query { limit: Some(5), offset: Some(10), ..query(None, vec![OrderKey::user("id", Dir::Asc)]) };
    assert!(compile_capped(&q, &pg).sql.ends_with(r#"ORDER BY "id" ASC LIMIT 5 OFFSET 10"#));
    assert!(!compile(&q, &pg).sql.contains("OFFSET"));
    let q = Query { offset: Some(10), ..query(None, vec![]) };
    assert!(compile_capped(&q, &pg).sql.ends_with(r#""users" OFFSET 10"#));
    assert!(compile_capped(&q, &my).sql.ends_with("`users` LIMIT 18446744073709551615 OFFSET 10"));
}

#[test]
fn compiled_statements_are_plain_reads() {
    for d in [Dialect::Postgres, MY] {
        let t = target(d);
        let q = Query {
            limit: Some(10),
            offset: Some(2),
            ..query(
                Some("id > 1 AND name LIKE 'a%'"),
                vec![OrderKey { nulls: Some(Nulls::First), ..OrderKey::user("name", Dir::Desc) }],
            )
        };
        for sql in [compile(&q, &t).sql, compile_capped(&q, &t).sql] {
            let risk = Classifier::classify_once(Language::Sql(d), &sql);
            assert_eq!((risk.class, risk.danger), (Class::Read, None), "{sql}");
        }
    }
}

#[test]
fn query_round_trips_through_serde() {
    let q = Query {
        filter: Some(Filter { text: "id = 1".into(), recognised: Some(cmp("id", CmpOp::Eq, num("1"))) }),
        limit: Some(3),
        ..query(None, vec![OrderKey::user("id", Dir::Desc)])
    };
    let s = serde_json::to_string(&q).unwrap();
    assert_eq!(serde_json::from_str::<Query>(&s).unwrap(), q);
}

// render

#[test]
fn render_literals_follow_the_column() {
    assert_eq!(render_pg(&cmp("id", CmpOp::Eq, num("42"))), "id = 42");
    assert_eq!(render_pg(&cmp("id", CmpOp::Ge, num("-1.5e3"))), "id >= -1.5e3");
    assert_eq!(render_pg(&cmp("price", CmpOp::Eq, num("NaN"))), "price = 'NaN'");
    assert_eq!(render_pg(&cmp("name", CmpOp::Eq, num("42"))), "name = '42'");
    assert_eq!(render_pg(&cmp("name", CmpOp::Ne, text("it's"))), "name <> 'it''s'");
    assert_eq!(render_pg(&cmp("name", CmpOp::Eq, text(r"C:\x"))), r"name = E'C:\\x'");
    assert_eq!(render_pg(&cmp("Mixed Case", CmpOp::Lt, text("b"))), r#""Mixed Case" < 'b'"#);
    assert_eq!(render_pg(&cmp("born", CmpOp::Gt, text("2026-01-01"))), "born > '2026-01-01'");
    assert_eq!(render_pg(&cmp("data", CmpOp::Eq, Value::Bytes(vec![1, 0xab]))), r"data = E'\\x01ab'::bytea");
    assert_eq!(render_my(&cmp("data", CmpOp::Eq, Value::Bytes(vec![1, 0xab])), true), "data = X'01AB'");
    assert_eq!(render_pg(&cmp("id", CmpOp::Eq, Value::Bool(true))), "id = TRUE");
    // MySQL text: plain only with a known utf8mb4 session and no backslash.
    assert_eq!(render_my(&cmp("name", CmpOp::Eq, text("it's")), true), "name = 'it''s'");
    assert_eq!(render_my(&cmp("name", CmpOp::Eq, text("ab")), false), "name = _utf8mb4 X'6162'");
    assert_eq!(render_my(&cmp("name", CmpOp::Eq, text(r"a\b")), true), "name = _utf8mb4 X'615C62'");
    assert_eq!(render_my(&cmp("Mixed Case", CmpOp::Eq, num("1")), true), "`Mixed Case` = '1'");
}

#[test]
fn render_null_safe_forms() {
    let nd = |negated| Cond::NotDistinct { col: "name".into(), value: Value::Null, negated };
    assert_eq!(render_pg(&nd(false)), "name IS NOT DISTINCT FROM NULL");
    assert_eq!(render_pg(&nd(true)), "name IS DISTINCT FROM NULL");
    assert_eq!(render_my(&nd(false), true), "name <=> NULL");
    assert_eq!(render_my(&nd(true), true), "NOT (name <=> NULL)");
    assert_eq!(render_pg(&Cond::IsNull { col: "name".into(), negated: true }), "name IS NOT NULL");
    let t = target(Dialect::Postgres);
    assert_eq!(render(&cmp("name", CmpOp::Eq, Value::Null), &t), Err(RenderError::NullCompare("name".into())));
}

#[test]
fn render_in_takes_null_out_of_the_list() {
    let r#in = |values: Vec<Value>, negated| Cond::In { col: "id".into(), values, negated };
    assert_eq!(render_pg(&r#in(vec![num("1"), num("2")], false)), "id IN (1, 2)");
    assert_eq!(render_pg(&r#in(vec![num("1"), Value::Null], false)), "(id IN (1) OR id IS NULL)");
    assert_eq!(render_pg(&r#in(vec![num("1"), Value::Null], true)), "(id NOT IN (1) AND id IS NOT NULL)");
    assert_eq!(render_pg(&r#in(vec![num("1")], true)), "id NOT IN (1)");
    assert_eq!(render_pg(&r#in(vec![Value::Null], true)), "id IS NOT NULL");
    let t = target(Dialect::Postgres);
    assert_eq!(render(&r#in(vec![], false), &t), Err(RenderError::EmptyList("id".into())));
}

#[test]
fn render_like_escapes_the_value() {
    assert_eq!(like_starts_with("50%_off!"), "50!%!_off!!%");
    assert_eq!(like_contains("a"), "%a%");
    let like = |col: &str, case| Cond::Like { col: col.into(), pattern: like_contains("x_"), negated: false, case };
    assert_eq!(render_pg(&like("name", CaseMode::Server)), "name LIKE '%x!_%' ESCAPE '!'");
    assert_eq!(render_pg(&like("name", CaseMode::Insensitive)), "name ILIKE '%x!_%' ESCAPE '!'");
    assert_eq!(render_my(&like("email", CaseMode::Insensitive), true), "email LIKE '%x!_%' ESCAPE '!'");
    assert_eq!(render_my(&like("name", CaseMode::Insensitive), true), "LOWER(name) LIKE LOWER('%x!_%') ESCAPE '!'");
    let mut t = target(Dialect::Postgres);
    t.columns[1].deterministic = false;
    assert_eq!(
        render(&like("name", CaseMode::Insensitive), &t),
        Err(RenderError::CaseInsensitiveRefused("name".into()))
    );
    assert!(render(&like("name", CaseMode::Server), &t).is_ok());
}

#[test]
fn render_groups_what_needs_it() {
    let a = cmp("id", CmpOp::Gt, num("1"));
    let b = Cond::IsNull { col: "name".into(), negated: false };
    let raw = Cond::Raw("lower(name) = 'x' OR id = 2".into());
    assert_eq!(render_pg(&Cond::All(vec![a.clone(), b.clone()])), "id > 1 AND name IS NULL");
    assert_eq!(render_pg(&Cond::Any(vec![a.clone(), b.clone()])), "(id > 1 OR name IS NULL)");
    assert_eq!(
        render_pg(&Cond::All(vec![Cond::Any(vec![a.clone(), b.clone()]), raw.clone()])),
        "(id > 1 OR name IS NULL) AND (lower(name) = 'x' OR id = 2)"
    );
    assert_eq!(render_pg(&Cond::Not(Box::new(a.clone()))), "NOT (id > 1)");
    assert_eq!(render_pg(&Cond::Not(Box::new(Cond::Any(vec![a.clone(), b])))), "NOT (id > 1 OR name IS NULL)");
    assert_eq!(render_pg(&raw), "(lower(name) = 'x' OR id = 2)");
    let between = Cond::Between { col: "id".into(), lo: num("1"), hi: num("9"), negated: true };
    assert_eq!(render_pg(&between), "id NOT BETWEEN 1 AND 9");
}

// recognise

/// Conditions that read back as themselves on both dialects.
fn representative() -> Vec<Cond> {
    let like = |col: &str, pattern: String, negated, case| Cond::Like { col: col.into(), pattern, negated, case };
    vec![
        cmp("id", CmpOp::Eq, num("1")),
        cmp("id", CmpOp::Ne, num("-2")),
        cmp("price", CmpOp::Le, num("3.25")),
        cmp("name", CmpOp::Gt, text("it's")),
        cmp("Mixed Case", CmpOp::Lt, text("ü ß")),
        cmp("born", CmpOp::Ge, text("2026-11-10 12:00:00+00")),
        cmp("id", CmpOp::Eq, Value::Bool(false)),
        cmp("data", CmpOp::Eq, Value::Bytes(vec![0, 1, 0xfe])),
        Cond::NotDistinct { col: "name".into(), value: text("a"), negated: false },
        Cond::NotDistinct { col: "name".into(), value: Value::Null, negated: true },
        Cond::IsNull { col: "name".into(), negated: false },
        Cond::IsNull { col: "id".into(), negated: true },
        Cond::In { col: "id".into(), values: vec![num("1"), num("2"), num("3")], negated: false },
        Cond::In { col: "name".into(), values: vec![text("a"), text("b,c")], negated: true },
        like("name", like_starts_with("50%_x!"), false, CaseMode::Server),
        like("name", like_contains("a'b"), true, CaseMode::Server),
        like("name", like_contains("q"), false, CaseMode::Insensitive),
        Cond::Between { col: "price".into(), lo: num("1"), hi: num("10"), negated: false },
        Cond::Between { col: "born".into(), lo: text("2026-01-01"), hi: text("2026-12-31"), negated: true },
        Cond::Raw("lower(name) = 'x'".into()),
        Cond::Not(Box::new(cmp("id", CmpOp::Eq, num("1")))),
        Cond::All(vec![cmp("id", CmpOp::Gt, num("1")), Cond::IsNull { col: "name".into(), negated: true }]),
        Cond::Any(vec![cmp("id", CmpOp::Lt, num("1")), cmp("id", CmpOp::Gt, num("9"))]),
        Cond::All(vec![
            Cond::Any(vec![cmp("id", CmpOp::Eq, num("1")), Cond::Raw("price * 2 > 10".into())]),
            Cond::Between { col: "id".into(), lo: num("1"), hi: num("5"), negated: false },
            Cond::All(vec![cmp("name", CmpOp::Eq, text("a")), cmp("name", CmpOp::Ne, text("b"))]),
            Cond::Not(Box::new(Cond::Any(vec![cmp("id", CmpOp::Eq, num("2")), cmp("id", CmpOp::Eq, num("3"))]))),
        ]),
    ]
}

#[test]
fn recognise_reads_back_what_render_writes() {
    for (d, utf8mb4) in [(Dialect::Postgres, false), (MY, true), (MY, false)] {
        let t = Target { utf8mb4, ..target(d) };
        for c in representative() {
            let text = render(&c, &t).unwrap();
            assert_eq!(recognise(&text, &t), Some(c.clone()), "{d:?} {text}");
        }
    }
}

#[test]
fn recognise_keeps_what_it_does_not_model() {
    let (tp, tm) = (target(Dialect::Postgres), target(MY));
    let pg = |s: &str| recognise(s, &tp);
    assert_eq!(
        pg("lower(name) = 'x' AND id > 3"),
        Some(Cond::All(vec![Cond::Raw("lower(name) = 'x'".into()), cmp("id", CmpOp::Gt, num("3"))]))
    );
    assert_eq!(pg("other = 1"), Some(Cond::Raw("other = 1".into())));
    // A bare name folds to lower case: `Name` is `name`, `"Name"` is no column.
    assert_eq!(pg("Name = 'a'"), Some(cmp("name", CmpOp::Eq, text("a"))));
    assert_eq!(pg(r#""Name" = 'a'"#), Some(Cond::Raw(r#""Name" = 'a'"#.into())));
    assert_eq!(pg("id=-1"), Some(cmp("id", CmpOp::Eq, num("-1"))));
    assert_eq!(pg("id != 1"), Some(cmp("id", CmpOp::Ne, num("1"))));
    assert_eq!(
        pg(r"name LIKE E'a\\%b!%'"),
        Some(Cond::Like { col: "name".into(), pattern: "a!%b!!%".into(), negated: false, case: CaseMode::Server })
    );
    assert_eq!(pg("NOT EXISTS (SELECT 1)"), Some(Cond::Not(Box::new(Cond::Raw("EXISTS (SELECT 1)".into())))));
    assert_eq!(pg("id = NULL"), Some(Cond::Raw("id = NULL".into())));
    // MySQL names compare without case.
    assert_eq!(recognise("ID = 1", &tm), Some(cmp("id", CmpOp::Eq, num("1"))));
    assert_eq!(recognise("id <=> 1", &tp), Some(Cond::Raw("id <=> 1".into())));
    // AND and OR inside a CASE belong to it, not to the filter.
    let case = "CASE WHEN id = 1 OR id = 2 THEN TRUE ELSE FALSE END";
    assert_eq!(pg(case), Some(Cond::Raw(case.into())));
    assert_eq!(
        pg(&format!("{case} AND id > 3")),
        Some(Cond::All(vec![Cond::Raw(case.into()), cmp("id", CmpOp::Gt, num("3"))]))
    );
    // So do the ones inside square brackets.
    let array = "ARRAY[TRUE OR FALSE] = ARRAY[TRUE]";
    assert_eq!(pg(array), Some(Cond::Raw(array.into())));
    // MySQL XOR binds between AND and OR, and `||` may be OR or concatenation: such a level
    // stays whole.
    for s in ["id = 1 XOR id = 2 AND price > 10", "id = 1 || id = 2 AND id = 3"] {
        assert_eq!(recognise(s, &tm), Some(Cond::Raw(s.into())), "{s}");
    }
    // NOT binds tighter than `&&` and `||`: such a level stays whole too.
    for s in ["NOT id = 1 && price > 10", "NOT id = 1 || price > 10"] {
        assert_eq!(recognise(s, &tm), Some(Cond::Raw(s.into())), "{s}");
    }
    // A NULL in a typed IN list follows SQL's three-valued logic, not the model's list.
    for s in ["id IN (1, NULL)", "id NOT IN (1, NULL)"] {
        assert_eq!(pg(s), Some(Cond::Raw(s.into())), "{s}");
    }
    // A bare number against a column that is not a number compares as a number on MySQL; the
    // model would write it back as a string. A NULL bound of a BETWEEN has no model form.
    for s in
        ["name = 1", "name IN (1, 2)", "name BETWEEN 1 AND 2", "id BETWEEN NULL AND 5", "id NOT BETWEEN 1 AND NULL"]
    {
        assert_eq!(recognise(s, &tm), Some(Cond::Raw(s.into())), "{s}");
    }
    assert_eq!(pg("price = 1.5"), Some(cmp("price", CmpOp::Eq, num("1.5"))));
    // A number the renderer would write as a string (hexadecimal, minus zero) stays as typed.
    for s in ["id = 0x1F", "id IN (1, 0x1F)", "id = -0"] {
        assert_eq!(recognise(s, &tm), Some(Cond::Raw(s.into())), "{s}");
    }
    // TRUE, FALSE and NULL are constants even when a column has such a name.
    let mut tf = target(Dialect::Postgres);
    tf.columns.push(ColumnFacts::new("true", ColumnKind::Other));
    for s in ["TRUE = FALSE", "true IS NULL"] {
        assert_eq!(recognise(s, &tf), Some(Cond::Raw(s.into())), "{s}");
    }
    // A plain PostgreSQL string with a backslash depends on standard_conforming_strings.
    for s in [r"name = 'a\tb'", r"data = '\x01'::bytea"] {
        assert_eq!(pg(s), Some(Cond::Raw(s.into())), "{s}");
    }
    assert_eq!(pg(r"name = E'a\\b'"), Some(cmp("name", CmpOp::Eq, text(r"a\b"))));
    // Under NO_BACKSLASH_ESCAPES a LIKE without ESCAPE has no escape character at all.
    let nbe = Target {
        dialect: Dialect::MySql(MySqlMode { no_backslash_escapes: true, ..MySqlMode::default() }),
        ..target(MY)
    };
    let like = r"name LIKE 'a\_%'";
    assert_eq!(recognise(like, &nbe), Some(Cond::Raw(like.into())));
    // `&&` is AND: cutting at OR around it keeps the meaning.
    assert_eq!(
        recognise("id = 1 && id = 2 OR id = 3", &tm),
        Some(Cond::Any(vec![Cond::Raw("id = 1 && id = 2".into()), cmp("id", CmpOp::Eq, num("3"))]))
    );
}

#[test]
fn recognise_fails_on_what_it_cannot_read() {
    let t = target(Dialect::Postgres);
    for s in ["", "  ", "id = 1 AND", "OR id = 1", "(id = 1", "id = 1)", "id = 'x", "id = 1 -- c", "id = 1; x", "()"] {
        assert_eq!(recognise(s, &t), None, "{s}");
    }
}

// check_filter

#[test]
fn plain_filters_are_clear() {
    for d in [Dialect::Postgres, MY] {
        for text in [
            "status = 'done'",
            "id > 3 AND name IS NOT NULL",
            "lower(name) LIKE 'a%' ESCAPE '!'",
            "id IN (SELECT id FROM shop.users WHERE price > 1)",
            "(id = 1) OR (id = 2)",
        ] {
            assert_eq!(check(d, text, true), Ok(FilterVerdict::Clear), "{d:?} {text}");
        }
    }
    assert_eq!(check(Dialect::Postgres, "pg_catalog.upper(name) = 'A'", true), Ok(FilterVerdict::Clear));
    assert_eq!(check(Dialect::Postgres, r"name = 'C:\temp'", true), Ok(FilterVerdict::Clear));
    // MySQL: a reading where the string is left open does not run; the session's is clean.
    assert_eq!(check(MY, r"name = 'it\'s'", true), Ok(FilterVerdict::Clear));
}

fn lexical(d: Dialect, text: &str) -> Option<(LexProblem, String)> {
    match check(d, text, false) {
        Err(BarRefusal::Lexical { problem, at }) => Some((problem, text[at].to_string())),
        _ => None,
    }
}

#[test]
fn the_lexical_check_refuses_with_the_place() {
    let both = [Dialect::Postgres, MY];
    for d in both {
        assert_eq!(lexical(d, "id = 1; DROP TABLE users"), Some((LexProblem::Semicolon, ";".into())));
        assert_eq!(lexical(d, "id = 1 -- x"), Some((LexProblem::Comment, "-- x".into())));
        assert_eq!(lexical(d, "id = 1 /* x */"), Some((LexProblem::Comment, "/* x */".into())));
        assert_eq!(lexical(d, "(id = 1"), Some((LexProblem::Unclosed, "(".into())));
        assert_eq!(lexical(d, "1=1) OR (1=1"), Some((LexProblem::ClosesOutside, ")".into())));
        assert_eq!(lexical(d, "name = 'abc"), Some((LexProblem::Unterminated, "'abc".into())));
        assert_eq!(check(d, "  ", false), Err(BarRefusal::Empty));
    }
    assert_eq!(lexical(MY, "id = 1 # x"), Some((LexProblem::Comment, "# x".into())));
    assert_eq!(lexical(MY, "id = 1 /*! OR 1=1 */"), Some((LexProblem::ExecutableComment, "/*!".into())));
    assert_eq!(lexical(MY, "id = 1 /*M! OR 1=1 */"), Some((LexProblem::ExecutableComment, "/*M! OR 1=1 */".into())));
    assert_eq!(lexical(MY, "id = 1 \\g"), Some((LexProblem::Semicolon, "\\g".into())));
    assert_eq!(lexical(MY, "name = `abc"), Some((LexProblem::Unterminated, "`abc".into())));
    assert_eq!(lexical(Dialect::Postgres, "name = $$abc"), Some((LexProblem::Unterminated, "$$abc".into())));
    // MySQL under NO_BACKSLASH_ESCAPES the string ends at the backslash and a comment follows.
    assert_eq!(lexical(MY, r"name = 'a\' # '"), Some((LexProblem::Comment, "# '".into())));
}

fn shape(d: Dialect, text: &str) -> Option<ShapeProblem> {
    match check(d, text, false) {
        Err(BarRefusal::Shape { problem, .. }) => Some(problem),
        _ => None,
    }
}

#[test]
fn the_shape_check_refuses_locks_and_clauses() {
    let pg = Dialect::Postgres;
    assert_eq!(shape(pg, "id IN (SELECT id FROM shop.users FOR UPDATE)"), Some(ShapeProblem::Locking));
    assert_eq!(shape(pg, "id IN (SELECT id FROM shop.users FOR SHARE SKIP LOCKED)"), Some(ShapeProblem::Locking));
    assert!(check(pg, "id IN (SELECT id INTO t FROM shop.users)", false).is_err());
    assert_eq!(shape(MY, "id IN (SELECT id FROM t FOR UPDATE)"), Some(ShapeProblem::Locking));
    assert_eq!(shape(MY, "id IN (SELECT id FROM t LOCK IN SHARE MODE)"), Some(ShapeProblem::Locking));
    assert_eq!(shape(MY, "id IN (SELECT id INTO @x FROM t)"), Some(ShapeProblem::Into));
    assert_eq!(shape(MY, "id = 1 UNION SELECT 1"), Some(ShapeProblem::SetOperation));
    assert_eq!(shape(MY, "id = 1 LIMIT 1"), Some(ShapeProblem::Clause));
    assert_eq!(shape(MY, "id = 1 ORDER BY id"), Some(ShapeProblem::Clause));
    assert_eq!(shape(MY, "(@x := id) > 1"), Some(ShapeProblem::Assignment));
    // A shape check on MySQL points at the word.
    match check(MY, "id = 1 LIMIT 1", false) {
        Err(BarRefusal::Shape { at: Some(at), .. }) => assert_eq!(at, 7..12),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_classifier_must_read_it_as_a_plain_read() {
    let pg = Dialect::Postgres;
    assert_eq!(
        check(pg, "pg_terminate_backend(1)", false),
        Err(BarRefusal::NotRead { class: Class::Read, danger: Some(Danger::ServerAction) })
    );
    assert!(matches!(
        check(pg, "id IN (WITH d AS (DELETE FROM shop.users RETURNING id) SELECT id FROM d)", false),
        Err(BarRefusal::NotRead { .. })
    ));
    assert!(matches!(check(pg, "id = = 1", false), Err(BarRefusal::NotRead { danger: Some(Danger::Unparsed), .. })));
    let long = format!("id = 0{}", " + 1".repeat(70_000));
    assert!(matches!(check(pg, &long, false), Err(BarRefusal::NotRead { danger: Some(Danger::TooComplex), .. })));
    assert!(matches!(check(MY, "LOAD_FILE('/etc/passwd') IS NULL", false), Err(BarRefusal::NotRead { .. })));
}

#[test]
fn calls_are_confirmed_refused_or_clear() {
    let pg = Dialect::Postgres;
    let confirm = |calls: &[&str]| {
        Ok(FilterVerdict::NeedsConfirm(vec![Confirm::Calls(calls.iter().map(|c| c.to_string()).collect())]))
    };
    assert_eq!(check(pg, "my_fn(id) > 0", false), confirm(&["my_fn"]));
    assert_eq!(check(pg, "public.lower(name) = 'a'", false), confirm(&["public.lower"]));
    assert_eq!(check(pg, "my_fn(id) > 0", true), Err(BarRefusal::UncheckedCalls(vec!["my_fn".into()])));
    assert_eq!(check(pg, "pg_sleep(1) IS NULL", false), Err(BarRefusal::RefusedCalls(vec!["pg_sleep".into()])));
    assert_eq!(
        check(pg, "set_config('a.b', 'c', false) = 'c'", false),
        Err(BarRefusal::RefusedCalls(vec!["set_config".into()]))
    );
    assert_eq!(check(MY, "my_fn(id) > 0", false), confirm(&["MY_FN"]));
    assert_eq!(check(MY, "shop.lower(name) = 'a'", false), confirm(&["LOWER"]));
    assert_eq!(check(MY, "SLEEP(1) = 0", false), Err(BarRefusal::RefusedCalls(vec!["SLEEP".into()])));
    assert!(check(MY, "GET_LOCK('a', 1) = 1", false).is_err());
    assert!(matches!(check(MY, "my_fn(id) > 0", true), Err(BarRefusal::UncheckedCalls(_) | BarRefusal::ReadOnly(_))));
    assert_eq!(
        check(MY, "BENCHMARK(1000, md5('a')) = 0", false),
        Err(BarRefusal::RefusedCalls(vec!["BENCHMARK".into()]))
    );
}

#[test]
fn the_call_lists_are_sorted_disjoint_built_ins() {
    for list in [PG_FILTER_CALLS, PG_REFUSED_CALLS, MYSQL_FILTER_CALLS, MYSQL_REFUSED_CALLS] {
        assert!(list.windows(2).all(|w| w[0] < w[1]), "{list:?}");
    }
    for f in PG_FILTER_CALLS {
        assert!(crate::sql::risk::builtin(f), "{f}");
        assert!(!PG_REFUSED_CALLS.contains(f), "{f}");
    }
    for f in MYSQL_FILTER_CALLS {
        assert!(crate::sql::risk::mysql::builtin(f), "{f}");
        assert!(!MYSQL_REFUSED_CALLS.contains(f), "{f}");
    }
}
