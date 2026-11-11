use super::*;
use datarig_core::query::{ColumnFacts, TableName};
use datarig_core::sql::dialect::MySqlMode;

const PG: Dialect = Dialect::Postgres;
const MY: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: false });

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
    Target { primary_key: vec!["id".into()], utf8mb4: true, ..Target::new(d, columns) }
}

fn users() -> Query {
    Query::table(TableName { database: None, schema: "shop".into(), name: "users".into() })
}

fn filtered(text: &str) -> Query {
    Query { filter: Some(Filter { text: text.into(), recognised: None }), ..users() }
}

fn row(col: &str, op: Op, values: Vec<Value>) -> Row {
    Row { col: col.into(), op, values, keep_nulls: false }
}

fn num(n: &str) -> Value {
    Value::Number(n.into())
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn info(name: &str, kind: ColumnKind) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        type_name: String::new(),
        kind,
        meta: None,
        enum_known: false,
        primary_key: false,
        foreign_key: false,
        not_null: false,
        invisible: false,
    }
}

fn meta(name: &str, type_name: &str, kind: ValueKind) -> ColumnMeta {
    ColumnMeta::new(name.into(), type_name.into(), kind, None)
}

fn chip_texts(w: &Where) -> Vec<(&str, bool)> {
    w.chips().iter().map(|c| (c.text, c.expr)).collect()
}

/// Texts whose terms the dialog shows, per dialect, with the chips they make (text, expr).
fn untouched(d: Dialect) -> Vec<(&'static str, Vec<(&'static str, bool)>)> {
    let mut cases = vec![
        ("  id = 1   AND\n  name = 'x'  ", vec![("id = 1", false), ("name = 'x'", false)]),
        ("id=1 and\tprice>2", vec![("id=1", false), ("price>2", false)]),
        ("price BETWEEN 1 AND 5 AND id > 2", vec![("price BETWEEN 1 AND 5", false), ("id > 2", false)]),
        (
            "CASE WHEN id > 1 AND price < 2 THEN 1 ELSE 0 END = 1 AND name = 'a'",
            vec![("CASE WHEN id > 1 AND price < 2 THEN 1 ELSE 0 END = 1", true), ("name = 'a'", false)],
        ),
        ("name = 'a AND b' AND email <> ' AND '", vec![("name = 'a AND b'", false), ("email <> ' AND '", false)]),
        ("(id = 1 OR id = 2) AND price < 3", vec![("(id = 1 OR id = 2)", true), ("price < 3", false)]),
        ("id = 1 OR name = 'a' AND price > 2", vec![("id = 1 OR name = 'a' AND price > 2", true)]),
        ("\n id IN (1, 2)\n", vec![("id IN (1, 2)", false)]),
    ];
    match d {
        Dialect::Postgres => cases.extend([
            (
                "\"Mixed Case\" = 'a AND b' AND tags[1] = 'x' AND tags[id:2] = '{a}'",
                vec![("\"Mixed Case\" = 'a AND b'", false), ("tags[1] = 'x'", true), ("tags[id:2] = '{a}'", true)],
            ),
            (
                "name ILIKE '%a!%%' ESCAPE '!' AND  id <> 3",
                vec![("name ILIKE '%a!%%' ESCAPE '!'", false), ("id <> 3", false)],
            ),
        ]),
        Dialect::MySql(_) => cases.extend([
            ("`Mixed Case` = 'a AND b' AND id = 2", vec![("`Mixed Case` = 'a AND b'", false), ("id = 2", false)]),
            ("id = 1 XOR price = 2", vec![("id = 1 XOR price = 2", true)]),
            ("id = 1 || price = 2 AND name = 'a'", vec![("id = 1 || price = 2 AND name = 'a'", true)]),
            ("id = 1 && price = 2", vec![("id = 1 && price = 2", true)]),
        ]),
    }
    cases
}

#[test]
fn an_untouched_where_is_sent_byte_for_byte() {
    for d in [PG, MY] {
        let t = target(d);
        for (sql, chips) in untouched(d) {
            let mut w = Where::from_text(sql, &t);
            assert_eq!(w.text(), sql, "{d:?}");
            assert_eq!(chip_texts(&w), chips, "{d:?} {sql}");
            // Each chip is the slice of the text it came from.
            for term in w.terms() {
                let Origin::Original(r) = &term.origin else { panic!("{sql}: an edited term") };
                assert_eq!(term.text.as_deref(), Some(&sql[r.clone()]));
            }
            // The SQL field read back as it was shown changes nothing.
            w.set_text(&w.text(), &t);
            assert!(!w.rebuilt());
            assert_eq!(w.text(), sql);
            // Opened from the query and applied again without a change.
            let draft = Draft::from_query(&filtered(sql), &t);
            let q = draft.to_query(&users(), &t).unwrap();
            assert_eq!(q.filter.as_ref().map(|f| f.text.as_str()), Some(sql), "{d:?}");
            assert!(draft.same_as(&Draft::from_query(&q, &t)));
        }
    }
}

#[test]
fn a_blank_where_has_no_terms_and_sends_no_filter() {
    let t = target(PG);
    let w = Where::from_text("  \n ", &t);
    assert!(w.terms().is_empty());
    assert_eq!(w.original(), "  \n ");
    let q = Draft::from_query(&filtered("  \n "), &t).to_query(&users(), &t).unwrap();
    assert_eq!(q.filter, None);
    assert!(Draft::from_query(&filtered(" "), &t).same_as(&Draft::from_query(&users(), &t)));
}

