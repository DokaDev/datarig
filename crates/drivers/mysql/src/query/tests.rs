use super::*;
use crate::engine::family_of;
use datarig_core::engine::{Fact, Version};
use datarig_core::sql::dialect::{DollarQuotes, MySqlMode};

const MODE: MySqlMode = MySqlMode {
    ansi_quotes: false,
    no_backslash_escapes: false,
    dollar_quotes: DollarQuotes::Read,
    ignore_space: false,
};
const MYSQL: Greeting = Greeting { mariadb: false, handshake: Version::new(8, 4, 0) };

#[test]
fn outcomes_name_the_command_or_count_rows() {
    let tag = |sql| command_tag(sql, Dialect::MySql(MODE));
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
        assert!(limit_applies(sql, Dialect::MySql(MODE)), "{sql}");
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
        assert!(!limit_applies(sql, Dialect::MySql(MODE)), "{sql}");
    }
    assert!(lists("SHOW FULL PROCESSLIST", Dialect::MySql(MODE)) && lists("DESCRIBE shop.users", Dialect::MySql(MODE)));
    assert!(
        !lists("SELECT 1", Dialect::MySql(MODE))
            && !lists("SHOW WARNINGS", Dialect::MySql(MODE))
            && !lists("DESC SELECT 1", Dialect::MySql(MODE))
    );
    assert!(
        reads_outcome("SELECT ROW_COUNT()", Dialect::MySql(MODE))
            && reads_outcome("select found_rows()", Dialect::MySql(MODE))
    );
    assert!(!reads_outcome("SELECT 'ROW_COUNT'", Dialect::MySql(MODE)));
    assert!(reads_outcome("SELECT ROW_COUNT ()", Dialect::MySql(MODE)));
    assert!(
        !reads_outcome("SELECT id AS row_count FROM t", Dialect::MySql(MODE))
            && !reads_outcome("SELECT found_rows FROM t", Dialect::MySql(MODE))
    );
    for sql in ["SELECT * FROM t FOR UPDATE", "select * from t for share nowait", "SELECT * FROM t LOCK IN SHARE MODE"]
    {
        assert!(locking(sql, Dialect::MySql(MODE)), "{sql}");
    }
    assert!(!locking("SELECT * FROM t", Dialect::MySql(MODE)) && !locking("SELECT 'FOR UPDATE'", Dialect::MySql(MODE)));
}

#[test]
fn explains_and_chains_are_read_from_the_text() {
    assert!(explains("EXPLAIN ANALYZE DELETE FROM t", Dialect::MySql(MODE)));
    assert!(explains("describe analyze select 1", Dialect::MySql(MODE)));
    assert!(!explains("SELECT 'EXPLAIN'", Dialect::MySql(MODE)));
    assert!(chains("COMMIT AND CHAIN", Dialect::MySql(MODE)));
    assert!(chains("rollback work and chain", Dialect::MySql(MODE)));
    assert!(!chains("COMMIT AND NO CHAIN", Dialect::MySql(MODE)));
    assert!(!chains("COMMIT", Dialect::MySql(MODE)));
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
        assert_eq!(sets_next_transaction(sql, Dialect::MySql(MODE), Judged::Own), SetsNext::Yes, "{sql}");
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
        assert_eq!(sets_next_transaction(sql, Dialect::MySql(MODE), Judged::Own), SetsNext::No, "{sql}");
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
    let sql = check_sql(
        &["users".into(), "u.email".into(), "shop.orders.id".into(), "o'k".into()],
        d,
        &MYSQL,
        &Fact::Known(MYSQL.handshake),
    )
    .concat();
    assert!(
        sql.starts_with(
            "SELECT /*+ SET_VAR(lock_wait_timeout=2) MAX_EXECUTION_TIME(10000) */ @@SESSION.transaction_isolation"
        ),
        "{sql}"
    );
    // Bare names (and the first part of `t.c`) in the current database; qualified pairs anywhere.
    assert!(sql.contains("LOWER(t.TABLE_NAME) IN ('o''k', 'u', 'users')"), "{sql}");
    assert!(sql.contains("IN (('shop', 'orders'), ('u', 'email'))"), "{sql}");
    // A table is a base table or MariaDB's system-versioned one; anything else is not a table.
    assert!(sql.contains("t.TABLE_TYPE NOT IN ('BASE TABLE', 'SYSTEM VERSIONED')"), "{sql}");
    // Nothing named: nothing can be a view.
    assert!(check_sql(&[], d, &MYSQL, &Fact::Known(MYSQL.handshake)).concat().contains("AND (FALSE)"));
    // Backslashes are written as the session reads them.
    let nbe = Dialect::MySql(MySqlMode { no_backslash_escapes: true, ..MODE });
    assert!(check_sql(&["a\\b".into()], nbe, &MYSQL, &Fact::Known(MYSQL.handshake)).concat().contains("('a\\b')"));
    assert!(check_sql(&["a\\b".into()], d, &MYSQL, &Fact::Known(MYSQL.handshake)).concat().contains("('a\\\\b')"));
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
    let sql = not_undone_sql(&["shop.t".into(), "u".into()], Dialect::MySql(MODE), family_of(false));
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
    let sql = not_undone_sql(&["shop.t".into()], Dialect::MySql(MODE), family_of(true));
    assert!(!sql.contains("partial_revokes"), "{sql}");
    assert!(sql.contains("FROM information_schema.USER_PRIVILEGES p WHERE p.GRANTEE = "), "{sql}");
    assert!(sql.contains("' (triggers not visible)'") && sql.ends_with("ORDER BY 1 LIMIT 20"), "{sql}");
}

