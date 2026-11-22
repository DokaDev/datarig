//! What each engine the driver serves says of a session's set-up and of the reads of the app's
//! own ([`Family`]): the oldest version the driver works with, the names and forms of the
//! session's settings by the server's version, how a read of the app's own is bounded, and what
//! a table's structure is read with.

use datarig_core::driver::DbError;
use datarig_core::engine::{EngineId, Fact, Version};
use std::time::Duration;

/// How long a bounded read of the app's own ([`Family::bounded_select`]) waits for a metadata
/// lock, in seconds: on MySQL the shortest `lock_wait_timeout` it takes, so never longer than the
/// session's. MySQL's own value cannot be read on the way: the allowlist's question sets it for
/// itself (`SET_VAR` in its hints, [`Family::own_read_bounds`]) and so reads its own value, and
/// asking apart would cost another round trip. MariaDB reads that hint as a comment, so its value
/// is asked with the question ([`Family::check_sql`]) and never made longer.
pub(crate) const BOUNDED_LOCK_WAIT: u64 = 1;

/// What a table's structure statement reads on a server, as it has it: each part is the SQL put
/// in its place, `NULL` (or `0`) where the server has nothing to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StructureShape {
    /// A key part's expression (functional key parts).
    pub(crate) expression: &'static str,
    /// Whether a key is visible.
    pub(crate) visible: &'static str,
    /// The checks' subquery, its `{schema}` and `{table}` to be filled.
    pub(crate) checks: &'static str,
    /// Whether a global grant may be revoked for a schema (partial revokes).
    pub(crate) partial_revokes: &'static str,
}

/// An engine of the family, as the driver sets a session up on it.
pub(crate) trait Family: Sync {
    /// The engine.
    fn engine(&self) -> EngineId;

    /// The oldest version the driver works with.
    fn min_version(&self) -> Version;

    /// The session variable that makes the session's transactions read-only, on a server of
    /// version `v`.
    fn read_only_var(&self, v: &Fact<Version>) -> &'static str;

    /// The session variable of the session's transaction isolation level, on a server of
    /// version `v`.
    fn isolation_var(&self, v: &Fact<Version>) -> &'static str;

    /// The `SET` assignment that stops a `SELECT` after `ms` milliseconds.
    fn statement_timeout(&self, ms: u64) -> String;

    /// The server's own setting that refuses the writes of every session, a privileged one's
    /// too (asked for the message of a session that is not read-only); `None` on an engine that
    /// has none.
    fn super_read_only(&self) -> Option<&'static str>;

    /// The bounds of a catalogue read of the session's own, as written around its `SELECT`: what
    /// goes before it and what goes right after it (with the space that follows). The read gives
    /// up after 2 s behind a lock another session holds or waits for, and stops after 10 s.
    fn own_read_bounds(&self) -> (String, String);

    /// `sql`, a `SELECT` of the app's own, stopped by the server after `statement` (already never
    /// longer than the session's own time limit) and given at most [`BOUNDED_LOCK_WAIT`] to wait
    /// for a metadata lock, for this statement alone; `lock` is the session's own lock wait in
    /// seconds, when it was asked, and `hints` go with the engine's optimizer hints, if it reads
    /// them. With the time limit that applies; `None` for a text that does not start with
    /// `SELECT`.
    fn bounded_select(
        &self,
        sql: &str,
        statement: Duration,
        lock: Option<u64>,
        hints: &str,
    ) -> Option<(String, Duration)>;

    /// A session's statement time limit as the server writes it, `None` for none (0) or a value
    /// that does not read.
    fn session_limit(&self, v: &str) -> Option<Duration>;

    /// The allowlist's question: the session's transaction isolation (session variable
    /// `isolation`) and the time limit of a `SELECT`, with `lookup` (a subquery, in
    /// parentheses: the first name that is a view or a table of the server's own), in one
    /// statement or more. Its answer is read as five values: the isolation, the lookup's answer,
    /// the time limit, the lock wait and the sort length.
    fn check_sql(&self, isolation: &str, lookup: &str) -> Vec<String>;

    /// The condition, with the `AND` before it, under which a global grant counts for every
    /// schema in the question whether a statement's tables undo (empty: it always does).
    fn not_undone_global(&self) -> &'static str;

    /// Keyset chunk `sql` (a plain `SELECT`) as it is sent: waiting at most
    /// [`datarig_core::export::keyset::LOCK_WAIT_SECS`] for a metadata lock. `None` for a text
    /// that does not start with `SELECT `: it is not sent unbounded.
    fn keyset_text(&self, sql: &str) -> Option<String>;

    /// What a table's structure statement reads on a server of version `v`.
    fn structure(&self, v: Version) -> StructureShape;

    /// Whether the catalogue writes the SQL text it keeps other than as SQL: MySQL's keeps an
    /// expression's string literals with their escapes undone once and a column's default as its
    /// bare value; `false` where both are SQL as written.
    fn escapes_sql_text(&self) -> bool;
}

/// What follows the `SELECT` that `sql` starts with (and the space or line break after it);
/// `None` for a text that does not start so.
pub(super) fn select_rest(sql: &str) -> Option<&str> {
    sql.strip_prefix("SELECT ").or_else(|| sql.strip_prefix("SELECT\n"))
}

/// The family's rules of engine `e`.
pub(crate) fn family(e: EngineId) -> &'static dyn Family {
    match e {
        EngineId::MySql => &super::mysql::MySql,
        EngineId::MariaDb => &super::mariadb::MariaDb,
        // Never the engine of this driver's sessions: MySQL's, as the rest of the driver reads it.
        EngineId::Postgres => &super::mysql::MySql,
    }
}

/// The name a server of engine `e` is called by in a message.
fn name(e: EngineId) -> &'static str {
    match e {
        EngineId::MariaDb => "MariaDB",
        EngineId::MySql | EngineId::Postgres => "MySQL",
    }
}

/// `MySQL 8.4.6`, `MariaDB 11.4.2`: a server of `f`'s engine at version `v`.
pub(crate) fn label(f: &dyn Family, v: Version) -> String {
    format!("{} {}.{}.{}", name(f.engine()), v.major, v.minor, v.patch)
}

/// Refused when a server of `f`'s engine at version `v` is older than the driver supports
/// (0.0.0: it says no version the client can read).
pub(crate) fn check(f: &dyn Family, v: Version) -> Result<(), DbError> {
    let (min, name) = (f.min_version(), name(f.engine()));
    if v >= min {
        return Ok(());
    }
    let server = if v == Version::new(0, 0, 0) { format!("{name} (unknown version)") } else { label(f, v) };
    Err(DbError::VersionUnsupported { server, needed: format!("{name} {}.{}", min.major, min.minor) })
}
