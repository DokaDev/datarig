use super::*;

fn ok(sql: &str) -> (String, bool) {
    let j = json(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    (j.sql, j.analyze)
}

fn no(sql: &str) -> NotJson {
    json(sql).expect_err(sql)
}

#[test]
fn a_plain_explain_asks_for_json() {
    assert_eq!(ok("EXPLAIN SELECT 1"), ("EXPLAIN (FORMAT JSON) SELECT 1".into(), false));
    assert_eq!(ok("explain select * from t;"), ("explain (FORMAT JSON) select * from t;".into(), false));
    // A parenthesis that opens the statement is not an option list.
    assert_eq!(ok("EXPLAIN (SELECT 1)"), ("EXPLAIN (FORMAT JSON) (SELECT 1)".into(), false));
    assert_eq!(ok("EXPLAIN ((SELECT 1))"), ("EXPLAIN (FORMAT JSON) ((SELECT 1))".into(), false));
    assert_eq!(ok("EXPLAIN (VALUES (1))"), ("EXPLAIN (FORMAT JSON) (VALUES (1))".into(), false));
}

#[test]
fn the_legacy_words_become_options() {
    assert_eq!(ok("EXPLAIN ANALYZE SELECT 1"), ("EXPLAIN (ANALYZE, FORMAT JSON) SELECT 1".into(), true));
    assert_eq!(
        ok("explain analyse verbose select 1"),
        ("explain (ANALYZE, VERBOSE, FORMAT JSON) select 1".into(), true)
    );
    assert_eq!(ok("EXPLAIN VERBOSE SELECT 1"), ("EXPLAIN (VERBOSE, FORMAT JSON) SELECT 1".into(), false));
    assert_eq!(
        ok("EXPLAIN ANALYZE DELETE FROM t WHERE a = 1"),
        ("EXPLAIN (ANALYZE, FORMAT JSON) DELETE FROM t WHERE a = 1".into(), true)
    );
}

#[test]
fn every_other_option_is_kept_as_written() {
    assert_eq!(
        ok("EXPLAIN (ANALYZE, BUFFERS, COSTS off, SETTINGS, WAL, TIMING false, SUMMARY) SELECT 1"),
        (
            "EXPLAIN (ANALYZE, BUFFERS, COSTS off, SETTINGS, WAL, TIMING false, SUMMARY, FORMAT JSON) SELECT 1".into(),
            true
        )
    );
    assert_eq!(ok("EXPLAIN (verbose) SELECT 1"), ("EXPLAIN (verbose, FORMAT JSON) SELECT 1".into(), false));
    assert_eq!(
        ok("EXPLAIN (GENERIC_PLAN) SELECT * FROM t WHERE a = $1"),
        ("EXPLAIN (GENERIC_PLAN, FORMAT JSON) SELECT * FROM t WHERE a = $1".into(), false)
    );
}

#[test]
fn a_format_is_replaced_by_json_where_it_is() {
    for (from, to) in [
        ("EXPLAIN (FORMAT TEXT) SELECT 1", "EXPLAIN (FORMAT JSON) SELECT 1"),
        ("EXPLAIN (format yaml, costs off) SELECT 1", "EXPLAIN (FORMAT JSON, costs off) SELECT 1"),
        ("EXPLAIN (VERBOSE, FORMAT XML) SELECT 1", "EXPLAIN (VERBOSE, FORMAT JSON) SELECT 1"),
        ("EXPLAIN (\"format\" 'yaml') SELECT 1", "EXPLAIN (FORMAT JSON) SELECT 1"),
        // The last one counts on the server: each one is JSON now.
        ("EXPLAIN (FORMAT JSON, FORMAT TEXT) SELECT 1", "EXPLAIN (FORMAT JSON, FORMAT JSON) SELECT 1"),
    ] {
        assert_eq!(ok(from), (to.to_string(), false), "{from}");
    }
}

#[test]
fn comments_case_and_quoting_are_read_as_the_server_reads_them() {
    assert_eq!(
        ok("-- why\n/* a */ Explain /* b */ (Analyze /* c */, Format Text) -- d\nSELECT ';' AS \"(x)\""),
        ("-- why\n/* a */ Explain /* b */ (Analyze /* c */, FORMAT JSON) -- d\nSELECT ';' AS \"(x)\"".into(), true)
    );
    // A quoted option name: the parser's reading counts too.
    assert_eq!(ok("EXPLAIN (\"analyze\") SELECT 1"), ("EXPLAIN (\"analyze\", FORMAT JSON) SELECT 1".into(), true));
    // The statement's own `;` in a string or a dollar body is not a second statement.
    assert_eq!(ok("EXPLAIN SELECT $$a;b$$, 'c;d'"), ("EXPLAIN (FORMAT JSON) SELECT $$a;b$$, 'c;d'".into(), false));
}

#[test]
fn analyze_is_found_in_every_form() {
    for sql in [
        "EXPLAIN ANALYZE SELECT 1",
        "EXPLAIN ANALYSE SELECT 1",
        "EXPLAIN (ANALYZE) SELECT 1",
        "EXPLAIN (analyze true) SELECT 1",
        "EXPLAIN (ANALYZE on) SELECT 1",
        "EXPLAIN (ANALYZE 1) SELECT 1",
        "EXPLAIN (ANALYZE 'true') SELECT 1",
        "EXPLAIN (ANALYZE false, ANALYZE) SELECT 1",
        "EXPLAIN (\"analyse\") SELECT 1",
        // The server takes the last one; the safety check counts any, and asks rather than not.
        "EXPLAIN (ANALYZE, ANALYZE off) SELECT 1",
    ] {
        assert!(ok(sql).1, "{sql}");
    }
    for sql in [
        "EXPLAIN SELECT 1",
        "EXPLAIN VERBOSE SELECT 1",
        "EXPLAIN (ANALYZE false) SELECT 1",
        "EXPLAIN (ANALYZE off) SELECT 1",
        "EXPLAIN (ANALYZE 0) SELECT 1",
        "EXPLAIN (COSTS) SELECT 1",
    ] {
        assert!(!ok(sql).1, "{sql}");
    }
}

#[test]
fn what_cannot_be_asked_again_is_refused() {
    assert_eq!(no("SELECT 1"), NotJson::NotExplain);
    assert_eq!(no(""), NotJson::NotExplain);
    assert_eq!(no("-- EXPLAIN SELECT 1"), NotJson::NotExplain);
    assert_eq!(no("\"explain\" SELECT 1"), NotJson::NotExplain);
    assert_eq!(no("EXPLAIN SELECT 1; SELECT 2"), NotJson::Several);
    assert_eq!(no("EXPLAIN (FORMAT JSON) SELECT 1"), NotJson::AlreadyJson);
    assert_eq!(no("EXPLAIN (format 'json', analyze) SELECT 1"), NotJson::AlreadyJson);
    assert_eq!(no("EXPLAIN (FORMAT TEXT, FORMAT \"json\") SELECT 1"), NotJson::AlreadyJson);
    for sql in [
        "EXPLAIN",
        "EXPLAIN;",
        "EXPLAIN ANALYZE",
        "EXPLAIN () SELECT 1",
        "EXPLAIN (ANALYZE,) SELECT 1",
        "EXPLAIN (ANALYZE SELECT 1",
        "EXPLAIN (ANALYZE)",
        "EXPLAIN (ANALYZE);",
        // The parser does not read it as an EXPLAIN.
        "EXPLAIN VERBOSE ANALYZE SELECT 1",
        "EXPLAIN SELEC 1",
    ] {
        assert_eq!(no(sql), NotJson::Unreadable, "{sql}");
    }
}

#[test]
fn the_lexer_alone_says_the_same_for_the_hint() {
    assert_eq!(json_text("EXPLAIN SELECT 1").map(|j| j.sql), Ok("EXPLAIN (FORMAT JSON) SELECT 1".into()));
    assert_eq!(json_text("SELECT 1"), Err(NotJson::NotExplain));
    assert_eq!(json_text("EXPLAIN (FORMAT JSON) SELECT 1"), Err(NotJson::AlreadyJson));
}
