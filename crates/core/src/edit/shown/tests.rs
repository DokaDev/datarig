use super::*;
use crate::sql::dialect::MySqlMode;

fn col(not_null: bool, default: Option<&str>, fill: ColumnFill) -> StructureColumn {
    StructureColumn {
        name: "c".into(),
        type_name: "text".into(),
        not_null,
        default: default.map(str::to_string),
        fill,
        enum_labels: Vec::new(),
        collation: None,
    }
}

const PG: Dialect = Dialect::Postgres;
const MY: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: false });

fn shown(d: Dialect, not_null: bool, default: Option<&str>) -> DefaultShown {
    default_shown(&col(not_null, default, ColumnFill::Default), d)
}

fn value(v: &str) -> DefaultShown {
    DefaultShown::Value(v.to_string())
}

fn expr(v: &str) -> DefaultShown {
    DefaultShown::Expression(v.to_string())
}

/// PostgreSQL prints a literal with its cast, a negative number in parentheses, a serial's
/// default as a call of `nextval`.
#[test]
fn postgres_defaults_show_their_value_or_expression() {
    use DefaultShown::*;
    assert_eq!(shown(PG, true, Some("'ACTIVE'::character varying")), value("ACTIVE"));
    assert_eq!(shown(PG, true, Some("'it''s'::text")), value("it's"));
    assert_eq!(shown(PG, false, Some("'{}'::jsonb")), value("{}"));
    assert_eq!(shown(PG, false, Some("'{a,b}'::text[]")), value("{a,b}"));
    assert_eq!(shown(PG, false, Some("'2020-01-01'::date")), value("2020-01-01"));
    assert_eq!(shown(PG, true, Some("0")), value("0"));
    assert_eq!(shown(PG, true, Some("1.5")), value("1.5"));
    assert_eq!(shown(PG, true, Some("(-1)")), value("-1"));
    assert_eq!(shown(PG, true, Some("'-1'::integer")), value("-1"));
    assert_eq!(shown(PG, true, Some("true")), value("true"));
    assert_eq!(shown(PG, false, Some("NULL::character varying")), Null);
    assert_eq!(shown(PG, true, Some("now()")), expr("now()"));
    assert_eq!(shown(PG, true, Some("CURRENT_TIMESTAMP")), expr("CURRENT_TIMESTAMP"));
    assert_eq!(shown(PG, true, Some("('a'::text || 'b'::text)")), expr("'a'::text || 'b'::text"));
    assert_eq!(shown(PG, true, Some("gen_random_uuid()")), expr("gen_random_uuid()"));
    assert_eq!(shown(PG, true, Some("nextval('shop.users_id_seq'::regclass)")), Generated);
    assert_eq!(shown(PG, true, Some("NEXTVAL('s'::regclass)")), Generated);
}

/// The MySQL driver writes a literal quoted, a number bare, an expression in parentheses and
/// what an `UPDATE` sets after it.
#[test]
fn mysql_defaults_show_their_value_or_expression() {
    use DefaultShown::*;
    assert_eq!(shown(MY, true, Some("'ACTIVE'")), value("ACTIVE"));
    assert_eq!(shown(MY, true, Some("'it\\'s'")), value("it's"));
    assert_eq!(shown(MY, true, Some("0")), value("0"));
    assert_eq!(shown(MY, false, Some("1.50")), value("1.50"));
    assert_eq!(shown(MY, true, Some("CURRENT_TIMESTAMP(3)")), expr("CURRENT_TIMESTAMP(3)"));
    assert_eq!(shown(MY, true, Some("CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP")), expr("CURRENT_TIMESTAMP"));
    assert_eq!(shown(MY, false, Some("NULL ON UPDATE CURRENT_TIMESTAMP")), Null);
    assert_eq!(shown(MY, true, Some("(rand())")), expr("rand()"));
    assert_eq!(shown(MY, true, Some("(1.5)")), value("1.5"));
    // A PostgreSQL cast is no MySQL syntax: not taken for a constant.
    assert_eq!(shown(MY, true, Some("'a'::text")), expr("'a'::text"));
    // A session in ANSI_QUOTES mode: the server still prints defaults in its default mode.
    let ansi = Dialect::MySql(MySqlMode { ansi_quotes: true, ..MySqlMode::default() });
    assert_eq!(shown(ansi, true, Some("\"x\"")), value("x"));
    // Nothing draws a number in MySQL but AUTO_INCREMENT.
    assert_eq!(shown(MY, true, Some("(nextval(s))")), expr("nextval(s)"));
}

/// Numbered and computed columns show `<generated>`; a column without a default shows NULL
/// when it takes it, else it needs a value (as the planner refuses it).
#[test]
fn generated_nullable_and_required_columns() {
    use DefaultShown::*;
    for fill in [
        ColumnFill::IdentityAlways,
        ColumnFill::IdentityByDefault,
        ColumnFill::AutoIncrement,
        ColumnFill::Stored("a + 1".into()),
        ColumnFill::Virtual("a + 1".into()),
    ] {
        for d in [PG, MY] {
            assert_eq!(default_shown(&col(true, None, fill.clone()), d), Generated, "{fill:?}");
        }
    }
    for d in [PG, MY] {
        assert_eq!(shown(d, false, None), Null);
        assert_eq!(shown(d, true, None), Needed);
        assert!(needs_value(&col(true, None, ColumnFill::Default)));
        assert!(!needs_value(&col(true, Some("0"), ColumnFill::Default)));
        assert!(!needs_value(&col(true, None, ColumnFill::AutoIncrement)));
        assert!(!needs_value(&col(false, None, ColumnFill::Default)));
    }
}

#[test]
fn parentheses_around_the_whole_go() {
    assert_eq!(PG.unparenthesized(" ((1)) "), "1");
    assert_eq!(PG.unparenthesized("(a) + (b)"), "(a) + (b)");
    assert_eq!(PG.unparenthesized("f(x)"), "f(x)");
    assert_eq!(PG.unparenthesized("()"), "");
}
