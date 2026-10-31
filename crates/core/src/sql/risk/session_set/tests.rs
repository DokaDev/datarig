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
        ("SET idle_in_transaction_session_timeout = 0", Owner::TxIdleRollback),
        ("SET default_transaction_read_only = off", Owner::ReadOnly),
        ("SET SESSION transaction_read_only = on", Owner::ReadOnly),
        ("SET application_name = 'x'", Owner::SessionName),
        ("SET client_encoding = 'LATIN1'", Owner::Encoding),
        ("SET DateStyle = 'German'", Owner::TextFormat),
        ("SET bytea_output = 'escape'", Owner::TextFormat),
        ("SET standard_conforming_strings = off", Owner::Literals),
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
    // A transaction takes its defaults at `BEGIN`: a `SET LOCAL` after it changes nothing.
    for sql in ["SET default_transaction_isolation = 'serializable'", "SET default_transaction_deferrable = on"] {
        let s = pg(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
        assert_eq!(s.per_transaction, Some(Err(NotLocal::Default)), "{sql}");
    }
    // What the pooled server connection did before decides whether they hold.
    assert_eq!(pg("SET temp_buffers = '16MB'").unwrap().per_transaction, Some(Err(NotLocal::TempBuffers)));
    assert_eq!(
        pg("SET transaction_timeout = '1min'").unwrap().per_transaction,
        Some(Err(NotLocal::TransactionTimeout))
    );
}

