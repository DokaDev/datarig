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
//! lock manager unless a session waits for a lock (then for at most `MAX_WAITERS` of them, and
//! for the lock table once only when the Locks view asks: [`locks`]).
//! Values come back as text in forms that do not depend on the session's settings: times as
//! microseconds since the Unix epoch on the server's clock, numbers as plain integers.

use datarig_core::monitor::action::{ActionKind, ActionTarget, StatementStamp};
use datarig_core::monitor::decode::pg::HIDDEN_QUERY;
use datarig_core::monitor::format::{Marker, PG_MARKERS, PgMonitor, Route};
use datarig_core::monitor::locks::{LOCK_ROWS, LOCK_WAITS};
use datarig_core::monitor::{MAX_WAITERS, QUERY_CHARS, SessionsWanted, Timestamp, Want};

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
/// waiters, else NULL). Without idle sessions, an idle session that blocks one of those waiters
/// (a session-level advisory lock) is listed all the same.
pub(crate) fn sessions(pg: &PgMonitor, wanted: SessionsWanted) -> String {
    let idle = if wanted.idle {
        ""
    } else {
        "WHERE a.state IS DISTINCT FROM 'idle' OR a.monitor OR a.pid IN (SELECT unnest(b.blockers) FROM b) "
    };
    session_rows(pg, idle, true)
}

/// The rows of [`sessions`] for the one session of `pid`, and the reading session's own (so
/// there is always a row, which the decoder needs; the caller picks the one of `pid`).
pub(crate) fn session(pg: &PgMonitor, pid: i64) -> String {
    session_rows(pg, &format!("WHERE a.pid = {pid} OR a.monitor "), false)
}

/// `all`: the blockers of every waiter asked for are joined in (the list, whose filter may name
/// them); else only those of the rows `filter` keeps are asked.
fn session_rows(pg: &PgMonitor, filter: &str, all: bool) -> String {
    let leader = if pg.leader_pid() { "s.leader_pid" } else { "NULL::int" };
    let blocked = match all {
        true => format!(
            ", b AS (SELECT pid, pg_blocking_pids(pid) AS blockers FROM w) \
             SELECT {now} AS now, a.*, b.blockers::text AS blocked_by FROM a LEFT JOIN b ON b.pid = a.pid ",
            now = us("now()")
        ),
        false => format!(
            " SELECT {now} AS now, a.*, \
             CASE WHEN a.pid IN (SELECT pid FROM w) THEN pg_blocking_pids(a.pid)::text END AS blocked_by FROM a ",
            now = us("now()")
        ),
    };
    format!(
        "WITH a AS (SELECT s.pid, {leader} AS leader_pid, s.usename::text AS usename, s.datname::text AS datname, \
         s.application_name, host(s.client_addr) AS client_addr, s.client_port, s.backend_type, s.state, \
         s.wait_event_type, s.wait_event, {backend_start} AS backend_start, {xact_start} AS xact_start, \
         {query_start} AS query_start, {state_change} AS state_change, \
         left(s.query, {QUERY_CHARS}) AS query, char_length(s.query) > {QUERY_CHARS} AS shortened, \
         octet_length(s.query) >= t.size - 4 AS maybe_cut, s.pid = pg_backend_pid() AS monitor \
         FROM pg_stat_activity s, \
         (SELECT setting::int AS size FROM pg_settings WHERE name = 'track_activity_query_size') t), \
         w AS (SELECT pid FROM a WHERE wait_event_type = 'Lock' ORDER BY state_change NULLS LAST, pid LIMIT {MAX_WAITERS})\
         {blocked}{filter}ORDER BY a.pid",
        backend_start = us("s.backend_start"),
        xact_start = us("s.xact_start"),
        query_start = us("s.query_start"),
        state_change = us("s.state_change"),
    )
}

