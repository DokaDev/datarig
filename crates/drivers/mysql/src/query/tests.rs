use super::*;

const MODE: MySqlMode = MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: true };
const MYSQL: Server = Server { version: (8, 4, 0), mariadb: false };

#[test]
fn outcomes_name_the_command_or_count_rows() {
    let tag = |sql| command_tag(sql, MODE);
    assert_eq!(tag("insert into t values (1)"), ("INSERT".into(), true));
    assert_eq!(tag("REPLACE INTO t VALUES (1)"), ("REPLACE".into(), true));
    assert_eq!(tag("WITH x AS (SELECT 1) DELETE FROM t WHERE id IN (SELECT * FROM x)"), ("DELETE".into(), true));
    assert_eq!(tag("LOAD DATA INFILE 'f' INTO TABLE t"), ("LOAD".into(), true));
    assert_eq!(tag("CREATE TEMPORARY TABLE IF NOT EXISTS t (a int)"), ("CREATE TABLE".into(), false));
    assert_eq!(
        tag("CREATE OR REPLACE ALGORITHM=MERGE DEFINER=CURRENT_USER SQL SECURITY INVOKER VIEW v AS SELECT 1").0,
        "CREATE VIEW"
    );
    assert_eq!(tag("CREATE UNIQUE INDEX i ON t (a)"), ("CREATE INDEX".into(), false));
    assert_eq!(tag("drop table t"), ("DROP TABLE".into(), false));
    assert_eq!(tag("SET @a = 1"), ("SET".into(), false));
    assert_eq!(tag("  -- c\n USE shop"), ("USE".into(), false));
    assert_eq!(tag(""), (String::new(), false));
}

#[test]
fn the_limit_is_set_only_before_queries() {
    for sql in [
        "SELECT 1",
        "with x as (select 1) select * from x",
        "TABLE t",
        "VALUES ROW(1)",
        "(SELECT 1)",
        "/*!80000 SELECT 1 */",
        // Listings the server cuts at the limit: set before them too (to none, they are read whole).
        "SHOW TABLES",
        "show session variables",
        "DESCRIBE t",
        "desc `t`",
    ] {
        assert!(limit_applies(sql, MODE), "{sql}");
    }
    for sql in [
        "SHOW WARNINGS",
        "SHOW ERRORS",
        "SHOW COUNT(*) WARNINGS",
        "EXPLAIN SELECT 1",
        "DESCRIBE SELECT 1",
        "DESC ANALYZE DELETE FROM t",
        "DESCRIBE FORMAT=TREE SELECT 1",
        "CALL p()",
        "INSERT INTO t VALUES (1)",
        "SET sql_mode = ''",
        "GET DIAGNOSTICS @n = NUMBER",
        "HELP 'select'",
    ] {
        assert!(!limit_applies(sql, MODE), "{sql}");
    }
    assert!(lists("SHOW FULL PROCESSLIST", MODE) && lists("DESCRIBE shop.users", MODE));
    assert!(!lists("SELECT 1", MODE) && !lists("SHOW WARNINGS", MODE) && !lists("DESC SELECT 1", MODE));
    assert!(reads_outcome("SELECT ROW_COUNT()", MODE) && reads_outcome("select found_rows()", MODE));
    assert!(!reads_outcome("SELECT 'ROW_COUNT'", MODE));
    assert!(reads_outcome("SELECT ROW_COUNT ()", MODE));
    assert!(!reads_outcome("SELECT id AS row_count FROM t", MODE) && !reads_outcome("SELECT found_rows FROM t", MODE));
    for sql in ["SELECT * FROM t FOR UPDATE", "select * from t for share nowait", "SELECT * FROM t LOCK IN SHARE MODE"]
    {
        assert!(locking(sql, MODE), "{sql}");
    }
    assert!(!locking("SELECT * FROM t", MODE) && !locking("SELECT 'FOR UPDATE'", MODE));
}