#[test]
fn text_that_is_not_one_conjunction_is_one_expression() {
    for (d, sql) in [
        (PG, "id = 1 OR name = 'a' AND price > 2"),
        (PG, "NOT id = 1 AND price = 2"),
        (MY, "id = 1 XOR price = 2"),
        (MY, "id = 1 || price = 2"),
        (MY, "!(id = 1) AND price = 2"),
        (PG, "id = 1 AND (price"),
    ] {
        let w = Where::from_text(&format!(" {sql} "), &target(d));
        assert_eq!(chip_texts(&w), vec![(sql, true)], "{d:?}");
        let [term] = w.terms() else { panic!("{sql}") };
        assert_eq!(term.origin, Origin::Original(1..1 + sql.len()));
    }
}

#[test]
fn removing_a_chip_keeps_the_others_as_written_and_in_order() {
    let t = target(PG);
    let mut w = Where::from_text("id  =  1 AND (name = 'a' OR name = 'b')  AND\nprice>2", &t);
    assert_eq!(w.chips().len(), 3);
    let gone = w.remove(0).unwrap();
    assert_eq!(gone.text.as_deref(), Some("id  =  1"));
    assert_eq!(w.text(), "(name = 'a' OR name = 'b') AND price>2");
    assert!(w.remove(5).is_none());
    w.remove(1);
    assert_eq!(w.text(), "(name = 'a' OR name = 'b')");
    w.remove(0);
    assert_eq!(w.text(), "");
    assert!(w.rebuilt());
}

#[test]
fn an_expression_with_or_is_put_in_parentheses_when_a_term_joins_it() {
    let t = target(PG);
    let mut w = Where::from_text("id = 1 OR id = 2", &t);
    w.add(row("name", Op::Eq, vec![text("a")]), &t);
    assert_eq!(w.text(), "(id = 1 OR id = 2) AND name = 'a'");
    assert_eq!(chip_texts(&w), vec![("id = 1 OR id = 2", true), ("name = 'a'", false)]);
}

#[test]
fn a_changed_row_is_written_again_in_its_place_alone() {
    for d in [PG, MY] {
        let t = target(d);
        let mut w = Where::from_text("price  >  2 AND id IN (1,2)  AND  born IS NULL", &t);
        let [_, list, _] = w.terms() else { panic!() };
        assert_eq!(list.row, Some(row("id", Op::InList, vec![num("1"), num("2")])));
        // A value ticked off the list.
        assert!(w.set(1, row("id", Op::InList, vec![num("1")]), &t));
        assert_eq!(w.text(), "price  >  2 AND id IN (1) AND born IS NULL");
        assert_eq!(w.terms()[1].origin, Origin::Edited);
        assert_eq!(w.terms()[0].origin, Origin::Original(0..11));
        assert!(!w.set(3, row("id", Op::IsNull, vec![]), &t));
    }
}

#[test]
fn an_incomplete_row_is_no_chip_and_never_sent() {
    let t = target(PG);
    let mut w = Where::from_text("id = 1", &t);
    let at = w.add(Row::new("price", Op::Between), &t);
    assert_eq!(at, 1);
    assert!(w.has_incomplete());
    assert_eq!(w.text(), "id = 1");
    assert_eq!(chip_texts(&w), vec![("id = 1", false)]);
    assert_eq!(w.rows_of("price"), vec![1]);
    // One bound is not enough.
    w.set(1, row("price", Op::Between, vec![num("1")]), &t);
    assert_eq!(w.text(), "id = 1");
    w.set(1, row("price", Op::Between, vec![num("1"), num("9")]), &t);
    assert!(!w.has_incomplete());
    assert_eq!(w.text(), "id = 1 AND price BETWEEN 1 AND 9");
    w.set(1, Row::new("price", Op::Le), &t);
    w.add(row("id", Op::IsNotNull, vec![]), &t);
    let at: Vec<usize> = w.chips().iter().map(|c| c.index).collect();
    assert_eq!(at, [0, 2]);
    // Only incomplete rows: nothing is sent.
    let mut w = Where::from_text("", &t);
    w.add(Row::new("name", Op::Eq), &t);
    assert_eq!(w.text(), "");
    let draft = Draft { where_: w, order: Vec::new(), limit: String::new() };
    assert_eq!(draft.to_query(&users(), &t).unwrap().filter, None);
}

#[test]
fn a_row_the_server_would_refuse_is_kept_with_why() {
    let t = target(PG);
    let mut w = Where::from_text("id = 1", &t);
    // PostgreSQL: `ILIKE` on a collation not known to be deterministic.
    let i = w.add(row("email", Op::Contains, vec![text("x")]), &t);
    let term = &w.terms()[i];
    assert_eq!(term.text, None);
    assert_eq!(term.problem, Some(RenderError::CollationUnknown("email".into())));
    assert_eq!(term.row, Some(row("email", Op::Contains, vec![text("x")])));
    assert!(w.has_incomplete());
    assert_eq!(w.text(), "id = 1");
    // An order on a ticked NULL compares with nothing.
    let i = w.add(row("price", Op::Lt, vec![Value::Null]), &t);
    assert_eq!(w.terms()[i].problem, Some(RenderError::NullCompare("price".into())));
}

