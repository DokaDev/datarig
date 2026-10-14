use super::*;

fn refused(sql: &str) -> NotRepeatable {
    repeatable(sql).expect_err(sql)
}

#[test]
fn plain_queries_may_run_again() {
    for sql in [
        "SELECT * FROM shop.users",
        "select id, name from shop.users where id > 3 order by id",
        "SELECT u.name, count(*) FROM shop.users u JOIN shop.orders o ON o.user_id = u.id GROUP BY 1",
        "WITH t AS (SELECT 1 AS x) SELECT x FROM t",
        "VALUES (1), (2)",
        "TABLE shop.users",
        "SELECT now(), lower('A'), generate_series(1, 3)",
        "SELECT 1 UNION ALL SELECT 2",
        "SELECT * FROM shop.users;",
        "  SELECT 1 -- a comment\n",
    ] {
        assert_eq!(repeatable(sql), Ok(()), "{sql}");
    }
}

#[test]
fn anything_else_is_refused_with_its_reason() {
    assert_eq!(refused("SELECT 1; SELECT 2"), NotRepeatable::NotOne);
    assert_eq!(refused(""), NotRepeatable::NotOne);
    for sql in [
        "EXPLAIN SELECT 1",
        "SHOW search_path",
        "FETCH 10 FROM c",
        "EXECUTE p",
        "DELETE FROM t",
        "INSERT INTO t VALUES (1) RETURNING *",
        "CALL p()",
    ] {
        assert_eq!(refused(sql), NotRepeatable::NotSelect, "{sql}");
    }
    for sql in [
        "SELECT * FROM t FOR UPDATE",
        "SELECT * INTO zz_copy FROM t",
        "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d",
        "SELECT pg_read_file('/etc/hostname')",
        "SELECT set_config('default_transaction_read_only', 'off', false)",
    ] {
        assert_eq!(refused(sql), NotRepeatable::Writes, "{sql}");
    }
    assert_eq!(refused("SELECT myschema.f(1)"), NotRepeatable::UserFunction);
    assert_eq!(refused("SELECT zz_unknown_function()"), NotRepeatable::UserFunction);
    for (sql, f) in [
        ("SELECT nextval('s')", "nextval"),
        ("SELECT random() FROM t", "random"),
        ("SELECT pg_advisory_lock(1)", "pg_advisory_lock"),
        ("SELECT pg_catalog.clock_timestamp()", "clock_timestamp"),
        ("SELECT pg_notify('c', 'x')", "pg_notify"),
        ("SELECT id FROM t WHERE id IN (SELECT gen_random_uuid())", "gen_random_uuid"),
        ("SELECT ('s'::regclass).nextval", "nextval"),
        ("SELECT * FROM t TABLESAMPLE SYSTEM (10)", "TABLESAMPLE"),
    ] {
        assert_eq!(refused(sql), NotRepeatable::Volatile(f.to_string()), "{sql}");
    }
    assert_eq!(refused("SELEC 1"), NotRepeatable::Unreadable);
    assert_eq!(refused(&format!("SELECT {}", "1 + ".repeat(20_000) + "1")), NotRepeatable::Unreadable);
}

#[test]
fn the_volatile_list_is_sorted_and_knows_the_usual_ones() {
    let names: Vec<&str> = VOLATILE.lines().collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(names, sorted, "one name per line, in byte order");
    for f in ["nextval", "setval", "random", "pg_advisory_lock", "set_config", "pg_sleep"] {
        assert!(volatile(f), "{f}");
    }
    for f in ["now", "lower", "count", "generate_series", "upper"] {
        assert!(!volatile(f), "{f}");
    }
}

#[test]
fn the_count_wraps_the_statement_on_lines_of_its_own() {
    assert_eq!(
        count_query("SELECT * FROM shop.users;").as_deref(),
        Ok("SELECT count(*) FROM (\nSELECT * FROM shop.users\n) AS datarig_count")
    );
    // A trailing comment ends before the parenthesis; several `;` and blanks go.
    assert_eq!(
        count_query("SELECT 1 -- the end ; \n ; ;").as_deref(),
        Ok("SELECT count(*) FROM (\nSELECT 1 -- the end ;\n) AS datarig_count")
    );
    assert_eq!(count_query("SELECT nextval('s')"), Err(NotRepeatable::Volatile("nextval".into())));
    assert_eq!(count_query("SHOW all"), Err(NotRepeatable::NotSelect));
}

#[test]
fn ordered_reads_the_top_order_by_only() {
    assert!(ordered("SELECT * FROM t ORDER BY id"));
    assert!(!ordered("SELECT * FROM (SELECT * FROM t ORDER BY id) s"));
    assert!(!ordered("SELECT * FROM t"));
}

