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

/// Where each statement runs on a session that commits by hand while no transaction is open:
/// `X` opens it first, `R` runs outside as a plain read, `O` runs as it is.
#[test]
fn where_statements_run_on_postgres() {
    let c = Classifier::new(PG);
    for (sql, step) in [
        ("SELECT 1", R),
        ("SELECT now()", R),
        ("select * from t where a = 1 order by b", R),
        ("WITH a AS (SELECT 1) SELECT * FROM a", R),
        ("VALUES (1)", R),
        ("SHOW search_path", R),
        ("EXPLAIN SELECT 1", R),
        ("EXPLAIN (COSTS OFF) DELETE FROM t", R),
        // A backslash does not open a transaction: the allowlist's rule on
        // standard_conforming_strings is about running a text again, and the classifier reads it
        // both ways for what it writes (the hidden DELETE below opens it).
        ("SELECT 'a\\b'", R),
        ("SELECT E'a\\\\b'", R),
        ("SELECT nextval('s')", X),
        ("SELECT random()", X),
        ("SELECT pg_advisory_xact_lock(1)", X),
        ("SELECT my_func()", X),
        ("SELECT * FROM t WHERE a = my_func()", X),
        ("SELECT * FROM t FOR UPDATE", X),
        ("SELECT * FROM t FOR SHARE", X),
        ("SELECT * INTO u FROM t", X),
        ("WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d", X),
        ("LOCK TABLE t", X),
        ("UPDATE t SET a = 1", X),
        ("INSERT INTO t VALUES (1)", X),
        ("DELETE FROM t WHERE id = 1", X),
        ("MERGE INTO t USING u ON t.id = u.id WHEN MATCHED THEN DELETE", X),
        ("COPY t FROM STDIN", X),
        ("COPY t TO STDOUT", X),
        ("NOTIFY c", X),
        ("CREATE TABLE t (a int)", X),
        ("ALTER TABLE t ADD b int", X),
        ("DROP TABLE t", X),
        ("TRUNCATE t", X),
        ("ALTER TYPE e ADD VALUE 'x'", X),
        ("ANALYZE t", X),
        ("REFRESH MATERIALIZED VIEW CONCURRENTLY mv", X),
        ("EXPLAIN SELECT my_func()", X),
        ("EXPLAIN EXECUTE p(1)", X),
        ("DO $$BEGIN END$$", X),
        ("CALL p()", X),
        ("FETCH 10 FROM c", X),
        ("EXECUTE unknown", X),
        ("LOAD 'x'", X),
        // Drops the session's temporary tables: a rollback undoes it.
        ("DISCARD TEMP", X),
        ("discard temporary", X),
        ("SET work_mem = '64MB'", O),
        ("SET search_path TO s", O),
        ("RESET ALL", O),
        ("LISTEN c", O),
        ("DISCARD PLANS", O),
        ("DISCARD SEQUENCES", O),
        ("DISCARD ALL", O),
        ("BEGIN", O),
        ("COMMIT", O),
        ("ROLLBACK", O),
        ("SAVEPOINT a", O),
        ("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE", O),
        ("EXPLAIN ANALYZE DELETE FROM t", O),
        ("EXPLAIN ANALYZE SELECT my_func()", O),
        ("VACUUM", O),
        ("VACUUM t", O),
        ("CREATE INDEX CONCURRENTLY i ON t (a)", O),
        ("COMMIT PREPARED 'x'", O),
        ("SELEC 1", X),
        ("", X),
        ("-- nothing", X),
        // One statement at a time: several open it, whatever they are.
        ("SELECT 1; SELECT 2", X),
        ("SET a.b = 1; DELETE FROM t", X),
        ("VACUUM; UPDATE t SET a = 1", X),
        ("EXPLAIN ANALYZE SELECT 1; DELETE FROM t", X),
        ("DISCARD TEMP; DISCARD PLANS", X),
        // A backslash reads two ways; in one of them this is two statements.
        ("SELECT 'a\\', 'b; DELETE FROM t; --'", X),
    ] {
        assert_eq!(c.tx_step(sql), step, "{sql}");
    }
}

