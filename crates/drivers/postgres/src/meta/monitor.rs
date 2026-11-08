//! The monitor's statements (PostgreSQL 12 on), next to the catalog's: one statement per read,
//! each giving the columns its decoder in the core names (`monitor::decode::pg::SESSIONS` and
//! the like; the tests here and the integration tests hold them to it). The monitor session
//! runs a tick's reads in one `READ ONLY` transaction (`crate::monitor`), so activity and
//! statistics come from one snapshot.
//!
//! Every statement only reads: the server's activity and statistics views and its settings. None
//! touches a user's table; the only locks taken are the brief `ACCESS SHARE` locks any read of a
//! system catalog takes (the views read `pg_database` and `pg_authid`), which conflict only with
//! an `ACCESS EXCLUSIVE` lock on that catalog, and [`begin`] bounds that wait. None asks the
//! lock manager unless a session waits for a lock (then for at most `MAX_WAITERS` of them).
//! Values come back as text in forms that do not depend on the session's settings: times as
//! microseconds since the Unix epoch on the server's clock, numbers as plain integers.

use datarig_core::monitor::decode::pg::HIDDEN_QUERY;
use datarig_core::monitor::format::{Marker, PG_MARKERS, PgMonitor};
use datarig_core::monitor::{MAX_WAITERS, QUERY_CHARS, SessionsWanted, Want};

/// A time as microseconds since the Unix epoch. `extract` gives an exact numeric from 14 on and
/// a double before, which still rounds to the exact microsecond for any date of this era.
fn us(expr: &str) -> String {
    format!("(extract(epoch FROM {expr}) * 1000000)::int8")
}

/// The probe: version, the account's privileges and the provider markers present, one row.
/// Columns: `version`, `superuser`, `read_all_stats`, `signal_backend`, `markers` (the names
/// found, comma separated).
pub(crate) fn probe() -> String {
    let roles: Vec<String> = PG_MARKERS
        .iter()
        .filter_map(|(m, _)| match m {
            Marker::Role(name) => Some(format!("'{name}'")),
            _ => None,
        })
        .collect();
    let functions: Vec<String> = PG_MARKERS
        .iter()
        .filter_map(|(m, _)| match m {
            Marker::Function(name) => Some(format!("SELECT '{name}' WHERE to_regproc('{name}') IS NOT NULL")),
            _ => None,
        })
        .collect();
    let mut found = format!("SELECT rolname::text FROM pg_roles WHERE rolname IN ({})", roles.join(", "));
    for f in functions {
        found.push_str(" UNION ALL ");
        found.push_str(&f);
    }
    format!(
        "SELECT current_setting('server_version_num') AS version, \
         (SELECT rolsuper FROM pg_roles WHERE rolname = current_user) AS superuser, \
         pg_has_role(current_user, 'pg_read_all_stats', 'USAGE') AS read_all_stats, \
         pg_has_role(current_user, 'pg_signal_backend', 'USAGE') AS signal_backend, \
         array_to_string(ARRAY(SELECT n FROM ({found}) m(n) ORDER BY 1), ',') AS markers"
    )
}

/// What the server is, one row. Columns: `now`, `version`, `started`, `in_recovery`,
/// `max_connections`, `shared_buffers` (bytes), `track_io_timing`.
pub(crate) fn server_info() -> String {
    format!(
        "SELECT {now} AS now, version() AS version, {started} AS started, pg_is_in_recovery() AS in_recovery, \
         current_setting('max_connections')::int8 AS max_connections, \
         pg_size_bytes(current_setting('shared_buffers')) AS shared_buffers, \
         current_setting('track_io_timing') AS track_io_timing",
        now = us("now()"),
        started = us("pg_postmaster_start_time()"),
    )
}

/// The counters and session counts of a tick, one row. Columns: `now`, `commits`, `rollbacks`,
/// `blks_read`, `blks_hit`, `tup_returned`, `tup_written`, `deadlocks`, `temp_bytes`,
/// `databases`, `stats_reset`, `sessions`, `active`, `idle_in_tx`, `idle`, `waiting`, `hidden`.
/// The sums cover every database; the monitor's own session is not counted as active.
pub(crate) fn counters() -> String {
    format!(
        "SELECT {now} AS now, d.*, s.* FROM \
         (SELECT sum(xact_commit)::int8 AS commits, sum(xact_rollback)::int8 AS rollbacks, \
         sum(blks_read)::int8 AS blks_read, sum(blks_hit)::int8 AS blks_hit, \
         sum(tup_returned)::int8 AS tup_returned, \
         sum(tup_inserted + tup_updated + tup_deleted)::int8 AS tup_written, \
         sum(deadlocks)::int8 AS deadlocks, sum(temp_bytes)::int8 AS temp_bytes, \
         count(*) AS databases, max({reset}) AS stats_reset FROM pg_stat_database) d, \
         (SELECT count(*) FILTER (WHERE backend_type = 'client backend') AS sessions, \
         count(*) FILTER (WHERE backend_type = 'client backend' AND state = 'active' AND pid <> pg_backend_pid()) AS active, \
         count(*) FILTER (WHERE state IN ('idle in transaction', 'idle in transaction (aborted)')) AS idle_in_tx, \
         count(*) FILTER (WHERE state = 'idle') AS idle, \
         count(*) FILTER (WHERE wait_event_type = 'Lock') AS waiting, \
         count(*) FILTER (WHERE state IS NULL AND query = '{hidden}') AS hidden \
         FROM pg_stat_activity) s",
        now = us("now()"),
        reset = us("stats_reset"),
        hidden = HIDDEN_QUERY,
    )
}

