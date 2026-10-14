use super::*;

fn class(sql: &str) -> Class {
    classify(sql).class
}

fn danger(sql: &str) -> Option<Danger> {
    classify(sql).danger
}

fn target(sql: &str) -> Option<String> {
    classify(sql).target
}

#[track_caller]
fn all(expected: Class, sqls: &[&str]) {
    for sql in sqls {
        assert_eq!(class(sql), expected, "{sql}");
    }
}

#[test]
fn reads() {
    all(
        Class::Read,
        &[
            "SELECT 1",
            "select * from shop.users where id = 1",
            "  -- a comment\n  SeLeCt now()",
            "/* block /* nested */ still */ SELECT 1",
            "VALUES (1), (2)",
            "TABLE shop.users",
            "SHOW search_path",
            "FETCH 10 FROM c",
            "MOVE NEXT IN c",
            "CLOSE c",
            "(SELECT 1) UNION (SELECT 2)",
            "WITH a AS (SELECT 1) SELECT * FROM a",
            "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r WHERE n < 5) SELECT * FROM r",
            "WITH update AS (SELECT 1) SELECT * FROM update",
            "WITH a AS (SELECT 1), delete AS (SELECT 2) SELECT * FROM delete",
            "COPY shop.users TO STDOUT",
            "COPY (SELECT 1) TO STDOUT WITH (FORMAT csv)",
            "DECLARE c CURSOR WITH HOLD FOR SELECT * FROM t",
            "SELECT 'DROP TABLE t; DELETE FROM t'",
            "SELECT $$DELETE FROM t$$, $x$DROP TABLE t$x$",
            "SELECT \"delete\", \"drop\" FROM t",
            "SELECT substring('abc' FROM 1 FOR 2)",
            "SELECT * FROM t WHERE x IN (SELECT y FROM u)",
        ],
    );
}

#[test]
fn writes() {
    all(
        Class::Write,
        &[
            "INSERT INTO t VALUES (1)",
            "insert into t (update, delete) values (1, 2)",
            "INSERT INTO t SELECT * FROM u ON CONFLICT (id) DO UPDATE SET x = excluded.x",
            "UPDATE t SET x = 1 WHERE id = 2",
            "DELETE FROM t WHERE id = 1",
            "MERGE INTO t USING u ON t.id = u.id WHEN MATCHED THEN DELETE",
            "COPY t FROM STDIN",
            "COPY t TO '/tmp/out.csv'",
            "COPY t TO PROGRAM 'rm -rf /'",
            "SELECT * FROM t FOR UPDATE",
            "SELECT * FROM t FOR NO KEY UPDATE SKIP LOCKED",
            "SELECT * FROM t FOR SHARE",
            "SELECT * FROM t FOR KEY SHARE",
            "DECLARE c CURSOR FOR SELECT * FROM t FOR UPDATE",
            "LOCK TABLE t IN ACCESS EXCLUSIVE MODE",
            "NOTIFY chan",
            "REFRESH MATERIALIZED VIEW CONCURRENTLY s.mv",
            "WITH d AS (DELETE FROM t WHERE id = 1 RETURNING *) SELECT * FROM d",
            "WITH i AS (INSERT INTO t VALUES (1) RETURNING id) SELECT id FROM i",
            "WITH u AS (UPDATE t SET x = 1 WHERE id = 2 RETURNING *) SELECT 1",
            "WITH a AS (SELECT 1) INSERT INTO t SELECT * FROM a",
            "WITH a AS (SELECT 1) DELETE FROM t WHERE id IN (SELECT * FROM a)",
        ],
    );
}

#[test]
fn ddl() {
    all(
        Class::Ddl,
        &[
            "CREATE TABLE t (id int)",
            "create or replace view v as select 1",
            "CREATE FUNCTION f() RETURNS int AS $$ DELETE FROM t; SELECT 1 $$ LANGUAGE sql",
            "ALTER TABLE t ADD COLUMN c int",
            "DROP TABLE t",
            "TRUNCATE t",
            "COMMENT ON TABLE t IS 'x'",
            "GRANT SELECT ON t TO r",
            "REVOKE ALL ON t FROM r",
            "SECURITY LABEL ON TABLE t IS 'x'",
            "IMPORT FOREIGN SCHEMA s FROM SERVER srv INTO l",
            "REASSIGN OWNED BY a TO b",
            "SELECT * INTO new_t FROM t",
            "SELECT a, b INTO TEMP TABLE new_t FROM t",
            "CREATE TABLE t2 AS SELECT * FROM t",
            "CREATE MATERIALIZED VIEW mv AS SELECT 1",
            "ALTER SYSTEM SET work_mem = '1GB'",
        ],
    );
    assert!(classify("SELECT * INTO new_t FROM t").writes);
    assert!(classify("CREATE TABLE t2 AS SELECT * FROM t").writes);
    assert!(classify("CREATE UNLOGGED TABLE IF NOT EXISTS t2 AS TABLE t").writes);
    assert!(classify("CREATE MATERIALIZED VIEW mv AS SELECT 1").writes);
    assert!(!classify("CREATE TABLE t (id int GENERATED ALWAYS AS (1) STORED)").writes);
    assert_eq!(target("SELECT * INTO new_t FROM t").as_deref(), Some("new_t"));
    assert_eq!(target("CREATE TEMP TABLE s.t2 AS SELECT 1").as_deref(), Some("s.t2"));
}

#[test]
fn transaction_control_session_maintenance_procedural_unknown() {
    all(
        Class::Tx,
        &[
            "BEGIN",
            "begin isolation level serializable",
            "START TRANSACTION READ ONLY",
            "COMMIT",
            "END",
            "ROLLBACK",
            "ABORT",
            "ROLLBACK TO SAVEPOINT a",
            "SAVEPOINT a",
            "RELEASE SAVEPOINT a",
            "PREPARE TRANSACTION 'x'",
            "COMMIT PREPARED 'x'",
            "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ",
            "SET CONSTRAINTS ALL DEFERRED",
            "SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY",
        ],
    );
    all(
        Class::Session,
        &[
            "SET search_path TO shop",
            "SET LOCAL statement_timeout = '5s'",
            "RESET work_mem",
            "RESET ALL",
            "DISCARD ALL",
            "LISTEN chan",
            "UNLISTEN *",
            "DEALLOCATE ALL",
            "PREPARE p AS SELECT 1",
            "SET ROLE reader",
            "SET TIME ZONE 'UTC'",
        ],
    );
    all(
        Class::Maintenance,
        &["VACUUM t", "VACUUM (ANALYZE) t", "ANALYZE t", "ANALYSE", "REINDEX TABLE t", "CLUSTER t", "CHECKPOINT"],
    );
    all(Class::Procedural, &["DO $$ BEGIN DELETE FROM t; END $$", "CALL p(1)"]);
    all(Class::Unknown, &["EXECUTE p", "LOAD 'auto_explain'", "FROBNICATE t", "", "-- only a comment", "; ;", "42"]);
}

#[test]
fn no_where_and_always_true_conditions() {
    let nw = |sql: &str| classify(sql).no_where;
    assert_eq!(nw("DELETE FROM t"), Some(NoWhere::Missing));
    assert_eq!(nw("UPDATE t SET x = 1"), Some(NoWhere::Missing));
    assert_eq!(nw("delete from ONLY s.t"), Some(NoWhere::Missing));
    assert_eq!(nw("DELETE FROM t RETURNING *"), Some(NoWhere::Missing));
    assert_eq!(nw("DELETE FROM t USING u"), Some(NoWhere::Missing));
    // A WHERE only inside parentheses does not filter the statement.
    assert_eq!(nw("UPDATE t SET x = (SELECT y FROM u WHERE u.id = 1)"), Some(NoWhere::Missing));
    assert_eq!(nw("UPDATE t SET x = 1 FROM (SELECT * FROM u WHERE a) s"), Some(NoWhere::Missing));
    // Words inside strings, comments and quoted identifiers are not a WHERE.
    assert_eq!(nw("DELETE FROM t -- WHERE id = 1"), Some(NoWhere::Missing));
    assert_eq!(nw("DELETE FROM t /* WHERE id = 1 */"), Some(NoWhere::Missing));
    assert_eq!(nw("UPDATE t SET note = 'WHERE id = 1'"), Some(NoWhere::Missing));
    assert_eq!(nw("UPDATE t SET \"where\" = 1"), Some(NoWhere::Missing));
    for always in [
        "DELETE FROM t WHERE true",
        "DELETE FROM t WHERE TRUE RETURNING *",
        "delete from t where 1=1",
        "DELETE FROM t WHERE 1 = 1",
        "DELETE FROM t WHERE (1 = 1)",
        "DELETE FROM t WHERE ((true))",
        "DELETE FROM t WHERE 'a' = 'a'",
        "DELETE FROM t WHERE NOT false",
        "DELETE FROM t WHERE id = id",
        "DELETE FROM t WHERE t.id = t.id",
        "DELETE FROM t WHERE 't'",
        "UPDATE t SET x = 1 WHERE id = 5 OR 1 = 1",
        "UPDATE t SET x = 1 WHERE true OR id = 5",
    ] {
        assert_eq!(nw(always), Some(NoWhere::AlwaysTrue), "{always}");
        assert!(danger(always).is_some(), "{always}");
    }
    for filtered in [
        "DELETE FROM t WHERE id = 1",
        "DELETE FROM t WHERE 1 = 1 AND id = 5",
        "DELETE FROM t WHERE (id = 5 OR true) AND x = 1",
        "DELETE FROM t WHERE id <= id",
        "DELETE FROM t WHERE id >= id",
        "DELETE FROM t WHERE a != a",
        "DELETE FROM t WHERE CURRENT OF c",
        "UPDATE t SET x = 1 FROM u WHERE t.id = u.id",
        "DELETE FROM t USING u WHERE t.id = u.id",
    ] {
        assert_eq!(nw(filtered), None, "{filtered}");
        assert_eq!(danger(filtered), None, "{filtered}");
    }
    // A condition that reads no column matches every row or none.
    for no_column in [
        "DELETE FROM t WHERE 'f'",
        "DELETE FROM t WHERE 1 <> 0",
        "UPDATE t SET x = 1 WHERE 2 > 1",
        "DELETE FROM t WHERE random() < 2",
        "DELETE FROM t WHERE EXISTS (SELECT 1)",
        "DELETE FROM t WHERE false",
    ] {
        assert_eq!(nw(no_column), Some(NoWhere::NoColumn), "{no_column}");
        assert!(danger(no_column).is_some(), "{no_column}");
    }
    assert_eq!(nw("DELETE FROM t WHERE true AND 1 = 1"), Some(NoWhere::AlwaysTrue));
    assert_eq!(nw("UPDATE t SET x = 1 WHERE true AND true"), Some(NoWhere::AlwaysTrue));
    assert_eq!(nw("DELETE FROM t WHERE id = id AND true"), Some(NoWhere::AlwaysTrue));
    assert_eq!(nw("DELETE FROM t WHERE id IN (SELECT 1)"), None);
    assert_eq!(nw("INSERT INTO t VALUES (1)"), None);
    assert_eq!(nw("SELECT * FROM t"), None);
}