/// MariaDB runs none of an executable comment whose version is five digits from 50700 to 99999
/// (MySQL 5.7 and later; MariaDB 11.4 checked): to it a statement that is only
/// `/*!80000 SET TRANSACTION … */` sets nothing. Any other text with an executable comment it
/// may run, as on MySQL: the comment ends at the first `*/` (a quote inside does not hide it),
/// and 99997 runs on a Galera node.
#[test]
fn mariadb_does_not_count_a_mysql8_set_transaction_comment() {
    let maria = Dialect::MariaDb(MODE);
    for sql in [
        "/*!80000 SET TRANSACTION ISOLATION LEVEL SERIALIZABLE */",
        " /*!50700 SET TRANSACTION READ ONLY */\n",
        "/*!99999 SET @@transaction_isolation = 'SERIALIZABLE' */",
        "/*!80000 SET TRANSACTION READ ONLY*/",
    ] {
        assert_eq!(sets_next_transaction(sql, maria, Judged::Own), SetsNext::No, "{sql}");
    }
    for sql in [
        "/*! SET TRANSACTION READ ONLY */",
        "/*!50699 SET TRANSACTION READ ONLY */",
        "/*!99997 SET TRANSACTION READ ONLY */",
        "/*!100000 SET TRANSACTION READ ONLY */",
        "/*!5070012 SET TRANSACTION READ ONLY */",
        "/*!80000 '*/ SET TRANSACTION READ ONLY /*' */",
        "/*!80000 -- ignored */ SET TRANSACTION READ ONLY",
        "/*!80000 SET TRANSACTION READ ONLY */ /*!50601 SET TRANSACTION READ ONLY */",
        "/*!80000 SET TRANSACTION READ ONLY",
        "SELECT 1 /*!80000 , 2 */",
        "CALL p()",
    ] {
        assert_ne!(sets_next_transaction(sql, maria, Judged::Own), SetsNext::No, "{sql}");
    }
}

/// MySQL runs an executable comment from its version on: what it holds is not read, so it may
/// set the next transaction, whatever the version.
#[test]
fn a_version_comment_may_set_the_next_transaction() {
    for sql in [
        "/*!80000 SET TRANSACTION ISOLATION LEVEL SERIALIZABLE */",
        "/*!50700 SET TRANSACTION READ ONLY */",
        "/*!99999 SET TRANSACTION READ ONLY */",
        "/*! SET TRANSACTION READ ONLY */",
        "SELECT 1 /*!80000 , 2 */",
    ] {
        assert_eq!(sets_next_transaction(sql, Dialect::MySql(MODE), Judged::Own), SetsNext::Maybe, "{sql}");
    }
}