const X: TxStep = TxStep::Opens;
const R: TxStep = TxStep::ReadOutside;
const O: TxStep = TxStep::Outside;

/// An `EXECUTE` opens it: its statement's risk is what the session prepared, and a read of
/// one is not checked by the allowlist. What the session prepared does not change.
#[test]
fn an_execute_opens_the_transaction() {
    let mut c = Classifier::new(PG);
    c.classify("PREPARE w AS UPDATE t SET a = 1");
    c.classify("PREPARE r AS SELECT 1");
    let before = c.clone();
    assert_eq!(c.tx_step("EXECUTE w"), X);
    assert_eq!(c.tx_step("EXECUTE r"), X);
    assert_eq!(c.tx_step("EXECUTE nothing"), X);
    assert_eq!(c.tx_step("EXPLAIN ANALYZE EXECUTE w"), O);
    // Preparing runs nothing.
    assert_eq!(c.tx_step("PREPARE x AS DELETE FROM t"), O);
    assert_eq!(c.tx_step("DEALLOCATE w"), O);
    assert_eq!(c, before);
    assert_eq!(c.tx_step("EXECUTE w"), X, "DEALLOCATE was only asked about");
}

#[test]
fn where_statements_run_on_mysql() {
    let mode = MySqlMode {
        ansi_quotes: false,
        no_backslash_escapes: false,
        dollar_quotes: false,
        mariadb: false,
        ignore_space: false,
    };
    let c = Classifier::new(Language::Sql(Dialect::MySql(mode)));
    for (sql, step) in [
        ("SELECT 1", R),
        ("SELECT NOW()", R),
        ("SELECT * FROM t WHERE a = 1 ORDER BY b", R),
        ("WITH a AS (SELECT 1) SELECT * FROM a", R),
        ("SELECT @x", R),
        ("SELECT @@autocommit", R),
        ("SELECT @x FROM t", R),
        ("SHOW TABLES", R),
        ("SHOW FULL PROCESSLIST", R),
        ("DESCRIBE t", R),
        ("DESC t", R),
        ("HELP 'select'", R),
        ("EXPLAIN SELECT * FROM t", R),
        // A schema of the server's own: as without the variable.
        ("SELECT * FROM sys.host_summary", X),
        ("SELECT @a FROM sys.host_summary", X),
        ("SELECT @a FROM performance_schema.threads", X),
        ("SELECT RAND()", X),
        ("SELECT GET_LOCK('x', 1)", X),
        ("SELECT my_func()", X),
        ("SELECT db.f(1)", X),
        ("SELECT * FROM t FOR UPDATE", X),
        ("SELECT * FROM t FOR SHARE", X),
        ("SELECT * FROM t LOCK IN SHARE MODE", X),
        ("SELECT * FROM t INTO OUTFILE '/tmp/x'", X),
        ("UPDATE t SET a = 1", X),
        ("INSERT INTO t VALUES (1)", X),
        ("DELETE FROM t WHERE id = 1", X),
        ("REPLACE INTO t VALUES (1)", X),
        ("CREATE TEMPORARY TABLE tt (a INT)", X),
        ("EXPLAIN SELECT my_func()", X),
        ("CALL p()", X),
        ("EXECUTE s", X),
        // A session statement that calls a function with an effect: as its query would.
        ("SET @x = my_func()", X),
        ("SET @a = GET_LOCK('x', 1)", X),
        ("SET @a = (SELECT GET_LOCK('x', 1))", X),
        ("SELECT GET_LOCK('x', 1) INTO @a", X),
        ("SELECT SLEEP(1) INTO @a", X),
        ("CREATE TABLE t (a INT)", O),
        ("ALTER TABLE t ADD b INT", O),
        ("DROP TABLE t", O),
        ("TRUNCATE t", O),
        ("LOCK TABLES t WRITE", O),
        ("SET @x = 1", O),
        ("SELECT 1 INTO @a", O),
        ("SET SESSION sql_select_limit = 10", O),
        ("USE shop", O),
        ("EXPLAIN ANALYZE SELECT * FROM t", O),
        ("START TRANSACTION", O),
        ("BEGIN", O),
        ("COMMIT", O),
        ("ROLLBACK", O),
        ("SELEC 1", X),
        ("SELECT 1; SELECT 2", X),
        ("/*! DELETE FROM t */ SELECT 1", X),
    ] {
        assert_eq!(c.tx_step(sql), step, "{sql}");
    }
}