#[test]
fn destructive_statements() {
    for (sql, d) in [
        ("DROP TABLE t", Danger::Drop),
        ("drop table if exists s.t, u cascade", Danger::Drop),
        ("DROP SCHEMA s CASCADE", Danger::Drop),
        ("DROP DATABASE prod", Danger::Drop),
        ("DROP INDEX CONCURRENTLY IF EXISTS i", Danger::Drop),
        ("DROP MATERIALIZED VIEW mv", Danger::Drop),
        ("DROP OWNED BY r", Danger::Drop),
        ("DROP FUNCTION f(int)", Danger::Drop),
        ("TRUNCATE t", Danger::Truncate),
        ("TRUNCATE TABLE ONLY a, b RESTART IDENTITY", Danger::Truncate),
        ("DELETE FROM t", Danger::DeleteAll),
        ("UPDATE t SET x = 1", Danger::UpdateAll),
        ("ALTER TABLE t DROP COLUMN c", Danger::DropColumn),
        ("ALTER TABLE t DROP c", Danger::DropColumn),
        ("ALTER TABLE t DROP \"Col\"", Danger::DropColumn),
        ("ALTER TABLE IF EXISTS ONLY t DROP COLUMN IF EXISTS c CASCADE", Danger::DropColumn),
        ("ALTER TABLE t ADD COLUMN d int, DROP COLUMN c", Danger::DropColumn),
        ("ALTER FOREIGN TABLE f DROP COLUMN c", Danger::DropColumn),
        ("ALTER TYPE ty DROP ATTRIBUTE a", Danger::DropColumn),
        ("ALTER PUBLICATION p DROP TABLE t", Danger::Drop),
        ("WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d", Danger::DeleteAll),
        ("WITH a AS (SELECT 1) UPDATE t SET x = 1", Danger::UpdateAll),
        ("EXPLAIN ANALYZE DELETE FROM t", Danger::DeleteAll),
        ("SELECT 1; DROP TABLE t", Danger::Drop),
    ] {
        assert_eq!(danger(sql), Some(d), "{sql}");
    }
    for keeps in [
        "ALTER TABLE t DROP CONSTRAINT c",
        "ALTER TABLE t ALTER COLUMN c DROP DEFAULT",
        "ALTER TABLE t ALTER c DROP NOT NULL",
        "ALTER TABLE t ALTER COLUMN c DROP EXPRESSION",
        "ALTER TABLE t ALTER COLUMN c DROP IDENTITY IF EXISTS",
        "ALTER TABLE t ADD COLUMN c int",
        "ALTER TABLE t RENAME COLUMN \"drop\" TO x",
        "CREATE TABLE drop_log (id int)",
        "SELECT 'DROP TABLE t'",
        "SELECT * FROM t -- ; DROP TABLE t",
        "SELECT $$; DROP TABLE t; $$",
        "SELECT $a$ $b$ ; DROP TABLE t $b$ $a$",
        "EXPLAIN DELETE FROM t",
        "INSERT INTO t VALUES (1)",
    ] {
        assert_eq!(danger(keeps), None, "{keeps}");
    }
}

#[test]
fn targets() {
    assert_eq!(target("DELETE FROM shop.orders").as_deref(), Some("shop.orders"));
    assert_eq!(target("delete from only \"Shop\".\"Orders\" where true").as_deref(), Some("\"Shop\".\"Orders\""));
    assert_eq!(target("UPDATE ONLY t AS x SET a = 1").as_deref(), Some("t"));
    assert_eq!(target("INSERT INTO s.t VALUES (1)").as_deref(), Some("s.t"));
    assert_eq!(target("MERGE INTO t USING u ON true WHEN MATCHED THEN DELETE").as_deref(), Some("t"));
    assert_eq!(target("DROP TABLE IF EXISTS a, s.b CASCADE").as_deref(), Some("a, s.b"));
    assert_eq!(target("DROP MATERIALIZED VIEW IF EXISTS mv").as_deref(), Some("mv"));
    assert_eq!(target("DROP FOREIGN DATA WRAPPER w").as_deref(), Some("w"));
    assert_eq!(target("DROP TEXT SEARCH CONFIGURATION c").as_deref(), Some("c"));
    assert_eq!(target("TRUNCATE TABLE ONLY a, b").as_deref(), Some("a, b"));
    assert_eq!(target("ALTER TABLE IF EXISTS s.t DROP COLUMN c").as_deref(), Some("s.t"));
    assert_eq!(target("ALTER SYSTEM SET work_mem = '1GB'"), None);
    assert_eq!(target("WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d").as_deref(), Some("t"));
    assert_eq!(target("EXPLAIN ANALYZE UPDATE s.t SET x = 1").as_deref(), Some("s.t"));
    assert_eq!(target("COPY s.t FROM STDIN").as_deref(), Some("s.t"));
    assert_eq!(target("SELECT 1"), None);
}

#[test]
fn explain_plans_and_explain_analyze_runs() {
    for plan in [
        "EXPLAIN SELECT 1",
        "EXPLAIN DELETE FROM t",
        "explain verbose update t set x = 1",
        "EXPLAIN (VERBOSE, COSTS off) DELETE FROM t",
        "EXPLAIN (ANALYZE false) DELETE FROM t",
        "EXPLAIN (ANALYZE off, BUFFERS) DELETE FROM t",
        "EXPLAIN (ANALYZE 0) DELETE FROM t",
        "EXPLAIN (FORMAT json) DELETE FROM t",
    ] {
        let r = classify(plan);
        assert_eq!((r.class, r.explain), (Class::Read, Explain::Plan), "{plan}");
        assert!(!r.rolls_back() && r.danger.is_none() && r.read_only().is_ok(), "{plan}");
    }
    for (sql, class) in [
        ("EXPLAIN ANALYZE DELETE FROM t", Class::Write),
        ("explain analyse delete from t", Class::Write),
        ("EXPLAIN ANALYZE VERBOSE UPDATE t SET x = 1 WHERE id = 1", Class::Write),
        ("EXPLAIN (ANALYZE) DELETE FROM t WHERE id = 1", Class::Write),
        ("EXPLAIN (ANALYZE true, BUFFERS) INSERT INTO t VALUES (1)", Class::Write),
        ("EXPLAIN (ANALYSE on) INSERT INTO t VALUES (1)", Class::Write),
        ("EXPLAIN (BUFFERS, ANALYZE 1) INSERT INTO t VALUES (1)", Class::Write),
        ("EXPLAIN (ANALYZE 'yes') INSERT INTO t VALUES (1)", Class::Write),
        // A value it does not know counts as on.
        ("EXPLAIN (ANALYZE maybe) INSERT INTO t VALUES (1)", Class::Write),
        ("EXPLAIN ANALYZE CREATE TABLE t2 AS SELECT 1", Class::Ddl),
        ("EXPLAIN ANALYZE WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d", Class::Write),
        ("EXPLAIN ANALYZE EXECUTE p", Class::Unknown),
        ("EXPLAIN ANALYZE SELECT * FROM t", Class::Read),
    ] {
        let r = classify(sql);
        assert_eq!((r.class, r.explain), (class, Explain::Analyze), "{sql}");
        assert!(r.rolls_back(), "{sql}");
        assert_eq!(r.rollback_matters(), class != Class::Read, "{sql}");
    }
    // Not a statement the parser accepts.
    let r = classify("EXPLAIN ANALYZE");
    assert_eq!((r.class, r.danger), (Class::Unknown, Some(Danger::Unparsed)));
    let r = classify("EXPLAIN ANALYZE DELETE FROM t");
    assert_eq!(r.no_where, Some(NoWhere::Missing));
    assert!(r.read_only().is_err());
}

#[test]
fn read_only_verdicts() {
    let ok = |sql: &str| classify(sql).read_only();
    for allowed in [
        "SELECT 1",
        "EXPLAIN DELETE FROM t",
        "EXPLAIN ANALYZE SELECT 1",
        "BEGIN",
        "BEGIN READ ONLY",
        "START TRANSACTION ISOLATION LEVEL SERIALIZABLE, READ ONLY",
        "COMMIT",
        "SET TRANSACTION READ ONLY",
        "SET search_path TO shop, public",
        "SET LOCAL statement_timeout = '5s'",
        "set work_mem = '64MB'",
        "SET enable_seqscan = off",
        "SET app.tenant_id = '42'",
        "SET \"search_path\" = shop",
        "SET TIME ZONE 'UTC'",
        "SET SCHEMA 'shop'",
        "SET ROLE reader",
        "RESET ALL",
        "RESET search_path",
        "DISCARD ALL",
        "SET default_transaction_read_only = on",
        "SET transaction_read_only TO true",
        "SHOW default_transaction_read_only",
    ] {
        assert_eq!(ok(allowed), Ok(()), "{allowed}");
    }
    for (blocked, why) in [
        ("INSERT INTO t VALUES (1)", ReadOnlyBlock::Class(Class::Write)),
        ("SELECT * FROM t FOR UPDATE", ReadOnlyBlock::Class(Class::Write)),
        ("CREATE TABLE t (id int)", ReadOnlyBlock::Class(Class::Ddl)),
        ("SELECT 1 INTO t2", ReadOnlyBlock::Class(Class::Ddl)),
        ("VACUUM", ReadOnlyBlock::Class(Class::Maintenance)),
        ("DO $$ BEGIN END $$", ReadOnlyBlock::Class(Class::Procedural)),
        ("EXECUTE p", ReadOnlyBlock::Class(Class::Unknown)),
        ("EXPLAIN ANALYZE DELETE FROM t WHERE id = 1", ReadOnlyBlock::Class(Class::Write)),
        ("WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d", ReadOnlyBlock::Class(Class::Write)),
        ("SET default_transaction_read_only = off", ReadOnlyBlock::ReadWrite),
        ("set default_transaction_read_only to false", ReadOnlyBlock::ReadWrite),
        ("SET SESSION default_transaction_read_only = 0", ReadOnlyBlock::ReadWrite),
        ("SET default_transaction_read_only TO DEFAULT", ReadOnlyBlock::ReadWrite),
        ("SET transaction_read_only = off", ReadOnlyBlock::ReadWrite),
        ("RESET default_transaction_read_only", ReadOnlyBlock::ReadWrite),
        ("SET TRANSACTION READ WRITE", ReadOnlyBlock::ReadWrite),
        ("SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE", ReadOnlyBlock::ReadWrite),
        ("BEGIN READ WRITE", ReadOnlyBlock::ReadWrite),
        ("START TRANSACTION ISOLATION LEVEL SERIALIZABLE, READ WRITE", ReadOnlyBlock::ReadWrite),
        ("COMMIT PREPARED 'x'", ReadOnlyBlock::ReadWrite),
        ("ROLLBACK PREPARED 'x'", ReadOnlyBlock::ReadWrite),
        ("SET session_replication_role = replica", ReadOnlyBlock::Setting),
        ("SET SESSION AUTHORIZATION admin", ReadOnlyBlock::Setting),
        ("RESET session_replication_role", ReadOnlyBlock::Setting),
    ] {
        assert_eq!(ok(blocked), Err(why), "{blocked}");
    }
}