#[test]
fn settings_of_the_next_transaction_only_are_seen_in_every_spelling() {
    let ansi = MySqlMode { ansi_quotes: true, ..MODE };
    for mode in [MODE, ansi] {
        let sets = |sql| sets_next_transaction(sql, Dialect::MySql(mode), Judged::Own) != SetsNext::No;
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
            assert_eq!(sets_next_transaction(sql, Dialect::MySql(mode), Judged::Own), SetsNext::Yes, "{sql}");
        }
        // Not read: may set them.
        for sql in ["CALL p()", "EXECUTE s", "/*!80000 SET TRANSACTION ISOLATION LEVEL SERIALIZABLE */"] {
            assert_eq!(sets_next_transaction(sql, Dialect::MySql(mode), Judged::Own), SetsNext::Maybe, "{sql}");
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
    assert!(sets_next_isolation("SET @@\"transaction_isolation\" = 'SERIALIZABLE'", Dialect::MySql(ansi)));
    assert!(sets_next_access("SET @@\"tx_read_only\" = 1", Dialect::MySql(ansi)));
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
        assert_eq!(sets_next_isolation(sql, Dialect::MySql(MODE)), isolation, "{sql}");
        assert_eq!(sets_next_access(sql, Dialect::MySql(MODE)), access, "{sql}");
    }
    for (sql, yes) in [
        ("START TRANSACTION READ ONLY", true),
        ("begin", true),
        ("COMMIT", true),
        ("rollback", true),
        ("SELECT 1", false),
    ] {
        assert_eq!(begins_or_ends(sql, Dialect::MySql(MODE)), yes, "{sql}");
    }
}

#[test]
fn a_bounded_read_is_limited_for_its_statement_and_never_longer_than_the_session() {
    let count = "SELECT COUNT(*) FROM (\nSELECT * FROM `t` WHERE (a > 1) LIMIT 10\n) AS datarig_count";
    let none = Asked { isolation: String::new(), statement: None, lock: None, sort_length: None };
    let (mysql, applied) = bounded_select(count, Duration::from_secs(30), &none, family_of(false), "").unwrap();
    assert_eq!(
        mysql,
        "SELECT /*+ MAX_EXECUTION_TIME(30000) SET_VAR(lock_wait_timeout=1) */ COUNT(*) FROM (\nSELECT * FROM `t` WHERE \
         (a > 1) LIMIT 10\n) AS datarig_count"
    );
    assert_eq!(applied, Duration::from_secs(30));
    let waits = Asked { isolation: String::new(), statement: None, lock: Some(50), sort_length: None };
    let (maria, _) = bounded_select(count, Duration::from_millis(1500), &waits, family_of(true), "").unwrap();
    assert_eq!(maria, format!("SET STATEMENT max_statement_time = 1.5, lock_wait_timeout = 1 FOR {count}"));
    // A shorter time limit of the session's stays; a longer one is not taken. The lock wait is
    // the shortest the server takes whatever the session's.
    let short =
        Asked { isolation: String::new(), statement: Some(Duration::from_secs(5)), lock: None, sort_length: None };
    let (text, applied) = bounded_select(count, Duration::from_secs(30), &short, family_of(false), "").unwrap();
    assert!(text.starts_with("SELECT /*+ MAX_EXECUTION_TIME(5000) SET_VAR(lock_wait_timeout=1) */ COUNT(*)"), "{text}");
    assert_eq!(applied, Duration::from_secs(5));
    let long =
        Asked { isolation: String::new(), statement: Some(Duration::from_secs(60)), lock: None, sort_length: None };
    let (text, applied) = bounded_select(count, Duration::from_secs(30), &long, family_of(false), "").unwrap();
    assert!(text.starts_with("SELECT /*+ MAX_EXECUTION_TIME(30000) SET_VAR(lock_wait_timeout=1) */"), "{text}");
    assert_eq!(applied, Duration::from_secs(30));
    // Only the app's own form, a text starting with `SELECT`.
    assert!(bounded_select(" SELECT 1", Duration::from_secs(1), &none, family_of(false), "").is_none());
    assert!(
        bounded_select("WITH x AS (SELECT 1) SELECT * FROM x", Duration::from_secs(1), &none, family_of(false), "")
            .is_none()
    );
    // An estimate's own hints go with the others.
    let (text, _) = bounded_select(count, Duration::from_secs(5), &none, family_of(false), ALL_ROWS).unwrap();
    assert!(
        text.starts_with(
            "SELECT /*+ MAX_EXECUTION_TIME(5000) SET_VAR(lock_wait_timeout=1) \
             SET_VAR(sql_select_limit=18446744073709551615) */ COUNT(*)"
        ),
        "{text}"
    );
    // The session's limits as the server writes them.
    assert_eq!(family_of(false).session_limit("0"), None);
    assert_eq!(family_of(false).session_limit("2500"), Some(Duration::from_millis(2500)));
    assert_eq!(family_of(true).session_limit("0.000000"), None);
    assert_eq!(family_of(true).session_limit("1.500000"), Some(Duration::from_millis(1500)));
    assert_eq!(family_of(false).session_limit("x"), None);
}

