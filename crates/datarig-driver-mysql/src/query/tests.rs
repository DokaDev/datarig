use super::*;

const MODE: MySqlMode = MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: true };

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
    ] {
        assert!(limit_applies(sql, MODE), "{sql}");
    }
    for sql in [
        "SHOW WARNINGS",
        "SHOW TABLES",
        "EXPLAIN SELECT 1",
        "CALL p()",
        "INSERT INTO t VALUES (1)",
        "SET sql_mode = ''",
        "GET DIAGNOSTICS @n = NUMBER",
        "HELP 'select'",
        "DESCRIBE t",
    ] {
        assert!(!limit_applies(sql, MODE), "{sql}");
    }
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
    let sql = check_sql(&["users".into(), "u.email".into(), "shop.orders.id".into(), "o'k".into()], d);
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
    assert!(check_sql(&[], d).contains("AND (FALSE)"));
    // Backslashes are written as the session reads them.
    let nbe = Dialect::MySql(MySqlMode { no_backslash_escapes: true, ..MODE });
    assert!(check_sql(&["a\\b".into()], nbe).contains("('a\\b')"));
    assert!(check_sql(&["a\\b".into()], d).contains("('a\\\\b')"));
}

#[test]
fn limits_the_server_reports() {
    assert_eq!(limit_value("501"), Some(501));
    assert_eq!(limit_value("18446744073709551615"), None);
    assert_eq!(limit_value("x"), None);
}