#[test]
fn confirmations() {
    let destructive = |sql: &str| classify(sql).confirm(false);
    let writes = |sql: &str| classify(sql).confirm(true);
    assert_eq!(destructive("DROP TABLE t"), Some(Why::Danger(Danger::Drop)));
    assert_eq!(writes("DROP TABLE t"), Some(Why::Danger(Danger::Drop)));
    assert_eq!(destructive("INSERT INTO t VALUES (1)"), None);
    assert_eq!(writes("INSERT INTO t VALUES (1)"), Some(Why::Class(Class::Write)));
    // Code the text does not show asks by default.
    assert_eq!(destructive("DO $$ $$"), Some(Why::Danger(Danger::Procedural)));
    assert_eq!(destructive("CALL p()"), Some(Why::Danger(Danger::Procedural)));
    assert_eq!(writes("DO $$ $$"), Some(Why::Danger(Danger::Procedural)));
    assert_eq!(writes("FROBNICATE"), Some(Why::Danger(Danger::Unparsed)));
    assert_eq!(writes("LOAD 'auto_explain'"), Some(Why::Class(Class::Unknown)));
    assert_eq!(writes("CREATE INDEX i ON t (c)"), Some(Why::Class(Class::Ddl)));
    assert_eq!(writes("EXPLAIN ANALYZE UPDATE t SET x = 1 WHERE id = 1"), Some(Why::Class(Class::Write)));
    for harmless in ["SELECT 1", "SET work_mem = '1GB'", "BEGIN", "COMMIT", "EXPLAIN DELETE FROM t", "SHOW all"] {
        assert_eq!(writes(harmless), None, "{harmless}");
    }
}

#[test]
fn adversarial_texts_do_not_hide_or_invent_statements() {
    // Case, comments, strings and dollar bodies.
    assert_eq!(danger("dRoP/**/TaBlE t"), Some(Danger::Drop));
    assert_eq!(danger("/* SELECT */ DELETE FROM t"), Some(Danger::DeleteAll));
    assert_eq!(danger("-- SELECT 1\nDELETE FROM t"), Some(Danger::DeleteAll));
    assert_eq!(class("SELECT E'\\'; DROP TABLE t; --'"), Class::Read);
    assert_eq!(class("SELECT $tag$ $$ ; DROP TABLE t $$ $tag$"), Class::Read);
    // What the parser rejects asks and is not a read.
    for unparsed in ["SELECT 1 /* unterminated ; DROP TABLE t", "SELECT 'unterminated ; DROP TABLE t", "FROBNICATE"] {
        assert_eq!((class(unparsed), danger(unparsed)), (Class::Unknown, Some(Danger::Unparsed)), "{unparsed}");
        assert!(classify(unparsed).read_only().is_err(), "{unparsed}");
    }
    // A quoted identifier is never a keyword.
    assert_eq!(class("\"DROP\" TABLE t"), Class::Unknown);
    assert_eq!(danger("SELECT * FROM \"drop\""), None);
    // A plain string with a backslash: a server with standard_conforming_strings = off ends the
    // string early and runs the DROP, so the worse reading counts.
    let sneaky = "SELECT 'x\\''; DROP TABLE t; --'";
    assert_eq!(Prepared::default().read(sneaky).danger, None, "a standard server reads one string");
    let r = classify(sneaky);
    assert_eq!((r.class, r.danger), (Class::Ddl, Some(Danger::Drop)));
    assert_eq!(class("SELECT 'C:\\path'"), Class::Read);
    // Several statements: the worst counts.
    assert_eq!(class("SELECT 1; INSERT INTO t VALUES (1)"), Class::Write);
    assert_eq!(danger("select 1;\n\ntruncate t;"), Some(Danger::Truncate));
    // Unicode and odd spacing.
    assert_eq!(target("DELETE FROM \"注文\"").as_deref(), Some("\"注文\""));
    assert_eq!(danger("DELETE FROM 注文 WHERE 番号 = 1"), None);
    // `update` and `delete` are not reserved: a column or a CTE may be named so.
    assert_eq!(danger("INSERT INTO t (update) VALUES (1)"), None);
    assert_eq!(danger("WITH delete AS (SELECT 1) SELECT * FROM delete"), None);
    // A rule's actions do not run when it is created.
    assert_eq!(danger("CREATE RULE r AS ON INSERT TO t DO INSTEAD (DELETE FROM u)"), None);
}

// ── adversarial probes and what the parse tree adds ─────────────

/// (1) `EXPLAIN ("analyze") DELETE FROM t`: the option name quoted (or in any case) is still
/// `ANALYZE`, so the DELETE runs, asks, rolls back and a read-only policy refuses it.
#[test]
fn explain_analyze_in_any_quoting_or_case_runs_its_statement() {
    for sql in [
        "EXPLAIN (\"analyze\") DELETE FROM t",
        "EXPLAIN (\"analyse\", buffers) DELETE FROM t",
        "EXPLAIN (Analyze) DELETE FROM t",
        "explain (costs off, ANALYZE TRUE) delete from t",
        "EXPLAIN (ANALYZE 'on') DELETE FROM t",
        "EXPLAIN (ANALYZE 1) DELETE FROM t",
        "EXPLAIN /* x */ ANALYZE\n-- y\r DELETE FROM t",
    ] {
        let r = classify(sql);
        assert_eq!((r.explain, r.class, r.danger), (Explain::Analyze, Class::Write, Some(Danger::DeleteAll)), "{sql}");
        assert!(r.rolls_back() && r.read_only().is_err(), "{sql}");
    }
    for off in [
        "EXPLAIN (\"analyze\" false) DELETE FROM t",
        "EXPLAIN (analyze of) DELETE FROM t",
        "EXPLAIN (ANALYZE fa) DELETE FROM t",
    ] {
        assert_eq!(classify(off).explain, Explain::Plan, "{off}");
    }
}

/// (2) A lone carriage return ends a `--` comment: `OR true` is part of the condition.
#[test]
fn a_carriage_return_ends_a_comment_for_the_classifier() {
    let r = classify("UPDATE t SET a = 0 WHERE id = 5 --\r OR true\nRETURNING id");
    assert_eq!((r.no_where, r.danger), (Some(NoWhere::AlwaysTrue), Some(Danger::UpdateAll)));
    let r = classify("DELETE FROM t --\rWHERE id = 1");
    assert_eq!(r.danger, None, "the WHERE after the carriage return counts");
}

/// (3) `x<NBSP>$$` and an emoji before `$$` are identifiers, not a dollar quote: the DELETE in
/// the second CTE is seen.
#[test]
fn a_dollar_after_a_non_ascii_character_hides_nothing() {
    for sql in [
        "WITH a AS (SELECT 1 AS x\u{a0}$$), d AS (DELETE FROM t RETURNING 1) SELECT 1 AS y\u{a0}$$",
        "WITH a AS (SELECT 1 AS \u{1f418}$$), d AS (DELETE FROM t RETURNING 1) SELECT 1 AS \u{1f418}$$",
    ] {
        let r = classify(sql);
        assert_eq!((r.class, r.danger), (Class::Write, Some(Danger::DeleteAll)), "{sql}");
    }
}

/// (4) `PREPARE p AS DELETE FROM t; EXECUTE p`: the EXECUTE has the DELETE's risk. An EXECUTE of
/// a name the session did not prepare is dangerous; DEALLOCATE and DISCARD ALL forget.
#[test]
fn execute_has_the_risk_of_what_was_prepared() {
    let r = classify("PREPARE pz AS DELETE FROM t; EXECUTE pz");
    assert_eq!((r.class, r.danger), (Class::Write, Some(Danger::DeleteAll)));
    let mut s = Prepared::default();
    let p = s.classify("PREPARE pz AS DELETE FROM t");
    assert_eq!((p.class, p.danger, p.read_only()), (Class::Session, None, Ok(())), "preparing runs nothing");
    assert!(s.knows("pz"));
    // (Each on a copy: running a DELETE may fire a trigger, after which no name is known.)
    assert_eq!(s.clone().classify("EXECUTE pz").danger, Some(Danger::DeleteAll));
    assert_eq!(s.clone().classify("execute PZ").danger, Some(Danger::DeleteAll), "names fold to lower case");
    assert_eq!(s.clone().classify("EXPLAIN ANALYZE EXECUTE pz").danger, Some(Danger::DeleteAll));
    assert_eq!(s.clone().classify("CREATE TABLE t2 AS EXECUTE pz").danger, Some(Danger::DeleteAll));
    s.classify("PREPARE r(int) AS SELECT * FROM t WHERE id = $1");
    let r = s.classify("EXECUTE r(1)");
    assert_eq!((r.class, r.danger, r.read_only()), (Class::Read, None, Ok(())));
    s.classify("DEALLOCATE pz");
    assert!(!s.knows("pz") && s.knows("r"));
    let r = s.classify("EXECUTE pz");
    assert_eq!((r.class, r.danger), (Class::Unknown, Some(Danger::UnknownPrepared)));
    assert!(r.read_only().is_err());
    s.classify("DISCARD ALL");
    assert!(!s.knows("r"));
    s.classify("PREPARE a AS SELECT 1; PREPARE b AS SELECT 2");
    s.classify("DEALLOCATE ALL");
    assert!(!s.knows("a") && !s.knows("b"));
    s.classify("PREPARE a AS SELECT 1");
    s.classify("DEALLOCATE PREPARE a");
    assert!(!s.knows("a"));
    // DISCARD PLANS keeps the prepared statements (it only drops their plans).
    s.classify("PREPARE c AS SELECT 1");
    s.classify("DISCARD PLANS");
    assert!(s.knows("c"));
}

/// Sent is not succeeded: a statement whose outcome is unknown forgets the names
/// it may have touched, and never gives them its new statement.
#[test]
fn forgetting_what_a_statement_may_have_done() {
    let mut s = Prepared::default();
    s.classify("PREPARE pz AS DELETE FROM t");
    s.classify("PREPARE ok AS SELECT 1");
    // `PREPARE pz AS SELECT 1` failed on the server (the name exists): pz is unknown, not a read.
    s.forget("PREPARE pz AS SELECT 1");
    assert!(!s.knows("pz") && s.knows("ok"));
    assert_eq!(s.classify("EXECUTE pz").danger, Some(Danger::UnknownPrepared));
    // A PREPARE that never ran adds nothing.
    s.forget("PREPARE fresh AS SELECT 1");
    assert!(!s.knows("fresh"));
    // Re-preparing the same statement touches nothing that changes.
    s.classify("PREPARE same AS SELECT 2");
    s.forget("PREPARE same AS SELECT 2");
    assert!(s.knows("same"));
    for sql in ["DEALLOCATE ok", "DEALLOCATE ALL", "DISCARD ALL", "DO $$ BEGIN END $$", "PREPARE ok AS SELEC"] {
        s.classify("PREPARE ok AS SELECT 1");
        s.forget(sql);
        assert!(!s.knows("ok"), "{sql}");
    }
    s.classify("PREPARE ok AS SELECT 1");
    s.forget("SELECT 1/0");
    assert!(s.knows("ok"), "a statement that cannot prepare touches no name");
    // Code the text does not show may prepare or deallocate: after DO or CALL nothing is known.
    s.classify("CALL p()");
    assert!(!s.knows("ok"));
}