/// The server's idle limit is a preset's to set (it is then the sessions' idle disconnect):
/// PostgreSQL's has no form for one transaction (the server counts it only outside one), and
/// a read-only environment allows PostgreSQL's (MySQL's idle limits are not among its safe
/// settings).
#[test]
fn the_servers_idle_limit_is_a_presets_to_set() {
    for sql in ["SET idle_session_timeout = '10min'", "SET SESSION idle_session_timeout TO 0"] {
        let s = pg(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
        assert_eq!(s.per_transaction, Some(Err(NotLocal::IdleSession)), "{sql}");
        assert_eq!(s.settings[0].0, "idle_session_timeout", "{sql}");
        assert!(s.risk.read_only().is_ok(), "{sql}");
    }
    for sql in ["SET SESSION wait_timeout = 600", "SET @@session.wait_timeout = 600"] {
        let s = mysql(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
        assert_eq!(s.settings, vec![("wait_timeout".to_string(), "600".to_string())], "{sql}");
        assert!(s.risk.read_only().is_err(), "{sql}");
    }
    // MySQL's must be one the app reads: the server counts it inside a transaction too.
    for sql in [
        "SET wait_timeout = DEFAULT",
        "SET SESSION wait_timeout = 0",
        "SET SESSION WAIT_TIMEOUT = 0",
        "SET wait_timeout = @@global.wait_timeout",
        "SET wait_timeout = '600'",
        "SET wait_timeout = 31536001",
    ] {
        assert_eq!(mysql(sql), Err(SetProblem::IdleValue), "{sql}");
    }
    assert!(matches!(
        mysql("SET SESSION interactive_timeout = 600"),
        Err(SetProblem::AppOwned { owner: Owner::Interactive, .. })
    ));
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

/// Two statements in one item without a `;` between them (as one line or on two) are not one
/// setting, in either database, and read-only does not change that.
#[test]
fn two_statements_without_a_semicolon_are_not_one_setting() {
    for sql in ["SET @c=3 SET @d=4", "SET @c = 3\nSET @d = 4", "SET SESSION a = 1 set b = 2"] {
        assert_eq!(mysql(sql).map(|_| ()), Err(SetProblem::NotOne), "{sql}");
    }
    for sql in ["SET a = 1 SET b = 2", "SET a = 1\nSET b = 2"] {
        assert!(pg(sql).is_err(), "{sql}");
    }
    // A string that says SET is a value.
    assert!(mysql("SET @c = 'SET @d = 4'").is_ok());
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
        ("SET SESSION interactive_timeout = 10", Owner::Interactive),
        ("SET @@session.transaction_read_only = 0", Owner::ReadOnly),
        ("SET SESSION TRANSACTION READ WRITE", Owner::ReadOnly),
        ("SET autocommit = 0", Owner::Commit),
        ("SET SESSION completion_type = 1", Owner::Completion),
        ("SET sql_select_limit = 10", Owner::Paging),
        ("SET NAMES latin1", Owner::Encoding),
        ("SET character_set_results = latin1", Owner::Encoding),
        ("SET collation_connection = latin1_swedish_ci", Owner::Encoding),
        ("SET resultset_metadata = NONE", Owner::Reports),
        ("SET session_track_schema = OFF", Owner::Reports),
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
    assert!(matches!(
        pg("SET standard_conforming_strings = off"),
        Err(SetProblem::AppOwned { owner: Owner::Literals, .. })
    ));
    for sql in [
        "SET resultset_metadata = NONE",
        "SET SESSION completion_type = 1",
        "SET `autocommit` = 1",
        "SET SESSION `interactive_timeout` = 1",
    ] {
        assert!(matches!(mysql(sql), Err(SetProblem::AppOwned { .. })), "{sql}: {:?}", mysql(sql));
    }
    match mysql("SET `sql_select_limit` = 1") {
        Err(SetProblem::AppOwned { name, owner }) => {
            assert_eq!((name.as_str(), owner), ("sql_select_limit", Owner::Paging))
        }
        other => panic!("{other:?}"),
    }
}

/// The lock wait limits (and MySQL's statement time limit) are a preset's to set for its
/// sessions: they override the environment's, and a read-only environment allows them (a limit
/// writes nothing).
#[test]
fn lock_wait_limits_are_a_presets_to_set() {
    for sql in ["SET lock_timeout = '3s'", "SET SESSION lock_timeout TO 500", "SET lock_timeout TO DEFAULT"] {
        let s = pg(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
        assert!(s.risk.read_only().is_ok(), "{sql}");
        assert!(matches!(s.per_transaction, Some(Ok(_))), "{sql}");
    }
    for sql in [
        "SET SESSION lock_wait_timeout = 3",
        "SET innodb_lock_wait_timeout = 3",
        "SET @@session.innodb_lock_wait_timeout = 3",
        "SET lock_wait_timeout = 3, innodb_lock_wait_timeout = 3",
        "SET SESSION max_execution_time = 1000",
    ] {
        let s = mysql(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
        assert!(s.risk.read_only().is_ok(), "{sql}: {:?}", s.risk.read_only());
    }
}

/// What an allowed statement sets: each setting's name and its value as written, in order (not a
/// user variable).
#[test]
fn an_allowed_statement_says_what_it_sets() {
    let set = |s: SessionSet| s.settings;
    let pairs = |p: &[(&str, &str)]| p.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect::<Vec<_>>();
    assert_eq!(set(pg("SET lock_timeout = '3s';").unwrap()), pairs(&[("lock_timeout", "'3s'")]));
    assert_eq!(set(pg("SET SESSION Lock_Timeout TO /* ms */ 500").unwrap()), pairs(&[("lock_timeout", "500")]));
    assert_eq!(set(pg("SET lock_timeout TO DEFAULT").unwrap()), pairs(&[("lock_timeout", "DEFAULT")]));
    assert_eq!(
        set(mysql("SET SESSION lock_wait_timeout = 3, @x = 1, @@session.innodb_lock_wait_timeout = 4").unwrap()),
        pairs(&[("lock_wait_timeout", "3"), ("innodb_lock_wait_timeout", "4")])
    );
    assert_eq!(set(mysql("SET lock_wait_timeout = DEFAULT").unwrap()), pairs(&[("lock_wait_timeout", "DEFAULT")]));
}