/// A count or a group count without a time limit (`[results] count_timeout = "off"`) carries the
/// lock wait bound alone on a session whose reports name one engine, in that engine's form, never
/// longer than the session's own wait (MariaDB's, when asked; 0 stays not waiting): it gives up
/// behind a lock as one with a time limit does, and no time limit is added. With a time limit it
/// is the bounded read as before. A session whose reports name two engines sends it as written.
#[test]
fn a_count_without_a_time_limit_is_bounded_behind_a_lock() {
    let count = "SELECT COUNT(*) FROM (\nSELECT * FROM `t` WHERE (a > 1) LIMIT 10\n) AS datarig_count";
    let group = "SELECT\n`a`, COUNT(*) FROM `t` GROUP BY `a`";
    let asked = |lock| Asked { isolation: String::new(), statement: None, lock, sort_length: None };
    let none = asked(None);
    let text = |sql, limit, asked: &Asked, mariadb, disagree| {
        let (text, applied) = count_text(sql, limit, asked, family_of(mariadb), disagree);
        (text.into_owned(), applied)
    };
    // MySQL: the hint alone, whatever the session's own wait.
    assert_eq!(
        text(count, None, &none, false, false),
        (
            "SELECT /*+ SET_VAR(lock_wait_timeout=1) */ COUNT(*) FROM (\nSELECT * FROM `t` WHERE (a > 1) LIMIT 10\n) \
             AS datarig_count"
                .to_string(),
            None
        )
    );
    assert_eq!(
        text(group, None, &asked(Some(50)), false, false).0,
        "SELECT /*+ SET_VAR(lock_wait_timeout=1) */ `a`, COUNT(*) FROM `t` GROUP BY `a`"
    );
    // MariaDB: `SET STATEMENT` with the lock wait alone, never longer than the session's.
    assert_eq!(
        text(count, None, &asked(Some(50)), true, false),
        (format!("SET STATEMENT lock_wait_timeout = 1 FOR {count}"), None)
    );
    assert_eq!(
        text(group, None, &asked(Some(0)), true, false).0,
        format!("SET STATEMENT lock_wait_timeout = 0 FOR {group}")
    );
    assert_eq!(text(count, None, &none, true, false).0, format!("SET STATEMENT lock_wait_timeout = 0 FOR {count}"));
    // A time limit of the session's own is not added.
    let short = Asked { statement: Some(Duration::from_secs(5)), ..asked(Some(50)) };
    assert_eq!(text(count, None, &short, true, false).0, format!("SET STATEMENT lock_wait_timeout = 1 FOR {count}"));
    assert!(
        text(count, None, &short, false, false).0.starts_with("SELECT /*+ SET_VAR(lock_wait_timeout=1) */ COUNT(*)")
    );
    // With a time limit: the bounded read, as before.
    for mariadb in [false, true] {
        for a in [&none, &short] {
            let thirty = Duration::from_secs(30);
            let (want, applied) = bounded_select(count, thirty, a, family_of(mariadb), "").unwrap();
            assert_eq!(text(count, Some(thirty), a, mariadb, false), (want.clone(), Some(applied)));
            assert_eq!(text(count, Some(thirty), a, mariadb, true), (want, Some(applied)), "disagreeing: as today");
        }
        // Reports naming two engines: as written without a time limit.
        assert_eq!(text(count, None, &asked(Some(50)), mariadb, true), (count.to_string(), None));
        assert_eq!(text(group, None, &none, mariadb, true), (group.to_string(), None));
        // Only the app's own form, a text starting with `SELECT`, is bounded.
        assert_eq!(text(" SELECT 1", None, &none, mariadb, false), (" SELECT 1".to_string(), None));
    }
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
    let older = check_sql(
        &[],
        d,
        &Greeting { mariadb: true, handshake: Version::new(10, 6, 0) },
        &Fact::Known(Version::new(10, 6, 0)),
    )
    .remove(0);
    assert!(older.contains("@@SESSION.tx_isolation,"), "{older}");
    assert!(
        older.ends_with("@@SESSION.max_statement_time, @@SESSION.lock_wait_timeout, @@SESSION.max_sort_length"),
        "{older}"
    );
    let newer = check_sql(
        &[],
        d,
        &Greeting { mariadb: true, handshake: Version::new(11, 4, 0) },
        &Fact::Known(Version::new(11, 4, 0)),
    )
    .remove(0);
    assert!(newer.contains("@@SESSION.transaction_isolation,"), "{newer}");
    assert!(
        check_sql(&[], d, &MYSQL, &Fact::Known(MYSQL.handshake))
            .concat()
            .ends_with("@@SESSION.max_execution_time, NULL, @@SESSION.max_sort_length")
    );
    let sql = "SELECT 1";
    let asked = |statement, lock| Asked { isolation: String::new(), statement, lock, sort_length: None };
    let limit = Duration::from_secs(30);
    let (text, _) = bounded_select(sql, limit, &asked(None, Some(0)), family_of(true), "").unwrap();
    assert_eq!(text, "SET STATEMENT max_statement_time = 30, lock_wait_timeout = 0 FOR SELECT 1");
    let (text, _) = bounded_select(sql, limit, &asked(None, None), family_of(true), "").unwrap();
    assert!(text.contains("lock_wait_timeout = 0 "), "not known: never a longer wait: {text}");
    let tiny = Duration::from_micros(500);
    let (text, applied) = bounded_select(sql, limit, &asked(Some(tiny), Some(1)), family_of(true), "").unwrap();
    assert_eq!(text, "SET STATEMENT max_statement_time = 0.0005, lock_wait_timeout = 1 FOR SELECT 1");
    assert_eq!(applied, tiny);
}