/// Code the text does not show may prepare or deallocate (a plpgsql function ran
/// `DEALLOCATE pf; PREPARE pf AS DELETE …` and a later `EXECUTE pf` deleted without asking).
/// After a statement that calls a function that is not a known built-in, or that may fire a
/// trigger (any write, DDL, maintenance, `COMMIT`), or runs a cursor's query (`FETCH`), no
/// name is known, whether it succeeded or its outcome is unknown.
#[test]
fn code_the_text_does_not_show_forgets_every_name() {
    let known = || {
        let mut s = Prepared::default();
        s.classify("PREPARE pf AS SELECT 1");
        assert!(s.knows("pf"));
        s
    };
    for sql in [
        "SELECT zz_swap()",
        "select public.zz_swap()",
        "SELECT pg_catalog.zz_swap()",
        "SELECT \"Lower\"('x')",
        "SELECT * FROM zz_rows()",
        "SELECT 1 WHERE zz_check()",
        "SELECT lower(zz_swap()::text)",
        "SELECT count(*) FILTER (WHERE zz_check()) FROM t",
        "VALUES (zz_swap())",
        "SELECT query_to_xml('SELECT zz_swap()', true, true, '')",
        "SELECT pg_catalog.ts_stat('SELECT zz_swap()')",
        "SELECT cursor_to_xml('c', 1, true, true, '')",
        "EXPLAIN SELECT zz_swap()",
        "EXECUTE pf(zz_swap())",
        "DECLARE c CURSOR FOR SELECT zz_swap()",
        "FETCH ALL FROM c",
        "MOVE NEXT IN c",
        "COMMIT",
        "END",
        "PREPARE TRANSACTION 'x'",
        "SET CONSTRAINTS ALL IMMEDIATE",
        "INSERT INTO t VALUES (1)",
        "UPDATE t SET a = 1 WHERE id = 1",
        "DELETE FROM t WHERE id = 1",
        "WITH d AS (DELETE FROM t WHERE id = 1 RETURNING 1) SELECT 1",
        "SELECT * FROM t FOR UPDATE",
        "CREATE TABLE zz (a int)",
        "ALTER TABLE t ADD b int",
        "CREATE INDEX ON t (zz_key(a))",
        "VACUUM t",
        "ANALYZE t",
        "LOAD 'zz'",
        "DO $$ BEGIN END $$",
        "CALL p()",
    ] {
        let mut s = known();
        s.classify(sql);
        assert!(!s.knows("pf"), "{sql}");
        let mut s = known();
        s.forget(sql);
        assert!(!s.knows("pf"), "{sql}: outcome unknown");
    }
    // In one run: the EXECUTE after the call asks, and a read-only policy refuses it.
    let r = known().classify("SELECT zz_swap(); EXECUTE pf");
    assert_eq!((r.class, r.danger), (Class::Unknown, Some(Danger::UnknownPrepared)));
    assert!(r.read_only().is_err());
    // Preparing such a statement runs nothing; executing it forgets every name, its own too.
    let mut s = known();
    s.classify("PREPARE sw AS SELECT zz_swap()");
    assert!(s.knows("pf") && s.knows("sw"), "preparing runs nothing");
    assert_eq!(s.classify("EXECUTE sw").class, Class::Read);
    assert!(!s.knows("pf") && !s.knows("sw"));
    let mut s = known();
    s.classify("PREPARE ins AS INSERT INTO t VALUES (1)");
    s.classify("EXECUTE ins");
    assert!(!s.knows("pf") && !s.knows("ins"), "an INSERT may fire a trigger");
    // Built-in functions, and statements that run no code, leave the names.
    for sql in [
        "SELECT 1",
        "SELECT 1/0",
        "SELECT count(*), now(), lower('A'), pg_catalog.upper('a'), length(name) FROM t",
        "SELECT coalesce(NULL, 1), greatest(1, 2), nullif(1, 2), CURRENT_USER, CURRENT_TIMESTAMP",
        "SELECT extract(year FROM now()), substring('abc' FROM 2), trim(' a '), now() AT TIME ZONE 'UTC'",
        "SELECT * FROM generate_series(1, 3) g, unnest(ARRAY[1, 2]) u",
        "SELECT set_config('application_name', 'x', false)",
        "SELECT jsonb_build_object('a', 1), string_agg(name, ',') FROM t",
        "EXPLAIN SELECT lower('a')",
        "EXECUTE pf",
        "DECLARE c CURSOR FOR SELECT 1",
        "CLOSE c",
        "SHOW search_path",
        "SET search_path = public",
        "RESET ALL",
        "LISTEN x",
        "DEALLOCATE other",
        "BEGIN",
        "START TRANSACTION READ ONLY",
        "SAVEPOINT a",
        "RELEASE a",
        "ROLLBACK TO a",
        "ROLLBACK",
    ] {
        let mut s = known();
        s.classify(sql);
        assert!(s.knows("pf"), "{sql}");
        let mut s = known();
        s.forget(sql);
        assert!(s.knows("pf"), "{sql}: outcome unknown");
    }
}