/// Each complete row of the dialog, with the text written for it on PostgreSQL and MySQL.
fn rendered() -> Vec<(Row, &'static str, &'static str)> {
    let kept = |mut r: Row| {
        r.keep_nulls = true;
        r
    };
    vec![
        (row("id", Op::Eq, vec![num("7")]), "id = 7", "id = 7"),
        (row("name", Op::Eq, vec![text("O'Brien")]), "name = 'O''Brien'", "name = 'O''Brien'"),
        (row("Mixed Case", Op::Eq, vec![text("x")]), "\"Mixed Case\" = 'x'", "`Mixed Case` = 'x'"),
        (row("born", Op::Eq, vec![text("2024-01-02")]), "born = '2024-01-02'", "born = '2024-01-02'"),
        (
            row("data", Op::Eq, vec![Value::Bytes(vec![0x0a, 0xff])]),
            "data = E'\\\\x0aff'::pg_catalog.bytea",
            "data = X'0AFF'",
        ),
        (
            row("price", Op::Between, vec![num("1.5"), num("-2e3")]),
            "price BETWEEN 1.5 AND -2e3",
            "price BETWEEN 1.5 AND -2e3",
        ),
        (row("id", Op::Ne, vec![num("7")]), "id <> 7", "id <> 7"),
        (row("name", Op::NeOrNull, vec![text("a")]), "name IS DISTINCT FROM 'a'", "NOT (name <=> 'a')"),
        (row("id", Op::Lt, vec![num("2")]), "id < 2", "id < 2"),
        (row("id", Op::Le, vec![num("2")]), "id <= 2", "id <= 2"),
        (row("id", Op::Gt, vec![num("2")]), "id > 2", "id > 2"),
        (row("id", Op::Ge, vec![num("2")]), "id >= 2", "id >= 2"),
        (
            row("name", Op::Contains, vec![text("5%_!")]),
            "name ILIKE '%5!%!_!!%' ESCAPE '!'",
            "LOWER(name) LIKE LOWER('%5!%!_!!%') ESCAPE '!'",
        ),
        (row("name", Op::StartsWith, vec![text("ab")]), "name LIKE 'ab%' ESCAPE '!'", "name LIKE 'ab%' ESCAPE '!'"),
        (row("id", Op::InList, vec![num("1"), num("2")]), "id IN (1, 2)", "id IN (1, 2)"),
        (row("id", Op::InList, vec![num("1"), Value::Null]), "(id IN (1) OR id IS NULL)", "(id IN (1) OR id IS NULL)"),
        (row("name", Op::NotInList, vec![text("a"), text("b")]), "name NOT IN ('a', 'b')", "name NOT IN ('a', 'b')"),
        (
            kept(row("name", Op::NotInList, vec![text("a"), text("b")])),
            "(name NOT IN ('a', 'b') OR name IS NULL)",
            "(name NOT IN ('a', 'b') OR name IS NULL)",
        ),
        (
            row("name", Op::NotInList, vec![text("a"), Value::Null]),
            "(name NOT IN ('a') AND name IS NOT NULL)",
            "(name NOT IN ('a') AND name IS NOT NULL)",
        ),
        (row("born", Op::IsNull, vec![]), "born IS NULL", "born IS NULL"),
        (row("born", Op::IsNotNull, vec![]), "born IS NOT NULL", "born IS NOT NULL"),
    ]
}

#[test]
fn each_operator_writes_its_condition_per_dialect() {
    for (r, pg, my) in rendered() {
        for (d, want) in [(PG, pg), (MY, my)] {
            let mut w = Where::from_text("", &target(d));
            w.add(r.clone(), &target(d));
            assert_eq!(w.text(), want, "{d:?} {r:?}");
        }
    }
    // MySQL: a `_ci` collation ignores case already.
    let mut w = Where::from_text("", &target(MY));
    w.add(row("email", Op::Contains, vec![text("x")]), &target(MY));
    assert_eq!(w.text(), "email LIKE '%x%' ESCAPE '!'");
}

#[test]
fn a_null_value_of_an_equality_is_a_test_of_null() {
    let t = target(PG);
    let cond = |op| row("id", op, vec![Value::Null]).cond().unwrap();
    assert_eq!(cond(Op::Eq), Cond::IsNull { col: "id".into(), negated: false });
    assert_eq!(cond(Op::Ne), Cond::IsNull { col: "id".into(), negated: true });
    assert_eq!(cond(Op::NeOrNull), Cond::IsNull { col: "id".into(), negated: true });
    // A list of NULL alone and a list that keeps NULL rows but excludes NULL.
    let mut kept = row("id", Op::NotInList, vec![num("1"), Value::Null]);
    kept.keep_nulls = true;
    assert_eq!(query::render(&kept.cond().unwrap(), &t).unwrap(), "(id NOT IN (1) AND id IS NOT NULL)");
    assert_eq!(query::render(&row("id", Op::InList, vec![Value::Null]).cond().unwrap(), &t).unwrap(), "id IS NULL");
}

#[test]
fn rows_read_back_from_what_they_write() {
    for d in [PG, MY] {
        let t = target(d);
        let mut rows: Vec<Row> = rendered().into_iter().map(|(r, ..)| r).collect();
        if d == MY {
            rows.push(row("email", Op::Contains, vec![text("x")]));
        }
        for r in rows {
            let sql = query::render(&r.cond().unwrap(), &t).unwrap();
            let back = query::recognise(&sql, &t).and_then(|c| Row::from_cond(&c, &t));
            assert_eq!(back.as_ref(), Some(&r), "{d:?} {sql}");
            // And as a chip of a text given.
            let w = Where::from_text(&format!("id > 0 AND {sql}"), &t);
            assert_eq!(w.terms()[1].row.as_ref(), Some(&r), "{d:?} {sql}");
        }
    }
}

#[test]
fn conditions_the_dialog_does_not_write_are_expressions() {
    for (d, sql) in [
        (PG, "name LIKE '%a%' ESCAPE '!'"),
        (PG, "name ILIKE 'a%' ESCAPE '!'"),
        (PG, "name NOT LIKE 'a%' ESCAPE '!'"),
        (PG, "name LIKE 'a_b%' ESCAPE '!'"),
        (PG, "id NOT BETWEEN 1 AND 2"),
        (PG, "id IS NOT DISTINCT FROM 1"),
        (PG, "id = 1 AND price = 2 OR id = 3"),
        (PG, "NOT (id = 1)"),
        (PG, "nope = 1"),
        (MY, "id <=> 1"),
        (MY, "name LIKE '%a%' ESCAPE '!'"),
    ] {
        let w = Where::from_text(sql, &target(d));
        assert_eq!(chip_texts(&w), vec![(sql, true)], "{d:?}");
    }
}

