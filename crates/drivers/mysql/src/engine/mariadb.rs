//! MariaDB's session set-up and reads of the app's own.

use super::family::{BOUNDED_LOCK_WAIT, Family, StructureShape, select_rest};
use datarig_core::engine::{EngineId, Fact, Version};
use datarig_core::export::keyset::LOCK_WAIT_SECS;
use std::time::Duration;

/// MariaDB.
pub(crate) struct MariaDb;

/// The first version that names the session's transaction settings `transaction_read_only` and
/// `transaction_isolation`; an older one has only `tx_read_only` and `tx_isolation`, which later
/// versions keep as deprecated aliases.
const TRANSACTION_NAMES: Version = Version::new(11, 1, 0);

/// The name of a transaction setting on a server of version `v`: `new` from
/// [`TRANSACTION_NAMES`] on, else `old`, also when the version is not known: the old names are
/// the ones every version the driver works with takes.
fn by_version(v: &Fact<Version>, new: &'static str, old: &'static str) -> &'static str {
    match v {
        Fact::Known(v) if *v >= TRANSACTION_NAMES => new,
        Fact::Known(_) | Fact::No(_) | Fact::Unknown(_) => old,
    }
}

/// MariaDB's bound of a read of the app's own, written before it (`SET STATEMENT … FOR `; it
/// reads MySQL's optimizer hints as a comment): it stops after `time_secs` seconds (no time limit
/// of its own with `None`) and waits at most `lock_secs` seconds for a lock. Neither is ever
/// longer than the session's own: a lock wait of 0 stays 0, a shorter time limit stays. In the
/// statement's settings `@@SESSION.x` is the session's value before it.
fn bound(time_secs: Option<u32>, lock_secs: u32) -> String {
    let lock = format!("lock_wait_timeout = LEAST(@@SESSION.lock_wait_timeout, {lock_secs})");
    match time_secs {
        Some(t) => format!(
            "SET STATEMENT max_statement_time = IF(@@SESSION.max_statement_time = 0, {t}, \
             LEAST(@@SESSION.max_statement_time, {t})), {lock} FOR "
        ),
        None => format!("SET STATEMENT {lock} FOR "),
    }
}

/// MariaDB's checks' subquery (a check's name is the table's; every check is enforced).
const CHECKS: &str = "(SELECT JSON_ARRAYAGG(JSON_OBJECT('name', cc.CONSTRAINT_NAME,
      'clause', cc.CHECK_CLAUSE, 'enforced', 'YES'))
    FROM information_schema.CHECK_CONSTRAINTS cc
    WHERE cc.CONSTRAINT_SCHEMA = {schema} AND cc.TABLE_NAME = {table})";

impl Family for MariaDb {
    fn engine(&self) -> EngineId {
        EngineId::MariaDb
    }

    /// A best effort.
    fn min_version(&self) -> Version {
        Version::new(10, 6, 0)
    }

    fn read_only_var(&self, v: &Fact<Version>) -> &'static str {
        by_version(v, "transaction_read_only", "tx_read_only")
    }

    fn isolation_var(&self, v: &Fact<Version>) -> &'static str {
        by_version(v, "transaction_isolation", "tx_isolation")
    }

    /// `max_statement_time`, in seconds.
    fn statement_timeout(&self, ms: u64) -> String {
        format!("SESSION max_statement_time = {}", ms as f64 / 1000.0)
    }

    /// None: MariaDB has no `super_read_only`.
    fn super_read_only(&self) -> Option<&'static str> {
        None
    }

    /// [`bound`] before the `SELECT`, with the same limits as MySQL's hints.
    fn own_read_bounds(&self) -> (String, String) {
        (bound(Some(10), 2), String::new())
    }

    /// `SET STATEMENT … FOR` before it; no hints.
    fn bounded_select(&self, sql: &str, statement: Duration, lock: Option<u64>, _: &str) -> Option<(String, Duration)> {
        select_rest(sql)?;
        // Its limits take fractions, and a lock wait of 0 is the session's choice not to wait: a
        // value that does not read is taken as that, never as a longer wait.
        let (secs, lock) = (statement.as_secs_f64(), lock.unwrap_or(0).min(BOUNDED_LOCK_WAIT));
        let text = format!("SET STATEMENT max_statement_time = {secs}, lock_wait_timeout = {lock} FOR {sql}");
        Some((text, statement))
    }

    /// `max_statement_time`, in seconds, with decimals.
    fn session_limit(&self, v: &str) -> Option<Duration> {
        let secs: f64 = v.trim().parse().ok()?;
        let d = (secs.is_finite() && secs > 0.0 && secs < 1e9).then(|| Duration::from_secs_f64(secs))?;
        (!d.is_zero()).then_some(d)
    }

    /// Two statements: the session's own values first, unbounded (it reads no table), then the
    /// lookup bounded by [`bound`], inside which `@@SESSION.x` would be the bound's value, not
    /// the session's.
    fn check_sql(&self, isolation: &str, lookup: &str) -> Vec<String> {
        let (bound, _) = self.own_read_bounds();
        vec![
            format!(
                "SELECT @@SESSION.{isolation}, @@SESSION.max_statement_time, @@SESSION.lock_wait_timeout, \
                 @@SESSION.max_sort_length"
            ),
            format!("{bound}SELECT {lookup}"),
        ]
    }

    /// None: MariaDB has no partial revokes (nor the variable), so a global grant counts.
    fn not_undone_global(&self) -> &'static str {
        ""
    }

    /// [`bound`] before it.
    fn keyset_text(&self, sql: &str) -> Option<String> {
        sql.strip_prefix("SELECT ")?;
        Some(format!("{}{sql}", bound(None, LOCK_WAIT_SECS)))
    }

    /// On every version: no functional key parts, no visibility, no partial revokes; its own
    /// checks' subquery.
    fn structure(&self, _: Version) -> StructureShape {
        StructureShape { expression: "NULL", visible: "NULL", checks: CHECKS, partial_revokes: "0" }
    }

    /// Its catalogue's text is SQL as written.
    fn escapes_sql_text(&self) -> bool {
        false
    }
}