/// The built-in functions a call may name without making the prepared names unknown: the
/// functions of PostgreSQL 17's own catalog, less those that run SQL they are given.
#[test]
fn builtin_functions() {
    let names: Vec<&str> = BUILTINS.lines().collect();
    assert!(names.len() > 2500, "{}", names.len());
    assert!(names.windows(2).all(|w| w[0] < w[1]), "sorted, no duplicates");
    assert!(names.iter().all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')));
    for name in ["count", "now", "lower", "generate_series", "set_config", "jsonb_build_object", "extract"] {
        assert!(builtin(name), "{name}");
    }
    for name in RUNS_CODE.iter().copied().chain(["zz_swap", "Lower", "", "plpgsql_call_handler"]) {
        assert!(!builtin(name), "{name}");
    }
}

/// `ts_rewrite(tsquery, text)` runs the query it is given (which may call a function
/// that re-prepares a name), so it forgets the prepared names like any built-in of
/// [`RUNS_CODE`], bare or qualified with `pg_catalog`, whether it succeeded or not.
#[test]
fn builtins_that_run_code_they_are_given_forget_the_prepared_names() {
    let known = || {
        let mut s = Prepared::default();
        s.classify("PREPARE pf AS SELECT 1");
        s
    };
    let swap = "SELECT ts_rewrite('a'::tsquery, 'SELECT ''a''::tsquery, ''b''::tsquery WHERE zz_swap() = 1')";
    let calls = RUNS_CODE.iter().flat_map(|f| [format!("SELECT {f}('x')"), format!("SELECT pg_catalog.{f}('x')")]);
    for sql in [swap.to_string(), "SELECT pg_input_is_valid('x', 'zz_dom')".into()].into_iter().chain(calls) {
        let mut s = known();
        s.classify(&sql);
        assert!(!s.knows("pf"), "{sql}");
        let mut s = known();
        s.forget(&sql);
        assert!(!s.knows("pf"), "{sql}: outcome unknown");
    }
    let r = known().classify(&format!("{swap}; EXECUTE pf"));
    assert_eq!(r.danger, Some(Danger::UnknownPrepared));
    assert!(r.read_only().is_err());
}

/// A built-in that reads, writes or lists files of the server, or acts on the
/// server beyond the transaction, asks under the default policy (naming the function), and a
/// read-only policy refuses it, since the server's read-only transaction does not stop it
/// (`lo_export` wrote a file on a read-only profile). Wherever the call runs; not where it is
/// only planned or stored. It runs no code of the user's: the prepared names stay.
#[test]
fn builtins_that_act_on_the_server_ask_and_are_refused_when_read_only() {
    for (sql, danger, name) in [
        ("SELECT lo_export(16400, '/tmp/zz')", Danger::ServerFile, "lo_export"),
        ("SELECT lo_import('/etc/passwd')", Danger::ServerFile, "lo_import"),
        ("select PG_CATALOG.PG_READ_FILE('/etc/passwd')", Danger::ServerFile, "pg_read_file"),
        ("SELECT pg_read_binary_file('x', 0, 10, true)", Danger::ServerFile, "pg_read_binary_file"),
        ("SELECT * FROM pg_ls_dir('.')", Danger::ServerFile, "pg_ls_dir"),
        ("SELECT (pg_stat_file('x')).size", Danger::ServerFile, "pg_stat_file"),
        ("SELECT * FROM pg_ls_waldir()", Danger::ServerFile, "pg_ls_waldir"),
        ("WITH f AS (SELECT pg_read_file('x') AS c) SELECT c FROM f", Danger::ServerFile, "pg_read_file"),
        ("SELECT 1 FROM t WHERE lo_export(t.o, t.p) = 1", Danger::ServerFile, "lo_export"),
        ("EXPLAIN ANALYZE SELECT pg_read_file('x')", Danger::ServerFile, "pg_read_file"),
        ("DECLARE c CURSOR FOR SELECT pg_ls_dir('.')", Danger::ServerFile, "pg_ls_dir"),
        ("SELECT 1; SELECT pg_ls_tmpdir()", Danger::ServerFile, "pg_ls_tmpdir"),
        ("SELECT pg_terminate_backend(pid) FROM pg_stat_activity", Danger::ServerAction, "pg_terminate_backend"),
        ("SELECT pg_cancel_backend(1)", Danger::ServerAction, "pg_cancel_backend"),
        ("SELECT pg_reload_conf()", Danger::ServerAction, "pg_reload_conf"),
        ("SELECT pg_catalog.pg_switch_wal()", Danger::ServerAction, "pg_switch_wal"),
        ("SELECT pg_create_restore_point('x')", Danger::ServerAction, "pg_create_restore_point"),
        (
            "SELECT pg_create_logical_replication_slot('s', 'pgoutput')",
            Danger::ServerAction,
            "pg_create_logical_replication_slot",
        ),
        ("SELECT pg_drop_replication_slot('s')", Danger::ServerAction, "pg_drop_replication_slot"),
        (
            "SELECT * FROM pg_logical_slot_get_changes('s', NULL, NULL)",
            Danger::ServerAction,
            "pg_logical_slot_get_changes",
        ),
        ("SELECT pg_stat_reset()", Danger::ServerAction, "pg_stat_reset"),
        ("SELECT brin_summarize_range('i', 0)", Danger::ServerAction, "brin_summarize_range"),
        ("SELECT pg_logical_emit_message(false, 'p', 'x')", Danger::ServerAction, "pg_logical_emit_message"),
        ("SELECT pg_promote()", Danger::ServerAction, "pg_promote"),
        ("INSERT INTO t SELECT pg_rotate_logfile()", Danger::ServerAction, "pg_rotate_logfile"),
    ] {
        let r = classify(sql);
        assert_eq!((r.danger, r.target.as_deref()), (Some(danger), Some(name)), "{sql}");
        assert_eq!(r.confirm(false), Some(Why::Danger(danger)), "{sql}");
        let block = if danger == Danger::ServerFile { ReadOnlyBlock::ServerFile } else { ReadOnlyBlock::ServerAction };
        assert_eq!(r.read_only(), Err(block), "{sql}");
    }
    // Prepared: the EXECUTE asks and is refused; preparing runs nothing.
    let mut s = Prepared::default();
    assert_eq!(s.classify("PREPARE e AS SELECT lo_export(1, '/tmp/zz')").danger, None);
    assert_eq!(s.classify("EXECUTE e").read_only(), Err(ReadOnlyBlock::ServerFile));
    assert_eq!(classify("EXECUTE pf(pg_read_file('x'))").danger, Some(Danger::UnknownPrepared));
    let mut s = Prepared::default();
    s.classify("PREPARE pf(text) AS SELECT $1");
    assert_eq!(s.classify("EXECUTE pf(pg_read_file('x'))").read_only(), Err(ReadOnlyBlock::ServerFile));
    // No code of the user's runs: the names stay.
    let mut s = Prepared::default();
    s.classify("PREPARE pf AS SELECT 1");
    s.classify("SELECT pg_read_file('x'), pg_terminate_backend(1)");
    assert!(s.knows("pf"));
    // Only planned or stored, a user's function of that name, and the harmless built-ins.
    for sql in [
        "EXPLAIN SELECT pg_read_file('x')",
        "CREATE VIEW v AS SELECT pg_read_file('x')",
        "CREATE FUNCTION f() RETURNS text BEGIN ATOMIC SELECT pg_read_file('x'); END",
        "CREATE RULE r AS ON INSERT TO t DO ALSO SELECT pg_terminate_backend(1)",
        "SELECT \"PG_READ_FILE\"('x')",
        "SELECT pg_notify('c', 'x'), pg_advisory_lock(1), pg_try_advisory_xact_lock(2), nextval('s')",
        "SELECT lo_get(1), pg_current_logfile(), pg_sleep(0), set_config('work_mem', '1MB', false)",
        "SELECT * FROM pg_logical_slot_peek_changes('s', NULL, NULL)",
    ] {
        let r = classify(sql);
        assert!(!matches!(r.danger, Some(Danger::ServerFile | Danger::ServerAction)), "{sql}: {r:?}");
    }
    assert_eq!(classify("SELECT pg_notify('c', 'x')").read_only(), Ok(()));
}

/// A built-in that acts on the server is recognised whatever the form of its
/// call. A name qualified with the database (`datarig.pg_catalog.lo_export(…)`, which the
/// server accepts) or with any other schema (a function of the user's with that name asks too),
/// and attribute notation (`('/etc/hostname'::text).pg_read_file`, `t.pg_ls_dir`: a column
/// reference or an indirection to the parser, which the server runs as a call of one argument)
/// were sent on a read-only profile, and `lo_export` wrote a file. Every position where a call
/// runs counts, as for the plain call; a real column with such a name asks too.
#[test]
fn builtins_that_act_on_the_server_are_seen_in_any_qualification_and_attribute_notation() {
    for (sql, danger, name) in [
        ("SELECT datarig.pg_catalog.lo_export(16400, '/tmp/zz')", Danger::ServerFile, "lo_export"),
        ("SELECT datarig.pg_catalog.pg_ls_dir('.')", Danger::ServerFile, "pg_ls_dir"),
        ("SELECT * FROM db.pg_catalog.pg_ls_dir('.')", Danger::ServerFile, "pg_ls_dir"),
        ("SELECT db.pg_catalog.pg_terminate_backend(1)", Danger::ServerAction, "pg_terminate_backend"),
        ("SELECT zz.pg_read_file('x')", Danger::ServerFile, "pg_read_file"),
        ("SELECT ('/etc/hostname'::text).pg_read_file", Danger::ServerFile, "pg_read_file"),
        ("SELECT ('base'::text).pg_ls_dir", Danger::ServerFile, "pg_ls_dir"),
        ("SELECT ('x'::text).pg_stat_file", Danger::ServerFile, "pg_stat_file"),
        ("SELECT ('/etc/passwd'::text).lo_import", Danger::ServerFile, "lo_import"),
        ("SELECT (0).pg_cancel_backend", Danger::ServerAction, "pg_cancel_backend"),
        ("SELECT (pid).pg_terminate_backend FROM pg_stat_activity", Danger::ServerAction, "pg_terminate_backend"),
        ("SELECT ('slot'::name).pg_drop_replication_slot", Danger::ServerAction, "pg_drop_replication_slot"),
        ("SELECT (x.c).pg_read_file FROM x", Danger::ServerFile, "pg_read_file"),
        ("SELECT ('base'::text).pg_catalog.pg_ls_dir", Danger::ServerFile, "pg_ls_dir"),
        ("SELECT t.pg_read_file FROM t", Danger::ServerFile, "pg_read_file"),
        ("SELECT v.c.pg_read_file FROM (VALUES ('x')) v(c)", Danger::ServerFile, "pg_read_file"),
        ("SELECT ((0).pg_cancel_backend).x", Danger::ServerAction, "pg_cancel_backend"),
        ("SELECT ('i'::regclass).brin_summarize_new_values", Danger::ServerAction, "brin_summarize_new_values"),
        // Where the call runs: a condition, a CTE, LATERAL, VALUES, ORDER BY, DML, EXPLAIN
        // ANALYZE, a cursor.
        ("SELECT 1 WHERE ('x'::text).pg_read_file IS NOT NULL", Danger::ServerFile, "pg_read_file"),
        ("WITH f AS (SELECT ('x'::text).pg_read_file) SELECT * FROM f", Danger::ServerFile, "pg_read_file"),
        ("SELECT * FROM t, LATERAL (SELECT t.pg_ls_dir) x", Danger::ServerFile, "pg_ls_dir"),
        ("VALUES ((0).pg_cancel_backend)", Danger::ServerAction, "pg_cancel_backend"),
        ("SELECT 1 ORDER BY ('x'::text).pg_read_file", Danger::ServerFile, "pg_read_file"),
        ("INSERT INTO t SELECT ('x'::text).pg_read_file", Danger::ServerFile, "pg_read_file"),
        ("UPDATE t SET a = ('x'::text).pg_read_file WHERE id = 1", Danger::ServerFile, "pg_read_file"),
        ("EXPLAIN ANALYZE SELECT ('x'::text).pg_read_file", Danger::ServerFile, "pg_read_file"),
        ("DECLARE c CURSOR FOR SELECT ('.'::text).pg_ls_dir", Danger::ServerFile, "pg_ls_dir"),
        ("EXPLAIN ANALYZE SELECT datarig.pg_catalog.pg_read_file('x')", Danger::ServerFile, "pg_read_file"),
    ] {
        let r = classify(sql);
        assert_eq!((r.danger, r.target.as_deref()), (Some(danger), Some(name)), "{sql}");
        assert_eq!(r.confirm(false), Some(Why::Danger(danger)), "{sql}");
        let block = if danger == Danger::ServerFile { ReadOnlyBlock::ServerFile } else { ReadOnlyBlock::ServerAction };
        assert_eq!(r.read_only(), Err(block), "{sql}");
    }
    // Prepared: the body at EXECUTE, and the parameters of an EXECUTE.
    let mut s = Prepared::default();
    s.classify("PREPARE e AS SELECT ('x'::text).pg_read_file");
    s.classify("PREPARE q AS SELECT datarig.pg_catalog.lo_export(1, '/tmp/zz')");
    s.classify("PREPARE pf(text) AS SELECT $1");
    assert_eq!(s.classify("EXECUTE e").read_only(), Err(ReadOnlyBlock::ServerFile));
    assert_eq!(s.classify("EXECUTE q").read_only(), Err(ReadOnlyBlock::ServerFile));
    assert_eq!(s.classify("EXECUTE pf(('x'::text).pg_read_file)").read_only(), Err(ReadOnlyBlock::ServerFile));
    assert_eq!(s.classify("EXECUTE pf(db.pg_catalog.pg_read_file('x'))").read_only(), Err(ReadOnlyBlock::ServerFile));
    // Only planned, a single name (a column, never a call), a name that is not a call, and
    // other built-ins in attribute notation.
    for sql in [
        "EXPLAIN SELECT ('x'::text).pg_read_file",
        "EXPLAIN SELECT datarig.pg_catalog.pg_read_file('x')",
        "CREATE VIEW v AS SELECT ('x'::text).pg_read_file",
        "SELECT pg_read_file FROM t",
        "SELECT 1 AS pg_read_file",
        "UPDATE t SET pg_read_file = 1 WHERE id = 1",
        "INSERT INTO t (pg_ls_dir) VALUES (1)",
        "SELECT ('a'::text).upper, t.lower FROM t",
    ] {
        let r = classify(sql);
        assert!(!matches!(r.danger, Some(Danger::ServerFile | Danger::ServerAction)), "{sql}: {r:?}");
    }
}

/// The built-in allowlist reads a name qualified with the database like one
/// qualified with `pg_catalog`, and another schema is a function of the user's; a built-in of
/// [`RUNS_CODE`] in attribute notation forgets the prepared names like its call. Other
/// functions in attribute notation are not seen (module docs).
#[test]
fn qualified_and_attribute_calls_and_the_prepared_names() {
    let known = || {
        let mut s = Prepared::default();
        s.classify("PREPARE pf AS SELECT 1");
        s
    };
    for sql in [
        "SELECT datarig.zz.lower('x')",
        "SELECT datarig.pg_catalog.ts_rewrite('a'::tsquery, 'SELECT 1')",
        "SELECT ('SELECT ''a''::tsvector'::text).ts_stat",
        "SELECT t.ts_stat FROM t",
        "SELECT ('i'::regclass).brin_summarize_new_values",
        "SELECT ('x'::text).pg_catalog.ts_stat",
    ] {
        let mut s = known();
        s.classify(sql);
        assert!(!s.knows("pf"), "{sql}");
    }
    for sql in
        ["SELECT datarig.pg_catalog.lower('x')", "SELECT ('a'::text).upper, t.lower FROM t", "SELECT ts_stat FROM t"]
    {
        let mut s = known();
        s.classify(sql);
        assert!(s.knows("pf"), "{sql}");
    }
}

/// `query_to_xml(<sql>, true, false, '')` around `sql`, `n` times (each level doubles the
/// quotes of the one inside it).
fn nest(sql: &str, n: usize) -> String {
    (0..n).fold(sql.to_string(), |q, _| format!("SELECT query_to_xml('{}', true, false, '')", q.replace('\'', "''")))
}

/// A built-in that runs a query given as text runs whatever that query calls, where
/// a read-only transaction does not stop it: on a read-only profile `SELECT query_to_xml('select
/// lo_export(…)', true, false, '')` wrote a file and `ts_stat('select to_tsvector(''simple'',
/// pg_read_file(…))')` read one. A string constant is read as a statement, and its risk is the
/// call's, in every form of the call and wherever it runs.
#[test]
fn builtins_that_run_a_query_given_as_text_have_the_risk_of_the_query() {
    let file = Danger::ServerFile;
    for (sql, danger, name) in [
        (
            "SELECT query_to_xml('select datarig.pg_catalog.lo_export(7964525, ''/tmp/zz_c'')', true, false, '')",
            file,
            "lo_export",
        ),
        (
            "SELECT word FROM ts_stat('select to_tsvector(''simple'', pg_read_file(''/etc/hostname''))')",
            file,
            "pg_read_file",
        ),
        ("SELECT * FROM pg_catalog.ts_stat('SELECT pg_ls_dir(''.'')::tsvector', 'a')", file, "pg_ls_dir"),
        (
            "SELECT ts_rewrite('a'::tsquery, 'SELECT pg_read_file(''x'')::tsquery, ''b''::tsquery')",
            file,
            "pg_read_file",
        ),
        (
            "SELECT query_to_xmlschema('SELECT pg_terminate_backend(-1)', true, false, '')",
            Danger::ServerAction,
            "pg_terminate_backend",
        ),
        (
            "SELECT query_to_xml_and_xmlschema($$SELECT pg_reload_conf()$$, true, false, '')",
            Danger::ServerAction,
            "pg_reload_conf",
        ),
        ("SELECT query_to_xml(E'SELECT pg_read_file(\\'x\\')', true, false, '')", file, "pg_read_file"),
        ("SELECT query_to_xml(U&'SELECT pg_read_file(\\0027x\\0027)', true, false, '')", file, "pg_read_file"),
        ("SELECT query_to_xml('SELECT pg_read_file(''x'')'::text, true, false, '')", file, "pg_read_file"),
        ("SELECT query_to_xml(CAST('SELECT pg_ls_dir(''.'')' AS pg_catalog.text), true, false, '')", file, "pg_ls_dir"),
        (
            "SELECT datarig.pg_catalog.query_to_xml('SELECT lo_import(''/etc/passwd'')', true, false, '')",
            file,
            "lo_import",
        ),
        ("SELECT zz.query_to_xml('SELECT pg_read_file(''x'')', true, false, '')", file, "pg_read_file"),
        // Named notation, in any order, and mixed.
        (
            "SELECT query_to_xml(nulls => true, tableforest => false, targetns => '', query => 'SELECT pg_ls_dir(''.'')')",
            file,
            "pg_ls_dir",
        ),
        (
            "SELECT query_to_xml('SELECT pg_ls_dir(''.'')', nulls => true, tableforest => false, targetns => '')",
            file,
            "pg_ls_dir",
        ),
        // Attribute notation (only `ts_stat` has a form of one argument).
        ("SELECT ('SELECT to_tsvector(pg_read_file(''x''))'::text).ts_stat", file, "pg_read_file"),
        ("SELECT (('SELECT pg_ls_dir(''.'')::tsvector'::text).ts_stat).word", file, "pg_ls_dir"),
        // A query text in a query text.
        (&nest("SELECT pg_read_file('x')", 2), file, "pg_read_file"),
        (&nest("SELECT pg_terminate_backend(-1)", 5), Danger::ServerAction, "pg_terminate_backend"),
        (
            "SELECT query_to_xml('SELECT * FROM ts_stat(''SELECT to_tsvector(pg_read_file(x))'')', true, false, '')",
            file,
            "pg_read_file",
        ),
        // The text as a server with `standard_conforming_strings = off` reads it.
        (
            "SELECT query_to_xml($q$SELECT 'x\\' AS a WHERE false --', pg_read_file('y')$q$, true, false, '')",
            file,
            "pg_read_file",
        ),
        // Wherever the call runs.
        (
            "WITH q AS (SELECT query_to_xml('SELECT pg_read_file(''x'')', true, false, '') x) SELECT * FROM q",
            file,
            "pg_read_file",
        ),
        (
            "SELECT 1 WHERE query_to_xml('SELECT pg_read_file(''x'')', true, false, '') IS NOT NULL",
            file,
            "pg_read_file",
        ),
        ("EXPLAIN ANALYZE SELECT query_to_xml('SELECT pg_read_file(''x'')', true, false, '')", file, "pg_read_file"),
        ("DECLARE c CURSOR FOR SELECT * FROM ts_stat('SELECT to_tsvector(pg_read_file(''x''))')", file, "pg_read_file"),
        ("INSERT INTO t SELECT query_to_xml('SELECT pg_read_file(''x'')', true, false, '')", file, "pg_read_file"),
        ("CREATE TABLE zz AS SELECT query_to_xml('SELECT pg_read_file(''x'')', true, false, '')", file, "pg_read_file"),
    ] {
        let r = classify(sql);
        assert_eq!((r.danger, r.target.as_deref()), (Some(danger), Some(name)), "{sql}");
        assert_eq!(r.confirm(false), Some(Why::Danger(danger)), "{sql}");
        let block = if danger == Danger::ServerFile { ReadOnlyBlock::ServerFile } else { ReadOnlyBlock::ServerAction };
        assert_eq!(r.read_only(), Err(block), "{sql}");
    }
    // The query's writes and settings count like the call's own (the server refuses its
    // writes, but the classifier does not rely on it).
    let r = classify("SELECT query_to_xml('DELETE FROM t', true, false, '')");
    assert_eq!((r.class, r.danger), (Class::Write, Some(Danger::DeleteAll)));
    assert_eq!(r.read_only(), Err(ReadOnlyBlock::Class(Class::Write)));
    let r = classify(
        "SELECT query_to_xml('SELECT set_config(''default_transaction_read_only'', ''off'', false)', true, false, '')",
    );
    assert_eq!(r.read_only(), Err(ReadOnlyBlock::ReadWrite));
    let r = classify("SELECT query_to_xml('EXPLAIN ANALYZE SELECT 1', true, false, '')");
    assert!(!r.rolls_back() && !r.stdio, "{r:?}");
    assert!(!classify("SELECT query_to_xml('COPY t TO STDOUT', true, false, '')").stdio);
    // A harmless query stays harmless: no confirm under any policy, and a read-only policy runs it.
    for sql in [
        "SELECT query_to_xml('select 1', true, false, '')",
        "SELECT * FROM ts_stat('SELECT to_tsvector(body) FROM docs')",
        "SELECT * FROM ts_stat('SELECT to_tsvector(body) FROM docs', 'ab')",
        "SELECT ts_rewrite('a & b'::tsquery, 'SELECT t, s FROM aliases')",
        "SELECT ts_rewrite('a'::tsquery, 'a'::tsquery, 'b'::tsquery)",
        "SELECT query_to_xmlschema('SELECT * FROM t WHERE id = 1', true, false, '')",
        "SELECT query_to_xml(query => 'SELECT 1', nulls => true, tableforest => false, targetns => '')",
        "SELECT ('SELECT to_tsvector(''a'')'::text).ts_stat",
        "SELECT cursor_to_xml('c', 1, true, true, '')",
        "SELECT ts_stat FROM t",
        &nest("SELECT 1", 2),
        &nest("SELECT 1", MAX_QUERY_TEXT_NESTING),
        "EXPLAIN SELECT query_to_xml('SELECT pg_read_file(''x'')', true, false, '')",
        "CREATE VIEW v AS SELECT query_to_xml('SELECT pg_read_file(''x'')', true, false, '')",
        "CREATE VIEW v AS SELECT query_to_xml(q, true, false, '') FROM t",
    ] {
        let r = classify(sql);
        assert_eq!(r.danger, None, "{sql}: {r:?}");
        assert_eq!(r.confirm(false), None, "{sql}");
        if !sql.starts_with("CREATE") {
            assert_eq!((r.class, r.read_only(), r.confirm(true)), (Class::Read, Ok(()), None), "{sql}");
        }
    }
}

/// When the text of the query is not a string constant the classifier can read, what
/// the query does is unknown: it asks under the default policy, naming the function, and a
/// read-only policy refuses it. So do a text the parser rejects and one over the caps.
#[test]
fn a_query_given_as_text_that_cannot_be_read_asks_and_is_refused_when_read_only() {
    let deep = nest("SELECT 1", MAX_QUERY_TEXT_NESTING + 1);
    let padding = format!("SELECT 1{}", " + 1".repeat(12_000));
    let wide = nest(&padding, 6);
    assert!(wide.len() < MAX_BYTES && depth(&wide) < MAX_DEPTH, "{}", wide.len());
    let tall = nest(&format!("SELECT 1{}", " + 1".repeat(MAX_DEPTH)), 1);
    assert!(depth(&tall) < MAX_DEPTH, "the outer text is shallow");
    for (sql, name) in [
        ("SELECT query_to_xml(q, true, false, '') FROM t", "query_to_xml"),
        ("SELECT query_to_xml('SELECT ' || 'pg_read_file(''x'')', true, false, '')", "query_to_xml"),
        ("SELECT query_to_xml(format('SELECT %s', 1), true, false, '')", "query_to_xml"),
        ("SELECT query_to_xml('SELECT 1'::varchar(8), true, false, '')", "query_to_xml"),
        ("SELECT query_to_xml('SELECT 1'::name, true, false, '')", "query_to_xml"),
        ("SELECT query_to_xml('SELECT 1' COLLATE \"C\", true, false, '')", "query_to_xml"),
        ("SELECT query_to_xml(VARIADIC ARRAY['SELECT 1'])", "query_to_xml"),
        ("SELECT query_to_xml(nulls => true, tableforest => false, targetns => '')", "query_to_xml"),
        ("SELECT query_to_xmlschema(current_setting('zz.q'), true, false, '')", "query_to_xmlschema"),
        ("SELECT * FROM ts_stat(current_setting('zz.q'))", "ts_stat"),
        ("SELECT * FROM ts_stat($1)", "ts_stat"),
        ("SELECT ts_rewrite('a'::tsquery, q) FROM t", "ts_rewrite"),
        ("SELECT t.ts_stat FROM t", "ts_stat"),
        ("SELECT (t.q).ts_stat FROM t", "ts_stat"),
        ("SELECT (ARRAY['SELECT 1'])[1].ts_stat", "ts_stat"),
        ("SELECT ('SELECT to_tsvector(''a'')'::text).pg_catalog.ts_stat", "ts_stat"),
        ("SELECT query_to_xml('SELEC 1', true, false, '')", "query_to_xml"),
        (&tall, "query_to_xml"),
        (&deep, "query_to_xml"),
        (&wide, "query_to_xml"),
        ("EXECUTE pf(query_to_xml(current_user, true, false, '')::text)", "query_to_xml"),
    ] {
        let mut s = Prepared::default();
        s.classify("PREPARE pf(text) AS SELECT $1");
        let start = std::time::Instant::now();
        let r = s.classify(sql);
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "{name}: {:?}", start.elapsed());
        assert_eq!((r.danger, r.target.as_deref()), (Some(Danger::RunsQueryText), Some(name)), "{sql}");
        assert_eq!(r.confirm(false), Some(Why::Danger(Danger::RunsQueryText)), "{sql}");
        assert_eq!(r.read_only(), Err(ReadOnlyBlock::RunsQueryText), "{sql}");
    }
    // A parameter in a prepared statement: the EXECUTE asks, whatever its argument.
    let mut s = Prepared::default();
    assert_eq!(s.classify("PREPARE q(text) AS SELECT query_to_xml($1, true, false, '')").read_only(), Ok(()));
    assert_eq!(s.clone().classify("EXECUTE q('SELECT 1')").read_only(), Err(ReadOnlyBlock::RunsQueryText));
    // A danger of the statement itself is the one said.
    let r = classify("DELETE FROM t WHERE query_to_xml(q, true, false, '') IS NULL OR true");
    assert_eq!(r.danger, Some(Danger::DeleteAll));
    let r = classify("SELECT pg_read_file('x'), query_to_xml(q, true, false, '') FROM t");
    assert_eq!((r.danger, r.target.as_deref()), (Some(Danger::ServerFile), Some("pg_read_file")));
}

