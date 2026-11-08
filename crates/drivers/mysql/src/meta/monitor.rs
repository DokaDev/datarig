//! The monitor's statements (MySQL 8.0.17 on), next to the catalog's: each giving the columns
//! its decoder in the core names (`monitor::decode::mysql::SESSIONS` and the like; the tests
//! here and the integration tests hold them to it). The driver sends one statement per round
//! trip; a tick takes at most three (the clock, the counters, the sessions). Every statement is a
//! read the account may be refused: a refusal for lack of a privilege is an answer (`Readable`),
//! not a failure. None touches a user's table.

use datarig_core::monitor::ActionKind;
use datarig_core::monitor::decode::mysql::STATUS;
use datarig_core::monitor::format::{MYSQL_MARKERS, Marker, MySqlMonitor, Readable};
use datarig_core::monitor::{QUERY_CHARS, SessionsWanted, Want};

/// The server's clock as microseconds since the Unix epoch.
const NOW: &str = "CAST(UNIX_TIMESTAMP(NOW(6)) * 1000000 AS SIGNED)";

/// The probe's statements, in this order (each answered or refused on its own):
/// 1. `version`, `performance_schema`, `user` (`CURRENT_USER()`), one row;
/// 2. the provider markers' variables present and `basedir`: `Variable_name`, `Value` rows;
/// 3. the account's direct grant of `PROCESS` (what shows other accounts' sessions; `SUPER`
///    does not): `PRIVILEGE_TYPE` rows;
/// 4. one row of `information_schema.INNODB_TRX`, to learn whether it is readable (an
///    `information_schema` table checks the privilege only when it is filled, so the probe
///    really reads it);
/// 5. whether the metadata lock instrument is on (`ENABLED`), from a statement that also names
///    `performance_schema.metadata_locks` and `threads`, so it is refused without `SELECT` on
///    them (a `performance_schema` table checks the privilege when it is opened);
/// 6. whether the statement event consumers are all on (`ENABLED`: `events_statements_current`
///    and the two above it, `NO` when one is off), from a statement that also names
///    `performance_schema.events_statements_current` and `threads`, refused without `SELECT`
///    on them.
pub(crate) fn probe() -> Vec<String> {
    let mut variables: Vec<String> = MYSQL_MARKERS
        .iter()
        .filter_map(|(m, _)| match m {
            Marker::Variable(name) => Some(format!("'{name}'")),
            _ => None,
        })
        .collect();
    variables.push("'basedir'".into());
    vec![
        "SELECT VERSION() AS version, @@performance_schema AS performance_schema, CURRENT_USER() AS user".into(),
        format!("SHOW GLOBAL VARIABLES WHERE Variable_name IN ({})", variables.join(", ")),
        "SELECT PRIVILEGE_TYPE FROM information_schema.USER_PRIVILEGES \
         WHERE GRANTEE = CONCAT(QUOTE(SUBSTRING_INDEX(CURRENT_USER(), '@', 1)), '@', QUOTE(SUBSTRING_INDEX(CURRENT_USER(), '@', -1))) \
         AND PRIVILEGE_TYPE = 'PROCESS'"
            .into(),
        "SELECT trx_id FROM information_schema.INNODB_TRX LIMIT 1".into(),
        "SELECT i.ENABLED, (SELECT COUNT(*) FROM performance_schema.metadata_locks m \
         JOIN performance_schema.threads t ON t.THREAD_ID = m.OWNER_THREAD_ID WHERE 1 = 0) AS probe \
         FROM performance_schema.setup_instruments i WHERE i.NAME = 'wait/lock/metadata/sql/mdl'"
            .into(),
        "SELECT MIN(c.ENABLED) AS ENABLED, (SELECT COUNT(*) FROM performance_schema.events_statements_current e \
         JOIN performance_schema.threads t ON t.THREAD_ID = e.THREAD_ID WHERE 1 = 0) AS probe \
         FROM performance_schema.setup_consumers c \
         WHERE c.NAME IN ('global_instrumentation', 'thread_instrumentation', 'events_statements_current')"
            .into(),
    ]
}

/// What the server is: 1. one row of `now`, `version`, `version_comment`, `read_only`,
/// `super_read_only`, `server_id`, `gtid_mode`, `log_bin`, `max_connections`,
/// `innodb_buffer_pool_size`, `performance_schema`; 2. `Uptime` (`Variable_name`, `Value`).
pub(crate) fn server_info() -> Vec<String> {
    vec![
        format!(
            "SELECT {NOW} AS now, VERSION() AS version, @@version_comment AS version_comment, \
             @@read_only AS read_only, @@super_read_only AS super_read_only, @@server_id AS server_id, \
             @@gtid_mode AS gtid_mode, @@log_bin AS log_bin, @@max_connections AS max_connections, \
             @@innodb_buffer_pool_size AS innodb_buffer_pool_size, @@performance_schema AS performance_schema"
        ),
        "SHOW GLOBAL STATUS WHERE Variable_name = 'Uptime'".into(),
    ]
}