#[test]
fn only_plain_reads_get_the_paging_limit() {
    for mode in [Dialect::MySql(MODE), Dialect::MariaDb(MODE)] {
        for sql in [
            "SELECT * FROM t",
            "select id, upper(name) from t where id > 3 order by id",
            "WITH x AS (SELECT 1 AS a) SELECT * FROM x",
            "TABLE t",
            "VALUES ROW(1), ROW(2)",
            "(SELECT 1) UNION (SELECT 2)",
        ] {
            assert!(
                pages_plainly(&risk::classify(sql, crate::engine::risk_judge(mode, Judged::Own)), sql, mode),
                "{sql}"
            );
        }
        for sql in [
            // The limit would cut the file the server writes.
            "SELECT * FROM t INTO OUTFILE '/tmp/t.csv'",
            "SELECT * FROM t INTO DUMPFILE '/tmp/t.bin'",
            "TABLE t INTO OUTFILE '/tmp/t.csv'",
            "SELECT * INTO OUTFILE '/tmp/t.csv' FROM t",
            // Sets the session's variables.
            "SELECT id INTO @v FROM t LIMIT 1",
            // A function the text does not show would run for the rows up to the limit only.
            "SELECT shop.process(id) FROM t",
            "SELECT id FROM t WHERE zz_f(id) > 0",
            // Code the classifier does not read.
            "/*!80000 SELECT zz_f(id) FROM t */",
        ] {
            assert!(
                !pages_plainly(&risk::classify(sql, crate::engine::risk_judge(mode, Judged::Own)), sql, mode),
                "{sql}"
            );
        }
    }
}

/// MySQL's own reads go as they always went: the hints in the text, one statement each.
#[test]
fn mysql_own_read_text_is_unchanged() {
    let d = Dialect::MySql(MODE);
    let names: Vec<String> = vec!["shop.t".into(), "u".into()];
    assert_eq!(not_undone_sql(&names, d, family_of(false)), NOT_UNDONE_MYSQL);
    assert_eq!(check_sql(&names, d, &MYSQL, &Fact::Known(MYSQL.handshake)), [CHECK_MYSQL]);
    // A keyset chunk: the hint after its `SELECT`, as the chunk was written before.
    let chunk = "SELECT * FROM `s`.`t` WHERE `id` > 7 ORDER BY `id` LIMIT 10000";
    assert_eq!(
        family_of(false).keyset_text(chunk).as_deref(),
        Some("SELECT /*+ SET_VAR(lock_wait_timeout=5) */ * FROM `s`.`t` WHERE `id` > 7 ORDER BY `id` LIMIT 10000")
    );
}