/// `EXPLAIN` without `ANALYZE` of `EXECUTE` evaluates the parameters on the server
/// to plan the prepared statement: `EXPLAIN EXECUTE zz_p(lo_export(…)::text)` wrote a file on
/// a read-only profile. Its parameters count as in an `EXECUTE` that runs; the rest of what a
/// plain `EXPLAIN` wraps is only planned.
#[test]
fn a_plain_explain_of_execute_evaluates_its_parameters() {
    let mut s = Prepared::default();
    s.classify("PREPARE zz_p(text) AS SELECT length($1)");
    let file = Danger::ServerFile;
    for (sql, danger, name) in [
        ("EXPLAIN EXECUTE zz_p(lo_export(7964525, '/tmp/zz_d')::text)", file, "lo_export"),
        ("EXPLAIN EXECUTE zz_p(pg_terminate_backend(-1)::text)", Danger::ServerAction, "pg_terminate_backend"),
        ("explain verbose execute zz_p(pg_catalog.pg_read_file('x'))", file, "pg_read_file"),
        ("EXPLAIN (COSTS off, GENERIC_PLAN) EXECUTE zz_p(('x'::text).pg_read_file)", file, "pg_read_file"),
        ("EXPLAIN (ANALYZE false) EXECUTE zz_p(db.pg_catalog.pg_ls_dir('.'))", file, "pg_ls_dir"),
        ("EXPLAIN (FORMAT json) EXECUTE zz_p(pg_read_file('x'))", file, "pg_read_file"),
        ("EXPLAIN CREATE TABLE zz AS EXECUTE zz_p(pg_ls_dir('.'))", file, "pg_ls_dir"),
        (
            "EXPLAIN EXECUTE zz_p(query_to_xml('SELECT pg_read_file(''x'')', true, false, '')::text)",
            file,
            "pg_read_file",
        ),
        ("EXPLAIN EXECUTE zz_other(pg_read_file('x'))", file, "pg_read_file"),
        (
            "EXPLAIN EXECUTE zz_p(query_to_xml(current_user, true, false, '')::text)",
            Danger::RunsQueryText,
            "query_to_xml",
        ),
    ] {
        let r = s.clone().classify(sql);
        assert_eq!(r.explain, Explain::Plan, "{sql}");
        assert_eq!((r.danger, r.target.as_deref()), (Some(danger), Some(name)), "{sql}");
        assert_eq!(r.confirm(false), Some(Why::Danger(danger)), "{sql}");
        let block = match danger {
            Danger::ServerFile => ReadOnlyBlock::ServerFile,
            Danger::ServerAction => ReadOnlyBlock::ServerAction,
            _ => ReadOnlyBlock::RunsQueryText,
        };
        assert_eq!(r.read_only(), Err(block), "{sql}");
    }
    let sql = "EXPLAIN EXECUTE zz_p(set_config('default_transaction_read_only', 'off', false))";
    assert_eq!(s.clone().classify(sql).read_only(), Err(ReadOnlyBlock::ReadWrite));
    // Only planned: harmless parameters, the prepared statement's own calls, other statements.
    s.classify("PREPARE zz_f AS SELECT pg_read_file('x')");
    for sql in [
        "EXPLAIN EXECUTE zz_p('x')",
        "EXPLAIN EXECUTE zz_p(lower('X'))",
        "EXPLAIN EXECUTE zz_f",
        "EXPLAIN SELECT pg_read_file('x')",
        "EXPLAIN CREATE TABLE zz AS SELECT pg_read_file('x')",
        "EXPLAIN DECLARE c CURSOR FOR SELECT pg_read_file('x')",
        "EXPLAIN (GENERIC_PLAN) SELECT pg_read_file($1)",
    ] {
        let r = s.clone().classify(sql);
        assert_eq!((r.class, r.explain, r.danger, r.read_only()), (Class::Read, Explain::Plan, None, Ok(())), "{sql}");
    }
    // The planner may fold a function of the prepared statement: one that is not a known
    // built-in forgets the names, as a plain `EXPLAIN` of a query that calls it does.
    s.classify("PREPARE zz_sw AS SELECT zz_swap()");
    s.clone().classify("EXPLAIN EXECUTE zz_p('x')");
    let mut after = s.clone();
    after.classify("EXPLAIN EXECUTE zz_p('x')");
    assert!(after.knows("zz_p") && after.knows("zz_sw"));
    after.classify("EXPLAIN EXECUTE zz_sw");
    assert!(!after.knows("zz_p"));
}