/// The counters of a tick: 1. the clock, one row (`now`); 2. the status variables
/// (`STATUS`: `Variable_name`, `Value` rows). `performance_schema.global_status` does not
/// hold the `Com_` counters, so they come from `SHOW GLOBAL STATUS`, which no statement can
/// join with the clock.
pub(crate) fn counters() -> Vec<String> {
    let names: Vec<String> = STATUS.iter().map(|n| format!("'{n}'")).collect();
    vec![format!("SELECT {NOW} AS now"), format!("SHOW GLOBAL STATUS WHERE Variable_name IN ({})", names.join(", "))]
}

/// The session list, one statement, one row per session (ordered by id). Columns: `now`,
/// `ID`, `USER`, `HOST`, `DB`, `COMMAND`, `TIME`, `STATE`, `INFO` (its first [`QUERY_CHARS`]
/// characters), `shortened`, `monitor` (the reading session itself); from `INNODB_TRX` when the
/// account may read it (else NULL): `trx_started`, `trx_state`, `trx_rows_locked`,
/// `trx_rows_modified`, then `trx_read` (1 when it was joined); from
/// `performance_schema.metadata_locks` when readable (else NULL): `in_tx` (it holds a lock until
/// its transaction ends), `explicit_locks` (it holds one past its statements outside a
/// transaction: `LOCK TABLES`, `GET_LOCK`), then `locks_read`; from `performance_schema.threads`
/// and `events_statements_current` when readable with their consumers on (else NULL):
/// `THREAD_ID`, and `EVENT_ID` of the running top-level statement (no `END_EVENT_ID`, nesting
/// level 0) of an instrumented thread (else NULL, as a finished event stays in the table);
/// last `info_md5`, the server's MD5 of the whole `INFO`.
///
/// From 8.0.22 with the performance schema on, the list comes from
/// `performance_schema.processlist`, which does not take the server's global thread list mutex;
/// before, or with it off, from `information_schema.PROCESSLIST`. Both list every session with
/// `PROCESS`, else only the account's own.
///
/// `INNODB_TRX.trx_started` is the server's system time (whatever the session's time zone), so
/// the monitor session works in the system time zone and both convert to the epoch alike.
pub(crate) fn sessions(my: &MySqlMonitor, wanted: SessionsWanted) -> Vec<String> {
    let trx = my.innodb_trx == Readable::Yes;
    let locks = my.metadata_locks == Readable::Yes;
    // An idle session is left out only when it is known to hold no transaction and no lock.
    let idle = if wanted.idle || !trx || !locks {
        String::new()
    } else {
        " WHERE p.COMMAND <> 'Sleep' OR t.trx_id IS NOT NULL OR l.id IS NOT NULL OR p.ID = CONNECTION_ID()".into()
    };
    vec![session_rows(my, trx, locks, &idle)]
}

/// The rows of [`sessions`] for the one session of `id`, and the reading session's own (so
/// there is always a row, which the decoder needs; the caller picks the one of `id`): what a
/// read after an action shows of it.
pub(crate) fn session(my: &MySqlMonitor, id: u64) -> String {
    let only = format!(" WHERE p.ID IN ({id}, CONNECTION_ID())");
    session_rows(my, my.innodb_trx == Readable::Yes, my.metadata_locks == Readable::Yes, &only)
}

/// The read right before an action on the session of `id`: the columns of [`sessions`] for it
/// and the reading session's own, its statement told apart as the tick tells it (the running
/// top-level statement event where the events are read, else the command, the MD5 of the whole
/// text and its time, from the same list as the tick's), without the transaction and lock
/// columns (NULL), which the check does not need and which would cost it a read of every
/// session's locks.
pub(crate) fn guard(my: &MySqlMonitor, id: u64) -> String {
    session_rows(my, false, false, &format!(" WHERE p.ID IN ({id}, CONNECTION_ID())"))
}