#[test]
fn typed_values_are_numbers_only_when_plain_numbers() {
    let id = info("id", ColumnKind::Number);
    let one = |s: &str, c: &ColumnInfo| parse_input(Op::Eq, &[s], c);
    for n in ["1", "-1", "1.5", ".5", "5.", "1e3", "1E-3", "2.5e+10"] {
        assert_eq!(one(n, &id), vec![num(n)], "{n}");
    }
    assert_eq!(one(" 42 ", &id), vec![num("42")]);
    for s in ["1 OR 1=1", "+1", "-0", "-0.0", "1.2.3", "1e", "0x1F", "NaN", "1;", "1--", "-", "."] {
        assert_eq!(one(s, &id), vec![text(s)], "{s}");
    }
    // Typed text never reaches the SQL as it is.
    for d in [PG, MY] {
        let t = target(d);
        let mut w = Where::from_text("", &t);
        w.add(Row { values: one("1 OR 1=1", &id), ..Row::new("id", Op::Eq) }, &t);
        assert_eq!(w.text(), "id = '1 OR 1=1'");
        w.add(Row { values: one("x' OR '1'='1", &info("name", ColumnKind::Text)), ..Row::new("name", Op::Eq) }, &t);
        assert_eq!(w.text(), "id = '1 OR 1=1' AND name = 'x'' OR ''1''=''1'");
    }
    // Text as typed, NULL as text, an empty field as no value.
    let name = info("name", ColumnKind::Text);
    assert_eq!(one(" a b ", &name), vec![text(" a b ")]);
    assert_eq!(one("NULL", &name), vec![text("NULL")]);
    assert_eq!(one("5", &name), vec![text("5")]);
    assert_eq!(one("", &name), Vec::<Value>::new());
    assert_eq!(parse_input(Op::Contains, &["12"], &id), vec![text("12")]);
    assert_eq!(parse_input(Op::IsNull, &["12"], &id), Vec::<Value>::new());
    // Bytes in hexadecimal, else text.
    let data = info("data", ColumnKind::Bytes);
    assert_eq!(one("\\x0aFF", &data), vec![Value::Bytes(vec![0x0a, 0xff])]);
    assert_eq!(one("0x0AFF", &data), vec![Value::Bytes(vec![0x0a, 0xff])]);
    assert_eq!(one("0x0AF", &data), vec![text("0x0AF")]);
    assert_eq!(one("abc", &data), vec![text("abc")]);
}

#[test]
fn typed_lists_and_ranges_split_into_values() {
    let id = info("id", ColumnKind::Number);
    assert_eq!(parse_input(Op::InList, &[" 1, 2 ,,x y, "], &id), vec![num("1"), num("2"), text("x y")]);
    assert_eq!(parse_input(Op::NotInList, &[""], &id), Vec::<Value>::new());
    assert_eq!(parse_input(Op::Between, &["1", "9"], &id), vec![num("1"), num("9")]);
    assert_eq!(parse_input(Op::Between, &["1", ""], &id), vec![num("1")]);
    assert_eq!(parse_input(Op::Between, &["", "9"], &id), Vec::<Value>::new());
    assert_eq!(parse_input(Op::Between, &["1"], &id), vec![num("1")]);
}

#[test]
fn a_rows_values_show_as_the_field_holds_them() {
    let data = info("data", ColumnKind::Bytes);
    let f = info("f", ColumnKind::Other);
    let r = row("data", Op::InList, vec![Value::Bytes(vec![0xab, 1]), num("2"), text("a b"), Value::Null]);
    assert_eq!(r.input_text(PG, &data), "\\xab01, 2, a b, NULL");
    assert_eq!(r.input_text(MY, &data), "0xAB01, 2, a b, NULL");
    assert_eq!(r.value_texts(PG).len(), 4);
    assert_eq!(row("f", Op::Eq, vec![Value::Bool(true)]).input_text(PG, &f), "true");
    assert_eq!(Row::new("f", Op::Eq).input_text(PG, &f), "");
    assert!(!Row::new("f", Op::Eq).complete());
    assert!(Row::new("f", Op::IsNull).complete());
}

#[test]
fn replacing_a_columns_conditions_keeps_everything_else() {
    let t = target(PG);
    let sql = "id > 1 AND (id < 5 OR price = 2) AND name = 'a' AND id <> 3";
    let mut w = Where::from_text(sql, &t);
    let at = w.replace_column(row("id", Op::Eq, vec![num("7")]), &t);
    assert_eq!(at, 0);
    // The expression naming the column stays: it is not taken apart.
    assert_eq!(w.text(), "id = 7 AND (id < 5 OR price = 2) AND name = 'a'");
    let at = w.replace_column(row("born", Op::IsNull, vec![]), &t);
    assert_eq!(at, 3);
    assert_eq!(w.text(), "id = 7 AND (id < 5 OR price = 2) AND name = 'a' AND born IS NULL");
    // An incomplete row of the column goes too.
    w.add(Row::new("name", Op::Contains), &t);
    w.replace_column(row("name", Op::InList, vec![text("b"), text("c")]), &t);
    assert_eq!(w.rows_of("name"), vec![2]);
    assert!(!w.has_incomplete());
    assert_eq!(w.text(), "id = 7 AND (id < 5 OR price = 2) AND name IN ('b', 'c') AND born IS NULL");
}

#[test]
fn clearing_takes_every_term_out() {
    let t = target(MY);
    let mut w = Where::from_text("id = 1 AND name = 'a'", &t);
    w.clear();
    assert!(w.terms().is_empty());
    assert_eq!(w.text(), "");
    assert!(w.chips().is_empty());
}