#[test]
fn explains_and_chains_are_read_from_the_text() {
    assert!(explains("EXPLAIN ANALYZE DELETE FROM t", MODE));
    assert!(explains("describe analyze select 1", MODE));
    assert!(!explains("SELECT 'EXPLAIN'", MODE));
    assert!(chains("COMMIT AND CHAIN", MODE));
    assert!(chains("rollback work and chain", MODE));
    assert!(!chains("COMMIT AND NO CHAIN", MODE));
    assert!(!chains("COMMIT", MODE));
}

/// What sets the next transaction's level alone is read as setting its own settings; the
/// session's own level is what the server reports.
#[test]
fn a_level_for_the_next_transaction_only_is_read_from_the_text() {
    for sql in [
        "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
        "set transaction read only, isolation level read committed",
        "SET @@transaction_isolation = 'SERIALIZABLE'",
        "SET @@`transaction_isolation` = 'SERIALIZABLE'",
        "SET @@`tx_isolation`='SERIALIZABLE'",
        "SET @@tx_isolation = 'SERIALIZABLE'",
        "SET time_zone = '+00:00', @@transaction_isolation = 'READ-COMMITTED'",
    ] {
        assert_eq!(sets_next_transaction(sql, MODE), SetsNext::Yes, "{sql}");
    }
    for sql in [
        "SET SESSION TRANSACTION ISOLATION LEVEL SERIALIZABLE",
        "SET LOCAL TRANSACTION ISOLATION LEVEL SERIALIZABLE",
        "SET GLOBAL TRANSACTION ISOLATION LEVEL SERIALIZABLE",
        "SET SESSION transaction_isolation = 'SERIALIZABLE'",
        "SET transaction_isolation = 'SERIALIZABLE'",
        "SET @@SESSION.transaction_isolation = 'SERIALIZABLE'",
        "SET @@session.`transaction_isolation` = 'SERIALIZABLE'",
        "SET @@global.tx_isolation = 'SERIALIZABLE'",
        "SELECT @@transaction_isolation",
        "SELECT 'SET TRANSACTION ISOLATION LEVEL SERIALIZABLE'",
    ] {
        assert_eq!(sets_next_transaction(sql, MODE), SetsNext::No, "{sql}");
    }
}

#[test]
fn names_are_split_unquoted_and_lower_case() {
    assert_eq!(name_parts("shop.users"), ["shop", "users"]);
    assert_eq!(name_parts("`My.Db`.`odd``name`"), ["my.db", "odd`name"]);
    assert_eq!(name_parts("\"A\".b"), ["a", "b"]);
    assert_eq!(name_parts("t.*"), ["t"]);
    assert_eq!(name_parts("Users"), ["users"]);
}

#[test]
fn the_servers_question_asks_every_reading_of_a_name() {
    let d = Dialect::MySql(MODE);
    let sql = check_sql(&["users".into(), "u.email".into(), "shop.orders.id".into(), "o'k".into()], d, &MYSQL);
    assert!(
        sql.starts_with(
            "SELECT /*+ SET_VAR(lock_wait_timeout=2) MAX_EXECUTION_TIME(10000) */ @@SESSION.transaction_isolation"
        ),
        "{sql}"
    );
    // Bare names (and the first part of `t.c`) in the current database; qualified pairs anywhere.
    assert!(sql.contains("LOWER(t.TABLE_NAME) IN ('o''k', 'u', 'users')"), "{sql}");
    assert!(sql.contains("IN (('shop', 'orders'), ('u', 'email'))"), "{sql}");
    assert!(sql.contains("t.TABLE_TYPE <> 'BASE TABLE'"), "{sql}");
    // Nothing named: nothing can be a view.
    assert!(check_sql(&[], d, &MYSQL).contains("AND (FALSE)"));
    // Backslashes are written as the session reads them.
    let nbe = Dialect::MySql(MySqlMode { no_backslash_escapes: true, ..MODE });
    assert!(check_sql(&["a\\b".into()], nbe, &MYSQL).contains("('a\\b')"));
    assert!(check_sql(&["a\\b".into()], d, &MYSQL).contains("('a\\\\b')"));
}

#[test]
fn limits_the_server_reports() {
    assert_eq!(limit_value("501"), Some(501));
    assert_eq!(limit_value("18446744073709551615"), None);
    assert_eq!(limit_value("x"), None);
}