fn session_rows(my: &MySqlMonitor, trx: bool, locks: bool, filter: &str) -> String {
    let source = if my.ps_processlist() { "performance_schema.processlist" } else { "information_schema.PROCESSLIST" };
    let mut columns = Vec::new();
    let mut joins = String::new();
    if trx {
        columns.push(
            "CAST(UNIX_TIMESTAMP(t.trx_started) * 1000000 AS SIGNED) AS trx_started, t.trx_state, \
             t.trx_rows_locked, t.trx_rows_modified, 1 AS trx_read",
        );
        joins.push_str(" LEFT JOIN information_schema.INNODB_TRX t ON t.trx_mysql_thread_id = p.ID");
    } else {
        columns.push(
            "NULL AS trx_started, NULL AS trx_state, NULL AS trx_rows_locked, NULL AS trx_rows_modified, 0 AS trx_read",
        );
    }
    if locks {
        columns.push("COALESCE(l.in_tx, 0) AS in_tx, COALESCE(l.explicit_locks, 0) AS explicit_locks, 1 AS locks_read");
        joins.push_str(
            " LEFT JOIN (SELECT th.PROCESSLIST_ID AS id, MAX(m.LOCK_DURATION = 'TRANSACTION') AS in_tx, \
             MAX(m.LOCK_DURATION = 'EXPLICIT') AS explicit_locks FROM performance_schema.metadata_locks m \
             JOIN performance_schema.threads th ON th.THREAD_ID = m.OWNER_THREAD_ID \
             WHERE m.LOCK_STATUS = 'GRANTED' AND m.LOCK_DURATION <> 'STATEMENT' GROUP BY th.PROCESSLIST_ID) l \
             ON l.id = p.ID",
        );
    } else {
        columns.push("NULL AS in_tx, NULL AS explicit_locks, 0 AS locks_read");
    }
    if my.performance_schema && my.statements == Readable::Yes {
        columns.push("pt.THREAD_ID, IF(pt.INSTRUMENTED = 'YES', e.EVENT_ID, NULL) AS EVENT_ID");
        joins.push_str(
            " LEFT JOIN performance_schema.threads pt ON pt.PROCESSLIST_ID = p.ID \
             LEFT JOIN performance_schema.events_statements_current e ON e.THREAD_ID = pt.THREAD_ID \
             AND e.NESTING_EVENT_LEVEL = 0 AND e.END_EVENT_ID IS NULL",
        );
    } else {
        columns.push("NULL AS THREAD_ID, NULL AS EVENT_ID");
    }
    columns.push("MD5(p.INFO) AS info_md5");
    format!(
        "SELECT {NOW} AS now, p.ID, p.USER, p.HOST, p.DB, p.COMMAND, p.TIME, p.STATE, \
         LEFT(p.INFO, {QUERY_CHARS}) AS INFO, CHAR_LENGTH(p.INFO) > {QUERY_CHARS} AS shortened, \
         p.ID = CONNECTION_ID() AS monitor, {} FROM {source} p{joins}{filter} ORDER BY p.ID",
        columns.join(", ")
    )
}

/// The statement that does `kind` to the session of `id` (an integer: nothing typed by a user
/// goes in it).
pub(crate) fn kill(kind: ActionKind, id: u64) -> String {
    match kind {
        ActionKind::Cancel => format!("KILL QUERY {id}"),
        ActionKind::Terminate => format!("KILL {id}"),
    }
}

/// The call of a provider's procedure (`procedure`, one of the core's route table) on the
/// session of `id`.
pub(crate) fn call(procedure: &str, id: u64) -> String {
    format!("CALL {procedure}({id})")
}

/// Whether the statement event `event_id` of thread `thread_id` ended with an error, and which:
/// at most one row, column `MYSQL_ERRNO` (0: none). The history keeps a thread's last few
/// statements only, so no row says nothing.
pub(crate) fn statement_errno(thread_id: u64, event_id: u64) -> String {
    format!(
        "SELECT MYSQL_ERRNO FROM performance_schema.events_statements_history \
         WHERE THREAD_ID = {thread_id} AND EVENT_ID = {event_id}"
    )
}

/// The session `id`'s statement in whole, to tell whether it is still the one the list showed:
/// none or one row. Columns: `now`, `ID`, `COMMAND`, `TIME`, `INFO`, `maybe_cut`.
///
/// Always from `information_schema.PROCESSLIST`: `performance_schema.processlist` keeps only the
/// first kilobyte of a statement. This one keeps its first [`INFO_BYTES`] bytes (`maybe_cut`:
/// within three bytes of that, so perhaps cut). It takes the server's thread list mutex while it
/// reads, which a single read on demand may.
pub(crate) fn query_text(id: i64) -> String {
    format!(
        "SELECT {NOW} AS now, p.ID, p.COMMAND, p.TIME, p.INFO, OCTET_LENGTH(p.INFO) >= {} AS maybe_cut \
         FROM information_schema.PROCESSLIST p WHERE p.ID = {id}",
        INFO_BYTES - 3
    )
}

/// How much of a statement `information_schema.PROCESSLIST` keeps, in bytes.
const INFO_BYTES: usize = 65535;

/// The statements of a tick, in this order: the counters ([`counters`], the clock first), then
/// the session list and what the server is when `want` asks for them. Each runs on its own in
/// autocommit; the counters come first, so the monitor's own statements counted when they were
/// read are exactly those sent until then.
pub(crate) fn tick(my: &MySqlMonitor, want: &Want) -> Vec<String> {
    let mut statements = counters();
    if let Some(wanted) = want.sessions {
        statements.extend(sessions(my, wanted));
    }
    if want.server {
        statements.extend(server_info());
    }
    statements
}

/// How long a `SELECT` of the monitor session may run (`max_execution_time` does not cover
/// `SHOW`; the driver's deadline does), in milliseconds, at least one (0 means no limit).
pub(crate) fn limit(limit: std::time::Duration) -> String {
    format!("SET SESSION max_execution_time = {}", limit.as_millis().clamp(1, u128::from(u32::MAX)))
}

#[cfg(test)]
mod tests;
