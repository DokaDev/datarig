use super::*;
use crate::sql::risk::classify;

const PG: Language = Language::Sql(Dialect::Postgres);

const STATEMENTS: &[&str] = &[
    "SELECT 1",
    "DELETE FROM t",
    "UPDATE t SET a = 1 WHERE id = 2",
    "DROP TABLE t",
    "EXPLAIN ANALYZE DELETE FROM t",
    "SET search_path = x",
    "COPY t TO STDOUT",
    "SELEC 1",
];

#[test]
fn new_and_default_are_postgres_with_nothing_prepared() {
    assert_eq!(Classifier::new(PG), Classifier::Pg(Prepared::default()));
    assert_eq!(Classifier::default(), Classifier::new(Language::default()));
    assert_eq!(Classifier::new(PG).language(), PG);
}

#[test]
fn classify_once_is_the_postgres_classifier() {
    for sql in STATEMENTS {
        assert_eq!(Classifier::classify_once(PG, sql), classify(sql), "{sql}");
    }
}

#[test]
fn classify_and_forget_follow_prepared() {
    let steps = [
        "PREPARE ok AS SELECT 1",
        "PREPARE bad AS DELETE FROM t",
        "EXECUTE ok",
        "EXECUTE bad",
        "EXECUTE unknown",
        "DEALLOCATE ok",
        "EXECUTE ok",
        "PREPARE again AS SELECT 2",
        "DISCARD ALL",
        "EXECUTE again",
    ];
    let mut c = Classifier::new(PG);
    let mut p = Prepared::default();
    for sql in steps {
        assert_eq!(c.classify(sql), p.classify(sql), "{sql}");
        assert_eq!(c, Classifier::Pg(p.clone()), "{sql}");
    }
    for sql in ["PREPARE a AS SELECT 1", "PREPARE b AS SELECT 2"] {
        c.classify(sql);
        p.classify(sql);
    }
    c.forget("DEALLOCATE a");
    p.forget("DEALLOCATE a");
    assert_eq!(c, Classifier::Pg(p.clone()));
    assert_eq!((c.knows("a"), c.knows("b")), (p.knows("a"), p.knows("b")));
    assert_eq!(c.classify("EXECUTE a"), p.classify("EXECUTE a"));
    assert_eq!(c.classify("EXECUTE b"), p.classify("EXECUTE b"));
}

#[test]
fn repeat_checks_are_the_postgres_ones() {
    let c = Classifier::new(PG);
    let texts = [
        "SELECT * FROM t ORDER BY a",
        "SELECT * FROM t",
        "SELECT random()",
        "DELETE FROM t",
        "SELECT 1; SELECT 2",
        "SELECT * FROM t;",
    ];
    for sql in texts {
        assert_eq!(c.repeatable(sql), repeat::repeatable(sql), "{sql}");
        assert_eq!(c.ordered(sql), repeat::ordered(sql), "{sql}");
        assert_eq!(c.count_query(sql), repeat::count_query(sql), "{sql}");
    }
}
