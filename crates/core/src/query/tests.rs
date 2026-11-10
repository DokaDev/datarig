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
    name.deterministic = Some(true);
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

/// A table without a primary key takes the shortest unique key whose columns are all `NOT
/// NULL` as the tie-breaker: the order is total, and its pages do not repeat or skip rows.
#[test]
fn compile_breaks_ties_with_a_unique_key_without_a_primary_key() {
    let t = Target {
        primary_key: vec![],
        unique_keys: vec![vec!["email".into(), "born".into()], vec!["price".into()], vec!["data".into()]],
        ..target(Dialect::Postgres)
    };
    let c = compile(&query(None, vec![OrderKey::user("name", Dir::Desc)]), &t);
    assert_eq!(c.sql, r#"SELECT * FROM "shop"."users" ORDER BY "name" DESC, "price" ASC"#);
    assert_eq!(c.order[1].origin, KeyOrigin::TieBreak);
    assert!(c.total);
    // A key the user's order holds already is not appended again.
    let c = compile(&query(None, vec![OrderKey::user("price", Dir::Desc)]), &t);
    assert_eq!(c.sql, r#"SELECT * FROM "shop"."users" ORDER BY "price" DESC"#);
    assert!(c.total);
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
    assert_eq!(render_pg(&cmp("data", CmpOp::Eq, Value::Bytes(vec![1, 0xab]))), r"data = E'\\x01ab'::pg_catalog.bytea");
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

/// A single-precision float (PostgreSQL `real`) takes a number as a string, which the server
/// reads as a `real`: a bare decimal is a `numeric`, compared after both widen to `double
/// precision`, where `0.1` is not the `real` 0.1. MySQL's `FLOAT` has no literal of its type: a
/// number is bare, and it is compared with an order only (never `=`, `<>`, `<=>` or `IN`).
#[test]
fn render_writes_a_real_number_that_compares_exactly() {
    let real = |d| Target::new(d, vec![ColumnFacts::new("r", ColumnKind::Real)]);
    let (pg, my) = (real(Dialect::Postgres), real(MY));
    let r#in = |values: Vec<Value>| Cond::In { col: "r".into(), values, negated: false };
    let nd = |value| Cond::NotDistinct { col: "r".into(), value, negated: true };
    let between = Cond::Between { col: "r".into(), lo: num("0.1"), hi: num("1.5"), negated: false };
    let ok = |c: &Cond, t: &Target| render(c, t).unwrap();
    assert_eq!(ok(&cmp("r", CmpOp::Eq, num("0.1")), &pg), "r = '0.1'");
    assert_eq!(ok(&cmp("r", CmpOp::Ne, num("3.4028235e+38")), &pg), "r <> '3.4028235e+38'");
    assert_eq!(ok(&nd(num("0.1")), &pg), "r IS DISTINCT FROM '0.1'");
    assert_eq!(ok(&r#in(vec![num("0.1"), num("1.5"), Value::Null]), &pg), "(r IN ('0.1', '1.5') OR r IS NULL)");
    assert_eq!(ok(&cmp("r", CmpOp::Lt, num("0.1")), &pg), "r < '0.1'");
    assert_eq!(ok(&between, &pg), "r BETWEEN '0.1' AND '1.5'");
    let inexact = Err(RenderError::InexactEquality("r".into()));
    for c in [cmp("r", CmpOp::Eq, num("0.1")), cmp("r", CmpOp::Ne, num("0.1")), nd(num("0.1")), r#in(vec![num("0.1")])]
    {
        assert_eq!(render(&c, &my), inexact, "{c:?}");
    }
    assert_eq!(render(&r#in(vec![Value::Null, num("1")]), &my), inexact);
    assert_eq!(ok(&r#in(vec![Value::Null]), &my), "r IS NULL");
    assert_eq!(ok(&nd(Value::Null), &my), "NOT (r <=> NULL)");
    assert_eq!(ok(&cmp("r", CmpOp::Ge, num("0.1")), &my), "r >= 0.1");
    assert_eq!(ok(&between, &my), "r BETWEEN 0.1 AND 1.5");
}

/// MySQL reads text compared with a number as a number without a word (`'abc'` as 0, `'12abc'`
/// as 12, `'true'` as 0): such text is not written for a number column. A number in a string
/// reads as that number; PostgreSQL refuses the others itself.
#[test]
fn render_refuses_mysql_text_that_is_no_number_on_a_number_column() {
    let t = Target { utf8mb4: true, ..target(MY) };
    let not_number = Err(RenderError::NotNumber("id".into()));
    let between = |lo, hi| Cond::Between { col: "id".into(), lo, hi, negated: false };
    for c in [
        cmp("id", CmpOp::Eq, text("abc")),
        cmp("id", CmpOp::Eq, text("12abc")),
        cmp("id", CmpOp::Eq, text("true")),
        cmp("id", CmpOp::Lt, text("")),
        Cond::NotDistinct { col: "id".into(), value: text("abc"), negated: true },
        Cond::In { col: "id".into(), values: vec![num("1"), text("abc")], negated: false },
        between(text("abc"), num("5")),
    ] {
        assert_eq!(render(&c, &t), not_number, "{c:?}");
    }
    assert_eq!(render(&cmp("id", CmpOp::Eq, text("12")), &t).as_deref(), Ok("id = 12"));
    assert_eq!(render(&cmp("id", CmpOp::Eq, Value::Bool(true)), &t).as_deref(), Ok("id = TRUE"));
    assert_eq!(render(&cmp("name", CmpOp::Eq, text("abc")), &t).as_deref(), Ok("name = 'abc'"));
    assert_eq!(render(&cmp("born", CmpOp::Eq, text("abc")), &t).as_deref(), Ok("born = 'abc'"));
    assert_eq!(render_pg(&cmp("id", CmpOp::Eq, text("abc"))), "id = 'abc'");
}

#[test]
fn render_writes_mysql_number_text_as_a_number() {
    let t = Target { utf8mb4: true, ..target(MY) };
    let wide = "9007199254740993";
    let r#in = |values: Vec<Value>, negated| Cond::In { col: "id".into(), values, negated };
    let between = |lo, hi| Cond::Between { col: "id".into(), lo, hi, negated: false };
    for (c, want) in [
        (
            r#in(vec![text("9007199254740992"), text("9007199254740994")], false),
            "id IN (9007199254740992, 9007199254740994)",
        ),
        (r#in(vec![text(wide), num("1")], true), "id NOT IN (9007199254740993, 1)"),
        (between(text("1.5"), text("-2e3")), "id BETWEEN 1.5 AND -2000"),
        (cmp("price", CmpOp::Ge, text(wide)), "price >= 9007199254740993"),
        (cmp("id", CmpOp::Eq, text("-0")), "id = '-0'"),
        (cmp("name", CmpOp::Eq, text(wide)), "name = '9007199254740993'"),
        (cmp("born", CmpOp::Eq, text(wide)), "born = '9007199254740993'"),
    ] {
        assert_eq!(render(&c, &t).as_deref(), Ok(want), "{c:?}");
    }
    // PostgreSQL reads a string as the column's type, exactly: kept as written.
    assert_eq!(render_pg(&r#in(vec![text(wide)], false)), "id IN ('9007199254740993')");
}

/// MySQL reads a number with an exponent as a double, in quotes or bare: on a number column it
/// is written as its exact decimal, and one with none within a DECIMAL is refused.
#[test]
fn render_writes_mysql_exponents_as_exact_decimals() {
    let t = Target { utf8mb4: true, ..target(MY) };
    let r#in = |values: Vec<Value>, negated| Cond::In { col: "id".into(), values, negated };
    let between = |lo, hi, negated| Cond::Between { col: "id".into(), lo, hi, negated };
    for (c, want) in [
        (
            r#in(vec![text("9007199254740992e0"), num("9007199254740994E+0")], false),
            "id IN (9007199254740992, 9007199254740994)",
        ),
        (
            r#in(vec![num("90071992547409.93e2"), text("9007199254740995")], true),
            "id NOT IN (9007199254740993, 9007199254740995)",
        ),
        (
            between(text("900719925474099e1"), num("90071992547409920e-1"), false),
            "id BETWEEN 9007199254740990 AND 9007199254740992",
        ),
        (between(num("-15e-1"), text("1.25E1"), true), "id NOT BETWEEN -1.5 AND 12.5"),
        (cmp("id", CmpOp::Eq, num("0e9999")), "id = 0"),
        (cmp("id", CmpOp::Lt, text("1e-30")), "id < 0.000000000000000000000000000001"),
        (cmp("id", CmpOp::Lt, num(".5e1")), "id < 5"),
        (cmp("id", CmpOp::Eq, num("12")), "id = 12"),
    ] {
        assert_eq!(render(&c, &t).as_deref(), Ok(want), "{c:?}");
    }
    let not_exact = Err(RenderError::NotExact("id".into()));
    for v in [num("1e300"), text("1e-31"), num("1e65"), text(&"9".repeat(66)), num("1e99999")] {
        assert_eq!(render(&cmp("id", CmpOp::Eq, v.clone()), &t), not_exact, "{v:?}");
        assert_eq!(render(&r#in(vec![num("1"), v.clone()], false), &t), not_exact, "{v:?}");
    }
    assert_eq!(render(&cmp("id", CmpOp::Eq, num(&"9".repeat(65))), &t), Ok(format!("id = {}", "9".repeat(65))));
    // PostgreSQL reads a number with an exponent as numeric, exactly: kept as written.
    assert_eq!(render_pg(&cmp("id", CmpOp::Eq, num("9007199254740993e0"))), "id = 9007199254740993e0");
}

/// A MySQL TIMESTAMP's value with the offset of the zone it was read in is written as that
/// instant in the session's zone when the statement runs: the same instant in any zone, on every
/// MySQL and MariaDB; it reads back as the same value and passes the filter checks.
#[test]
fn a_mysql_timestamp_with_its_offset_is_written_as_its_instant_in_the_session_zone() {
    let t = Target {
        utf8mb4: true,
        ..Target::new(
            MY,
            vec![
                ColumnFacts { zoned: true, ..ColumnFacts::new("at", ColumnKind::Other) },
                ColumnFacts::new("born", ColumnKind::Other),
            ],
        )
    };
    let instant = |at: &str, off: &str| format!("CONVERT_TZ('{at}', '{off}', @@session.time_zone)");
    let v = "2026-11-10 09:30:00+00:00";
    for (c, want) in [
        (cmp("at", CmpOp::Eq, text(v)), format!("at = {}", instant("2026-11-10 09:30:00", "+00:00"))),
        (
            Cond::In { col: "at".into(), values: vec![text(v), text("2026-11-11 10:00:00.250-08:00")], negated: false },
            format!(
                "at IN ({}, {})",
                instant("2026-11-10 09:30:00", "+00:00"),
                instant("2026-11-11 18:00:00.250", "+00:00")
            ),
        ),
        // Typed without an offset: the session's local time, as written.
        (cmp("at", CmpOp::Lt, text("2026-11-10 09:30:00")), "at < '2026-11-10 09:30:00'".to_string()),
        (cmp("at", CmpOp::Eq, text("+05:30")), "at = '+05:30'".to_string()),
        // Not a TIMESTAMP: a string.
        (cmp("born", CmpOp::Eq, text(v)), format!("born = '{v}'")),
    ] {
        let sql = render(&c, &t).unwrap();
        assert_eq!(sql, want, "{c:?}");
        // Read back as its instant in UTC, which is written the same.
        let back = recognise(&sql, &t).unwrap();
        assert_eq!(render(&back, &t).unwrap(), sql);
        if !sql.contains("-08:00") && !format!("{c:?}").contains("-08:00") {
            assert_eq!(back, c, "{sql}");
        }
        for read_only in [false, true] {
            let r = check_filter(&sql, &users(), &t, &Classifier::new(Language::Sql(MY)), read_only);
            assert_eq!(r, Ok(FilterVerdict::Clear), "{sql}");
        }
    }
    // PostgreSQL's timestamptz reads the offset itself.
    let pg =
        Target::new(Dialect::Postgres, vec![ColumnFacts { zoned: true, ..ColumnFacts::new("at", ColumnKind::Other) }]);
    assert_eq!(render(&cmp("at", CmpOp::Eq, text(v)), &pg).unwrap(), format!("at = '{v}'"));
}

/// A MySQL TIMESTAMP's instant is worked out in UTC and written from `+00:00`, which every
/// MySQL and MariaDB converts: an offset only MySQL 8.0.19 and later reads (`+14:00`, `-13:59`)
/// never reaches the server, and a pick runs again the same from the history in any session. A
/// value with an offset that is no instant the column holds is refused, never written as a
/// conversion that comes out NULL (or is left unconverted).
#[test]
fn a_mysql_timestamp_instant_is_written_from_utc_and_refused_when_it_is_none() {
    let t = Target {
        utf8mb4: true,
        ..Target::new(MY, vec![ColumnFacts { zoned: true, ..ColumnFacts::new("at", ColumnKind::Other) }])
    };
    let instant = |at: &str| format!("CONVERT_TZ('{at}', '+00:00', @@session.time_zone)");
    for (v, utc) in [
        // The offsets at both ends of what a session may have, crossing a day and a year.
        ("2026-11-10 14:00:00+14:00", "2026-11-10 00:00:00"),
        ("2026-11-10 09:30:00+14:00", "2026-11-09 19:30:00"),
        ("2026-12-31 20:00:00.5-13:59", "2027-01-01 09:59:00.5"),
        ("2026-11-10 09:30:00+13:00", "2026-11-09 20:30:00"),
        ("2026-11-10 09:30:00-12:59", "2026-11-10 22:29:00"),
        ("2026-11-10 09:30:00+05:45", "2026-11-10 03:45:00"),
        // A leap day.
        ("2024-02-29 23:30:00-01:00", "2024-03-01 00:30:00"),
        ("2024-03-01 00:30:00+01:00", "2024-02-29 23:30:00"),
        // The ends of the TIMESTAMP range.
        ("1970-01-01 00:00:01+00:00", "1970-01-01 00:00:01"),
        ("1970-01-01 14:00:01+14:00", "1970-01-01 00:00:01"),
        ("2038-01-19 03:14:07.999999+00:00", "2038-01-19 03:14:07.999999"),
        ("2038-01-18 13:15:07-13:59", "2038-01-19 03:14:07"),
    ] {
        let c = cmp("at", CmpOp::Eq, text(v));
        let sql = render(&c, &t).unwrap();
        assert_eq!(sql, format!("at = {}", instant(utc)), "{v}");
        // The history's text read back is that instant, and is written the same again.
        let back = recognise(&sql, &t).unwrap();
        assert_eq!(back, cmp("at", CmpOp::Eq, text(&format!("{utc}+00:00"))), "{sql}");
        assert_eq!(render(&back, &t).unwrap(), sql);
        // Excluded NULL-safely as the same instant.
        let not = Cond::NotDistinct { col: "at".into(), value: text(v), negated: true };
        assert_eq!(render(&not, &t).unwrap(), format!("NOT (at <=> {})", instant(utc)), "{v}");
    }
    let refused = Err(RenderError::NotInstant("at".into()));
    for v in [
        // No session has these offsets.
        "2026-11-10 09:30:00+14:01",
        "2026-11-10 09:30:00-14:00",
        "2026-11-10 09:30:00+15:00",
        "2026-11-10 09:30:00+05:60",
        "2026-11-10 09:30:00+99:99",
        // No such date or time.
        "2026-02-29 09:30:00+00:00",
        "2026-04-31 09:30:00+00:00",
        "2026-13-01 09:30:00+00:00",
        "0000-00-00 00:00:00+00:00",
        "2026-11-10 24:00:00+00:00",
        "2026-11-10 09:60:00+00:00",
        "2026-11-10 09:30:60+00:00",
        // Outside the TIMESTAMP range in UTC, which the server would not convert.
        "1970-01-01 00:00:00+00:00",
        "1970-01-01 05:00:00+05:30",
        "2038-01-19 03:14:08+00:00",
        "2038-01-19 03:14:07-00:01",
    ] {
        assert_eq!(render(&cmp("at", CmpOp::Eq, text(v)), &t), refused, "{v}");
        let list =
            Cond::In { col: "at".into(), values: vec![text("2026-11-10 09:30:00+00:00"), text(v)], negated: true };
        assert_eq!(render(&list, &t), refused, "{v}");
        let range =
            Cond::Between { col: "at".into(), lo: text("2026-11-10 09:30:00+00:00"), hi: text(v), negated: false };
        assert_eq!(render(&range, &t), refused, "{v}");
    }
}

/// The instant `render` writes for a MySQL TIMESTAMP is read back only where it would be written
/// again: compared with a column known to be a TIMESTAMP, in exactly the form written. On a
/// VARCHAR, a DATETIME or a column of unknown type (as from the history before the table's types
/// are read) it stays as written: an edit would write its value back as a string.
#[test]
fn a_convert_tz_is_read_as_an_instant_only_on_a_timestamp_in_the_form_written() {
    let facts = |name: &str, kind, zoned| ColumnFacts { zoned, ..ColumnFacts::new(name, kind) };
    let t = Target {
        utf8mb4: true,
        ..Target::new(
            MY,
            vec![
                facts("at", ColumnKind::Other, true),
                facts("name", ColumnKind::Text, false),
                facts("made", ColumnKind::Other, false),
            ],
        )
    };
    let instant = "CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone)";
    let v = text("2026-11-10 09:30:00+00:00");
    for (sql, want) in [
        (format!("at = {instant}"), cmp("at", CmpOp::Eq, v.clone())),
        (format!("at <=> {instant}"), Cond::NotDistinct { col: "at".into(), value: v.clone(), negated: false }),
        (format!("at IN ({instant})"), Cond::In { col: "at".into(), values: vec![v.clone()], negated: false }),
        (
            format!("at BETWEEN {instant} AND '2026-11-11'"),
            Cond::Between { col: "at".into(), lo: v.clone(), hi: text("2026-11-11"), negated: false },
        ),
    ] {
        assert_eq!(recognise(&sql, &t), Some(want), "{sql}");
    }
    let raw = |sql: &str| Some(Cond::Raw(sql.to_string()));
    for sql in [
        // A VARCHAR, a DATETIME: `render` writes the value back as a string.
        format!("name = {instant}"),
        format!("name IN ('a', {instant})"),
        format!("made = {instant}"),
        format!("made BETWEEN {instant} AND '2026-11-11'"),
        // Not the form written: another source zone, another destination, not an instant.
        "at = CONVERT_TZ('2026-11-10 09:30:00', '+05:30', @@session.time_zone)".to_string(),
        "at = CONVERT_TZ('2026-11-10 09:30:00', '+0:00', @@session.time_zone)".to_string(),
        "at = CONVERT_TZ('2026-11-10 09:30:00', 'UTC', @@session.time_zone)".to_string(),
        "at = CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@time_zone)".to_string(),
        "at = CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@global.time_zone)".to_string(),
        "at = CONVERT_TZ('2026-11-10 09:30:00', '+00:00', '+00:00')".to_string(),
        "at = CONVERT_TZ('2026-02-30 09:30:00', '+00:00', @@session.time_zone)".to_string(),
        "at = CONVERT_TZ('1970-01-01 00:00:00', '+00:00', @@session.time_zone)".to_string(),
        "at = CONVERT_TZ('2026-11-10 09:30:00+00:00', '+00:00', @@session.time_zone)".to_string(),
        // Not a comparison.
        format!("at LIKE {instant}"),
    ] {
        assert_eq!(recognise(&sql, &t), raw(&sql), "{sql}");
    }
    // The table's types not read yet (the history's filter before they are): every column of
    // unknown type, so the instant stays as written; with them read, it is the instant.
    let names = Target::new(MY, vec![ColumnFacts::new("at", ColumnKind::Other)]);
    let sql = format!("at = {instant}");
    assert_eq!(recognise(&sql, &names), raw(&sql));
    assert_eq!(recognise(&sql, &t), Some(cmp("at", CmpOp::Eq, v.clone())));
    // PostgreSQL has no such function: as written.
    let pg = Target::new(Dialect::Postgres, vec![facts("at", ColumnKind::Other, true)]);
    assert_eq!(recognise(&sql, &pg), raw(&sql));
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
    t.columns[1].deterministic = Some(false);
    assert_eq!(
        render(&like("name", CaseMode::Insensitive), &t),
        Err(RenderError::CaseInsensitiveRefused("name".into()))
    );
    assert_eq!(render(&like("name", CaseMode::Server), &t), Err(RenderError::LikeUnknown("name".into())));
}

/// PostgreSQL before 18 refuses `LIKE` on a nondeterministic collation (18 takes it; `ILIKE`
/// never): "starts with" is written on a collation known deterministic, or on a server known to
/// take it; refused when both are known not to, not written when either is not known.
#[test]
fn render_like_on_a_nondeterministic_collation_needs_postgres_18() {
    let like =
        Cond::Like { col: "name".into(), pattern: like_starts_with("a"), negated: false, case: CaseMode::Server };
    let ilike =
        Cond::Like { col: "name".into(), pattern: like_contains("a"), negated: false, case: CaseMode::Insensitive };
    let written = "name LIKE 'a%' ESCAPE '!'";
    for takes in [None, Some(false), Some(true)] {
        let mut t = Target { like_nondeterministic: takes, ..target(Dialect::Postgres) };
        // Known deterministic: whatever the server.
        assert_eq!(render(&like, &t).unwrap(), written, "{takes:?}");
        for det in [Some(false), None] {
            t.columns[1].deterministic = det;
            let want = match (det, takes) {
                (_, Some(true)) => Ok(written.to_string()),
                (Some(false), Some(false)) => Err(RenderError::LikeRefused("name".into())),
                _ => Err(RenderError::LikeUnknown("name".into())),
            };
            assert_eq!(render(&like, &t), want, "{det:?} {takes:?}");
            // `ILIKE` stays refused on a collation not known deterministic, on 18 too.
            assert!(render(&ilike, &t).is_err(), "{det:?} {takes:?}");
        }
    }
    // MySQL: its collations take `LIKE` (known by name, never refused for this).
    let mut t = Target { utf8mb4: true, ..target(MY) };
    t.columns[1].deterministic = None;
    assert_eq!(render(&like, &t).unwrap(), written);
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

#[test]
fn render_values_of_a_cell_per_type_and_dialect() {
    // Numbers bare on a number column, quoted on any other; NaN quoted for the server to read.
    assert_eq!(render_pg(&cmp("price", CmpOp::Lt, num("19.990"))), "price < 19.990");
    assert_eq!(render_my(&cmp("price", CmpOp::Gt, num("-0.5")), true), "price > -0.5");
    assert_eq!(render_pg(&cmp("price", CmpOp::Eq, num("-0"))), "price = '-0'");
    assert_eq!(render_my(&cmp("name", CmpOp::Eq, num("7")), true), "name = '7'");
    // Times as the server showed them, quoted: the session reads back what it wrote.
    let ts = text("2026-11-10 09:30:00.123456+09");
    assert_eq!(render_pg(&cmp("born", CmpOp::Ge, ts)), "born >= '2026-11-10 09:30:00.123456+09'");
    assert_eq!(render_my(&cmp("born", CmpOp::Le, text("2026-11-10 09:30:00")), true), "born <= '2026-11-10 09:30:00'");
    assert_eq!(
        render_my(&cmp("born", CmpOp::Le, text("2026-11-10")), false),
        "born <= _utf8mb4 X'323032362D31312D3130'"
    );
    // Bytes: never text.
    let b = || Value::Bytes(vec![0, 0x27, 0x5c, 0xff]);
    assert_eq!(render_pg(&cmp("data", CmpOp::Eq, b())), r"data = E'\\x00275cff'::pg_catalog.bytea");
    assert_eq!(render_my(&cmp("data", CmpOp::Ne, b()), true), "data <> X'00275CFF'");
    assert_eq!(render_my(&cmp("data", CmpOp::Ne, b()), false), "data <> X'00275CFF'");
    // NULL-safe not equal keeps the NULL rows; a NULL cell is `IS NULL`.
    let ne = Cond::NotDistinct { col: "Mixed Case".into(), value: text("a'b"), negated: true };
    assert_eq!(render_pg(&ne), r#""Mixed Case" IS DISTINCT FROM 'a''b'"#);
    assert_eq!(render_my(&ne, true), "NOT (`Mixed Case` <=> 'a''b')");
    assert_eq!(render_my(&Cond::IsNull { col: "data".into(), negated: false }, true), "data IS NULL");
}

/// A value with a line break is written on one line, read back as the value: the filter line
/// holds one line, and the text it shows is the text that runs. PostgreSQL: an escape string
/// (`E'…\n…'`); MySQL: its UTF-8 bytes. The same for a pattern and in a list.
#[test]
fn render_writes_line_breaks_on_one_line() {
    let my_t = Target { utf8mb4: true, ..target(MY) };
    let pg_t = target(Dialect::Postgres);
    let like =
        |v: &str| Cond::Like { col: "name".into(), pattern: like_contains(v), negated: false, case: CaseMode::Server };
    let list = |v: &str| Cond::In { col: "name".into(), values: vec![text(v), text("x")], negated: false };
    let cases: &[(&str, &str, &str)] = &[
        ("a\nb", r"E'a\nb'", "X'610A62'"),
        ("a\rb", r"E'a\rb'", "X'610D62'"),
        ("a\r\nb", r"E'a\r\nb'", "X'610D0A62'"),
        ("it's\\\n", r"E'it''s\\\n'", "X'697427735C0A'"),
    ];
    for &(v, pg, my) in cases {
        let r = |c: &Cond, t: &Target| render(c, t).unwrap();
        assert_eq!(r(&cmp("name", CmpOp::Eq, text(v)), &pg_t), format!("name = {pg}"), "{v:?}");
        assert_eq!(r(&cmp("name", CmpOp::Eq, text(v)), &my_t), format!("name = _utf8mb4 {my}"), "{v:?}");
        let pg_like = format!("name LIKE E'%{}%' ESCAPE '!'", &pg[2..pg.len() - 1]);
        assert_eq!(r(&like(v), &pg_t), pg_like, "{v:?}");
        assert_eq!(r(&list(v), &pg_t), format!("name IN ({pg}, 'x')"), "{v:?}");
        assert_eq!(r(&list(v), &my_t), format!("name IN (_utf8mb4 {my}, 'x')"), "{v:?}");
        for (t, d) in [(&pg_t, Dialect::Postgres), (&my_t, MY)] {
            for c in [cmp("name", CmpOp::Eq, text(v)), like(v), list(v)] {
                let out = r(&c, t);
                assert!(!out.contains(['\n', '\r']), "one line: {out:?}");
                assert_eq!(check(d, &out, false), Ok(FilterVerdict::Clear), "{out}");
            }
        }
    }
    // The escape string reads back as the value.
    let open = crate::sql::dialect::LiteralOpen { prefix: 1, quote: '\'', backslash: true };
    assert_eq!(Dialect::Postgres.unescape_literal_body(r"it''s\\\r\n", open), "it's\\\r\n");
}

#[test]
fn render_in_lists_keep_the_null_forms() {
    let values = |null| {
        let mut v = vec![text("a"), text("b'c")];
        if null {
            v.push(Value::Null);
        }
        v
    };
    let r#in = |null, negated| Cond::In { col: "name".into(), values: values(null), negated };
    // NOT IN that keeps the rows whose value is NULL.
    let not_in_nulls = Cond::Any(vec![r#in(false, true), Cond::IsNull { col: "name".into(), negated: false }]);
    for (d, utf8mb4) in [(Dialect::Postgres, false), (MY, true)] {
        let t = Target { utf8mb4, ..target(d) };
        let r = |c: &Cond| render(c, &t).unwrap();
        assert_eq!(r(&r#in(false, false)), "name IN ('a', 'b''c')");
        assert_eq!(r(&r#in(true, false)), "(name IN ('a', 'b''c') OR name IS NULL)");
        assert_eq!(r(&r#in(false, true)), "name NOT IN ('a', 'b''c')");
        assert_eq!(r(&r#in(true, true)), "(name NOT IN ('a', 'b''c') AND name IS NOT NULL)");
        assert_eq!(r(&not_in_nulls), "(name NOT IN ('a', 'b''c') OR name IS NULL)");
    }
    let t = Target { utf8mb4: false, ..target(MY) };
    assert_eq!(render(&r#in(false, false), &t).unwrap(), "name IN (_utf8mb4 X'61', _utf8mb4 X'622763')");
}

#[test]
fn render_like_patterns_read_the_same_in_every_mysql_mode() {
    let modes =
        [MY, Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: true, dollar_quotes: false })];
    let starts = |value: &str| Cond::Like {
        col: "name".into(),
        pattern: like_starts_with(value),
        negated: false,
        case: CaseMode::Server,
    };
    for d in modes {
        let t = Target { utf8mb4: true, ..target(d) };
        // `%`, `_` and `!` escaped with `!`, never with a backslash.
        assert_eq!(render(&starts("5%_!"), &t).unwrap(), "name LIKE '5!%!_!!%' ESCAPE '!'", "{d:?}");
        // A backslash is a character of the value: written as bytes, read the same in both modes.
        assert_eq!(render(&starts(r"a\"), &t).unwrap(), "name LIKE _utf8mb4 X'615C25' ESCAPE '!'", "{d:?}");
    }
    let contains = |col: &str| Cond::Like {
        col: col.into(),
        pattern: like_contains("A_"),
        negated: false,
        case: CaseMode::Insensitive,
    };
    // A `_ci` column compares without case already; a column of no known collation is lowered.
    assert_eq!(render_my(&contains("email"), true), "email LIKE '%A!_%' ESCAPE '!'");
    let unknown = contains("Mixed Case");
    assert_eq!(render_my(&unknown, true), "LOWER(`Mixed Case`) LIKE LOWER('%A!_%') ESCAPE '!'");
    // PostgreSQL: `ILIKE` only on a collation known deterministic (a nondeterministic one is
    // refused by the server); not known, it is not written.
    let t = target(Dialect::Postgres);
    assert_eq!(render(&unknown, &t), Err(RenderError::CollationUnknown("Mixed Case".into())));
    let mut known = t.clone();
    known.columns[3].deterministic = Some(true);
    assert_eq!(render(&unknown, &known).unwrap(), r#""Mixed Case" ILIKE '%A!_%' ESCAPE '!'"#);
}

#[test]
fn and_filter_appends_or_wraps_first() {
    let pg = |base: &str| and_filter(base, "id = 1", Dialect::Postgres).unwrap();
    let my = |base: &str| and_filter(base, "id = 1", MY).unwrap();
    // Nothing yet: the condition is the text.
    assert_eq!(pg(""), "id = 1");
    assert_eq!(pg("   "), "id = 1");
    // A chain of ANDs (and anything that binds tighter) takes it as one more operand.
    assert_eq!(pg("a > 2"), "a > 2 AND id = 1");
    assert_eq!(pg("a > 2 AND b IS NULL  "), "a > 2 AND b IS NULL AND id = 1");
    assert_eq!(pg("NOT a = 2 AND b BETWEEN 1 AND 3"), "NOT a = 2 AND b BETWEEN 1 AND 3 AND id = 1");
    assert_eq!(pg("a::int = 2"), "a::int = 2 AND id = 1");
    assert_eq!(pg("name = 'x OR y'"), "name = 'x OR y' AND id = 1");
    assert_eq!(pg(r#""or" = 1"#), r#""or" = 1 AND id = 1"#);
    // Already in parentheses, an OR inside them does not count.
    assert_eq!(pg("(a = 1 OR b = 2)"), "(a = 1 OR b = 2) AND id = 1");
    assert_eq!(pg("(a = 1 OR b = 2) AND c = 3"), "(a = 1 OR b = 2) AND c = 3 AND id = 1");
    // An OR at the top is wrapped, visibly.
    assert_eq!(pg("a = 1 OR b = 2"), "(a = 1 OR b = 2) AND id = 1");
    assert_eq!(pg("(a = 1) or (b = 2)"), "((a = 1) or (b = 2)) AND id = 1");
    assert_eq!(pg("CASE WHEN a OR b THEN 1 END = 1"), "(CASE WHEN a OR b THEN 1 END = 1) AND id = 1");
    assert_eq!(pg("a || b = 'x'"), "(a || b = 'x') AND id = 1");
    // MySQL's XOR, `||` and an assignment bind more loosely than AND.
    assert_eq!(my("a = 1 XOR b = 2"), "(a = 1 XOR b = 2) AND id = 1");
    assert_eq!(my("a = 1 || b = 2"), "(a = 1 || b = 2) AND id = 1");
    assert_eq!(my("@v := 1"), "(@v := 1) AND id = 1");
    assert_eq!(my("a = 1 && b = 2"), "a = 1 && b = 2 AND id = 1");
    assert_eq!(my("`or` = 1"), "`or` = 1 AND id = 1");
    // A condition added to a chain is still one: the next one appends again.
    let twice = and_filter(&pg("a = 1 OR b = 2"), "c = 3", Dialect::Postgres).unwrap();
    assert_eq!(twice, "(a = 1 OR b = 2) AND id = 1 AND c = 3");
    // What it wrote keeps its meaning as a filter: it passes the bar's checks.
    let t = target(Dialect::Postgres);
    let c = Cond::In { col: "id".into(), values: vec![num("1"), Value::Null], negated: false };
    let text = and_filter("id > 0 OR name IS NULL", &render(&c, &t).unwrap(), Dialect::Postgres).unwrap();
    assert_eq!(text, "(id > 0 OR name IS NULL) AND (id IN (1) OR id IS NULL)");
    assert_eq!(check(Dialect::Postgres, &text, false), Ok(FilterVerdict::Clear));
}

/// Only the blanks the dialect's lexer reads as blanks are left out at the end of the filter:
/// PostgreSQL reads any character outside ASCII as part of a name, so a name that ends in a
/// no-break space or an ideographic space keeps it (it is another column than the one without).
#[test]
fn and_filter_keeps_the_end_of_a_name_the_server_reads() {
    let pg = |base: &str| and_filter(base, "id = 1", Dialect::Postgres).unwrap();
    for space in ['\u{00A0}', '\u{3000}'] {
        let base = format!("paid{space}");
        assert_eq!(pg(&base), format!("paid{space} AND id = 1"), "{space:?}");
        assert_eq!(pg(&format!("{base} \t\n")), format!("paid{space} AND id = 1"), "{space:?}");
        assert_eq!(pg(&format!("a = 1 OR {base}")), format!("(a = 1 OR {base}) AND id = 1"), "{space:?}");
    }
    // ASCII blanks at the end are left out on both dialects.
    assert_eq!(pg("a = 1 \t\r\n"), "a = 1 AND id = 1");
    assert_eq!(and_filter("a = 1  ", "id = 1", MY).unwrap(), "a = 1 AND id = 1");
}

/// A filter that does not stand on its own as one condition (a parenthesis that closes one it
/// did not open or is left open, a comment, a statement's end, a string left open) is refused,
/// said why, and never put in parentheses (which could make it read as another condition).
#[test]
fn and_filter_refuses_a_filter_that_does_not_stand_alone() {
    let refused = |d: Dialect, base: &str| match and_filter(base, "id = 1", d) {
        Err(BarRefusal::Lexical { problem, .. }) => problem,
        other => panic!("{base:?}: {other:?}"),
    };
    for d in [Dialect::Postgres, MY] {
        assert_eq!(refused(d, "a = 1) OR (b = 2"), LexProblem::ClosesOutside);
        assert_eq!(refused(d, "(a = 1"), LexProblem::Unclosed);
        assert_eq!(refused(d, "a = 1 -- x"), LexProblem::Comment);
        assert_eq!(refused(d, "a = 1 /* x */"), LexProblem::Comment);
        assert_eq!(refused(d, "a = 1; DROP TABLE t"), LexProblem::Semicolon);
        assert_eq!(refused(d, "name = 'x"), LexProblem::Unterminated);
    }
    assert_eq!(refused(MY, "a = 1 # x"), LexProblem::Comment);
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
    // Other escapes stand for bytes or characters this reader does not decode as the server does.
    for s in [r"name = E'\303\251'", r"name = E'\xC3\xA9'", r"name = E'a\tb'", r"name = E'\u00e9'"] {
        assert_eq!(pg(s), Some(Cond::Raw(s.into())), "{s}");
    }
    for s in [r"name = 'a\0b'", r"name = 'a\Zb'", r"name = 'a\nb'"] {
        assert_eq!(recognise(s, &tm), Some(Cond::Raw(s.into())), "{s}");
    }
    assert_eq!(recognise(r"name = 'it\'s'", &tm), Some(cmp("name", CmpOp::Eq, text("it's"))));
    // Under NO_BACKSLASH_ESCAPES a LIKE without ESCAPE has no escape character at all.
    let nbe = Target {
        dialect: Dialect::MySql(MySqlMode { no_backslash_escapes: true, ..MySqlMode::default() }),
        ..target(MY)
    };
    let like = r"name LIKE 'a\_%'";
    assert_eq!(recognise(like, &nbe), Some(Cond::Raw(like.into())));
    // A MySQL NOT without parentheses depends on HIGH_NOT_PRECEDENCE; with them it is read.
    for s in ["NOT id BETWEEN -5 AND 5", "NOT id = 1"] {
        assert_eq!(recognise(s, &tm), Some(Cond::Raw(s.into())), "{s}");
    }
    assert_eq!(recognise("NOT (id = 1)", &tm), Some(Cond::Not(Box::new(cmp("id", CmpOp::Eq, num("1"))))));
    // `&&` is AND: cutting at OR around it keeps the meaning.
    assert_eq!(
        recognise("id = 1 && id = 2 OR id = 3", &tm),
        Some(Cond::Any(vec![Cond::Raw("id = 1 && id = 2".into()), cmp("id", CmpOp::Eq, num("3"))]))
    );
}

#[test]
fn recognise_fails_on_what_it_cannot_read() {
    let t = target(Dialect::Postgres);
    // Deep nesting is not read rather than overflowing the stack.
    let deep_not = format!("{}TRUE", "NOT ".repeat(50_000));
    let deep_parens = format!("{}id = 1{}", "(".repeat(50_000), ")".repeat(50_000));
    for s in [&deep_not, &deep_parens] {
        assert_eq!(recognise(s, &t), None);
    }
    assert!(recognise(&format!("{}id = 1{}", "(".repeat(20), ")".repeat(20)), &t).is_some());
    // The filter check does not overflow either: PostgreSQL refuses such a text as too complex,
    // MySQL's token checks read it flat (the server answers for its own parser's limits).
    for s in [&deep_not, &deep_parens] {
        assert!(check(Dialect::Postgres, s, false).is_err());
        let _ = check(MY, s, false);
    }
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
    // A block comment left open runs past the text: refused, never read beyond its end.
    assert_eq!(lexical(MY, "id = 1 /*"), Some((LexProblem::Comment, "/*".into())));
    assert_eq!(lexical(MY, "id = 1 /*M"), Some((LexProblem::Comment, "/*M".into())));
    assert_eq!(lexical(Dialect::Postgres, "id = 1 /*"), Some((LexProblem::Comment, "/*".into())));
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

/// The refused part of `text` when it is refused as a row lock.
fn lock_at(d: Dialect, text: &str) -> Option<Option<String>> {
    match check(d, text, false) {
        Err(BarRefusal::Shape { problem: ShapeProblem::Locking, at }) => Some(at.map(|r| text[r].to_string())),
        _ => None,
    }
}

#[test]
fn a_postgres_row_lock_is_named_and_placed_as_mysql_does() {
    let pg = Dialect::Postgres;
    // Written after the condition (the statement does not parse): a row lock, not "not read".
    assert_eq!(lock_at(pg, "rating > 3 FOR UPDATE"), Some(Some("FOR UPDATE".into())));
    assert_eq!(lock_at(pg, "rating > 3 for no key update"), Some(Some("for no key update".into())));
    assert_eq!(lock_at(pg, "rating > 3 FOR KEY SHARE"), Some(Some("FOR KEY SHARE".into())));
    // In a subquery (it parses): placed too.
    assert_eq!(lock_at(pg, "id IN (SELECT id FROM shop.users FOR SHARE SKIP LOCKED)"), Some(Some("FOR SHARE".into())));
    // `FOR` of another construct is not a lock.
    assert_eq!(check(pg, "substring(name FROM 1 FOR 2) = 'ab'", false), Ok(FilterVerdict::Clear));
    // MySQL says the same of its own.
    assert_eq!(lock_at(MY, "rating > 3 FOR UPDATE"), Some(Some("FOR".into())));
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
    // The outcome of the statement before: for a table's statement, one the app sent.
    for f in ["FOUND_ROWS", "ROW_COUNT"] {
        let text = format!("{f}() >= 0");
        assert_eq!(check(MY, &text, true), Err(BarRefusal::RefusedCalls(vec![f.into()])), "{text}");
    }
    assert!(check(MY, "GET_LOCK('a', 1) = 1", false).is_err());
    assert!(matches!(check(MY, "my_fn(id) > 0", true), Err(BarRefusal::UncheckedCalls(_) | BarRefusal::ReadOnly(_))));
    assert_eq!(
        check(MY, "BENCHMARK(1000, md5('a')) = 0", false),
        Err(BarRefusal::RefusedCalls(vec!["BENCHMARK".into()]))
    );
}

/// The server's files and its large objects are out of a filter's reach on every profile:
/// each call is refused, by a bare name, `pg_catalog.`-qualified or in capitals, before it can
/// ask (MySQL's `LOAD_FILE` is refused alike).
/// MariaDB's sequences: `NEXTVAL` and `SETVAL` are refused calls, and `NEXT VALUE FOR` (no
/// call, a form of its own) is refused anywhere in the text, inside parentheses too, on every
/// profile: it would advance the sequence for each row read. Reading one (`LASTVAL`,
/// `PREVIOUS VALUE FOR`) stays a call that asks, or a clause outside parentheses.
#[test]
fn mariadb_sequences_are_not_advanced_by_a_filter() {
    for read_only in [false, true] {
        for text in ["id < NEXTVAL(s)", "id < nextval(s)", "SETVAL(s, 5) > 0"] {
            let r = check(MY, text, read_only);
            assert!(matches!(&r, Err(BarRefusal::RefusedCalls(n)) if !n.is_empty()), "{text}: {r:?}");
        }
        for text in ["id < (NEXT VALUE FOR s)", "id IN (1, (next value for s))"] {
            let r = check(MY, text, read_only);
            let at = text.to_ascii_uppercase().find("NEXT").unwrap();
            assert_eq!(r, Err(BarRefusal::Shape { problem: ShapeProblem::Sequence, at: Some(at..at + 4) }), "{text}");
        }
    }
    assert!(matches!(check(MY, "id < LASTVAL(s)", false), Ok(FilterVerdict::NeedsConfirm(_))));
}

#[test]
fn mariadb_oracle_mode_sequence_names_are_not_advanced_or_read_unasked() {
    for read_only in [false, true] {
        // `s.nextval` advances the sequence in the ORACLE sql mode: refused in every mode.
        for (text, name) in [
            ("id < s.nextval", "nextval"),
            ("id < S.NEXTVAL", "NEXTVAL"),
            ("id < `s`.`nextval`", "`nextval`"),
            ("id < shop.s.nextval", "nextval"),
            ("id < (s.nextval)", "nextval"),
            ("id IN (1, (s . nextval))", "nextval"),
        ] {
            let r = check(MY, text, read_only);
            let at = text.rfind(name).unwrap();
            assert_eq!(
                r,
                Err(BarRefusal::Shape { problem: ShapeProblem::Sequence, at: Some(at..at + name.len()) }),
                "{text} (read-only: {read_only})"
            );
        }
        // The last value read back changes nothing: asked about, refused when read-only, as
        // `LASTVAL(s)` is.
        for (text, name) in [
            ("id < s.currval", "CURRVAL"),
            ("id < `shop`.`s`.`CurrVal`", "CURRVAL"),
            ("id < (PREVIOUS VALUE FOR s)", "PREVIOUS VALUE FOR"),
        ] {
            let r = check(MY, text, read_only);
            match read_only {
                false => {
                    assert_eq!(r, Ok(FilterVerdict::NeedsConfirm(vec![Confirm::Calls(vec![name.into()])])), "{text}")
                }
                true => assert_eq!(r, Err(BarRefusal::UncheckedCalls(vec![name.into()])), "{text}"),
            }
        }
    }
    // A name that is no sequence's in attribute notation stays a column.
    assert_eq!(check(MY, "users.id < 3", false), Ok(FilterVerdict::Clear));
}

#[test]
fn server_file_and_large_object_calls_are_refused_on_every_profile() {
    let pg = Dialect::Postgres;
    let calls = [
        "pg_read_file('/etc/passwd') IS NOT NULL",
        "pg_read_binary_file('x', 0, 10, true) IS NOT NULL",
        "(pg_stat_file('x')).size > 0",
        "EXISTS (SELECT 1 FROM pg_ls_dir('.'))",
        "EXISTS (SELECT 1 FROM pg_ls_logdir())",
        "EXISTS (SELECT 1 FROM pg_ls_waldir())",
        "EXISTS (SELECT 1 FROM pg_ls_tmpdir())",
        "EXISTS (SELECT 1 FROM pg_ls_archive_statusdir())",
        "EXISTS (SELECT 1 FROM pg_ls_logicalsnapdir())",
        "EXISTS (SELECT 1 FROM pg_ls_logicalmapdir())",
        "EXISTS (SELECT 1 FROM pg_ls_replslotdir('s'))",
        "EXISTS (SELECT 1 FROM pg_ls_summariesdir())",
        "lo_import('/etc/passwd') > 0",
        "lo_export(16400, '/tmp/zz') = 1",
        "lo_get(16400) IS NOT NULL",
        "lo_put(16400, 0, decode('00', 'hex')) IS NULL",
        "lo_from_bytea(0, decode('00', 'hex')) > 0",
        "lo_create(0) > 0",
        "lo_creat(-1) > 0",
        "lo_unlink(16400) = 1",
        "lo_open(16400, 262144) >= 0",
        "lowrite(0, decode('00', 'hex')) > 0",
        "loread(0, 10) IS NOT NULL",
        "lo_truncate(0, 0) = 0",
        "lo_lseek(0, 0, 0) = 0",
        "lo_tell(0) = 0",
        "lo_close(0) = 0",
        "id = 1 OR pg_catalog.lo_get(id) IS NOT NULL",
        "PG_CATALOG.PG_READ_FILE('x') IS NOT NULL",
        "LO_GET(16400) IS NOT NULL",
    ];
    for text in calls {
        for read_only in [false, true] {
            let r = check(pg, text, read_only);
            let refused = match &r {
                Err(BarRefusal::RefusedCalls(names)) => !names.is_empty(),
                // The classifier names a server file first where it knows the call as one.
                Err(BarRefusal::NotRead { danger: Some(Danger::ServerFile), .. }) => true,
                _ => false,
            };
            assert!(refused, "{text} (read-only: {read_only}): {r:?}");
        }
    }
    for f in ["lo_get", "lo_put", "lo_create", "lo_unlink", "lo_from_bytea", "loread", "lowrite"] {
        assert!(PG_REFUSED_CALLS.contains(&f), "{f}");
    }
    for read_only in [false, true] {
        assert!(check(MY, "LOAD_FILE('/etc/passwd') IS NULL", read_only).is_err());
    }
}

#[test]
fn refused_calls_in_attribute_notation_are_refused_on_every_profile() {
    let pg = Dialect::Postgres;
    for (text, f) in [
        ("(id::pg_catalog.oid).lo_get IS NOT NULL", "lo_get"),
        ("(0::pg_catalog.oid).lo_create > 0", "lo_create"),
        ("(16400::oid).lo_unlink = 1", "lo_unlink"),
        ("(id::oid).LO_GET IS NOT NULL", "lo_get"),
        ("((id::oid).\"lo_get\") IS NOT NULL", "lo_get"),
        ("id = 1 OR (id::oid).lo_get IS NOT NULL", "lo_get"),
        ("EXISTS (SELECT 1 WHERE (id::oid).lo_unlink = 1)", "lo_unlink"),
        ("users.lo_get IS NOT NULL", "lo_get"),
        ("(0.5).pg_sleep IS NULL", "pg_sleep"),
        ("('zz_s'::regclass).nextval > 0", "nextval"),
    ] {
        for read_only in [false, true] {
            let r = check(pg, text, read_only);
            assert_eq!(r, Err(BarRefusal::RefusedCalls(vec![f.to_string()])), "{text} (read-only: {read_only})");
        }
    }
    // A field or column in attribute notation that is no refused built-in is not a call.
    for text in ["users.id = 1", "(users).id = 1", "(id::text) = 'a'"] {
        assert_eq!(check(pg, text, false), Ok(FilterVerdict::Clear), "{text}");
    }
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

// order

fn order(d: Dialect, text: &str) -> Result<Vec<OrderKey>, OrderError> {
    parse_order(text, &target(d))
}

fn order_err(d: Dialect, text: &str) -> (std::ops::Range<usize>, OrderProblem) {
    let e = order(d, text).unwrap_err();
    (e.range, e.kind)
}

#[test]
fn order_reads_keys_directions_and_nulls() {
    let pg = Dialect::Postgres;
    assert_eq!(order(pg, ""), Ok(vec![]));
    assert_eq!(order(pg, "   "), Ok(vec![]));
    assert_eq!(order(pg, "id"), Ok(vec![OrderKey::user("id", Dir::Asc)]));
    assert_eq!(
        order(pg, " name desc , id ASC "),
        Ok(vec![OrderKey::user("name", Dir::Desc), OrderKey::user("id", Dir::Asc)])
    );
    let nulls = |col: &str, dir, n| OrderKey { nulls: Some(n), ..OrderKey::user(col, dir) };
    assert_eq!(
        order(pg, "price DESC NULLS LAST, born nulls first"),
        Ok(vec![nulls("price", Dir::Desc, Nulls::Last), nulls("born", Dir::Asc, Nulls::First)])
    );
    for key in order(pg, "id, name").unwrap() {
        assert_eq!(key.origin, KeyOrigin::User);
    }
}

#[test]
fn order_names_match_exactly_then_folded() {
    let pg = Dialect::Postgres;
    // PostgreSQL folds a bare name to lower case; a quoted name is that name.
    assert_eq!(order(pg, "ID"), Ok(vec![OrderKey::user("id", Dir::Asc)]));
    assert_eq!(order(pg, "\"Mixed Case\" DESC"), Ok(vec![OrderKey::user("Mixed Case", Dir::Desc)]));
    assert_eq!(order_err(pg, "\"ID\""), (0..4, OrderProblem::UnknownColumn("ID".into())));
    assert_eq!(order_err(pg, "nope"), (0..4, OrderProblem::UnknownColumn("nope".into())));
    // A bare name that is exactly a column's is that column, before folding.
    let t = Target::new(pg, vec![ColumnFacts::new("Total", ColumnKind::Number)]);
    assert_eq!(parse_order("Total", &t), Ok(vec![OrderKey::user("Total", Dir::Asc)]));
    // MySQL: backticks, and names compared without regard to case.
    assert_eq!(order(MY, "`Mixed Case`"), Ok(vec![OrderKey::user("Mixed Case", Dir::Asc)]));
    assert_eq!(order(MY, "NAME desc"), Ok(vec![OrderKey::user("name", Dir::Desc)]));
    assert_eq!(order(MY, "`mixed case`"), Ok(vec![OrderKey::user("Mixed Case", Dir::Asc)]));
    assert_eq!(order_err(MY, "\"name\""), (0..6, OrderProblem::NotAColumn));
    let ansi = Dialect::MySql(MySqlMode { ansi_quotes: true, ..MySqlMode::default() });
    assert_eq!(order(ansi, "\"name\""), Ok(vec![OrderKey::user("name", Dir::Asc)]));
    let twins =
        Target::new(MY, vec![ColumnFacts::new("Code", ColumnKind::Text), ColumnFacts::new("CODE", ColumnKind::Text)]);
    assert_eq!(parse_order("CODE", &twins), Ok(vec![OrderKey::user("CODE", Dir::Asc)]));
    assert_eq!(parse_order("code", &twins).unwrap_err().kind, OrderProblem::AmbiguousColumn("code".into()));
}

#[test]
fn order_refuses_what_is_not_a_list_of_columns() {
    for d in [Dialect::Postgres, MY] {
        assert_eq!(order_err(d, "id,,name"), (3..4, OrderProblem::MissingColumn));
        assert_eq!(order_err(d, ", id"), (0..1, OrderProblem::MissingColumn));
        assert_eq!(order_err(d, "id, "), (2..3, OrderProblem::MissingColumn));
        assert_eq!(order_err(d, "1"), (0..1, OrderProblem::NotAColumn));
        assert_eq!(order_err(d, "'id'"), (0..4, OrderProblem::NotAColumn));
        assert_eq!(order_err(d, "lower(name)"), (5..6, OrderProblem::NotAColumn));
        assert_eq!(order_err(d, "users.id"), (5..6, OrderProblem::NotAColumn));
        assert_eq!(order_err(d, "id + 1"), (3..4, OrderProblem::Unexpected));
        assert_eq!(order_err(d, "id ASC DESC"), (7..11, OrderProblem::Unexpected));
        assert_eq!(order_err(d, "id; DROP TABLE users"), (2..3, OrderProblem::Unexpected));
        assert_eq!(order_err(d, "id -- x"), (3..7, OrderProblem::Unexpected));
        assert_eq!(order_err(d, "id /* x */"), (3..10, OrderProblem::Unexpected));
        assert_eq!(order_err(d, "id, ID DESC").1, OrderProblem::Duplicate("id".into()));
        assert_eq!(order_err(d, "id NULLS"), (3..8, OrderProblem::NullsIncomplete));
        assert_eq!(order_err(d, "id NULLS MIDDLE"), (3..8, OrderProblem::NullsIncomplete));
    }
    assert_eq!(order_err(Dialect::Postgres, "\"id"), (0..3, OrderProblem::Unterminated));
    assert_eq!(order_err(MY, "`id"), (0..3, OrderProblem::Unterminated));
    assert_eq!(order_err(MY, "id # x"), (3..6, OrderProblem::Unexpected));
    // MySQL has no NULLS FIRST / LAST.
    assert_eq!(order_err(MY, "id DESC NULLS LAST"), (8..18, OrderProblem::NullsUnsupported));
}

#[test]
fn order_with_no_columns_known_refuses_every_name() {
    let t = Target::new(Dialect::Postgres, vec![]);
    assert_eq!(parse_order("id", &t).unwrap_err().kind, OrderProblem::UnknownColumn("id".into()));
    assert_eq!(parse_order("", &t), Ok(vec![]));
}

#[test]
fn order_text_reads_back_as_the_same_keys() {
    let pg = Dialect::Postgres;
    let keys = vec![
        OrderKey::user("Mixed Case", Dir::Desc),
        OrderKey { nulls: Some(Nulls::Last), ..OrderKey::user("price", Dir::Asc) },
        OrderKey { origin: KeyOrigin::TieBreak, ..OrderKey::user("id", Dir::Asc) },
    ];
    assert_eq!(order_text(&keys, pg), "\"Mixed Case\" DESC, price ASC NULLS LAST");
    assert_eq!(order(pg, &order_text(&keys, pg)), Ok(keys[..2].to_vec()));
    let my_keys = vec![OrderKey::user("Mixed Case", Dir::Asc), OrderKey::user("name", Dir::Desc)];
    assert_eq!(order_text(&my_keys, MY), "`Mixed Case` ASC, name DESC");
    assert_eq!(order(MY, &order_text(&my_keys, MY)), Ok(my_keys));
    assert_eq!(order_text(&[], pg), "");
    // A keyword column is quoted.
    let t = Target::new(pg, vec![ColumnFacts::new("order", ColumnKind::Number)]);
    let k = vec![OrderKey::user("order", Dir::Asc)];
    assert_eq!(order_text(&k, pg), "\"order\" ASC");
    assert_eq!(parse_order(&order_text(&k, pg), &t), Ok(k));
}

// sort actions

fn tie(col: &str) -> OrderKey {
    OrderKey { origin: KeyOrigin::TieBreak, ..OrderKey::user(col, Dir::Asc) }
}

#[test]
fn sorting_by_one_column_goes_up_down_then_off_and_drops_the_others() {
    use SortStep::Only;
    let asc = |c: &str| OrderKey::user(c, Dir::Asc);
    let desc = |c: &str| OrderKey::user(c, Dir::Desc);
    assert_eq!(sort_column(&[], "name", Only), vec![asc("name")]);
    assert_eq!(sort_column(&[asc("name")], "name", Only), vec![desc("name")]);
    assert_eq!(sort_column(&[desc("name")], "name", Only), vec![]);
    // Another column: it alone, from its own direction (none: up).
    assert_eq!(sort_column(&[asc("name"), desc("price")], "id", Only), vec![asc("id")]);
    // A column among several goes on from its own direction, the others go.
    assert_eq!(sort_column(&[asc("name"), asc("price")], "price", Only), vec![desc("price")]);
    assert_eq!(sort_column(&[asc("name"), desc("price")], "price", Only), vec![]);
    // Where its NULLs go is the server's again.
    let nulls_first = OrderKey { nulls: Some(Nulls::First), ..asc("price") };
    assert_eq!(sort_column(&[nulls_first], "price", Only), vec![desc("price")]);
}

#[test]
fn adding_a_column_puts_it_last_then_turns_it_then_takes_it_out() {
    use SortStep::Add;
    let asc = |c: &str| OrderKey::user(c, Dir::Asc);
    let desc = |c: &str| OrderKey::user(c, Dir::Desc);
    assert_eq!(sort_column(&[], "name", Add), vec![asc("name")]);
    let keys = vec![desc("name")];
    let keys = sort_column(&keys, "price", Add);
    assert_eq!(keys, vec![desc("name"), asc("price")]);
    let keys = sort_column(&keys, "price", Add);
    assert_eq!(keys, vec![desc("name"), desc("price")]);
    let keys = sort_column(&keys, "price", Add);
    assert_eq!(keys, vec![desc("name")]);
    // A key in the middle turns where it is; the others keep their NULLs.
    let born = OrderKey { nulls: Some(Nulls::Last), ..desc("born") };
    let keys = vec![asc("name"), asc("price"), born.clone()];
    assert_eq!(sort_column(&keys, "price", Add), vec![asc("name"), desc("price"), born.clone()]);
    assert_eq!(sort_column(&[asc("name"), desc("price"), born.clone()], "price", Add), vec![asc("name"), born]);
}

#[test]
fn a_tie_breaker_key_is_not_the_users() {
    // The app appended `id` to make the order total: a sort on it starts it as the user's
    // own key, and the tie-breaker keys never come back from a sort action.
    let keys = vec![OrderKey::user("name", Dir::Asc), tie("id")];
    assert_eq!(sort_column(&keys, "id", SortStep::Only), vec![OrderKey::user("id", Dir::Asc)]);
    assert_eq!(
        sort_column(&keys, "id", SortStep::Add),
        vec![OrderKey::user("name", Dir::Asc), OrderKey::user("id", Dir::Asc)]
    );
    assert_eq!(sort_column(&keys, "name", SortStep::Add), vec![OrderKey::user("name", Dir::Desc)]);
    assert_eq!(sort_column(&keys, "name", SortStep::Only), vec![OrderKey::user("name", Dir::Desc)]);
    // Compiled, the order is made total again by the app.
    let q = query(None, sort_column(&keys, "name", SortStep::Only));
    assert_eq!(
        compile(&q, &target(Dialect::Postgres)).sql,
        r#"SELECT * FROM "shop"."users" ORDER BY "name" DESC, "id" ASC"#
    );
}

#[test]
fn the_levels_of_the_keys_follow_their_order() {
    // Three keys added one after the other: the order text lists them as added.
    let mut keys = Vec::new();
    for c in ["price", "name", "born"] {
        keys = sort_column(&keys, c, SortStep::Add);
    }
    assert_eq!(order_text(&keys, Dialect::Postgres), "price ASC, name ASC, born ASC");
    let keys = sort_column(&keys, "name", SortStep::Add);
    let keys = sort_column(&keys, "name", SortStep::Add);
    assert_eq!(order_text(&keys, Dialect::Postgres), "price ASC, born ASC");
}

#[test]
fn a_condition_names_the_columns_it_compares() {
    let t = target(Dialect::Postgres);
    let c =
        recognise("id > 1 AND (name = 'a' OR price IS NULL) AND NOT id = 3 AND lower(email) = 'x'", &t).expect("read");
    assert_eq!(c.columns(), vec!["id", "name", "price"]);
    // Text kept as written names no column.
    assert_eq!(Cond::Raw("price > 1".into()).columns(), Vec::<&str>::new());
}

#[test]
fn a_subquery_is_seen_in_every_form() {
    for d in [Dialect::Postgres, MY] {
        for text in [
            "id IN (SELECT id FROM t)",
            "EXISTS(select 1)",
            "id = ( \n WITH x AS (SELECT 1) SELECT * FROM x)",
            "(id) IN (VALUES (1))",
            "id IN ((SELECT 1))",
        ] {
            assert!(has_subquery(text, d), "{d:?} {text}");
        }
        for text in ["id > 1", "(id > 1) AND (name = 'select')", "lower(name) = 'x'", "id IN (1, 2)", "\"select\" = 1"]
        {
            assert!(!has_subquery(text, d), "{d:?} {text}");
        }
        // A text the lexical check refuses may hold one, as far as is known.
        assert!(has_subquery("id = 1 -- (select", d));
        assert!(has_subquery("name = 'open", d));
    }
    assert!(has_subquery("id IN (TABLE t)", MY));
}

#[test]
fn an_estimate_is_offered_only_for_a_filter_without_unknown_calls_or_mysql_subqueries() {
    let confirm = FilterVerdict::NeedsConfirm(vec![Confirm::Calls(vec!["f(x)".into()])]);
    assert_eq!(estimate_allowed(&FilterVerdict::Clear, "id > 1", Dialect::Postgres), Ok(()));
    assert_eq!(estimate_allowed(&confirm, "f(id) > 1", Dialect::Postgres), Err(EstimateOff::Calls));
    assert_eq!(estimate_allowed(&confirm, "f(id) > 1", MY), Err(EstimateOff::Calls));
    let sub = "id IN (SELECT id FROM t)";
    assert_eq!(estimate_allowed(&FilterVerdict::Clear, sub, Dialect::Postgres), Ok(()));
    assert_eq!(estimate_allowed(&FilterVerdict::Clear, sub, MY), Err(EstimateOff::Subquery));
    assert_eq!(estimate_allowed(&FilterVerdict::Clear, "id > 1", MY), Ok(()));
}