#[test]
fn the_sql_field_read_back_keeps_the_text_unless_it_changed() {
    let t = target(PG);
    let mut w = Where::from_text("id = 1", &t);
    w.add(row("name", Op::Eq, vec![text("a")]), &t);
    let shown = w.text();
    let before = w.clone();
    w.set_text(&shown, &t);
    assert_eq!(w, before);
    // Typed in the field: the text given anew, sent as typed.
    let typed = "name = 'a'  AND id IN (1,2)";
    w.set_text(typed, &t);
    assert!(!w.rebuilt());
    assert_eq!(w.original(), typed);
    assert_eq!(w.text(), typed);
    assert_eq!(chip_texts(&w), vec![("name = 'a'", false), ("id IN (1,2)", false)]);
    w.set_text("", &t);
    assert!(w.terms().is_empty());
}

#[test]
fn a_draft_is_the_query_it_was_opened_on() {
    let t = target(PG);
    let mut q = filtered("id > 1");
    q.order = vec![
        OrderKey::user("name", Dir::Desc),
        OrderKey { origin: KeyOrigin::TieBreak, ..OrderKey::user("id", Dir::Asc) },
    ];
    q.limit = Some(50);
    q.offset = Some(3);
    let draft = Draft::from_query(&q, &t);
    assert_eq!(draft.order, vec![OrderKey::user("name", Dir::Desc)]);
    assert_eq!(draft.limit, "50");
    let back = draft.to_query(&users(), &t).unwrap();
    assert_eq!(back.source, users().source);
    assert_eq!(back.order, vec![OrderKey::user("name", Dir::Desc)]);
    assert_eq!(back.limit, Some(50));
    assert_eq!(back.offset, None);
    let f = back.filter.unwrap();
    assert_eq!(f.text, "id > 1");
    assert_eq!(f.recognised, query::recognise("id > 1", &t));
    // The base's table and offset are kept.
    let other =
        Query { offset: Some(9), ..Query::table(TableName { database: None, schema: "s".into(), name: "o".into() }) };
    let q2 = draft.to_query(&other, &t).unwrap();
    assert_eq!((q2.source, q2.offset), (other.source, Some(9)));
}

#[test]
fn a_limit_that_is_no_cap_is_refused_with_where() {
    let t = target(MY);
    let mut draft = Draft::from_query(&users(), &t);
    assert_eq!(draft.limit, "");
    assert_eq!(draft.to_query(&users(), &t).unwrap().limit, None);
    for (text, problem, at) in [
        (" 1x ", LimitProblem::NotANumber, 1..3),
        ("0", LimitProblem::Zero, 0..1),
        ("99999999999999999999", LimitProblem::TooLarge, 0..20),
    ] {
        draft.limit = text.into();
        assert_eq!(draft.to_query(&users(), &t), Err((at, problem)), "{text}");
    }
    draft.limit = " 20 ".into();
    assert_eq!(draft.to_query(&users(), &t).unwrap().limit, Some(20));
}

#[test]
fn drafts_are_the_same_when_they_send_the_same_query() {
    let t = target(PG);
    let a = Draft::from_query(&filtered("id = 1"), &t);
    let mut b = a.clone();
    assert!(a.same_as(&b));
    b.limit = " ".into();
    assert!(a.same_as(&b));
    b.limit = "5".into();
    assert!(!a.same_as(&b));
    let mut c = a.clone();
    c.limit = " 5".into();
    assert!(b.same_as(&c));
    c.limit = "x".into();
    assert!(!b.same_as(&c));
    let mut d = a.clone();
    d.set_sort("id", Some(Dir::Asc));
    assert!(!a.same_as(&d));
    // A term added then taken out: the same text is sent.
    let mut e = a.clone();
    let i = e.where_.add(row("id", Op::Gt, vec![num("1")]), &t);
    assert!(!a.same_as(&e));
    e.where_.remove(i);
    assert!(a.same_as(&e));
    // An incomplete row sends nothing more.
    e.where_.add(Row::new("name", Op::Eq), &t);
    assert!(a.same_as(&e));
    assert!(!a.same_as(&Draft::from_query(&filtered("id =  1"), &t)));
}

#[test]
fn sort_keys_are_set_moved_and_taken_out() {
    let t = target(PG);
    let mut d = Draft::from_query(&users(), &t);
    assert_eq!(d.sort_of("id"), None);
    d.set_sort("id", Some(Dir::Asc));
    d.set_sort("name", Some(Dir::Desc));
    d.set_sort("price", Some(Dir::Asc));
    assert_eq!(d.sort_of("name"), Some((Dir::Desc, 1)));
    d.set_sort("id", Some(Dir::Desc));
    assert_eq!(d.sort_of("id"), Some((Dir::Desc, 0)));
    assert!(d.move_key("price", -1));
    assert!(d.move_key("price", -5));
    assert!(!d.move_key("price", -1));
    assert!(!d.move_key("nope", 1));
    let cols = |d: &Draft| d.order.iter().map(|k| k.col.clone()).collect::<Vec<_>>();
    assert_eq!(cols(&d), ["price", "id", "name"]);
    assert!(d.move_key("price", 9));
    assert_eq!(cols(&d), ["id", "name", "price"]);
    d.set_sort("name", None);
    d.set_sort("nope", None);
    assert_eq!(cols(&d), ["id", "price"]);
    assert_eq!(d.sort_of("price"), Some((Dir::Asc, 1)));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Value,
    Limit,
}

fn limited(n: usize) -> Draft {
    Draft { limit: n.to_string(), ..Draft::from_query(&users(), &target(PG)) }
}

#[test]
fn undo_keeps_the_last_hundred_steps() {
    let mut u: Undo<Field> = Undo::default();
    assert!(u.is_empty());
    for n in 0..150 {
        u.record(&limited(n), None);
    }
    assert_eq!(u.len(), UNDO_STEPS);
    assert_eq!(u.undo().unwrap().limit, "149");
    let mut last = None;
    while let Some(d) = u.undo() {
        last = Some(d);
    }
    assert_eq!(last.unwrap().limit, "50");
    assert!(u.is_empty());
}