const NOT_UNDONE_MYSQL: &str = r#"SELECT /*+ SET_VAR(lock_wait_timeout=2) MAX_EXECUTION_TIME(10000) */ CONCAT(t.TABLE_SCHEMA, '.', t.TABLE_NAME, ' (', COALESCE(t.ENGINE, t.TABLE_TYPE), ')') FROM information_schema.TABLES t WHERE (t.TABLE_TYPE = 'VIEW' OR UPPER(COALESCE(t.ENGINE, '')) NOT IN ('INNODB', 'NDBCLUSTER', 'NDB')) AND ((LOWER(t.TABLE_SCHEMA) = LOWER(DATABASE()) AND LOWER(t.TABLE_NAME) IN ('shop', 'u')) OR (LOWER(t.TABLE_SCHEMA), LOWER(t.TABLE_NAME)) IN (('shop', 't'))) UNION SELECT CONCAT(t.TABLE_SCHEMA, '.', t.TABLE_NAME, ' (trigger)') FROM information_schema.TABLES t WHERE EXISTS (SELECT 1 FROM information_schema.TRIGGERS g WHERE g.EVENT_OBJECT_SCHEMA = t.TABLE_SCHEMA AND g.EVENT_OBJECT_TABLE = t.TABLE_NAME) AND ((LOWER(t.TABLE_SCHEMA) = LOWER(DATABASE()) AND LOWER(t.TABLE_NAME) IN ('shop', 'u')) OR (LOWER(t.TABLE_SCHEMA), LOWER(t.TABLE_NAME)) IN (('shop', 't'))) UNION SELECT CONCAT(t.TABLE_SCHEMA, '.', t.TABLE_NAME, ' (triggers not visible)') FROM information_schema.TABLES t WHERE t.TABLE_TYPE IN ('BASE TABLE', 'SYSTEM VERSIONED') AND ((LOWER(t.TABLE_SCHEMA) = LOWER(DATABASE()) AND LOWER(t.TABLE_NAME) IN ('shop', 'u')) OR (LOWER(t.TABLE_SCHEMA), LOWER(t.TABLE_NAME)) IN (('shop', 't'))) AND NOT EXISTS (SELECT 1 FROM information_schema.USER_PRIVILEGES p WHERE p.GRANTEE = CONCAT(QUOTE(SUBSTRING_INDEX(CURRENT_USER(), '@', 1)), '@', QUOTE(SUBSTRING_INDEX(CURRENT_USER(), '@', -1))) AND p.PRIVILEGE_TYPE = 'TRIGGER' AND @@GLOBAL.partial_revokes = 0) AND NOT EXISTS (SELECT 1 FROM information_schema.SCHEMA_PRIVILEGES p WHERE p.GRANTEE = CONCAT(QUOTE(SUBSTRING_INDEX(CURRENT_USER(), '@', 1)), '@', QUOTE(SUBSTRING_INDEX(CURRENT_USER(), '@', -1))) AND p.PRIVILEGE_TYPE = 'TRIGGER' AND p.TABLE_SCHEMA = t.TABLE_SCHEMA) AND NOT EXISTS (SELECT 1 FROM information_schema.TABLE_PRIVILEGES p WHERE p.GRANTEE = CONCAT(QUOTE(SUBSTRING_INDEX(CURRENT_USER(), '@', 1)), '@', QUOTE(SUBSTRING_INDEX(CURRENT_USER(), '@', -1))) AND p.PRIVILEGE_TYPE = 'TRIGGER' AND p.TABLE_SCHEMA = t.TABLE_SCHEMA AND p.TABLE_NAME = t.TABLE_NAME) ORDER BY 1 LIMIT 20"#;

const CHECK_MYSQL: &str = r#"SELECT /*+ SET_VAR(lock_wait_timeout=2) MAX_EXECUTION_TIME(10000) */ @@SESSION.transaction_isolation, (SELECT CONCAT(t.TABLE_SCHEMA, '.', t.TABLE_NAME) FROM information_schema.TABLES t WHERE t.TABLE_TYPE NOT IN ('BASE TABLE', 'SYSTEM VERSIONED') AND ((LOWER(t.TABLE_SCHEMA) = LOWER(DATABASE()) AND LOWER(t.TABLE_NAME) IN ('shop', 'u')) OR (LOWER(t.TABLE_SCHEMA), LOWER(t.TABLE_NAME)) IN (('shop', 't'))) LIMIT 1), @@SESSION.max_execution_time, NULL, @@SESSION.max_sort_length"#;

