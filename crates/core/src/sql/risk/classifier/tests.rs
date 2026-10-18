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

/// Which statements open the transaction of a session that commits by hand (true) and which
/// run outside it (false).
#[test]
fn what_opens_the_transaction_on_postgres() {
    let c = Classifier::new(PG);
    for (sql, opens) in [
        ("SELECT 1", false),
        ("SELECT now()", false),
        ("select * from t where a = 1 order by b", false),
        ("WITH a AS (SELECT 1) SELECT * FROM a", false),
        ("VALUES (1)", false),
        ("SELECT nextval('s')", true),
        ("SELECT random()", true),
        ("SELECT pg_advisory_xact_lock(1)", true),
        ("SELECT my_func()", true),
        ("SELECT * FROM t WHERE a = my_func()", true),
        ("SELECT * FROM t FOR UPDATE", true),
        ("SELECT * FROM t FOR SHARE", true),
        ("SELECT * INTO u FROM t", true),
        ("WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d", true),
        ("LOCK TABLE t", true),
        ("UPDATE t SET a = 1", true),
        ("INSERT INTO t VALUES (1)", true),
        ("DELETE FROM t WHERE id = 1", true),
        ("MERGE INTO t USING u ON t.id = u.id WHEN MATCHED THEN DELETE", true),
        ("COPY t FROM STDIN", true),
        ("NOTIFY c", true),
        ("CREATE TABLE t (a int)", true),
        ("ALTER TABLE t ADD b int", true),
        ("DROP TABLE t", true),
        ("TRUNCATE t", true),
        ("ALTER TYPE e ADD VALUE 'x'", true),
        ("ANALYZE t", true),
        ("REFRESH MATERIALIZED VIEW CONCURRENTLY mv", true),
        ("SET work_mem = '64MB'", false),
        ("SET search_path TO s", false),
        ("RESET ALL", false),
        ("LISTEN c", false),
        ("SHOW search_path", false),
        ("BEGIN", false),
        ("COMMIT", false),
        ("ROLLBACK", false),
        ("SAVEPOINT a", false),
        ("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE", false),
        ("EXPLAIN SELECT 1", false),
        ("EXPLAIN (COSTS OFF) DELETE FROM t", false),
        ("EXPLAIN SELECT my_func()", true),
        ("EXPLAIN EXECUTE p(1)", true),
        ("EXPLAIN ANALYZE DELETE FROM t", false),
        ("EXPLAIN ANALYZE SELECT my_func()", false),
        ("DO $$BEGIN END$$", true),
        ("CALL p()", true),
        ("VACUUM", false),
        ("VACUUM t", false),
        ("CREATE INDEX CONCURRENTLY i ON t (a)", false),
        ("DISCARD ALL", false),
        ("COMMIT PREPARED 'x'", false),
        ("FETCH 10 FROM c", true),
        ("EXECUTE unknown", true),
        ("LOAD 'x'", true),
        ("SELEC 1", true),
        ("", true),
        ("-- nothing", true),
        // One statement at a time: several open it, whatever they are.
        ("SELECT 1; SELECT 2", true),
        ("SET a.b = 1; DELETE FROM t", true),
        ("VACUUM; UPDATE t SET a = 1", true),
        ("EXPLAIN ANALYZE SELECT 1; DELETE FROM t", true),
        // A backslash reads two ways; in one of them this is two statements.
        ("SELECT 'a\\', 'b; DELETE FROM t; --'", true),
        ("SELECT 'a\\b'", false),
    ] {
        assert_eq!(c.opens_tx(sql), opens, "{sql}");
    }
}

/// An `EXECUTE` opens it: its statement's risk is what the session prepared, and a read of
/// one is not checked by the allowlist. What the session prepared does not change.
#[test]
fn an_execute_opens_the_transaction() {
    let mut c = Classifier::new(PG);
    c.classify("PREPARE w AS UPDATE t SET a = 1");
    c.classify("PREPARE r AS SELECT 1");
    let before = c.clone();
    assert!(c.opens_tx("EXECUTE w"));
    assert!(c.opens_tx("EXECUTE r"));
    assert!(c.opens_tx("EXECUTE nothing"));
    assert!(!c.opens_tx("EXPLAIN ANALYZE EXECUTE w"));
    // Preparing runs nothing.
    assert!(!c.opens_tx("PREPARE x AS DELETE FROM t"));
    assert!(!c.opens_tx("DEALLOCATE w"));
    assert_eq!(c, before);
    assert!(c.opens_tx("EXECUTE w"), "DEALLOCATE was only asked about");
}

#[test]
fn what_opens_the_transaction_on_mysql() {
    let mode = MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: false };
    let c = Classifier::new(Language::Sql(Dialect::MySql(mode)));
    for (sql, opens) in [
        ("SELECT 1", false),
        ("SELECT NOW()", false),
        ("SELECT * FROM t WHERE a = 1 ORDER BY b", false),
        ("WITH a AS (SELECT 1) SELECT * FROM a", false),
        ("SELECT @x", false),
        ("SELECT @@autocommit", false),
        ("SELECT RAND()", true),
        ("SELECT my_func()", true),
        ("SELECT db.f(1)", true),
        ("SELECT * FROM t FOR UPDATE", true),
        ("SELECT * FROM t FOR SHARE", true),
        ("SELECT * FROM t LOCK IN SHARE MODE", true),
        ("SELECT * FROM t INTO OUTFILE '/tmp/x'", true),
        ("UPDATE t SET a = 1", true),
        ("INSERT INTO t VALUES (1)", true),
        ("DELETE FROM t WHERE id = 1", true),
        ("REPLACE INTO t VALUES (1)", true),
        ("CREATE TEMPORARY TABLE tt (a INT)", true),
        ("CREATE TABLE t (a INT)", false),
        ("ALTER TABLE t ADD b INT", false),
        ("DROP TABLE t", false),
        ("TRUNCATE t", false),
        ("LOCK TABLES t WRITE", false),
        ("SET @x = 1", false),
        ("SET SESSION sql_select_limit = 10", false),
        ("SET @x = my_func()", true),
        ("USE shop", false),
        ("SHOW TABLES", false),
        ("SHOW FULL PROCESSLIST", false),
        ("DESCRIBE t", false),
        ("DESC t", false),
        ("HELP 'select'", false),
        ("EXPLAIN SELECT * FROM t", false),
        ("EXPLAIN SELECT my_func()", true),
        ("EXPLAIN ANALYZE SELECT * FROM t", false),
        ("START TRANSACTION", false),
        ("BEGIN", false),
        ("COMMIT", false),
        ("ROLLBACK", false),
        ("CALL p()", true),
        ("EXECUTE s", true),
        ("SELEC 1", true),
        ("SELECT 1; SELECT 2", true),
        ("/*! DELETE FROM t */ SELECT 1", true),
    ] {
        assert_eq!(c.opens_tx(sql), opens, "{sql}");
    }
}
