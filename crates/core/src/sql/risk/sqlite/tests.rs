use super::*;
use crate::sql::risk::{Class, Classifier, Danger, ReadOnlyBlock, Why};

#[test]
fn a_select_is_a_read_a_policy_allows() {
    let risk = classify("SELECT a FROM t WHERE a > 1");
    assert_eq!(risk.class, Class::Read);
    assert!(risk.read_only().is_ok());
    assert!(risk.confirm(true).is_none());
}

#[test]
fn an_insert_writes_and_a_read_only_policy_refuses_it() {
    let risk = classify("INSERT INTO t VALUES (1)");
    assert_eq!(risk.class, Class::Write);
    assert!(risk.writes);
    assert_eq!(risk.read_only(), Err(ReadOnlyBlock::Class(Class::Write)));
    assert!(matches!(risk.confirm(true), Some(Why::Class(Class::Write))));
}

#[test]
fn a_delete_without_where_is_dangerous_and_one_with_where_is_not() {
    assert_eq!(classify("DELETE FROM t").danger, Some(Danger::DeleteAll));
    assert_eq!(classify("UPDATE t SET a = 1").danger, Some(Danger::UpdateAll));
    let with_where = classify("DELETE FROM t WHERE a = 1");
    assert_eq!(with_where.danger, None);
    assert_eq!(with_where.class, Class::Write);
}

#[test]
fn a_drop_or_a_dropped_column_is_dangerous() {
    assert_eq!(classify("DROP TABLE t").danger, Some(Danger::Drop));
    assert_eq!(classify("ALTER TABLE t DROP COLUMN a").danger, Some(Danger::DropColumn));
    assert_eq!(classify("ALTER TABLE t RENAME TO u").danger, None);
}

#[test]
fn ddl_and_maintenance_are_their_own_classes() {
    assert_eq!(classify("CREATE TABLE t (a INTEGER)").class, Class::Ddl);
    assert_eq!(classify("VACUUM").class, Class::Maintenance);
    assert_eq!(classify("BEGIN").class, Class::Tx);
}

#[test]
fn a_pragma_that_only_reads_is_a_read_and_one_that_sets_is_not() {
    assert!(classify("PRAGMA table_info(t)").read_only().is_ok());
    assert!(classify("PRAGMA main.index_list(t)").read_only().is_ok());
    assert_eq!(classify("PRAGMA user_version = 3").read_only(), Err(ReadOnlyBlock::Setting));
    assert_eq!(classify("PRAGMA writable_schema = ON").danger, Some(Danger::Setting));
}

#[test]
fn attach_and_detach_are_refused_by_a_read_only_policy_and_asked_otherwise() {
    let attach = classify("ATTACH DATABASE '/tmp/other.db' AS other");
    assert_eq!(attach.read_only(), Err(ReadOnlyBlock::FileAccess));
    assert!(matches!(attach.confirm(false), Some(Why::Danger(Danger::FileAccess))));
    assert_eq!(classify("DETACH other").danger, Some(Danger::FileAccess));
}

#[test]
fn several_statements_count_as_their_worst() {
    let risk = classify("SELECT 1; DELETE FROM t");
    assert_eq!(risk.class, Class::Write);
    assert!(risk.read_only().is_err());
}

#[test]
fn text_with_a_keyword_inside_a_string_or_comment_is_not_a_write() {
    assert_eq!(classify("SELECT 'DELETE FROM t'").class, Class::Read);
    assert_eq!(classify("-- DROP TABLE t\nSELECT 1").class, Class::Read);
}

#[test]
fn an_empty_text_or_an_unknown_statement_is_unknown() {
    assert_eq!(classify("").class, Class::Unknown);
    assert_eq!(classify("LOAD_EXTENSION('x')").class, Class::Unknown);
    assert!(classify("LOAD_EXTENSION('x')").read_only().is_err());
}

#[test]
fn a_query_is_never_repeatable_here_and_no_count_is_known() {
    let classifier = Classifier::new(crate::sql::dialect::Language::Sql(crate::sql::dialect::Dialect::Sqlite));
    assert!(classifier.repeatable("SELECT 1").is_err());
    assert!(classifier.count_query("SELECT 1").is_err());
}
