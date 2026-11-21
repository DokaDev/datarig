//! MySQL's session set-up and reads of the app's own.

use super::family::{BOUNDED_LOCK_WAIT, Family, StructureShape, select_rest};
use datarig_core::engine::{EngineId, Fact, Version};
use datarig_core::export::keyset::LOCK_WAIT_SECS;
use std::time::Duration;

/// MySQL.
pub(crate) struct MySql;

/// A `SET_VAR` hint for the session's own catalog reads: they give up after 2 s behind a lock
/// another session holds or waits for, and stop after 10 s.
const OWN_READ_HINTS: &str = "/*+ SET_VAR(lock_wait_timeout=2) MAX_EXECUTION_TIME(10000) */";

/// The checks' subquery of MySQL 8.0.16 and later (`ENFORCED`; a check's name is the
/// database's).
const CHECKS: &str = "(SELECT JSON_ARRAYAGG(JSON_OBJECT('name', tc.CONSTRAINT_NAME,
      'clause', cc.CHECK_CLAUSE, 'enforced', tc.ENFORCED))
    FROM information_schema.TABLE_CONSTRAINTS tc JOIN information_schema.CHECK_CONSTRAINTS cc
      ON cc.CONSTRAINT_SCHEMA = tc.CONSTRAINT_SCHEMA AND cc.CONSTRAINT_NAME = tc.CONSTRAINT_NAME
    WHERE tc.TABLE_SCHEMA = {schema} AND tc.TABLE_NAME = {table} AND tc.CONSTRAINT_TYPE = 'CHECK')";

impl Family for MySql {
    fn engine(&self) -> EngineId {
        EngineId::MySql
    }

    fn min_version(&self) -> Version {
        Version::new(8, 0, 0)
    }

    /// `transaction_read_only` on every version the driver works with (8.0 has no other name).
    fn read_only_var(&self, _: &Fact<Version>) -> &'static str {
        "transaction_read_only"
    }

    /// `transaction_isolation` on every version the driver works with.
    fn isolation_var(&self, _: &Fact<Version>) -> &'static str {
        "transaction_isolation"
    }

    /// The lock wait's optimizer hint alone, as in [`Family::bounded_select`].
    fn lock_bounded_select(&self, sql: &str, _: Option<u64>) -> Option<String> {
        let rest = select_rest(sql)?;
        Some(format!("SELECT /*+ SET_VAR(lock_wait_timeout={BOUNDED_LOCK_WAIT}) */ {rest}"))
    }

    /// `max_execution_time`, in milliseconds.
    fn statement_timeout(&self, ms: u64) -> String {
        format!("SESSION max_execution_time = {ms}")
    }

    fn super_read_only(&self) -> Option<&'static str> {
        Some("super_read_only")
    }

    /// The hints after the `SELECT` ([`OWN_READ_HINTS`], with the space that follows them).
    fn own_read_bounds(&self) -> (String, String) {
        (String::new(), format!("{OWN_READ_HINTS} "))
    }

    /// The optimizer hints after its `SELECT` (on the outermost `SELECT`: one in a derived table
    /// is ignored); the lock wait is the shortest the server takes, whatever the session's.
    fn bounded_select(
        &self,
        sql: &str,
        statement: Duration,
        _: Option<u64>,
        hints: &str,
    ) -> Option<(String, Duration)> {
        let rest = select_rest(sql)?;
        // The limit is in whole milliseconds, 0 for none.
        let statement = statement.max(Duration::from_millis(1));
        let (ms, lock) = (statement.as_millis().min(u128::from(u32::MAX)), BOUNDED_LOCK_WAIT);
        let text = format!("SELECT /*+ MAX_EXECUTION_TIME({ms}) SET_VAR(lock_wait_timeout={lock}){hints} */ {rest}");
        Some((text, statement))
    }

    /// `max_execution_time`, in milliseconds.
    fn session_limit(&self, v: &str) -> Option<Duration> {
        let d = Duration::from_millis(v.trim().parse::<u64>().ok()?);
        (!d.is_zero()).then_some(d)
    }

    /// One statement, bounded by [`OWN_READ_HINTS`] (which leave the session's
    /// `max_execution_time` as it has it); it has no lock wait to read.
    fn check_sql(&self, isolation: &str, lookup: &str) -> Vec<String> {
        vec![format!(
            "SELECT {OWN_READ_HINTS} @@SESSION.{isolation}, {lookup}, @@SESSION.max_execution_time, NULL, \
             @@SESSION.max_sort_length"
        )]
    }

    /// Only without partial revokes: with them on, a global grant may be revoked for the table's
    /// schema, which the privilege tables do not show.
    fn not_undone_global(&self) -> &'static str {
        " AND @@GLOBAL.partial_revokes = 0"
    }

    /// The hint after the `SELECT`.
    fn keyset_text(&self, sql: &str) -> Option<String> {
        let rest = sql.strip_prefix("SELECT ")?;
        Some(format!("SELECT /*+ SET_VAR(lock_wait_timeout={LOCK_WAIT_SECS}) */ {rest}"))
    }

    /// Functional key parts from 8.0.13; checks and `partial_revokes` from 8.0.16.
    fn structure(&self, v: Version) -> StructureShape {
        let checked = v >= Version::new(8, 0, 16);
        StructureShape {
            expression: if v >= Version::new(8, 0, 13) { "x.EXPRESSION" } else { "NULL" },
            visible: "x.IS_VISIBLE",
            checks: if checked { CHECKS } else { "NULL" },
            partial_revokes: if checked { "@@GLOBAL.partial_revokes" } else { "0" },
        }
    }

    fn escapes_sql_text(&self) -> bool {
        true
    }
}