#[test]
fn only_builtin_operators_and_types_may_run_again() {
    for sql in [
        "SELECT a + 1, b || 'x', c ~~ 'y%', d IS DISTINCT FROM e FROM t",
        "SELECT a OPERATOR(pg_catalog.+) 1 FROM t WHERE b BETWEEN 1 AND 2 AND c NOT BETWEEN SYMMETRIC 3 AND 4",
        "SELECT x FROM t WHERE x = ANY (ARRAY[1, 2]) AND y < ALL (SELECT 1) ORDER BY x USING <",
        "SELECT '1'::int, CAST(x AS double precision), date '2020-01-01', x::text[], 'a'::varchar(3) FROM t",
        "SELECT '1'::pg_catalog.int8, 't'::regclass, now()::timestamptz",
        "SELECT * FROM json_to_record('{}') AS r(a int, b text)",
        "SELECT JSON_VALUE('1', '$' RETURNING int)",
    ] {
        assert_eq!(repeatable(sql), Ok(()), "{sql}");
    }
    for (sql, op) in [
        ("SELECT id #~# 0 FROM t", "#~#"),
        ("SELECT a OPERATOR(public.+) 1 FROM t", "public.+"),
        ("SELECT a OPERATOR(pg_catalog.#~#) 1 FROM t", "pg_catalog.#~#"),
        ("SELECT * FROM t ORDER BY a USING #<#", "#<#"),
        ("SELECT a FROM t WHERE a #=# ANY (SELECT 1)", "#=#"),
    ] {
        assert_eq!(refused(sql), NotRepeatable::UserOperator(op.to_string()), "{sql}");
    }
    for (sql, ty) in [
        ("SELECT x::zz_dom FROM t", "zz_dom"),
        ("SELECT CAST(x AS public.int4) FROM t", "public.int4"),
        ("SELECT zz_ct '(1)'", "zz_ct"),
        ("SELECT * FROM json_to_record('{}') AS r(c zz_dom)", "zz_dom"),
        ("SELECT x::zz_dom[] FROM t", "zz_dom"),
    ] {
        assert_eq!(refused(sql), NotRepeatable::UserType(ty.to_string()), "{sql}");
    }
    assert_eq!(refused("SELECT * FROM otherdb.public.t"), NotRepeatable::NotATable("otherdb.public.t".into()));
}

#[test]
fn the_names_are_what_the_server_is_asked_about() {
    let n = names(
        "WITH w AS (SELECT 1 AS id) SELECT abs(t.id), (t).f, x.y.z + 1, 'a'::text, '1'::pg_catalog.int8 \
         FROM s.t t, w, u WHERE t.id BETWEEN 1 AND 2 ORDER BY 1",
    )
    .unwrap();
    assert_eq!(n.relations, [("s".into(), "t".into()), (String::new(), "w".into()), (String::new(), "u".into())]);
    assert_eq!(n.ctes, ["w"]);
    assert_eq!(n.functions, ["abs"]);
    assert_eq!(n.attributes, ["id", "f", "y", "z"]);
    assert_eq!(n.operators, ["+", ">=", "<="]);
    assert_eq!(n.types, ["text"]);
    let q = check_query(&n).unwrap();
    assert!(q.contains("($datarig$$datarig$, $datarig$w$datarig$, true)"), "{q}");
    assert!(q.contains("($datarig$s$datarig$, $datarig$t$datarig$, false)"), "{q}");
    // Every operator of the query is pg_catalog's, whatever search_path says.
    for op in [" = ", " < ", " > ", " <> ", " >= ", " <= ", " || "] {
        assert!(!q.contains(op), "{op} in {q}");
    }
    assert!(q.contains("OPERATOR(pg_catalog.=) ANY") && q.contains("OPERATOR(pg_catalog.>=) 10000"));
    // A name holding the quote cannot be sent.
    let odd = names("SELECT * FROM \"a$datarig$b\"").unwrap();
    assert_eq!(check_query(&odd), None);
    assert_eq!(check_query(&names("SELECT 1").unwrap()).map(|q| q.contains("WHERE false")), Some(true));
    assert_eq!(NotRepeatable::from_check("relation", "v"), Some(NotRepeatable::NotATable("v".into())));
    assert_eq!(NotRepeatable::from_check("what", "v"), None);
}

#[test]
fn the_builtin_operator_and_type_lists_are_sorted() {
    for list in [OPERATORS, TYPES] {
        let names: Vec<&str> = list.lines().collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted, "one name per line, in byte order");
    }
    assert!(builtin_operator("=") && builtin_operator("||") && !builtin_operator("#~#"));
    assert!(builtin_type("int4") && builtin_type("text") && builtin_type("date") && !builtin_type("int"));
}