/// The escape: `set_config()` of a read-only setting is refused by a read-only policy, in any
/// statement, at any depth, and whatever the value.
#[test]
fn set_config_of_a_read_only_setting_asks_for_read_write() {
    for sql in [
        "SELECT set_config('default_transaction_read_only','off',false)",
        "SELECT pg_catalog.set_config('transaction_read_only', 'off', true)",
        "SELECT SET_CONFIG('Default_Transaction_Read_Only', 'on', false)",
        "SELECT 1 FROM t WHERE set_config('default_transaction_read_only', 'off', false) IS NOT NULL",
        "WITH x AS (SELECT set_config('default_transaction_read_only', 'off', false)) SELECT * FROM x",
        "VALUES (set_config('default_transaction_read_only', 'off', false))",
        "SELECT set_config(name, 'off', false) FROM pg_settings",
        "SELECT set_config('default_transaction' || '_read_only', 'off', false)",
        "EXPLAIN ANALYZE SELECT set_config('default_transaction_read_only', 'off', false)",
    ] {
        assert_eq!(classify(sql).read_only(), Err(ReadOnlyBlock::ReadWrite), "{sql}");
    }
    let mut s = Prepared::default();
    assert_eq!(s.classify("PREPARE e AS SELECT set_config('transaction_read_only', 'off', true)").read_only(), Ok(()));
    assert_eq!(s.classify("EXECUTE e").read_only(), Err(ReadOnlyBlock::ReadWrite));
    for fine in [
        "SELECT set_config('search_path', 'shop', false)",
        "SELECT current_setting('default_transaction_read_only')",
        "EXPLAIN SELECT set_config('default_transaction_read_only', 'off', false)",
    ] {
        assert_eq!(classify(fine).read_only(), Ok(()), "{fine}");
    }
    // Not a confirmation under the default policy: calling functions does not ask.
    assert_eq!(classify("SELECT set_config('default_transaction_read_only','off',false)").confirm(false), None);
}

/// MERGE that updates or deletes always asks; one that only inserts does not.
#[test]
fn merge_that_updates_or_deletes_asks() {
    for sql in [
        "MERGE INTO t USING u ON true WHEN MATCHED THEN DELETE",
        "MERGE INTO t USING u ON t.id = u.id WHEN MATCHED THEN UPDATE SET x = u.x",
        "MERGE INTO t USING u ON t.id = u.id WHEN NOT MATCHED THEN INSERT VALUES (u.id) WHEN MATCHED AND u.gone THEN DELETE",
        "WITH s AS (SELECT 1 AS id) MERGE INTO t USING s ON t.id = s.id WHEN NOT MATCHED BY SOURCE THEN DELETE",
    ] {
        let r = classify(sql);
        assert_eq!(
            (r.class, r.danger, r.confirm(false)),
            (Class::Write, Some(Danger::Merge), Some(Why::Danger(Danger::Merge))),
            "{sql}"
        );
    }
    let r = classify("MERGE INTO t USING u ON t.id = u.id WHEN NOT MATCHED THEN INSERT VALUES (u.id)");
    assert_eq!((r.class, r.danger), (Class::Write, None));
}

/// Any change of a column's type asks: with `USING` it rewrites every value, and without it
/// the conversion can still lose data (`numeric(10,2)` to `int` rounds 1.75 to 2).
#[test]
fn alter_column_type_asks() {
    let r = classify("ALTER TABLE s.t ALTER COLUMN c TYPE int USING NULL");
    assert_eq!((r.danger, r.target.as_deref()), (Some(Danger::AlterColumnType), Some("s.t")));
    assert_eq!(danger("ALTER TABLE t ALTER c SET DATA TYPE text USING c::text"), Some(Danger::AlterColumnType));
    assert_eq!(danger("ALTER TABLE t ALTER c TYPE int"), Some(Danger::AlterColumnType));
    assert_eq!(danger("ALTER TABLE t ALTER COLUMN c SET DATA TYPE bigint"), Some(Danger::AlterColumnType));
    assert_eq!(danger("ALTER TYPE comp ALTER ATTRIBUTE a TYPE int"), Some(Danger::AlterColumnType));
    assert_eq!(danger("ALTER TABLE t ALTER c SET DEFAULT 1"), None);
    assert_eq!(danger("ALTER TABLE t ALTER c TYPE int USING NULL, DROP COLUMN x"), Some(Danger::DropColumn));
}

/// Data-modifying statements are found at any depth the parser allows them, and nothing in a
/// string, a comment, a rule or a function body counts.
#[test]
fn data_modifying_statements_anywhere_in_a_query() {
    for (sql, d) in [
        ("WITH a AS (WITH b AS (DELETE FROM t RETURNING 1) SELECT * FROM b) SELECT * FROM a", Some(Danger::DeleteAll)),
        ("WITH a AS (UPDATE t SET x = 1 WHERE id = 1 RETURNING *) INSERT INTO u SELECT * FROM a", None),
        ("INSERT INTO u WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d", Some(Danger::DeleteAll)),
        ("COPY (DELETE FROM t RETURNING *) TO STDOUT", Some(Danger::DeleteAll)),
        ("COPY (UPDATE t SET x = 1 WHERE id = 1 RETURNING *) TO STDOUT", None),
        ("WITH m AS (MERGE INTO t USING u ON true WHEN MATCHED THEN DELETE RETURNING *) SELECT 1", Some(Danger::Merge)),
    ] {
        let r = classify(sql);
        assert_eq!((r.class, r.danger), (Class::Write, d), "{sql}");
    }
    assert_eq!(class("COPY (SELECT 1) TO STDOUT"), Class::Read);
    assert_eq!(class("COPY (SELECT 1) TO '/tmp/x'"), Class::Write);
    assert_eq!(class("SELECT * INTO t2 FROM a UNION SELECT * FROM b"), Class::Ddl, "SELECT INTO in a set operation");
    assert_eq!(target("SELECT * INTO t2 FROM a UNION SELECT * FROM b").as_deref(), Some("t2"));
    assert_eq!(class("SELECT * FROM (SELECT * FROM t FOR UPDATE) s"), Class::Write);
    assert_eq!(class("CREATE VIEW v AS SELECT set_config('x.y', '1', false)"), Class::Ddl);
}

/// The second reading as a server with `standard_conforming_strings = off`, and texts the
/// parser rejects.
#[test]
fn backslashes_and_what_the_parser_rejects() {
    assert_eq!(non_conforming("SELECT 'a'"), None);
    assert_eq!(non_conforming("SELECT E'a\\b'"), None, "E'' strings read the same");
    assert_eq!(
        non_conforming("SELECT 'x\\''; DROP TABLE t; --'").as_deref(),
        Some("SELECT E'x\\''; DROP TABLE t; --'")
    );
    let r = classify("SELECT 'a\\' OR 1=1 --'");
    assert_eq!(r.class, Class::Read);
    let r = classify("UPDATE t SET a = 'x\\' WHERE id = 1 --'");
    assert_eq!(r.danger, Some(Danger::UpdateAll), "one of the readings has no WHERE");
    // A NUL byte never reaches the server as SQL: the parser cannot read it.
    assert_eq!(danger("SELECT 1\0; DROP TABLE t"), Some(Danger::Unparsed));
}

