//! The monitor's statements (MySQL 8.0.17 on), next to the catalog's: each giving the columns
//! its decoder in the core names (`monitor::decode::mysql::SESSIONS` and the like; the tests
//! here and the integration tests hold them to it). The driver sends one statement per round
//! trip; a tick takes at most three (the clock, the counters, the sessions). Every statement is a
//! read the account may be refused: a refusal for lack of a privilege is an answer (`Readable`),
//! not a failure. None touches a user's table.

use datarig_core::monitor::ActionKind;
use datarig_core::monitor::decode::mysql::{STATUS, other_account_sql};
use datarig_core::monitor::format::{MYSQL_MARKERS, Marker, MySqlMonitor, Readable};
use datarig_core::monitor::locks::LOCK_ROWS;
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

/// The counters of a tick: 1. the clock and the threads by protocol command, one row (`now`,
/// `cmd_query`, `cmd_sleep`, `cmd_daemon`, `cmd_connect`, `cmd_binlog_dump`, `cmd_other`,
/// `others`); 2. the status variables (`STATUS`: `Variable_name`, `Value` rows).
/// `performance_schema.global_status` does not hold the `Com_` counters, so they come from
/// `SHOW GLOBAL STATUS`, which no statement can join with the clock.
///
/// The threads are counted from the session list's own process list ([`sessions`]), so they
/// see what it sees (every thread with `PROCESS`, else the account's own). Each falls in one
/// group only: `Query` and `Execute` (the monitor's own thread left out), `Sleep`, `Daemon`,
/// `Connect` and `Connect Out`, `Binlog Dump` and `Binlog Dump GTID`, any other command.
/// `others` counts the threads of another account than the monitor's, by the core's own rule
/// (`decode::mysql::other_account`, here as `other_account_sql`).
pub(crate) fn counters(my: &MySqlMonitor) -> Vec<String> {
    let names: Vec<String> = STATUS.iter().map(|n| format!("'{n}'")).collect();
    let source = process_list(my);
    vec![
        format!(
            "SELECT {NOW} AS now, \
             COUNT(CASE WHEN {c} IN ('Query', 'Execute') AND p.ID <> CONNECTION_ID() THEN 1 END) AS cmd_query, \
             COUNT(CASE WHEN {c} = 'Sleep' THEN 1 END) AS cmd_sleep, \
             COUNT(CASE WHEN {c} = 'Daemon' THEN 1 END) AS cmd_daemon, \
             COUNT(CASE WHEN {c} IN ('Connect', 'Connect Out') THEN 1 END) AS cmd_connect, \
             COUNT(CASE WHEN {c} IN ('Binlog Dump', 'Binlog Dump GTID') THEN 1 END) AS cmd_binlog_dump, \
             COUNT(CASE WHEN {c} NOT IN ('Query', 'Execute', 'Sleep', 'Daemon', 'Connect', 'Connect Out', \
             'Binlog Dump', 'Binlog Dump GTID') THEN 1 END) AS cmd_other, \
             COUNT(CASE WHEN {others} THEN 1 END) AS others \
             FROM {source} p",
            // A thread without a command is "other", never in no group.
            c = "COALESCE(p.COMMAND, '')",
            others = other_account_sql("p.USER", "CURRENT_USER()"),
        ),
        format!("SHOW GLOBAL STATUS WHERE Variable_name IN ({})", names.join(", ")),
    ]
}

/// The process list the session list and the thread counts read: from 8.0.22 with the
/// performance schema on, `performance_schema.processlist`, which does not take the server's
/// global thread list mutex; before, or with it off, `information_schema.PROCESSLIST`.
fn process_list(my: &MySqlMonitor) -> &'static str {
    if my.ps_processlist() { "performance_schema.processlist" } else { "information_schema.PROCESSLIST" }
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
    let source = process_list(my);
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