/// The question about tables a rollback does not undo: each kind is asked for, and a global
/// TRIGGER grant counts only while partial revokes are off (with them on it may be revoked for
/// the table's schema, which the privilege tables do not show).
#[test]
fn the_not_undone_question_asks_for_every_kind() {
    let sql = not_undone_sql(&["shop.t".into(), "u".into()], Dialect::MySql(MODE), false);
    for part in ["'INNODB', 'NDBCLUSTER', 'NDB'", "TABLE_TYPE = 'VIEW'", "' (trigger)'", "' (triggers not visible)'"] {
        assert!(sql.contains(part), "{part}: {sql}");
    }
    assert!(
        sql.contains("p.PRIVILEGE_TYPE = 'TRIGGER' AND @@GLOBAL.partial_revokes = 0)"),
        "a global grant counts only without partial revokes: {sql}"
    );
    // Schema and table grants count whatever partial revokes are.
    assert_eq!(sql.matches("@@GLOBAL.partial_revokes").count(), 1, "{sql}");
    assert!(sql.ends_with("ORDER BY 1 LIMIT 20"), "{sql}");
}

/// MariaDB has no partial revokes, nor the variable that says whether they are on: its question
/// never reads it (the server would refuse the whole question), and a global grant counts.
#[test]
fn the_not_undone_question_of_mariadb_reads_no_partial_revokes() {
    let sql = not_undone_sql(&["shop.t".into()], Dialect::MySql(MODE), true);
    assert!(!sql.contains("partial_revokes"), "{sql}");
    assert!(sql.contains("FROM information_schema.USER_PRIVILEGES p WHERE p.GRANTEE = "), "{sql}");
    assert!(sql.contains("' (triggers not visible)'") && sql.ends_with("ORDER BY 1 LIMIT 20"), "{sql}");
}

#[test]
fn settings_of_the_next_transaction_only_are_seen_in_every_spelling() {
    let ansi = MySqlMode { ansi_quotes: true, ..MODE };
    for mode in [MODE, ansi] {
        let sets = |sql| sets_next_transaction(sql, mode) != SetsNext::No;
        for sql in [
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
            "set transaction read write",
            "SET /* c */ TRANSACTION READ ONLY, ISOLATION LEVEL READ COMMITTED",
            "SET @@transaction_isolation = 'SERIALIZABLE'",
            "SET @@TX_ISOLATION := 'READ-COMMITTED'",
            "SET @@`transaction_isolation` = 'SERIALIZABLE'",
            "SET @@transaction_read_only = 1",
            "SET @@tx_read_only = OFF",
            "SET @a = 1, @@transaction_isolation = 'SERIALIZABLE'",
            "SET @a = (SELECT 1), @@tx_read_only := 1",
        ] {
            assert_eq!(sets_next_transaction(sql, mode), SetsNext::Yes, "{sql}");
        }
        // Not read: may set them.
        for sql in ["CALL p()", "EXECUTE s", "/*!80000 SET TRANSACTION ISOLATION LEVEL SERIALIZABLE */"] {
            assert_eq!(sets_next_transaction(sql, mode), SetsNext::Maybe, "{sql}");
        }
        for sql in [
            "SET SESSION TRANSACTION ISOLATION LEVEL SERIALIZABLE",
            "SET GLOBAL TRANSACTION READ ONLY",
            "SET @@SESSION.transaction_isolation = 'SERIALIZABLE'",
            "SET @@session.`transaction_isolation` = 'SERIALIZABLE'",
            "SET @@LOCAL.transaction_read_only = 1",
            "SET @@GLOBAL.transaction_isolation = 'SERIALIZABLE'",
            "SET SESSION transaction_isolation = 'SERIALIZABLE'",
            "SET transaction_isolation = 'SERIALIZABLE'",
            "SET @transaction_isolation = 1",
            "SELECT @@transaction_isolation",
            "SELECT 'SET TRANSACTION READ WRITE'",
            // Read, not assigned.
            "SET @saved = @@transaction_isolation",
            "SET @a = IF(@@tx_read_only = 1, 1, 0)",
            "SET @a = 1, @b = @@transaction_read_only",
            "START TRANSACTION",
        ] {
            assert!(!sets(sql), "{sql}");
        }
    }
    // Under `ANSI_QUOTES` a double-quoted name is an identifier too.
    let ansi = MySqlMode { ansi_quotes: true, ..MODE };
    assert!(sets_next_isolation("SET @@\"transaction_isolation\" = 'SERIALIZABLE'", ansi));
    assert!(sets_next_access("SET @@\"tx_read_only\" = 1", ansi));
    // Each characteristic on its own.
    for (sql, isolation, access) in [
        ("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE", true, false),
        ("SET TRANSACTION READ ONLY", false, true),
        ("SET TRANSACTION READ WRITE, ISOLATION LEVEL READ COMMITTED", true, true),
        ("SET @@`tx_isolation` = 'READ-COMMITTED'", true, false),
        ("SET @@transaction_read_only = 1", false, true),
        ("SET @a = @@transaction_isolation, @@tx_read_only = 0", false, true),
        ("SET SESSION TRANSACTION READ ONLY", false, false),
        ("SET TRANSACTION ISOLATION LEVEL READ COMMITTED", true, false),
        ("SET TRANSACTION ISOLATION LEVEL READ UNCOMMITTED", true, false),
        ("SET TRANSACTION ISOLATION LEVEL READ COMMITTED, READ ONLY", true, true),
    ] {
        assert_eq!(sets_next_isolation(sql, MODE), isolation, "{sql}");
        assert_eq!(sets_next_access(sql, MODE), access, "{sql}");
    }
    for (sql, yes) in [
        ("START TRANSACTION READ ONLY", true),
        ("begin", true),
        ("COMMIT", true),
        ("rollback", true),
        ("SELECT 1", false),
    ] {
        assert_eq!(begins_or_ends(sql, MODE), yes, "{sql}");
    }
}

