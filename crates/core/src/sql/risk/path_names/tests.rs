use super::path_names;

fn names(sql: &str) -> Vec<String> {
    path_names(sql).unwrap_or_else(|| panic!("the parser rejects {sql}"))
}

#[test]
fn qualified_names_are_not_left_to_the_path() {
    let sql = "SELECT pg_catalog.count(*) FROM pg_catalog.pg_class c \
               WHERE c.relpages OPERATOR(pg_catalog.>=) 0::pg_catalog.int4 \
               AND c.relkind OPERATOR(pg_catalog.=) ANY (ARRAY['r', 'p']::pg_catalog.\"char\"[]) \
               AND c.oid OPERATOR(pg_catalog.<>) ALL (SELECT 0::pg_catalog.oid) \
               AND c.relname COLLATE pg_catalog.\"C\" OPERATOR(pg_catalog.~~) 'a%' \
               AND CASE WHEN c.relpages OPERATOR(pg_catalog.>) 1 THEN true END \
               AND position('a' IN c.relname::pg_catalog.text) OPERATOR(pg_catalog.>) 0 \
               AND c.reltuples::int OPERATOR(pg_catalog.>) 0 AND coalesce(true, false) IS NOT NULL \
               ORDER BY c.relname USING OPERATOR(pg_catalog.<)";
    assert_eq!(names(sql), Vec::<String>::new());
}

#[test]
fn every_kind_left_to_the_path_is_listed() {
    let cases = [
        ("SELECT 1 >= 0", "operator >="),
        ("SELECT count(*) FROM t", "function count"),
        ("SELECT 'a'::text", "type text"),
        ("SELECT 1::oid", "type oid"),
        ("SELECT 'a' COLLATE \"C\"", "collation C"),
        ("SELECT 1 IN (1, 2)", "operator ="),
        ("SELECT 1 NOT IN (1, 2)", "operator <>"),
        ("SELECT 1 BETWEEN 0 AND 2", "operator BETWEEN"),
        ("SELECT 1 IS DISTINCT FROM 2", "operator ="),
        ("SELECT NULLIF(1, 2)", "operator ="),
        ("SELECT 'a' LIKE 'b'", "operator ~~"),
        ("SELECT 'a' NOT LIKE 'b'", "operator !~~"),
        ("SELECT 'a' ILIKE 'b'", "operator ~~*"),
        ("SELECT 1 IN (SELECT 1)", "operator = (IN subquery)"),
        ("SELECT 1 NOT IN (SELECT 1)", "operator = (IN subquery)"),
        ("SELECT 1 = ANY (ARRAY[1])", "operator ="),
        ("SELECT CASE 1 WHEN 1 THEN 2 END", "operator = (CASE x WHEN)"),
        ("SELECT 'r:' || 1::pg_catalog.oid", "operator ||"),
        ("SELECT pg_catalog.count(*) WHERE (1, 2) > (0, 0)", "operator >"),
        ("SELECT x FROM t ORDER BY lower(x)", "function lower"),
        ("SELECT x FROM t ORDER BY x USING <", "operator <"),
        ("SELECT 1 FROM a JOIN b USING (id)", "operator = (JOIN USING)"),
        ("SELECT 1 FROM a NATURAL JOIN b", "operator = (JOIN USING)"),
        ("SELECT (1, 2) = (SELECT 1, 2)", "operator ="),
        ("CREATE TABLE t (a text)", "type text"),
        ("CREATE FUNCTION f(x pg_catalog.int4) RETURNS text LANGUAGE sql AS 'SELECT 1'", "type text"),
        ("CREATE FUNCTION f(x text) RETURNS pg_catalog.int4 LANGUAGE sql AS 'SELECT 1'", "type text"),
        ("CREATE TABLE t OF mytype", "type mytype"),
        ("CREATE TABLE t (a pg_catalog.text COLLATE \"C\")", "collation C"),
        ("SELECT 1 FROM pg_catalog.pg_class TABLESAMPLE sampler(1)", "function sampler"),
    ];
    for (sql, name) in cases {
        assert!(names(sql).contains(&name.to_string()), "{sql}: {:?}", names(sql));
    }
}

#[test]
fn every_statement_of_a_text_is_read() {
    assert_eq!(names("SELECT pg_catalog.now(); SELECT now()"), ["function now"]);
}

#[test]
fn relations_without_a_schema_are_listed_unless_they_are_with_queries() {
    let sql = "WITH RECURSIVE w AS (SELECT 1 FROM w) SELECT 1 FROM pg_catalog.pg_class, pg_class, w, t";
    assert_eq!(super::bare_relations(sql), Some(vec!["pg_class".to_string(), "t".to_string()]));
    assert_eq!(super::bare_relations("SELEC"), None);
}

#[test]
fn a_text_the_parser_rejects_is_no_answer() {
    assert_eq!(path_names("SELEC 1"), None);
}