/// MariaDB reads the hints as a comment: its own reads go bounded by `SET STATEMENT … FOR`, never
/// longer than the session's own limits, and its session values are read apart, before the
/// bounded lookup (inside it they would read as the bound's). A keyset chunk waits at most 5 s
/// for a lock; a chunk that is not a plain `SELECT` is not sent unbounded.
#[test]
fn mariadb_own_reads_are_bounded_by_set_statement() {
    let d = Dialect::MySql(MODE);
    let names: Vec<String> = vec!["shop.t".into(), "u".into()];
    let bound = "SET STATEMENT max_statement_time = IF(@@SESSION.max_statement_time = 0, 10, \
                 LEAST(@@SESSION.max_statement_time, 10)), lock_wait_timeout = LEAST(@@SESSION.lock_wait_timeout, 2) FOR ";
    assert_eq!(family_of(true).own_read_bounds().0, bound);
    let sql = not_undone_sql(&names, d, family_of(true));
    assert!(sql.starts_with(&format!("{bound}SELECT CONCAT(t.TABLE_SCHEMA")), "{sql}");
    assert!(!sql.contains("/*+"), "{sql}");
    let ask = check_sql(
        &names,
        d,
        &Greeting { mariadb: true, handshake: Version::new(11, 4, 0) },
        &Fact::Known(Version::new(11, 4, 0)),
    );
    assert_eq!(
        ask[0],
        "SELECT @@SESSION.transaction_isolation, @@SESSION.max_statement_time, @@SESSION.lock_wait_timeout, \
         @@SESSION.max_sort_length"
    );
    assert!(
        ask[1].starts_with(&format!("{bound}SELECT (SELECT CONCAT(t.TABLE_SCHEMA, '.', t.TABLE_NAME) ")),
        "{ask:?}"
    );
    assert!(ask[1].ends_with("LIMIT 1)"), "{ask:?}");
    // The lookup reads no session value besides the bound's own.
    assert_eq!(ask[1].matches("@@SESSION.").count(), bound.matches("@@SESSION.").count(), "{ask:?}");
    assert_eq!(ask.len(), 2);
    let chunk = "SELECT * FROM `s`.`t` ORDER BY `id` LIMIT 10000";
    assert_eq!(
        family_of(true).keyset_text(chunk).as_deref(),
        Some(
            "SET STATEMENT lock_wait_timeout = LEAST(@@SESSION.lock_wait_timeout, 5) FOR SELECT * FROM `s`.`t` ORDER BY `id` LIMIT 10000"
        )
    );
    for mariadb in [false, true] {
        assert_eq!(family_of(mariadb).keyset_text("WITH x AS (SELECT 1) SELECT * FROM x"), None);
        assert_eq!(family_of(mariadb).keyset_text("select * from t"), None);
    }
}

/// The dialect of a session whose greeting names MySQL `greeting` and whose `VERSION()` line is
/// `line`.
fn reported_dialect(greeting: (u16, u16, u16), line: &str) -> Dialect {
    let g = crate::engine::Greeting {
        mariadb: false,
        handshake: datarig_core::engine::Version::new(greeting.0, greeting.1, greeting.2),
    };
    let report = crate::engine::judge(&g, Some(mysql_async::Value::Bytes(line.as_bytes().to_vec()))).expect("a line");
    let facts = crate::engine::facts(&g, &report, Some(true));
    let datarig_core::sql::dialect::Language::Sql(d) = crate::engine::derive(&facts, &Tracked::default()).language;
    d
}

/// On a session whose two reports disagree on whether the server reads dollar quotes (a MySQL
/// 8.0 greeting in front of 8.4), a statement counts as setting the next transaction when it
/// does in either reading, the stronger answer winning (`Maybe`, then `Yes`): read without
/// dollar quotes, the executable comment below is inside a string, read with them it runs. A
/// session whose reports agree reads it its one way, as before.
#[test]
fn an_unknown_dollar_reading_counts_a_hidden_set_transaction() {
    let hidden = "SELECT $$'$$ /*!80000 SET TRANSACTION READ ONLY */ '";
    let disagreeing = reported_dialect((8, 0, 36), "8.4.6");
    assert_eq!(sets_next_transaction(hidden, disagreeing, Judged::Own), SetsNext::Maybe, "{disagreeing:?}");
    let shown = "SELECT $$'$$; SET TRANSACTION READ ONLY; '";
    assert_eq!(
        sets_next_transaction(shown, disagreeing, Judged::Own),
        SetsNext::No,
        "{disagreeing:?}: one statement, a SELECT"
    );
    let plain = "SET TRANSACTION READ ONLY";
    assert_eq!(sets_next_transaction(plain, disagreeing, Judged::Own), SetsNext::Yes);
    // Agreeing reports: one reading each, as before.
    let read = reported_dialect((8, 4, 6), "8.4.6");
    assert_eq!(sets_next_transaction(hidden, read, Judged::Own), SetsNext::Maybe, "{read:?}");
    let not_read = reported_dialect((8, 0, 36), "8.0.36");
    assert_eq!(sets_next_transaction(hidden, not_read, Judged::Own), SetsNext::No, "{not_read:?}");
}