#[test]
fn typing_in_one_field_is_one_step_until_it_is_left() {
    let mut u = Undo::default();
    u.record(&limited(1), Some(Field::Value));
    u.record(&limited(2), Some(Field::Value));
    u.record(&limited(3), Some(Field::Value));
    assert_eq!(u.len(), 1);
    // Another field is another step.
    u.record(&limited(4), Some(Field::Limit));
    assert_eq!(u.len(), 2);
    u.leave_field();
    u.record(&limited(5), Some(Field::Limit));
    assert_eq!(u.len(), 3);
    // Any other change is a step and ends the typing.
    u.record(&limited(6), None);
    u.record(&limited(7), Some(Field::Limit));
    assert_eq!(u.len(), 5);
    assert_eq!(u.undo().unwrap().limit, "7");
    // After an undo, typing again is a new step.
    u.record(&limited(8), Some(Field::Limit));
    assert_eq!(u.len(), 5);
    let steps: Vec<String> = std::iter::from_fn(|| u.undo()).map(|d| d.limit).collect();
    assert_eq!(steps, ["8", "6", "5", "4", "1"]);
}

fn offered(c: &ColumnInfo, t: &Target) -> Vec<(Op, Result<(), OpOff>)> {
    ops_for(c, t)
}

#[test]
fn operators_follow_what_the_column_compares_by() {
    let t = target(PG);
    // A number held: everything but patterns.
    let id = info("id", ColumnKind::Number).with_meta(&meta("id", "int4", ValueKind::Integer));
    let ops = offered(&id, &t);
    assert_eq!(ops.iter().map(|(op, _)| *op).collect::<Vec<_>>(), Op::ALL.to_vec());
    for (op, off) in &ops {
        let want = if op.pattern() { Err(OpOff::NotText) } else { Ok(()) };
        assert_eq!(*off, want, "{op:?}");
    }
    // Text held: no `BETWEEN`, patterns as the collation lets them be written.
    let name = ColumnInfo::from_meta(&meta("name", "text", ValueKind::Text));
    assert_eq!(name.kind, ColumnKind::Text);
    let ops = offered(&name, &t);
    assert!(!ops.iter().any(|(op, _)| *op == Op::Between));
    assert!(ops.iter().all(|(_, off)| off.is_ok()), "{ops:?}");
    let email = ColumnInfo::from_meta(&meta("email", "text", ValueKind::Text));
    let off = |ops: &[(Op, Result<(), OpOff>)], op| ops.iter().find(|(o, _)| *o == op).map(|(_, off)| *off);
    let ops = offered(&email, &t);
    assert_eq!(off(&ops, Op::Contains), Some(Err(OpOff::CollationUnknown)));
    assert_eq!(off(&ops, Op::StartsWith), Some(Err(OpOff::LikeUnknown)));
    let mut nd = target(PG);
    nd.columns[2].deterministic = Some(false);
    nd.like_nondeterministic = Some(false);
    let ops = offered(&email, &nd);
    assert_eq!(off(&ops, Op::Contains), Some(Err(OpOff::CaseInsensitiveRefused)));
    assert_eq!(off(&ops, Op::StartsWith), Some(Err(OpOff::LikeRefused)));
    // Equality only: no order.
    let doc = ColumnInfo::from_meta(&meta("doc", "jsonb", ValueKind::Json));
    let ops = offered(&doc, &t);
    for op in [Op::Lt, Op::Le, Op::Gt, Op::Ge] {
        assert_eq!(off(&ops, op), Some(Err(OpOff::NoOrder)), "{op:?}");
    }
    assert_eq!(off(&ops, Op::Eq), Some(Ok(())));
    assert_eq!(off(&ops, Op::Contains), Some(Err(OpOff::NotText)));
    // No equality: only the tests of NULL.
    let json = ColumnInfo::from_meta(&meta("doc", "json", ValueKind::Json));
    for (op, off) in offered(&json, &target(MY)) {
        let want = if op.arity() == Arity::Zero { Ok(()) } else { Err(OpOff::NoEquality) };
        assert_eq!(off, want, "{op:?}");
    }
    // A PostgreSQL enum orders by its labels.
    let mut mood = ColumnInfo::from_meta(&meta("mood", "mood", ValueKind::Other));
    assert_eq!(off(&offered(&mood, &t), Op::Eq), Some(Err(OpOff::NoEquality)));
    mood.enum_known = true;
    assert_eq!(off(&offered(&mood, &t), Op::Lt), Some(Ok(())));
    // Dates and times are offered `BETWEEN`.
    let at = ColumnInfo::from_meta(&meta("at", "timestamptz", ValueKind::TimestampTz));
    assert_eq!(off(&offered(&at, &t), Op::Between), Some(Ok(())));
}

#[test]
fn operators_of_a_column_whose_type_is_not_known_yet_are_not_dimmed() {
    let t = target(PG);
    let off = |c: &ColumnInfo, op| ops_for(c, &t).into_iter().find(|(o, _)| *o == op).map(|(_, off)| off);
    let cat = |name: &str, ty: &str| {
        let c = CatalogColumn {
            name: name.into(),
            type_name: ty.into(),
            not_null: true,
            primary_key: name == "id",
            foreign_key: false,
            invisible: false,
        };
        ColumnInfo::from_catalogue(PG, &c)
    };
    let doc = cat("doc", "json");
    assert_eq!(doc.kind, ColumnKind::Other);
    assert!(doc.not_null && !doc.primary_key && !doc.foreign_key && !doc.invisible);
    // Unknown is not absent: the server decides.
    assert_eq!(off(&doc, Op::Lt), Some(Ok(())));
    assert_eq!(off(&doc, Op::Contains), Some(Err(OpOff::TypeUnknown)));
    assert_eq!(off(&doc, Op::Between), None);
    let id = cat("id", "bigint");
    assert!(id.primary_key);
    assert_eq!(off(&id, Op::Between), Some(Ok(())));
    assert_eq!(off(&id, Op::StartsWith), Some(Err(OpOff::NotText)));
    let name = cat("name", "character varying(20)");
    assert_eq!(off(&name, Op::Contains), Some(Ok(())));
    let born = cat("born", "timestamp(3) without time zone");
    assert_eq!(off(&born, Op::Between), Some(Ok(())));
    // A number typed before rows are held is written bare.
    let mut w = Where::from_text("", &t);
    w.add(Row { values: parse_input(Op::Gt, &["10"], &id), ..Row::new("id", Op::Gt) }, &t);
    assert_eq!(w.text(), "id > 10");
}