#[test]
fn a_bounded_read_is_limited_for_its_statement_and_never_longer_than_the_session() {
    let count = "SELECT COUNT(*) FROM (\nSELECT * FROM `t` WHERE (a > 1) LIMIT 10\n) AS datarig_count";
    let none = Asked { isolation: String::new(), statement: None, lock: None, sort_length: None };
    let (mysql, applied) = bounded_select(count, Duration::from_secs(30), &none, false, "").unwrap();
    assert_eq!(
        mysql,
        "SELECT /*+ MAX_EXECUTION_TIME(30000) SET_VAR(lock_wait_timeout=1) */ COUNT(*) FROM (\nSELECT * FROM `t` WHERE \
         (a > 1) LIMIT 10\n) AS datarig_count"
    );
    assert_eq!(applied, Duration::from_secs(30));
    let waits = Asked { isolation: String::new(), statement: None, lock: Some(50), sort_length: None };
    let (maria, _) = bounded_select(count, Duration::from_millis(1500), &waits, true, "").unwrap();
    assert_eq!(maria, format!("SET STATEMENT max_statement_time = 1.5, lock_wait_timeout = 1 FOR {count}"));
    // A shorter time limit of the session's stays; a longer one is not taken. The lock wait is
    // the shortest the server takes whatever the session's.
    let short =
        Asked { isolation: String::new(), statement: Some(Duration::from_secs(5)), lock: None, sort_length: None };
    let (text, applied) = bounded_select(count, Duration::from_secs(30), &short, false, "").unwrap();
    assert!(text.starts_with("SELECT /*+ MAX_EXECUTION_TIME(5000) SET_VAR(lock_wait_timeout=1) */ COUNT(*)"), "{text}");
    assert_eq!(applied, Duration::from_secs(5));
    let long =
        Asked { isolation: String::new(), statement: Some(Duration::from_secs(60)), lock: None, sort_length: None };
    let (text, applied) = bounded_select(count, Duration::from_secs(30), &long, false, "").unwrap();
    assert!(text.starts_with("SELECT /*+ MAX_EXECUTION_TIME(30000) SET_VAR(lock_wait_timeout=1) */"), "{text}");
    assert_eq!(applied, Duration::from_secs(30));
    // Only the app's own form, a text starting with `SELECT`.
    assert!(bounded_select(" SELECT 1", Duration::from_secs(1), &none, false, "").is_none());
    assert!(bounded_select("WITH x AS (SELECT 1) SELECT * FROM x", Duration::from_secs(1), &none, false, "").is_none());
    // An estimate's own hints go with the others.
    let (text, _) = bounded_select(count, Duration::from_secs(5), &none, false, ALL_ROWS).unwrap();
    assert!(
        text.starts_with(
            "SELECT /*+ MAX_EXECUTION_TIME(5000) SET_VAR(lock_wait_timeout=1) \
             SET_VAR(sql_select_limit=18446744073709551615) */ COUNT(*)"
        ),
        "{text}"
    );
    // The session's limits as the server writes them.
    assert_eq!(session_limit("0", false), None);
    assert_eq!(session_limit("2500", false), Some(Duration::from_millis(2500)));
    assert_eq!(session_limit("0.000000", true), None);
    assert_eq!(session_limit("1.500000", true), Some(Duration::from_millis(1500)));
    assert_eq!(session_limit("x", false), None);
}

