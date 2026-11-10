//! The risk classifier against the engine, for a corpus of statements. Two properties:
//!
//! * A statement the classifier allows under a read-only policy must leave the file unchanged
//!   when a read-only session runs it: the engine, not the text, is what stops a write.
//! * A statement that changes the file in a read-write session must not be allowed under a
//!   read-only policy: the classifier does not let a write through as a read.

mod common;

use common::*;
use datarig_core::driver::SessionRole;
use datarig_core::sql::dialect::{Dialect, Language};
use datarig_core::sql::risk::Classifier;

/// The statements tried, against a file with one table `t` of two rows. `{other}` is a path
/// that does not exist yet (an `ATTACH` target), filled in per test.
const CORPUS: &[&str] = &[
    "SELECT a FROM t",
    "SELECT count(*) FROM t",
    "WITH x AS (SELECT 1 AS n) SELECT n FROM x",
    "EXPLAIN SELECT a FROM t",
    "PRAGMA table_info(t)",
    "PRAGMA user_version",
    "PRAGMA user_version = 5",
    "PRAGMA journal_mode = WAL",
    "PRAGMA writable_schema = ON",
    "PRAGMA foreign_keys = ON",
    "BEGIN",
    "SAVEPOINT s",
    "INSERT INTO t VALUES (3)",
    "INSERT INTO t SELECT a FROM t",
    "REPLACE INTO t VALUES (4)",
    "UPDATE t SET a = 9",
    "UPDATE t SET a = 9 WHERE a = 1",
    "DELETE FROM t",
    "DELETE FROM t WHERE a = 1",
    "DROP TABLE t",
    "CREATE TABLE u (x)",
    "CREATE TABLE IF NOT EXISTS t (a)",
    "ALTER TABLE t RENAME TO t2",
    "ALTER TABLE t ADD COLUMN b",
    "VACUUM",
    "ANALYZE",
    "REINDEX",
    "SELECT 1; DELETE FROM t",
    "DELETE FROM t; SELECT 1",
    "WITH d AS (SELECT 1) DELETE FROM t",
    "ATTACH DATABASE '{other}' AS other",
    "DETACH DATABASE main",
];

fn fixture(file: &std::path::Path) {
    create(file, "CREATE TABLE t (a INTEGER); INSERT INTO t VALUES (1), (2);");
}

/// Runs `sql` in a session of the given policy on a fresh copy of the fixture, and returns the
/// file's bytes before and after.
fn engine(scratch: &Scratch, name: &str, sql: &str, read_only: bool) -> (Vec<u8>, Vec<u8>) {
    let file = scratch.file(name);
    fixture(&file);
    let before = std::fs::read(&file).unwrap();
    {
        let (session, mut rx) = connect(&file, SessionRole::Query, read_only);
        let _ = run(&session, &mut rx, 1, sql);
    }
    let after = std::fs::read(&file).unwrap();
    (before, after)
}

#[test]
fn a_statement_the_policy_allows_never_changes_the_file_in_a_read_only_session() {
    let scratch = Scratch::new();
    for (i, template) in CORPUS.iter().enumerate() {
        let sql = template.replace("{other}", &scratch.file(&format!("other{i}.db")).display().to_string());
        let allowed = Classifier::classify_once(Language::Sql(Dialect::Sqlite), &sql).read_only().is_ok();
        let (before, after) = engine(&scratch, &format!("ro{i}.db"), &sql, true);
        assert!(
            !(allowed && before != after),
            "the classifier allows `{sql}` under a read-only policy, but the engine changed the file"
        );
    }
}

#[test]
fn a_statement_that_changes_the_file_is_not_allowed_under_a_read_only_policy() {
    let scratch = Scratch::new();
    let mut changing = 0;
    for (i, template) in CORPUS.iter().enumerate() {
        let sql = template.replace("{other}", &scratch.file(&format!("other{i}.db")).display().to_string());
        let allowed = Classifier::classify_once(Language::Sql(Dialect::Sqlite), &sql).read_only().is_ok();
        let (before, after) = engine(&scratch, &format!("rw{i}.db"), &sql, false);
        let changed = before != after;
        changing += usize::from(changed);
        assert!(
            !(changed && allowed),
            "`{sql}` changes the file, yet the classifier allows it under a read-only policy"
        );
    }
    // The corpus must exercise real writes, or the property above proves nothing.
    assert!(changing >= 10, "only {changing} statements of the corpus changed the file");
}