/// The lexer's estimate of the nesting: chains and groups count, lists and flattened
/// `AND`/`OR` do not; joins and set operations count whatever separates them.
#[test]
fn depth_estimate() {
    assert!(depth("SELECT 1") < 5);
    let plus = |n: usize| format!("SELECT 1{}", " + 1".repeat(n));
    assert!((2000..2010).contains(&depth(&plus(1000))), "{}", depth(&plus(1000)));
    let list = format!("SELECT * FROM t WHERE x IN ({})", vec!["1"; 50_000].join(", "));
    assert!(depth(&list) < 20);
    let rows = format!("INSERT INTO t VALUES {}", vec!["(1, -2, 'x')"; 20_000].join(", "));
    assert!(depth(&rows) < 20);
    let and = format!("SELECT 1 WHERE {}", vec!["a = 1"; 20_000].join(" AND "));
    assert!(depth(&and) < 20);
    // What separates the links of these chains does not hide them.
    let joins: String = (0..1000).map(|i| format!(" JOIN t{i} ON a = {i} AND b = 1, u")).collect();
    assert!(depth(&format!("SELECT * FROM t{joins}")) >= 2000);
    let unions = format!("SELECT 1, 2{}", " UNION SELECT 1, 2".repeat(1000));
    assert!(depth(&unions) >= 2000);
    let cases = format!("SELECT {}1{}", "CASE WHEN a AND b THEN ".repeat(1000), " END".repeat(1000));
    assert!(depth(&cases) >= 1000);
    let parens = format!("SELECT {}1{}", "(".repeat(1000), ")".repeat(1000));
    assert!(depth(&parens) >= 1000);
    let unclosed = format!("SELECT {}1", "f(1, ".repeat(1000));
    assert!(depth(&unclosed) >= 1000);
    // Each statement of a text on its own.
    assert!(depth(&format!("{};{}", plus(1000), plus(1000))) < 2010);
    // The second reading counts too: here only it sees the operators outside a string.
    let hidden = format!("SELECT 'a\\' || '{}' --'", " + 1".repeat(1000));
    assert!(depth(&hidden) >= 2000, "{}", depth(&hidden));
}

/// Texts too long or too deeply nested are not parsed (a crash found in review: 30 000 terms
/// overflowed the UI thread's stack): they ask, a read-only policy refuses them, and the
/// check takes no time. Nothing crashes at any size.
#[test]
fn too_complex_texts_are_dangerous_and_fast() {
    let plus = |n: usize| format!("SELECT 1{}", " + 1".repeat(n));
    let megabyte = format!("SELECT 1{}", " + 1".repeat(262_144));
    for sql in [plus(30_000), plus(100_000), megabyte, format!("SELECT '{}'", "x".repeat(MAX_BYTES))] {
        let start = std::time::Instant::now();
        let r = classify(&sql);
        assert!(start.elapsed() < std::time::Duration::from_secs(2), "{} bytes: {:?}", sql.len(), start.elapsed());
        assert_eq!((r.class, r.danger), (Class::Unknown, Some(Danger::TooComplex)), "{} bytes", sql.len());
        assert_eq!(r.confirm(false), Some(Why::Danger(Danger::TooComplex)));
        assert_eq!(r.read_only(), Err(ReadOnlyBlock::TooComplex));
    }
    assert_eq!(classify("SELECT 1 +").read_only(), Err(ReadOnlyBlock::Unparsed));
    // What a text that was not read may have prepared or deallocated is unknown.
    let mut s = Prepared::default();
    s.classify("PREPARE p AS SELECT 1");
    assert!(s.knows("p"));
    s.classify(&format!("PREPARE q AS {}", plus(30_000)));
    assert!(!s.knows("p"));
    s.classify("PREPARE p AS SELECT 1");
    s.classify(&plus(30_000));
    assert!(s.knows("p"), "a text that cannot prepare leaves the names");
    s.classify("DEALLOCATE p x");
    assert!(!s.knows("p"), "an unparsed DEALLOCATE may have run");
}

/// Just under the depth cap, every shape of deep text is read on the parse thread without
/// overflowing its stack (in the debug build too), in bounded time. The left-deep chains are
/// read to the end (reads); PostgreSQL's parser itself gives up on the right-nested ones
/// ("memory exhausted"), which then ask.
#[test]
fn deep_texts_under_the_cap_do_not_crash() {
    let n = MAX_DEPTH / 2 - 10;
    let joins: String = (0..n).map(|_| " JOIN t ON true").collect();
    for sql in [
        format!("SELECT 1{}", " + 1".repeat(n)),
        format!("SELECT 'a'{}", " || 'a'".repeat(MAX_DEPTH / 3 - 10)),
        format!("SELECT * FROM t{joins}"),
        format!("SELECT 1{}", " UNION SELECT 1".repeat(n)),
        format!("SELECT {}1{}", "(".repeat(MAX_DEPTH / 2 - 10), ")".repeat(MAX_DEPTH / 2 - 10)),
    ] {
        assert!(depth(&sql) <= MAX_DEPTH, "{}: {}", &sql[..40], depth(&sql));
        let start = std::time::Instant::now();
        assert_eq!(class(&sql), Class::Read, "{}", &sql[..40]);
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "{}: {:?}", &sql[..40], start.elapsed());
    }
    for sql in [
        format!("SELECT {}1", "- ".repeat(MAX_DEPTH - 10)),
        format!("SELECT {}true", "NOT ".repeat(MAX_DEPTH - 10)),
        format!("SELECT {}1{}", "(SELECT ".repeat(MAX_DEPTH / 3 - 10), ")".repeat(MAX_DEPTH / 3 - 10)),
        format!("SELECT {}1{}", "ARRAY[".repeat(MAX_DEPTH / 3 - 10), "]".repeat(MAX_DEPTH / 3 - 10)),
        format!("SELECT {}1{}", "CASE WHEN true THEN ".repeat(MAX_DEPTH / 5 - 10), " END".repeat(MAX_DEPTH / 5 - 10)),
    ] {
        assert!(depth(&sql) <= MAX_DEPTH, "{}: {}", &sql[..40], depth(&sql));
        let start = std::time::Instant::now();
        let r = classify(&sql);
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "{}: {:?}", &sql[..40], start.elapsed());
        assert_ne!(r.danger, Some(Danger::TooComplex), "{}", &sql[..40]);
    }
}

/// Ordinary long queries are reads (prost's limit of 100 nested messages made
/// about 46 chained operators or 50 JOINs unreadable, and a read-only policy refused them).
#[test]
fn long_chains_and_many_joins_are_reads() {
    let joins: String = (0..200).map(|i| format!(" JOIN t{i} ON t{i}.id = t.id")).collect();
    for sql in [
        format!("SELECT 1{}", " + 1".repeat(60)),
        format!("SELECT a{}", " || a".repeat(500)),
        format!("SELECT * FROM t{joins}"),
        format!("SELECT * FROM t WHERE {}", vec!["(a = 1 OR b = 2)"; 500].join(" AND ")),
    ] {
        let r = classify(&sql);
        assert_eq!((r.class, r.danger, r.read_only()), (Class::Read, None, Ok(())), "{}", &sql[..40]);
    }
}

/// `COPY … FROM STDIN` and `COPY … TO STDOUT` go through the connection's COPY protocol.
#[test]
fn copy_through_the_connection() {
    for sql in [
        "COPY t FROM STDIN",
        "COPY t (a) FROM STDIN WITH (FORMAT csv)",
        "COPY t TO STDOUT",
        "COPY (SELECT 1) TO STDOUT",
    ] {
        assert!(classify(sql).stdio, "{sql}");
    }
    for sql in ["COPY t FROM '/tmp/x'", "COPY t TO '/tmp/x'", "COPY t FROM PROGRAM 'cat'", "SELECT 1"] {
        assert!(!classify(sql).stdio, "{sql}");
    }
}

/// A program or a file on the server, in either direction, is dangerous: it asks
/// under the default policy, and a read-only policy refuses it.
#[test]
fn copy_with_a_program_or_a_server_file_is_dangerous() {
    for (sql, expected) in [
        ("COPY t FROM PROGRAM 'id'", Danger::CopyProgram),
        ("COPY t TO PROGRAM 'cat'", Danger::CopyProgram),
        ("copy (SELECT 1) to program 'sh -c x' WITH (FORMAT csv)", Danger::CopyProgram),
        ("COPY t (a) FROM PROGRAM 'id' WHERE a > 1", Danger::CopyProgram),
        ("COPY t FROM '/etc/passwd'", Danger::CopyFile),
        ("COPY t TO '/tmp/x'", Danger::CopyFile),
        ("COPY (SELECT * FROM t) TO '/tmp/x' (FORMAT binary)", Danger::CopyFile),
        ("COPY (DELETE FROM t RETURNING *) TO '/tmp/x'", Danger::CopyFile),
        ("SELECT 1; COPY t FROM '/etc/passwd'", Danger::CopyFile),
    ] {
        let r = classify(sql);
        assert_eq!(r.danger, Some(expected), "{sql}");
        assert_eq!(r.confirm(false), Some(Why::Danger(expected)), "{sql}");
        assert_eq!(r.read_only(), Err(ReadOnlyBlock::Class(Class::Write)), "{sql}");
        assert!(!r.stdio, "{sql}");
    }
    for sql in ["COPY t FROM STDIN", "COPY t TO STDOUT"] {
        assert_eq!(danger(sql), None, "{sql}: refused as not supported, not asked");
    }
}

/// `COPY <table> TO '<file>'` only reads the table: what it acts on is the file
/// (or the program), which the confirm names. `COPY … FROM` writes its table.
#[test]
fn copy_to_a_file_or_a_program_targets_it() {
    for (sql, expected) in [
        ("COPY shop.users TO '/tmp/x'", "'/tmp/x'"),
        ("COPY (SELECT * FROM t) TO '/tmp/it''s' (FORMAT csv)", "'/tmp/it''s'"),
        ("COPY t TO PROGRAM 'gzip > /tmp/x.gz'", "'gzip > /tmp/x.gz'"),
        ("COPY shop.users FROM '/etc/passwd'", "shop.users"),
        ("COPY t FROM PROGRAM 'id'", "t"),
    ] {
        assert_eq!(target(sql).as_deref(), Some(expected), "{sql}");
    }
}

/// What sets `search_path` for the session (a tab whose path is set per transaction
/// behind a pooler warns about it): `SET` without `LOCAL`, `SET SESSION`, `set_config()` whose
/// third argument is not the constant `true` (also of a setting named by an expression). Not
/// `SET LOCAL`, not `set_config(…, true)`, not another setting, not `RESET`.
#[test]
fn what_sets_the_path_for_the_session() {
    for sql in [
        "SET search_path TO shop",
        "set search_path = shop, public",
        "SET SESSION search_path TO DEFAULT",
        "SET search_path FROM CURRENT",
        "SELECT set_config('search_path', 'shop', false)",
        "SELECT pg_catalog.set_config('SEARCH_PATH', 'shop', 'off')",
        "SELECT set_config('search_path', 'shop', $1)",
        "SELECT 1; SET search_path = x",
        "WITH x AS (SELECT set_config('search_path', 's', false)) SELECT * FROM x",
    ] {
        assert!(classify(sql).session_path, "{sql}");
    }
    // Only a warning: a read-only policy still allows it as before.
    assert_eq!(classify("SET search_path TO shop").read_only(), Ok(()));
    // A setting named by an expression could be it (and a read-only policy refuses it anyway).
    assert!(classify("SELECT set_config(name, 'shop', false) FROM pg_settings").session_path);
    let mut s = Prepared::default();
    assert!(!s.classify("PREPARE p AS SELECT set_config('search_path', 's', false)").session_path, "not run yet");
    assert!(s.classify("EXECUTE p").session_path, "EXECUTE has its statement's");
    for sql in [
        "SET LOCAL search_path TO shop",
        "SELECT set_config('search_path', 'shop', true)",
        "SELECT set_config('work_mem', '64MB', false)",
        "SET work_mem = '64MB'",
        "RESET search_path",
        "RESET ALL",
        "SHOW search_path",
        "SELECT current_setting('search_path')",
        "SELECT 'set search_path = x'",
        "EXPLAIN SELECT set_config('search_path', 'shop', false)",
    ] {
        assert!(!classify(sql).session_path, "{sql}");
    }
}