#[test]
fn catalogue_types_say_how_literals_are_written() {
    for (d, ty, kind) in [
        (PG, "integer", ColumnKind::Number),
        (PG, "numeric(10,2)", ColumnKind::Number),
        (PG, "double precision", ColumnKind::Number),
        (PG, "money", ColumnKind::Other),
        (PG, "character varying(20)", ColumnKind::Text),
        (PG, "character(1)", ColumnKind::Text),
        (PG, "text", ColumnKind::Text),
        (PG, "\"char\"", ColumnKind::Other),
        (PG, "text[]", ColumnKind::Other),
        (PG, "integer[]", ColumnKind::Other),
        (PG, "bytea", ColumnKind::Bytes),
        (PG, "jsonb", ColumnKind::Other),
        (PG, "my_domain", ColumnKind::Other),
        (MY, "int(11) unsigned zerofill", ColumnKind::Number),
        (MY, "tinyint(1)", ColumnKind::Number),
        (MY, "decimal(10,2)", ColumnKind::Number),
        (MY, "year", ColumnKind::Number),
        (MY, "varchar(255)", ColumnKind::Text),
        (MY, "enum('a','b (c)')", ColumnKind::Text),
        (MY, "longtext", ColumnKind::Text),
        (MY, "varbinary(16)", ColumnKind::Bytes),
        (MY, "blob", ColumnKind::Bytes),
        (MY, "bit(3)", ColumnKind::Other),
        (MY, "json", ColumnKind::Other),
        (MY, "datetime(3)", ColumnKind::Other),
    ] {
        assert_eq!(kind_of_type(d, ty), kind, "{d:?} {ty}");
    }
    assert!(temporal_type("timestamp(3) with time zone"));
    assert!(temporal_type("DATETIME(6)"));
    assert!(!temporal_type("text"));
}

#[test]
fn word_operators_show_their_sql() {
    assert_eq!(Op::Between.sql_hint(PG), Some("BETWEEN a AND b"));
    assert_eq!(Op::Contains.sql_hint(PG), Some("ILIKE '%…%'"));
    assert_eq!(Op::Contains.sql_hint(MY), Some("LIKE '%…%'"));
    assert_eq!(Op::NeOrNull.sql_hint(MY), Some("NOT (… <=> …)"));
    assert_eq!(Op::InList.sql_hint(MY), Some("IN (a, b, …)"));
    assert_eq!(Op::IsNotNull.sql_hint(PG), Some("IS NOT NULL"));
    for op in [Op::Eq, Op::Ne, Op::Lt, Op::Le, Op::Gt, Op::Ge] {
        assert_eq!(op.sql_hint(PG), None);
        assert_eq!(op.arity(), Arity::One);
    }
    assert_eq!(Op::Between.arity(), Arity::Two);
    assert_eq!(Op::NotInList.arity(), Arity::Many);
    assert_eq!(Op::IsNull.arity(), Arity::Zero);
}

#[test]
fn a_list_field_reads_back_to_the_values_it_shows() {
    let name = info("name", ColumnKind::Text);
    let id = info("id", ColumnKind::Number);
    let data = info("data", ColumnKind::Bytes);
    let cases: Vec<(&ColumnInfo, Vec<Value>, &str)> = vec![
        (&name, vec![text("paid"), Value::Null], "paid, NULL"),
        (&name, vec![text("NULL"), text("null")], "'NULL', 'null'"),
        (&name, vec![text("a,b"), text("x")], "'a,b', x"),
        (&name, vec![text("a\nb"), text("c\rd"), text("x")], "'a\\nb', 'c\\rd', x"),
        (&name, vec![text("a\\nb"), text("q\\'\n")], "a\\nb, 'q\\\\''\\n'"),
        (&name, vec![text(""), text(" a "), text("it's"), text("'q")], "'', ' a ', it's, '''q'"),
        (&id, vec![num("1"), text("12"), text("x y")], "1, '12', x y"),
        (&data, vec![Value::Bytes(vec![1]), text("0x01")], "\\x01, '0x01'"),
    ];
    for (c, values, shown) in cases {
        let written = list_text(&values, PG, c);
        assert_eq!(written, shown);
        assert_eq!(parse_list(&written, c, &[]), values, "{shown}");
        assert_eq!(parse_list(&written, c, &list_kept(&values, PG, c)), values, "{shown}");
    }
    // A value added after the others leaves them as they were.
    let values = vec![text("a,b"), Value::Null, Value::Bool(true)];
    let kept = list_kept(&values, PG, &name);
    let more = format!("{}, z", list_text(&values, PG, &name));
    assert_eq!(parse_list(&more, &name, &kept), vec![text("a,b"), Value::Null, Value::Bool(true), text("z")]);
    // Typed: bare NULL in any case is NULL, a quote left open runs to the end, text after a
    // closing quote is the text as written, empty values are left out.
    assert_eq!(parse_list("Null, 'a, b", &name, &[]), vec![Value::Null, text("a, b")]);
    assert_eq!(parse_list("'a'b, , c ,", &name, &[]), vec![text("'a'b"), text("c")]);
    assert_eq!(list_items("x, 'y").last().map(|i| i.quoted.clone()), Some(Some("y".into())));
    // In quotes a backslash before anything else is itself.
    assert_eq!(parse_list("'C:\\temp\\x'", &name, &[]), vec![text("C:\\temp\\x")]);
}