/// Every sql mode, the `mariadb` flag set or not.
fn every_mode() -> Vec<MySqlMode> {
    (0..32u8)
        .map(|b| MySqlMode {
            ansi_quotes: b & 1 != 0,
            no_backslash_escapes: b & 2 != 0,
            dollar_quotes: b & 4 != 0,
            mariadb: b & 8 != 0,
            ignore_space: b & 16 != 0,
        })
        .collect()
}

/// A MariaDB classifier answers every question as the MySQL one of the same mode: the mode's
/// `mariadb` flag picks the calls MariaDB has.
#[test]
fn a_mariadb_classifier_is_the_flagged_mysql_one() {
    let texts = [
        "SELECT 1",
        "SELECT * FROM t ORDER BY a",
        "SELECT @@time_zone, NOW()",
        "SELECT @a",
        "SELECT BIN_TO_UUID(a) FROM t",
        "SELECT POINT(1, 2)",
        "SELECT SYSDATE ()",
        "SELECT `DATABASE`()",
        "SELECT LATERAL(1)",
        "SELECT \"a\" FROM t",
        "SELECT 'a\\'; DROP TABLE t; -- '",
        "SELECT $$a; b$$",
        "SELECT 1 /*!50000 ; SELECT 2 */; SELECT 3",
        "/*M! DROP TABLE t */ SELECT 1",
        "DELETE FROM t",
        "UPDATE t SET a = 1 WHERE id = 2",
        "INSERT INTO t VALUES (1) RETURNING id",
        "ANALYZE SELECT 1",
        "SET SESSION sql_mode = 'ANSI_QUOTES'",
        "SET TRANSACTION READ ONLY",
        "/*!80000 SET TRANSACTION READ ONLY */",
        "START TRANSACTION",
        "LOCK TABLES t READ",
        "CREATE TABLE t (a int)",
        "SHOW TABLES",
        "EXPLAIN SELECT 1",
        "EXECUTE s",
        "PREPARE s FROM 'DELETE FROM t'",
        "SELECT 1; SELECT 2",
        "SELEC 1",
        "",
    ];
    for mode in every_mode() {
        let mysql = Classifier::new(Language::Sql(Dialect::MySql(mode)));
        let mariadb = Classifier::new(Language::Sql(Dialect::MariaDb(mode)));
        assert_eq!(mariadb, Classifier::MariaDb(mode));
        assert_eq!(mariadb.language(), Language::Sql(Dialect::MariaDb(mode)));
        for sql in texts {
            let at = format!("{sql:?} in {mode:?}");
            assert_eq!(mariadb.clone().classify(sql), mysql.clone().classify(sql), "{at}");
            assert_eq!(
                Classifier::classify_once(mariadb.language(), sql),
                Classifier::classify_once(mysql.language(), sql),
                "{at}"
            );
            assert_eq!(mariadb.repeatable(sql), mysql.repeatable(sql), "{at}");
            assert_eq!(mariadb.reads_zone(sql), mysql.reads_zone(sql), "{at}");
            assert_eq!(mariadb.ordered(sql), mysql.ordered(sql), "{at}");
            assert_eq!(mariadb.tx_step(sql), mysql.tx_step(sql), "{at}");
            assert_eq!(mariadb.count_query(sql), mysql.count_query(sql), "{at}");
            assert_eq!(mariadb.knows(sql), mysql.knows(sql), "{at}");
            assert_eq!(
                Classifier::session_set(mariadb.language(), sql),
                Classifier::session_set(mysql.language(), sql),
                "{at}"
            );
            let (mut a, mut b) = (mariadb.clone(), mysql.clone());
            a.forget(sql);
            b.forget(sql);
            assert_eq!((a, b), (mariadb.clone(), mysql.clone()), "{at}");
        }
    }
}