#[test]
fn a_bounded_read_stopped_by_the_server_is_told_apart_from_a_cancel_and_a_lock_wait() {
    let server = |code: u16| {
        mysql_async::Error::Server(mysql_async::ServerError { code, message: "stopped".into(), state: "HY000".into() })
    };
    let limit = Some(Duration::from_secs(30));
    assert_eq!(bounded_failure(&server(3024), limit, false), DbError::TimedOut(Duration::from_secs(30)));
    assert_eq!(bounded_failure(&server(1969), limit, true), DbError::TimedOut(Duration::from_secs(30)));
    assert_eq!(bounded_failure(&server(1317), limit, true), DbError::Cancelled);
    assert!(matches!(bounded_failure(&server(1205), limit, false), DbError::LockTimeout(_)));
    // A kill the user did not ask for is the server's message.
    assert!(matches!(bounded_failure(&server(1317), limit, false), DbError::Server(_)));
    // Without a limit of the app's, a server's own limit is its message.
    assert!(matches!(bounded_failure(&server(3024), None, false), DbError::Server(_)));
}

/// MariaDB's question names the isolation variable its version has and reads the session's lock
/// wait; a session that does not wait for locks stays so, and a limit below a millisecond stays
/// what it is.
#[test]
fn mariadbs_bounded_read_fits_its_version_and_the_sessions_own_limits() {
    let d = Dialect::MySql(MODE);
    let older = check_sql(&[], d, &Server { version: (10, 6, 0), mariadb: true });
    assert!(older.contains("@@SESSION.tx_isolation,"), "{older}");
    assert!(
        older.ends_with("@@SESSION.max_statement_time, @@SESSION.lock_wait_timeout, @@SESSION.max_sort_length"),
        "{older}"
    );
    let newer = check_sql(&[], d, &Server { version: (11, 4, 0), mariadb: true });
    assert!(newer.contains("@@SESSION.transaction_isolation,"), "{newer}");
    assert!(check_sql(&[], d, &MYSQL).ends_with("@@SESSION.max_execution_time, NULL, @@SESSION.max_sort_length"));
    let sql = "SELECT 1";
    let asked = |statement, lock| Asked { isolation: String::new(), statement, lock, sort_length: None };
    let limit = Duration::from_secs(30);
    let (text, _) = bounded_select(sql, limit, &asked(None, Some(0)), true, "").unwrap();
    assert_eq!(text, "SET STATEMENT max_statement_time = 30, lock_wait_timeout = 0 FOR SELECT 1");
    let (text, _) = bounded_select(sql, limit, &asked(None, None), true, "").unwrap();
    assert!(text.contains("lock_wait_timeout = 0 "), "not known: never a longer wait: {text}");
    let tiny = Duration::from_micros(500);
    let (text, applied) = bounded_select(sql, limit, &asked(Some(tiny), Some(1)), true, "").unwrap();
    assert_eq!(text, "SET STATEMENT max_statement_time = 0.0005, lock_wait_timeout = 1 FOR SELECT 1");
    assert_eq!(applied, tiny);
}