#[test]
fn a_mysql_catalogue_type_reads_as_the_driver_describes_its_result_column() {
    for (ty, name, kind) in [
        ("int unsigned", "int unsigned", ValueKind::Integer),
        ("tinyint(1)", "tinyint(1)", ValueKind::Integer),
        ("bigint", "bigint", ValueKind::Integer),
        ("decimal(10,2)", "decimal", ValueKind::Decimal),
        ("double", "double", ValueKind::Float),
        ("varchar(20)", "varchar", ValueKind::Text),
        ("enum('a','b')", "enum", ValueKind::Text),
        ("varbinary(16)", "varbinary", ValueKind::Bytes),
        ("datetime(3)", "datetime", ValueKind::Timestamp),
        ("timestamp", "timestamp", ValueKind::TimestampTz),
        ("time", "time", ValueKind::Interval),
        ("bit(1)", "bit", ValueKind::Bit),
        ("json", "json", ValueKind::Json),
        ("point", "geometry", ValueKind::Bytes),
        ("vector(3)", "vector", ValueKind::Bytes),
        ("something new", "something new", ValueKind::Other),
    ] {
        let m = catalogue_meta(MY, "c", ty).expect("MySQL");
        assert_eq!((m.type_name.as_str(), m.kind), (name, kind), "{ty}");
    }
    assert!(catalogue_meta(PG, "c", "integer").is_none());
    // Only a column no result describes is counted by its catalogue type.
    let mut c = info("c", ColumnKind::Number);
    c.type_name = "int".into();
    assert!(c.count_meta(MY).is_none());
    c.invisible = true;
    assert_eq!(c.count_meta(MY).map(|m| m.kind), Some(ValueKind::Integer));
}

#[test]
fn editing_a_long_list_reads_it_in_one_pass() {
    // 4000 values, as many as the field holds (`IN ('x', 'x', ...)`), the last one edited.
    let name = info("name", ColumnKind::Text);
    let values: Vec<Value> = (0..4000).map(|_| text("x")).collect();
    let kept = list_kept(&values, PG, &name);
    let edited = format!("{}y", list_text(&values, PG, &name));
    LIST_PROBES.with(|p| p.set(0));
    let read = parse_list(&edited, &name, &kept);
    let probes = LIST_PROBES.with(|p| p.get());
    assert_eq!(read.len(), 4000);
    assert_eq!(read.last(), Some(&text("xy")));
    assert!(read[..3999].iter().all(|v| *v == text("x")));
    // A look per value of the field, not a search through every value held for each.
    assert!(probes <= 2 * 4000, "{probes}");
}

#[test]
fn a_term_is_followed_to_its_place_after_the_terms_change() {
    let t = target(PG);
    let before = Where::from_text("id > 1 AND id < 10 AND price = 3", &t);
    // One taken out: those before it stay, those after it move up, it is gone.
    let mut w = before.clone();
    w.remove(1);
    assert_eq!(follow(before.terms(), w.terms(), 0), Moved::To(0));
    assert_eq!(follow(before.terms(), w.terms(), 1), Moved::Gone(1));
    assert_eq!(follow(before.terms(), w.terms(), 2), Moved::To(1));
    // Put back (undone): those after it move down again.
    assert_eq!(follow(w.terms(), before.terms(), 1), Moved::To(2));
    // One changed in its place keeps it; one added after the others moves nothing.
    let mut w = before.clone();
    w.set(1, row("id", Op::Lt, vec![num("20")]), &t);
    assert_eq!(follow(before.terms(), w.terms(), 1), Moved::To(1));
    let mut w = before.clone();
    w.add(row("name", Op::IsNull, Vec::new()), &t);
    assert_eq!(follow(before.terms(), w.terms(), 2), Moved::To(2));
    // All taken out: every term is gone.
    let mut w = before.clone();
    w.clear();
    assert_eq!(follow(before.terms(), w.terms(), 2), Moved::Gone(0));
}

#[test]
fn a_field_of_one_value_reads_back_to_the_value_it_shows() {
    let name = info("name", ColumnKind::Text);
    let id = info("id", ColumnKind::Number);
    for (col, op, v, shown) in [
        (&name, Op::Eq, text("a\nb"), "'a\\nb'"),
        (&name, Op::Lt, text("c\rd"), "'c\\rd'"),
        (&name, Op::Contains, text("x\\y\n"), "'x\\\\y\\n'"),
        (&name, Op::Eq, text("'q'"), "'''q'''"),
        (&name, Op::Eq, text(""), "''"),
        (&name, Op::Eq, text("it's"), "it's"),
        (&name, Op::Eq, text(" a "), " a "),
        (&id, Op::Eq, text("12"), "'12'"),
        (&id, Op::Eq, num("12"), "12"),
    ] {
        assert_eq!(field_text(&v, op, PG, col), shown, "{v:?}");
        assert_eq!(parse_input(op, &[shown], col), vec![v.clone()], "{shown}");
    }
    // A quote left open runs to the end; text after the closing quote is the text as written.
    assert_eq!(parse_input(Op::Eq, &["'a\\nb"], &name), vec![text("a\nb")]);
    assert_eq!(parse_input(Op::Eq, &["'a'b"], &name), vec![text("'a'b")]);
    // The list narrows by the value typed in quotes.
    assert_eq!(crate::app::filter_dialog::list::fragment(Op::Eq, "'a\\nb'"), Some("a\nb".to_string()));
}