/// The row lock waits (InnoDB), one row per waiting request and what blocks it, as the engine
/// names them (`performance_schema.data_lock_waits`, needs `SELECT` on it and on `threads`), at
/// most [`LOCK_ROWS`]. The engine finds these by going through its waiting transactions only;
/// `data_locks` (the request's table and mode) is not read: the engine has no way to find a lock
/// there but going through every lock it holds, which a large transaction makes millions.
/// Columns: `waiter` (the waiting session's id), `blocker` (the session that has the blocking
/// transaction now, NULL when none or not known), `blocking_trx` (the blocking transaction's
/// engine id), `blocking_session_read` (1: the transaction was found and has no session; 0:
/// not found or not readable, so its session is not known).
///
/// A lock keeps the thread that took it, which is not the transaction's session once an XA
/// transaction is prepared and detached (that session goes on with other work): the session
/// is the transaction's own, from `INNODB_TRX` (needs `PROCESS`); without it, only the
/// transaction is named.
pub(crate) fn row_lock_waits(my: &MySqlMonitor) -> String {
    let (blocker, sessionless, join) = match my.innodb_trx == Readable::Yes {
        true => (
            "NULLIF(bx.trx_mysql_thread_id, 0)",
            "bx.trx_id IS NOT NULL AND bx.trx_mysql_thread_id = 0",
            " LEFT JOIN information_schema.INNODB_TRX bx ON bx.trx_id = w.BLOCKING_ENGINE_TRANSACTION_ID",
        ),
        false => ("CAST(NULL AS UNSIGNED)", "0", ""),
    };
    format!(
        "SELECT rt.PROCESSLIST_ID AS waiter, {blocker} AS blocker, \
         w.BLOCKING_ENGINE_TRANSACTION_ID AS blocking_trx, {sessionless} AS blocking_session_read \
         FROM performance_schema.data_lock_waits w \
         LEFT JOIN performance_schema.threads rt ON rt.THREAD_ID = w.REQUESTING_THREAD_ID{join} \
         ORDER BY waiter, blocker LIMIT {LOCK_ROWS}"
    )
}

/// The metadata lock requests waiting (`PENDING`), and the granted locks of other threads on
/// the same objects (`performance_schema.metadata_locks` with `threads`), pending first, at most
/// [`LOCK_ROWS`]. Which granted lock a request waits for is not told: the core pairs them by
/// the server's compatibility of lock types. Columns: `id` (the owner's session id, NULL for a
/// thread with none), `thread` (its thread id), `OBJECT_TYPE`, `OBJECT_SCHEMA`, `OBJECT_NAME`,
/// `LOCK_TYPE`, `LOCK_DURATION`, `LOCK_STATUS`.
pub(crate) fn metadata_lock_waits() -> String {
    format!(
        "WITH m AS (SELECT t.PROCESSLIST_ID AS id, m.OWNER_THREAD_ID AS thread, m.OBJECT_TYPE, m.OBJECT_SCHEMA, \
         m.OBJECT_NAME, m.LOCK_TYPE, m.LOCK_DURATION, m.LOCK_STATUS FROM performance_schema.metadata_locks m \
         LEFT JOIN performance_schema.threads t ON t.THREAD_ID = m.OWNER_THREAD_ID \
         WHERE m.LOCK_STATUS IN ('PENDING', 'GRANTED')) \
         SELECT * FROM m WHERE m.LOCK_STATUS = 'PENDING' UNION ALL \
         SELECT g.* FROM m g WHERE g.LOCK_STATUS = 'GRANTED' AND EXISTS (SELECT 1 FROM m p \
         WHERE p.LOCK_STATUS = 'PENDING' AND p.OBJECT_TYPE = g.OBJECT_TYPE AND p.OBJECT_SCHEMA <=> g.OBJECT_SCHEMA \
         AND p.OBJECT_NAME <=> g.OBJECT_NAME AND p.thread <> g.thread) \
         ORDER BY LOCK_STATUS DESC, OBJECT_TYPE, OBJECT_SCHEMA, OBJECT_NAME, thread LIMIT {LOCK_ROWS}"
    )
}

/// The lock reads of a tick when the Locks view asks: the row lock waits, then the metadata
/// lock waits when the account may read them with the instrument on (else what would show them
/// is said instead). The driver sends them only when a session may be waiting for a lock
/// (`decode::mysql::may_wait`).
///
/// Before MySQL 8.0.38 the row lock waits are not read (`MySqlMonitor::lock_waits`): the
/// statement list starts with the metadata lock waits then.
pub(crate) fn locks(my: &MySqlMonitor) -> Vec<String> {
    let mut statements = Vec::new();
    if my.lock_waits() {
        statements.push(row_lock_waits(my));
    }
    if my.metadata_locks == Readable::Yes {
        statements.push(metadata_lock_waits());
    }
    statements
}

/// The statements of a tick, in this order: the counters ([`counters`], the clock first), then
/// the session list and what the server is when `want` asks for them. Each runs on its own in
/// autocommit; the counters come first, so the monitor's own statements counted when they were
/// read are exactly those sent until then. The lock reads ([`locks`]) come after the session
/// list, sent by the driver only when one may wait.
pub(crate) fn tick(my: &MySqlMonitor, want: &Want) -> Vec<String> {
    let mut statements = counters(my);
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

pub(crate) mod replication;

#[cfg(test)]
mod tests;