/// The session list, one row per session (ordered by pid). Columns: `now`, `pid`, `leader_pid`,
/// `usename`, `datname`, `application_name`, `client_addr`, `client_port`, `backend_type`,
/// `state`, `wait_event_type`, `wait_event`, `backend_start`, `xact_start`, `query_start`,
/// `state_change`, `query` (its first [`QUERY_CHARS`] characters), `shortened`, `maybe_cut`
/// (the text may have been cut by the server at `track_activity_query_size`, which cuts on a
/// character boundary, so up to three bytes short of its limit), `monitor` (the reading session
/// itself), `blocked_by` (`pg_blocking_pids`, only for the [`MAX_WAITERS`] longest lock
/// waiters, else NULL).
pub(crate) fn sessions(pg: &PgMonitor, wanted: SessionsWanted) -> String {
    let leader = if pg.leader_pid() { "s.leader_pid" } else { "NULL::int" };
    let idle = if wanted.idle { "" } else { "WHERE a.state IS DISTINCT FROM 'idle' OR a.monitor " };
    format!(
        "WITH a AS (SELECT s.pid, {leader} AS leader_pid, s.usename::text AS usename, s.datname::text AS datname, \
         s.application_name, host(s.client_addr) AS client_addr, s.client_port, s.backend_type, s.state, \
         s.wait_event_type, s.wait_event, {backend_start} AS backend_start, {xact_start} AS xact_start, \
         {query_start} AS query_start, {state_change} AS state_change, \
         left(s.query, {QUERY_CHARS}) AS query, char_length(s.query) > {QUERY_CHARS} AS shortened, \
         octet_length(s.query) >= t.size - 4 AS maybe_cut, s.pid = pg_backend_pid() AS monitor \
         FROM pg_stat_activity s, \
         (SELECT setting::int AS size FROM pg_settings WHERE name = 'track_activity_query_size') t), \
         w AS (SELECT pid FROM a WHERE wait_event_type = 'Lock' ORDER BY state_change NULLS LAST, pid LIMIT {MAX_WAITERS}) \
         SELECT {now} AS now, a.*, \
         CASE WHEN a.pid IN (SELECT pid FROM w) THEN pg_blocking_pids(a.pid)::text END AS blocked_by \
         FROM a {idle}ORDER BY a.pid",
        backend_start = us("s.backend_start"),
        xact_start = us("s.xact_start"),
        query_start = us("s.query_start"),
        state_change = us("s.state_change"),
        now = us("now()"),
    )
}

/// The session `pid`'s statement in whole (as the server keeps it), to tell whether it is still
/// the one a list showed: none or one row. Columns: `pid`, `backend_start`, `state`,
/// `query_start`, `query`, `maybe_cut` (as in [`sessions`]).
pub(crate) fn query_text(pid: i64) -> String {
    format!(
        "SELECT s.pid, {backend_start} AS backend_start, s.state, {query_start} AS query_start, s.query, \
         octet_length(s.query) >= t.size - 4 AS maybe_cut FROM pg_stat_activity s, \
         (SELECT setting::int AS size FROM pg_settings WHERE name = 'track_activity_query_size') t \
         WHERE s.pid = {pid}",
        backend_start = us("s.backend_start"),
        query_start = us("s.query_start"),
    )
}

/// The statements of a tick, in this order: the counters, then the session list and what the
/// server is when `want` asks for them. The driver runs them in one read-only transaction.
pub(crate) fn tick(pg: &PgMonitor, want: &Want) -> Vec<String> {
    let mut statements = vec![counters()];
    statements.extend(want.sessions.map(|wanted| sessions(pg, wanted)));
    if want.server {
        statements.push(server_info());
    }
    statements
}

/// The statement that opens a tick's transaction. Read-only, and `READ COMMITTED NOT DEFERRABLE`
/// whatever the account's defaults: a `SERIALIZABLE DEFERRABLE` default would wait for a safe
/// snapshot. The system catalogs first in the search path, so a function or view of the
/// account's own schemas never stands in for a built-in the statements name. A lock wait of at
/// most a second (the only locks taken are a catalog's `ACCESS SHARE`), each statement under
/// `limit`, no JIT compiling of a short read. Everything it sets ends with the transaction, so
/// nothing stays on a server connection a pooler hands to another client.
pub(crate) fn begin(limit: std::time::Duration) -> String {
    format!(
        "BEGIN ISOLATION LEVEL READ COMMITTED READ ONLY NOT DEFERRABLE; \
         SET LOCAL search_path = pg_catalog, pg_temp; \
         SET LOCAL lock_timeout = '1s'; SET LOCAL statement_timeout = '{}ms'; \
         SELECT pg_catalog.set_config('jit', 'off', true) \
         WHERE pg_catalog.current_setting('server_version_num')::int >= 110000",
        limit.as_millis().max(1)
    )
}

#[cfg(test)]
mod tests;
