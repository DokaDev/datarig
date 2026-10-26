use super::*;
use crate::sql::dialect::{Dialect, Language, MySqlMode};
use crate::sql::risk::Classifier;

const PG: Language = Language::Sql(Dialect::Postgres);

fn my() -> Language {
    Language::Sql(Dialect::MySql(MySqlMode::default()))
}

fn pg(sql: &str) -> Result<SessionSet, SetProblem> {
    Classifier::session_set(PG, sql)
}

fn mysql(sql: &str) -> Result<SessionSet, SetProblem> {
    Classifier::session_set(my(), sql)
}

#[test]
fn pg_allows_session_sets_with_their_transaction_form() {
    let cases = [
        ("SET statement_timeout = '30s'", "SET LOCAL statement_timeout = '30s'"),
        ("set work_mem to '64MB';", "SET LOCAL work_mem to '64MB'"),
        ("SET SESSION work_mem = '64MB'", "SET LOCAL work_mem = '64MB'"),
        ("SET TIME ZONE 'UTC'", "SET LOCAL TIME ZONE 'UTC'"),
        ("SET statement_timeout TO DEFAULT", "SET LOCAL statement_timeout TO DEFAULT"),
        ("SET /* c */ my.flag = 'on' -- trailing", "SET LOCAL my . flag = 'on'"),
        ("SET session.x = 1", "SET LOCAL session . x = 1"),
    ];
    for (sql, local) in cases {
        let s = pg(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
        assert_eq!(s.per_transaction, Some(Ok(local.to_string())), "{sql}");
        assert!(s.risk.read_only().is_ok(), "{sql}: a safe setting");
    }
}

#[test]
fn pg_refuses_what_is_not_a_session_set() {
    let cases = [
        ("", SetProblem::NotOne),
        ("SET a.b = 1; SET a.c = 2", SetProblem::NotOne),
        ("SET LOCAL work_mem = '1MB'", SetProblem::Local),
        ("RESET work_mem", SetProblem::NotSet),
        ("RESET ALL", SetProblem::NotSet),
        ("DISCARD ALL", SetProblem::NotSet),
        ("SELECT set_config('work_mem', '1MB', false)", SetProblem::NotSet),
        ("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE", SetProblem::NotSet),
        ("SHOW work_mem", SetProblem::NotSet),
        ("SET x = (SELECT 1)", SetProblem::Unread),
        ("SET work_mem =", SetProblem::Unread),
    ];
    for (sql, want) in cases {
        assert_eq!(pg(sql), Err(want), "{sql}");
    }
}

#[test]
fn pg_refuses_the_settings_the_app_keeps() {
    let cases = [
        ("SET search_path = a", Owner::Schema),
        ("SET lock_timeout = '5s'", Owner::LockWait),
        ("SET idle_session_timeout = 0", Owner::IdleDisconnect),
        ("SET idle_in_transaction_session_timeout = 0", Owner::TxIdleRollback),
        ("SET default_transaction_read_only = off", Owner::ReadOnly),
        ("SET SESSION transaction_read_only = on", Owner::ReadOnly),
        ("SET application_name = 'x'", Owner::App),
        ("SET client_encoding = 'LATIN1'", Owner::App),
        ("SET DateStyle = 'German'", Owner::App),
        ("SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE", Owner::ReadOnly),
    ];
    for (sql, owner) in cases {
        match pg(sql) {
            Err(SetProblem::AppOwned { owner: o, .. }) => assert_eq!(o, owner, "{sql}"),
            other => panic!("{sql}: {other:?}"),
        }
    }
}

#[test]
fn pg_settings_without_a_transaction_form() {
    let identity = pg("SET ROLE reporting").unwrap();
    assert_eq!(identity.per_transaction, Some(Err(NotLocal::Identity)));
    let auth = pg("SET SESSION AUTHORIZATION reporting").unwrap();
    assert_eq!(auth.per_transaction, Some(Err(NotLocal::Identity)));
    let chars = pg("SET SESSION CHARACTERISTICS AS TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
    assert_eq!(chars.per_transaction, Some(Err(NotLocal::Form)));
}

#[test]
fn pg_read_only_refusal_is_the_classifiers() {
    // Not on the read-only allowlist: allowed as a setting, refused by a read-only environment.
    let s = pg("SET session_replication_role = replica").unwrap();
    assert!(s.risk.read_only().is_err());
    assert!(pg("SET work_mem = '1MB'").unwrap().risk.read_only().is_ok());
}

#[test]
fn mysql_allows_session_sets() {
    for sql in [
        "SET SESSION sql_mode = 'ANSI_QUOTES'",
        "SET @@session.time_zone = '+00:00'",
        "SET time_zone = '+00:00', @x = 1",
        "SET @@max_heap_table_size = 16777216",
        "SET SESSION TRANSACTION ISOLATION LEVEL READ COMMITTED",
        "SET LOCAL group_concat_max_len = 4096;",
    ] {
        let s = mysql(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
        assert_eq!(s.per_transaction, None, "{sql}");
    }
}

#[test]
fn mysql_refuses_what_is_not_a_session_set() {
    let cases = [
        ("SET GLOBAL max_connections = 10", SetProblem::NotSet),
        ("SET PERSIST max_connections = 10", SetProblem::NotSet),
        ("SET @@global.sql_mode = ''", SetProblem::NotSet),
        ("SET SESSION a = 1, GLOBAL b = 2", SetProblem::NotSet),
        ("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE", SetProblem::NotSet),
        ("SET PASSWORD = 'x'", SetProblem::NotSet),
        ("USE other", SetProblem::NotSet),
        ("SELECT 1", SetProblem::NotSet),
        ("SET a = 1; SET b = 2", SetProblem::NotOne),
    ];
    for (sql, want) in cases {
        assert_eq!(mysql(sql), Err(want), "{sql}");
    }
}

#[test]
fn mysql_refuses_the_settings_the_app_keeps() {
    let cases = [
        ("SET innodb_lock_wait_timeout = 1", Owner::LockWait),
        ("SET SESSION wait_timeout = 10", Owner::IdleDisconnect),
        ("SET @@session.transaction_read_only = 0", Owner::ReadOnly),
        ("SET SESSION TRANSACTION READ WRITE", Owner::ReadOnly),
        ("SET autocommit = 0", Owner::Commit),
        ("SET sql_select_limit = 10", Owner::App),
        ("SET NAMES latin1", Owner::App),
        ("SET character_set_results = latin1", Owner::App),
    ];
    for (sql, owner) in cases {
        match mysql(sql) {
            Err(SetProblem::AppOwned { owner: o, .. }) => assert_eq!(o, owner, "{sql}"),
            other => panic!("{sql}: {other:?}"),
        }
    }
}

#[test]
fn mysql_reads_with_ansi_quotes_too() {
    // Under ANSI_QUOTES `"x"` is a name: the setting is then assigned a column, not a string.
    // Either reading must be a plain setting; a risky one is refused only in read-only.
    let s = mysql("SET SESSION foreign_key_checks = 0").unwrap();
    assert_eq!(s.risk.danger, Some(Danger::Setting));
    assert!(s.risk.read_only().is_err());
    assert_eq!(mysql("SET x = (SELECT 1); DROP TABLE t"), Err(SetProblem::NotOne));
}

/// Read in every sql mode: a backslash that ends a string in one mode and not in another must not
/// hide a server setting (`NO_BACKSLASH_ESCAPES` reads `'a\'` as a whole string).
#[test]
fn mysql_reads_every_mode_and_refuses_what_they_read_differently() {
    let hidden = r"SET @x = 'a\', @@global.max_connections = 1 -- '";
    assert_eq!(mysql(hidden), Err(SetProblem::NotSet), "the global setting another mode reads");
    // Readable both ways but as other settings: refused.
    let differs = r#"SET @x = 'a\', @y = 'b'"#;
    assert!(mysql(differs).is_err(), "{:?}", mysql(differs));
    assert!(mysql("SET @x = 'plain'").is_ok());
}

#[test]
fn transaction_only_settings_are_refused_with_where_the_default_is() {
    for sql in ["SET transaction_isolation = 'serializable'", "SET transaction_deferrable = on"] {
        assert!(matches!(pg(sql), Err(SetProblem::TransactionOnly { .. })), "{sql}: {:?}", pg(sql));
    }
    assert!(pg("SET default_transaction_isolation = 'serializable'").is_ok());
    assert!(matches!(
        pg("SET SESSION transaction_read_only = off"),
        Err(SetProblem::AppOwned { owner: Owner::ReadOnly, .. })
    ));
    // MySQL: `@@name` without a scope is the next transaction only; with SESSION, the session's.
    assert!(matches!(mysql("SET @@transaction_isolation = 'READ-COMMITTED'"), Err(SetProblem::TransactionOnly { .. })));
    assert!(mysql("SET @@session.transaction_isolation = 'READ-COMMITTED'").is_ok());
    assert!(mysql("SET SESSION transaction_isolation = 'READ-COMMITTED'").is_ok());
}

#[test]
fn more_settings_the_app_keeps() {
    assert!(matches!(pg("SET standard_conforming_strings = off"), Err(SetProblem::AppOwned { owner: Owner::App, .. })));
    for sql in [
        "SET resultset_metadata = NONE",
        "SET SESSION completion_type = 1",
        "SET `lock_wait_timeout` = 1",
        "SET SESSION `wait_timeout` = 1",
    ] {
        assert!(matches!(mysql(sql), Err(SetProblem::AppOwned { .. })), "{sql}: {:?}", mysql(sql));
    }
    match mysql("SET `innodb_lock_wait_timeout` = 1") {
        Err(SetProblem::AppOwned { name, owner }) => {
            assert_eq!((name.as_str(), owner), ("innodb_lock_wait_timeout", Owner::LockWait))
        }
        other => panic!("{other:?}"),
    }
}
