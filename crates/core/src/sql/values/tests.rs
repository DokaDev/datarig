use super::*;
use crate::sql::dialect::MySqlMode;

fn check(expression: &str, columns: &[&str]) -> CheckConstraint {
    CheckConstraint {
        name: "c".into(),
        expression: expression.into(),
        columns: columns.iter().map(|c| c.to_string()).collect(),
        definition: format!("CHECK ({expression})"),
    }
}

fn pg(expression: &str, column: &str) -> Option<Vec<String>> {
    let found = check_values(Dialect::Postgres, &[check(expression, &[column])]);
    found.into_iter().find(|v| v.column == column).map(|v| v.values)
}

const MYSQL: Dialect =
    Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: false });

fn my(expression: &str, column: &str) -> Option<Vec<String>> {
    let found = check_values(MYSQL, &[check(expression, &[column])]);
    found.into_iter().find(|v| v.column == column).map(|v| v.values)
}

fn v(xs: &[&str]) -> Option<Vec<String>> {
    Some(xs.iter().map(|x| x.to_string()).collect())
}

/// What PostgreSQL 17's `pg_get_expr(conbin, conrelid, true)` writes for checks listing
/// values, and what older servers (and `pretty` off) write: casts and parentheses around every
/// part.
#[test]
fn postgres_deparsed_lists() {
    let status = "status = ANY (ARRAY['pending'::text, 'paid'::text, 'shipped'::text, 'cancelled'::text])";
    assert_eq!(pg(status, "status"), v(&["pending", "paid", "shipped", "cancelled"]));
    assert_eq!(pg(&format!("({status})"), "status"), v(&["pending", "paid", "shipped", "cancelled"]));
    // varchar: the column and the array are cast to text.
    let varchar = "a::text = ANY (ARRAY['x'::character varying, 'y''z'::character varying]::text[])";
    assert_eq!(pg(varchar, "a"), v(&["x", "y'z"]));
    let old = "((a)::text = ANY ((ARRAY['x'::character varying, 'y'::character varying])::text[]))";
    assert_eq!(pg(old, "a"), v(&["x", "y"]));
    assert_eq!(pg("c = 'p'::bpchar", "c"), v(&["p"]));
    assert_eq!(pg("b = 'k'::text OR b = 'm'::text", "b"), v(&["k", "m"]));
    assert_eq!(pg("((b = 'k'::text) OR (b = 'm'::text))", "b"), v(&["k", "m"]));
    assert_eq!(pg("g = ANY (ARRAY['a'::text, NULL::text])", "g"), v(&["a"]), "NULL left out");
    assert_eq!(pg("h = 'b\\c'::text", "h"), v(&["b\\c"]), "a backslash is a character");
    assert_eq!(pg("h = E'b\\\\c'::text", "h"), v(&["b\\c"]));
    assert_eq!(pg("\"Mixed\" = ANY (ARRAY['a'::text])", "Mixed"), v(&["a"]));
    assert_eq!(pg("t.k = 'a'::text", "k"), v(&["a"]));
    assert_eq!(pg("s IN ('a', 'b')", "s"), v(&["a", "b"]));
}

/// Conditions that do not list a column's values give nothing.
#[test]
fn other_conditions_give_nothing() {
    for (e, c) in [
        ("e <> 'n'::text", "e"),
        ("f = ANY (ARRAY[1, 2])", "f"),
        ("d = ANY ('{q,r}'::text[])", "d"),
        ("j = 'w'::text AND length(j) > 0", "j"),
        ("j = 'w'::text AND k", "j"),
        ("price >= 0::numeric", "price"),
        ("NOT (s = ANY (ARRAY['a'::text]))", "s"),
        ("s NOT IN ('a')", "s"),
        ("a = 'x'::text OR b = 'y'::text", "a"),
        ("lower(s) = 'a'::text", "s"),
        ("s = 'a'::text || 'b'::text", "s"),
        ("s = ANY (ARRAY['a'::text, upper('b'::text)])", "s"),
        ("s = ANY ARRAY['a']", "s"),
        ("s >= 'a'::text", "s"),
        ("s = B'101'", "s"),
        ("s = X'ff'", "s"),
        ("", "s"),
    ] {
        assert_eq!(pg(e, c), None, "{e}");
    }
    // A column the check does not read.
    assert_eq!(check_values(Dialect::Postgres, &[check("s = 'a'::text", &["other"])]), vec![]);
}

/// MySQL's `CHECK_CLAUSE` (as the driver unescapes it): backticks, charset introducers,
/// backslash escapes; MariaDB writes the strings bare.
#[test]
fn mysql_clauses() {
    assert_eq!(my("`status` in (_utf8mb4'a',_utf8mb4'b')", "status"), v(&["a", "b"]));
    assert_eq!(my("(`s` in (_latin1'a',_latin1'it\\'s',_latin1'b\\\\c'))", "s"), v(&["a", "it's", "b\\c"]));
    assert_eq!(my("((`u` = _latin1'k') or (`u` = _latin1'm'))", "u"), v(&["k", "m"]));
    assert_eq!(my("`Status` IN ('a','b')", "status"), v(&["a", "b"]), "a name in another case");
    assert_eq!(my("`s` in ('a',NULL)", "s"), v(&["a"]));
    assert_eq!(my("`s` in (1,2)", "s"), None);
    assert_eq!(my("`s` not in ('a')", "s"), None);
    // Whatever the session's mode: the server writes the condition in its default one.
    let ansi = Dialect::MySql(MySqlMode { ansi_quotes: true, no_backslash_escapes: true, dollar_quotes: true });
    let found = check_values(ansi, &[check("`s` in (_utf8mb4'it\\'s')", &["s"])]);
    assert_eq!(found, vec![CheckValues { column: "s".into(), values: vec!["it's".into()] }]);
}

/// Only enforced checks count; two checks on one column allow what both list, in the first's
/// order.
#[test]
fn checks_combine() {
    let mut not_enforced = check("`s` in ('x')", &["s"]);
    not_enforced.definition.push_str(CheckConstraint::MYSQL_NOT_ENFORCED);
    assert_eq!(check_values(MYSQL, &[not_enforced]), vec![]);
    let both = [check("s IN ('a', 'b', 'c')", &["s"]), check("s IN ('c', 'a')", &["s"]), check("t = 'z'", &["t"])];
    assert_eq!(
        check_values(Dialect::Postgres, &both),
        vec![
            CheckValues { column: "s".into(), values: vec!["a".into(), "c".into()] },
            CheckValues { column: "t".into(), values: vec!["z".into()] },
        ]
    );
}