/// The lock waits and the locks held on what they wait for, from `pg_locks`, read once (the
/// lock manager gives all its locks at each read, under all its partition locks, whatever the
/// statement keeps of them): nothing at all while no session the account may see waits for a
/// lock (without `pg_read_all_stats`, other roles' waits are hidden from it, and so from the
/// tree, which is drawn from the session list's blockers). Rows: `role`
/// `wait` (a session's request not granted, the [`LOCK_WAITS`] longest first where the server
/// tells since when) or `hold` (a granted lock on the same object), at most [`LOCK_ROWS`],
/// waits first. Columns: `role`, `key` (the object, the same text for the same object),
/// `pid` (NULL: a prepared transaction's), `backend_start`, `gid`, `owner`, `prepared`,
/// `prepared_database` (a prepared transaction's, told by its transaction id lock: its locks
/// keep the session's virtual transaction id that prepared it), `locktype`, `mode`, `database`
/// (the object's, when another than the monitor's), `relation` (by name, in the monitor's
/// database or a shared catalog), `detail` (a transaction id, a tuple's `(page,tuple)`, a page,
/// an advisory lock's or object's ids), `since` (when the wait started, 14 on), `query_start`
/// (the waiter's statement's start), `waits` (the requests not granted in all).
pub(crate) fn locks(pg: &PgMonitor) -> String {
    let waitstart = if pg.waitstart() { "l.waitstart" } else { "NULL::timestamptz" };
    format!(
        "WITH l AS MATERIALIZED (SELECT l.locktype, l.database, l.relation, l.page, l.tuple, l.virtualxid, \
         l.transactionid, l.classid, l.objid, l.objsubid, l.virtualtransaction, l.pid, l.mode, l.granted, \
         {waitstart} AS waitstart, \
         (l.locktype, l.database, l.relation, l.page, l.tuple, l.virtualxid, l.transactionid, l.classid, \
         l.objid, l.objsubid)::text AS key \
         FROM pg_locks l WHERE EXISTS (SELECT 1 FROM pg_stat_activity WHERE wait_event_type = 'Lock')), \
         w AS (SELECT * FROM l WHERE NOT l.granted AND l.pid IS NOT NULL \
         ORDER BY l.waitstart NULLS LAST, l.pid LIMIT {LOCK_WAITS}), \
         r AS (SELECT 'wait' AS role, w.* FROM w UNION ALL \
         SELECT 'hold', l.* FROM l WHERE l.granted AND l.key IN (SELECT key FROM w)), \
         p AS (SELECT x.gid, x.owner::text AS owner, x.database::text AS database, x.prepared, t.virtualtransaction \
         FROM pg_prepared_xacts x JOIN l t ON t.locktype = 'transactionid' AND t.transactionid = x.transaction \
         AND t.pid IS NULL), \
         d AS (SELECT oid FROM pg_database WHERE datname = current_database()) \
         SELECT r.role, r.key, r.pid, {backend_start} AS backend_start, p.gid, p.owner, {prepared} AS prepared, \
         p.database AS prepared_database, r.locktype, r.mode, \
         CASE WHEN r.database NOT IN (0, (SELECT oid FROM d)) \
         THEN (SELECT datname::text FROM pg_database WHERE oid = r.database) END AS database, \
         CASE WHEN r.relation IS NOT NULL AND r.database IN (0, (SELECT oid FROM d)) \
         THEN r.relation::regclass::text END AS relation, \
         CASE r.locktype WHEN 'transactionid' THEN r.transactionid::text WHEN 'virtualxid' THEN r.virtualxid \
         WHEN 'tuple' THEN '(' || r.page || ',' || r.tuple || ')' WHEN 'page' THEN r.page::text \
         WHEN 'advisory' THEN r.classid || ',' || r.objid || ',' || r.objsubid \
         WHEN 'object' THEN r.classid || ',' || r.objid || ',' || r.objsubid END AS detail, \
         {since} AS since, {query_start} AS query_start, (SELECT count(*) FROM l WHERE NOT l.granted) AS waits \
         FROM r LEFT JOIN pg_stat_activity a ON a.pid = r.pid \
         LEFT JOIN p ON r.pid IS NULL AND p.virtualtransaction = r.virtualtransaction \
         ORDER BY r.role DESC, r.key, r.pid NULLS FIRST LIMIT {LOCK_ROWS}",
        backend_start = us("a.backend_start"),
        prepared = us("p.prepared"),
        since = us("r.waitstart"),
        query_start = us("a.query_start"),
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

/// The statements of a tick, in this order: the counters, then the session list, the locks and
/// what the server is when `want` asks for them. The driver runs them in one read-only
/// transaction.
pub(crate) fn tick(pg: &PgMonitor, want: &Want) -> Vec<String> {
    let mut statements = vec![counters()];
    statements.extend(want.sessions.map(|wanted| sessions(pg, wanted)));
    if want.locks {
        statements.push(locks(pg));
    }
    if want.server {
        statements.push(server_info());
    }
    statements
}

/// The statement of an action of `kind` on `target`, run in a transaction of its own
/// ([`begin`]): it reads the target's pid and sends the signal in the same statement, only when
/// the session is still the one the row showed (its `backend_start`, to the microsecond as the
/// tick read it), its last statement is still the one the row showed (its `query_start`: the
/// session may be a pooler's server connection, which serves another client once its
/// statement ends, whatever way the monitor itself connects) and it is not the reading
/// session; a cancel also when that statement still runs, and so does an end where the
/// monitor goes through a pooler or its route is not known. At most one row (none: no session
/// of that pid). Never without a statement start then and now: where the server does not track
/// the session's activity (`track_activities` off) its `query_start` is NULL whatever it runs,
/// so a NULL proves nothing. Columns: `own` (the reading session), `session` (the same
/// session), `tracked` (a start then, its activity tracked now), `statement` (the same statement, and running where that is needed), `sent` (what
/// the signal function answered, NULL when it was not called). Every value in it is an integer
/// the tick read; nothing typed by a user.
pub(crate) fn action(pg: &PgMonitor, kind: ActionKind, target: &ActionTarget) -> String {
    let started = match target.key.started {
        Some(Timestamp(t)) => t.to_string(),
        None => "NULL::int8".into(),
    };
    let running = kind == ActionKind::Cancel || pg.route != Route::Direct;
    let tracked = match &target.stamp {
        Some(StatementStamp::Pg { query_start: Some(_) }) => "COALESCE(a.state <> 'disabled', false)",
        _ => "false",
    };
    let statement = match &target.stamp {
        Some(StatementStamp::Pg { query_start }) => {
            let at = query_start.map_or_else(|| "NULL::int8".into(), |Timestamp(t)| t.to_string());
            let still = match running {
                true => " AND a.state NOT IN ('idle', 'idle in transaction', 'idle in transaction (aborted)')",
                false => "",
            };
            format!(
                "COALESCE({query_start} IS NOT DISTINCT FROM {at}{still}, false)",
                query_start = us("a.query_start")
            )
        }
        _ => "false".into(),
    };
    let function = match kind {
        ActionKind::Cancel => "pg_cancel_backend",
        ActionKind::Terminate => "pg_terminate_backend",
    };
    format!(
        "SELECT g.own, g.session, g.tracked, g.statement, \
         CASE WHEN NOT g.own AND g.session AND g.tracked AND g.statement THEN {function}(g.pid) END AS sent \
         FROM (SELECT a.pid, a.pid = pg_backend_pid() AS own, \
         COALESCE({backend_start} = {started}, false) AS session, \
         {tracked} AS tracked, {statement} AS statement \
         FROM pg_stat_activity a WHERE a.pid = {pid}) g",
        backend_start = us("a.backend_start"),
        pid = target.key.id,
    )
}

/// The reading session's own server process id, which the driver compares with the one the
/// server sent at start-up to tell whether the connection reaches the server directly. One
/// row, column `pid`.
pub(crate) fn backend_pid() -> String {
    "SELECT pg_backend_pid() AS pid".into()
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