/// On a session whose greeting names MariaDB and whose `VERSION()` line names MySQL 8 (judged on
/// both engines), a statement that is only a MySQL 8 version comment (`/*!80000 … */`, which
/// MariaDB reads as a comment and MySQL runs) may set the next transaction: each engine's
/// reading answers, `Maybe` over `Yes` over `No`. Judged on MariaDB alone it sets nothing, as
/// before; plain statements answer as on either engine.
#[test]
fn a_disagreeing_session_counts_a_mysql8_comment_as_maybe() {
    let comment = "/*!80000 SET TRANSACTION READ ONLY */";
    let maria = Dialect::MariaDb(MODE);
    let my = Dialect::MySql(MODE);
    assert_eq!(sets_next_transaction(comment, maria, Judged::Own), SetsNext::No);
    assert_eq!(sets_next_transaction(comment, maria, Judged::WithSibling), SetsNext::Maybe);
    assert_eq!(sets_next_transaction(comment, my, Judged::Own), SetsNext::Maybe);
    assert_eq!(sets_next_transaction(comment, my, Judged::WithSibling), SetsNext::Maybe);
    for d in [maria, my] {
        let both = |sql| sets_next_transaction(sql, d, Judged::WithSibling);
        assert_eq!(both("SET TRANSACTION READ ONLY"), SetsNext::Yes, "{d:?}");
        assert_eq!(both("SELECT 1"), SetsNext::No, "{d:?}");
        assert_eq!(both("CALL p()"), SetsNext::Maybe, "{d:?}");
    }
    // The facts of such a session say so.
    let g = crate::engine::Greeting { mariadb: true, handshake: datarig_core::engine::Version::new(11, 4, 13) };
    let report = crate::engine::judge(&g, Some(mysql_async::Value::Bytes(b"8.4.6".to_vec()))).expect("a line");
    assert_eq!(crate::engine::facts(&g, &report, None).judged(), Judged::WithSibling);
    let report =
        crate::engine::judge(&g, Some(mysql_async::Value::Bytes(b"11.4.13-MariaDB".to_vec()))).expect("a line");
    assert_eq!(crate::engine::facts(&g, &report, None).judged(), Judged::Own);
}

/// Which server session is the connection's own, as answered: direct only from a confirmed
/// marker (`Some(true)`), not direct when its id is not the handshake's; otherwise not known,
/// with the id shown for display only when the server named the connection by the handshake's
/// id (the monitor marks the tab's row by it). The shown id never comes with a direct answer.
#[test]
fn the_shown_id_is_for_display_only() {
    use datarig_core::driver::OwnSession;
    assert_eq!(own_answer_of(Some(true), true, 7), (OwnSession::Direct { id: 7 }, None));
    assert_eq!(own_answer_of(Some(true), false, 7), (OwnSession::Direct { id: 7 }, None));
    assert_eq!(own_answer_of(Some(false), true, 9), (OwnSession::NotDirect, None));
    assert_eq!(own_answer_of(None, true, 7), (OwnSession::NotKnown, Some(7)));
    assert_eq!(own_answer_of(None, false, 7), (OwnSession::NotKnown, None));
    assert_eq!(own_answer_of(None, true, u64::MAX), (OwnSession::NotKnown, None), "no id to show");
}

/// Only text the user wrote is checked for MariaDB's own executable comments: a run's
/// statements, a statement run again or checked before it is; not a count or an export the app
/// writes around values it quotes itself.
#[test]
fn only_user_written_text_is_checked_for_mariadb_comments() {
    use datarig_core::driver::{DbCommand, PagingMode, TxRule};
    let sql = "SELECT '/*M!' AS v".to_string();
    let run = DbCommand::Execute {
        id: 1,
        statements: vec![sql.clone()],
        paging: PagingMode::NoHold,
        tx: TxRule::Auto,
        max_rows: None,
    };
    assert_eq!(user_written(&run), [sql.as_str()]);
    let check = DbCommand::CheckRepeat { id: 2, sql: sql.clone() };
    assert_eq!(user_written(&check), [sql.as_str()]);
    let count = DbCommand::Count { id: 3, sql: "SELECT COUNT(*) FROM t WHERE `v` = '/*M!'".into(), limit: None };
    assert!(user_written(&count).is_empty());
    let judged = DbCommand::Judged { judged: Judged::WithSibling, command: Box::new(count) };
    assert!(user_written(&judged).is_empty());
}
