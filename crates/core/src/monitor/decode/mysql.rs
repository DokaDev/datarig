//! MySQL rows read into the model (the statements are the driver's: its `meta::monitor`).

pub mod replication;

use super::{Answer, DecodeError, Row, Rows, malformed, name_values, one};
use crate::monitor::action::StatementStamp;
use crate::monitor::format::{self, MYSQL_MARKERS, MYSQL_MIN_VERSION, MySqlMonitor, Probe, Readable};
use crate::monitor::locks::{self as lk, Blame, Holder, LockHolder, LockList, LockObject, LockWait};
use crate::monitor::model::{
    Counter, CounterKind, Epoch, Extra, ExtraKey, FullText, Gauge, GaugeKind, Hidden, Need, Own, Part, Proof,
    QueryText, Sample, ServerInfo, SessionKey, SessionKind, SessionList, SessionRow, SessionState, Timestamp, Tx,
    Visibility, Wait, WaitClass,
};
use crate::monitor::model::{SessionsWanted, Snapshot, TEXT_MIN_AGE_US, TEXT_SAME_START_US, TextAsk, Want};

/// The status variables a tick reads (`SHOW GLOBAL STATUS`), by name: what [`counters`] knows.
pub const STATUS: &[&str] = &[
    "Uptime",
    "Questions",
    "Com_commit",
    "Com_rollback",
    "Threads_connected",
    "Threads_running",
    "Innodb_buffer_pool_read_requests",
    "Innodb_buffer_pool_reads",
    "Innodb_rows_read",
    "Innodb_rows_inserted",
    "Innodb_rows_updated",
    "Innodb_rows_deleted",
    "Bytes_received",
    "Bytes_sent",
    "Innodb_row_lock_waits",
    "Slow_queries",
    "Created_tmp_disk_tables",
    "Aborted_connects",
    "Performance_schema_thread_instances_lost",
    "Performance_schema_metadata_lock_lost",
    "Com_select",
    "Com_insert",
    "Com_insert_select",
    "Com_replace",
    "Com_replace_select",
    "Com_update",
    "Com_update_multi",
    "Com_delete",
    "Com_delete_multi",
];

/// The columns of the probe's first row: the version line, `@@performance_schema`, `CURRENT_USER()`, in order.
pub const PROBE: &[&str] = &["version", "performance_schema", "user"];

/// The columns of the probe's last row: whether the metadata lock instrument is on, and a count that only names the lock tables, in order.
pub const PROBE_LOCKS: &[&str] = &["ENABLED", "probe"];

/// The columns of the probe's statement of the statement events: whether their consumers are all on, and a count that only names the event and thread tables, in order.
pub const PROBE_STATEMENTS: &[&str] = &["ENABLED", "probe"];

/// The columns of `SHOW GLOBAL STATUS` and `SHOW GLOBAL VARIABLES`, in order.
pub const NAME_VALUE: &[&str] = &["Variable_name", "Value"];

/// The columns of the server's row, in order.
pub const SERVER_INFO: &[&str] = &[
    "now",
    "version",
    "version_comment",
    "read_only",
    "super_read_only",
    "server_id",
    "gtid_mode",
    "log_bin",
    "max_connections",
    "innodb_buffer_pool_size",
    "performance_schema",
];

/// The columns of the clock's row (`now` in microseconds since the Unix epoch; then the threads of the session list's process list by command group, `cmd_query` without the monitor's own thread; `others` the threads of another account than the monitor's), in order.
pub const CLOCK: &[&str] =
    &["now", "cmd_query", "cmd_sleep", "cmd_daemon", "cmd_connect", "cmd_binlog_dump", "cmd_other", "others"];

/// The columns of a session's row (`INFO` its first `QUERY_CHARS` characters; the `trx_` columns NULL and `trx_read` 0 without `INNODB_TRX`, `in_tx` and `explicit_locks` NULL and `locks_read` 0 without the metadata lock table; `THREAD_ID` and `EVENT_ID` NULL without the statement events, `EVENT_ID` the running top-level statement's on an instrumented thread, else NULL; `info_md5` the server's MD5 of the whole `INFO`), in order.
pub const SESSIONS: &[&str] = &[
    "now",
    "ID",
    "USER",
    "HOST",
    "DB",
    "COMMAND",
    "TIME",
    "STATE",
    "INFO",
    "shortened",
    "monitor",
    "trx_started",
    "trx_state",
    "trx_rows_locked",
    "trx_rows_modified",
    "trx_read",
    "in_tx",
    "explicit_locks",
    "locks_read",
    "THREAD_ID",
    "EVENT_ID",
    "info_md5",
];

/// The columns of a session's row read for its statement's whole text (`now` in microseconds since the Unix epoch, `TIME` in seconds, `maybe_cut` the server may have kept only the start of `INFO`), in order.
pub const QUERY_TEXT: &[&str] = &["now", "ID", "COMMAND", "TIME", "INFO", "maybe_cut"];

/// The columns of a row lock wait's row (`waiter` and `blocker` session ids, `blocker` NULL when no session is known to have the blocking transaction, `blocking_trx` its engine transaction id, `blocking_session_read` 1 when the transaction was found and has no session), in order.
pub const ROW_LOCK_WAITS: &[&str] = &["waiter", "blocker", "blocking_trx", "blocking_session_read"];

/// The columns of a metadata lock's row (`id` the owner's session id, NULL for a thread without one; `thread` its thread id), in order.
pub const METADATA_LOCKS: &[&str] =
    &["id", "thread", "OBJECT_TYPE", "OBJECT_SCHEMA", "OBJECT_NAME", "LOCK_TYPE", "LOCK_DURATION", "LOCK_STATUS"];

/// The server's error codes that refuse a read for lack of a privilege: 1044 (database), 1142
/// (table), 1143 (column), 1227 (a privilege such as `PROCESS`).
const REFUSED: &[u32] = &[1044, 1142, 1143, 1227];

/// The server refused a read with error `code` for lack of a privilege.
pub fn refused(code: u32) -> bool {
    REFUSED.contains(&code)
}

fn readable(answer: &Answer) -> Readable {
    match answer {
        Answer::Rows(_) => Readable::Yes,
        Answer::Error { code: Some(code), .. } if REFUSED.contains(code) => Readable::Refused,
        Answer::Error { .. } => Readable::Unknown,
    }
}

/// `8.4.11`, `8.0.35-0ubuntu0.22.04.1`, `8.0.28-log` as `major * 10000 + minor * 100 + patch`.
fn version_number(version: &str) -> Option<u32> {
    let mut parts = version.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    let patch = parts.next()?;
    let digits = patch.find(|c: char| !c.is_ascii_digit()).unwrap_or(patch.len());
    let patch: u32 = patch[..digits].parse().ok()?;
    (minor < 100 && patch < 100).then_some(major * 10000 + minor * 100 + patch)
}

/// The probe's answers, in their order: the version line ([`PROBE`]), the provider variables
/// (name and value), the account's `PROCESS` grant, one row of `INNODB_TRX`, the metadata lock
/// instrument ([`PROBE_LOCKS`]), the statement event consumers ([`PROBE_STATEMENTS`]).
pub fn probe(answers: &[Answer]) -> Result<MySqlMonitor, DecodeError> {
    let [first, variables, grants, trx, locks, statements] = answers else {
        return Err(malformed("probe", format!("{} answers, expected 6", answers.len())));
    };
    let rows = match first {
        Answer::Rows(rows) => rows,
        Answer::Error { message, .. } => return Err(DecodeError::ProbeFailed(message.clone())),
    };
    let row = one(rows, "probe", PROBE)?;
    let line = row.required("version")?;
    let version = match version_number(line) {
        Some(v) if v >= MYSQL_MIN_VERSION && !line.contains("MariaDB") => v,
        _ => return Err(DecodeError::Unsupported(format!("MySQL {line}"))),
    };
    let current = row.required("user")?;
    let user = current.rsplit_once('@').map_or(current, |(user, _)| user).to_owned();
    // Unreadable variables leave the flavour standard: no marker was seen.
    let mut found = Probe::default();
    if let Answer::Rows(rows) = variables {
        for (name, value) in name_values(rows, "probe variables")? {
            if name.eq_ignore_ascii_case("basedir") {
                found.basedir = value.map(str::to_owned);
            } else {
                found.names.push(name.to_ascii_lowercase());
            }
        }
    }
    // Only a direct grant shows here; none found (or the list refused) cannot tell.
    let process = match grants {
        Answer::Rows(rows) if rows.iter().any(|r| r.first().is_some_and(|c| c.as_deref() == Some("PROCESS"))) => {
            Readable::Yes
        }
        _ => Readable::Unknown,
    };
    Ok(MySqlMonitor {
        version,
        performance_schema: row.required_bool("performance_schema")?,
        user,
        process,
        innodb_trx: readable(trx),
        // Readable, and the instrument on: else the table is empty, which is not "no locks".
        metadata_locks: enabled(locks),
        // Readable, and the consumers on: else no statement is current.
        statements: enabled(statements),
        flavour: format::flavour(MYSQL_MARKERS, &found),
    })
}

/// A probe of a table whose rows depend on a switch: readable, and the switch (the answer's
/// first cell) `YES`.
fn enabled(answer: &Answer) -> Readable {
    match answer {
        Answer::Rows(rows) if rows.first().and_then(|r| r.first()).is_some_and(|c| c.as_deref() == Some("YES")) => {
            Readable::Yes
        }
        Answer::Rows(_) => Readable::Unknown,
        refused => readable(refused),
    }
}

/// The server's rows: the facts and its `Uptime`.
pub fn server_info(rows: &[Rows]) -> Result<ServerInfo, DecodeError> {
    let [facts, status] = rows else {
        return Err(malformed("server info", format!("{} statements, expected 2", rows.len())));
    };
    let row = one(facts, "server info", SERVER_INFO)?;
    let now = row.required_time("now")?;
    let uptime = status_value(&name_values(status, "server info uptime")?, "Uptime")?;
    let version = match row.text("version_comment") {
        Some(comment) if !comment.is_empty() => format!("{} {comment}", row.required("version")?),
        _ => row.required("version")?.to_owned(),
    };
    let mut extra = Vec::new();
    for (key, i) in [
        (ExtraKey::ServerId, "server_id"),
        (ExtraKey::GtidMode, "gtid_mode"),
        (ExtraKey::LogBin, "log_bin"),
        (ExtraKey::SuperReadOnly, "super_read_only"),
        (ExtraKey::PerformanceSchema, "performance_schema"),
    ] {
        if let Some(value) = row.string(i) {
            extra.push(Extra { key, value });
        }
    }
    Ok(ServerInfo {
        version,
        started: uptime.and_then(|u| i64::try_from(u).ok()).map(|u| Timestamp(now.0 - u * 1_000_000)),
        read_at: now,
        read_only: row.required_bool("read_only")? || row.bool("super_read_only")?.unwrap_or(false),
        max_connections: row.uint("max_connections")?,
        cache_bytes: row.uint("innodb_buffer_pool_size")?,
        extra,
    })
}

/// A status variable's value, `None` when the server does not have it.
fn status_value(values: &[(&str, Option<&str>)], name: &str) -> Result<Option<u64>, DecodeError> {
    match values.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
        None | Some((_, None)) => Ok(None),
        Some((_, Some(v))) => {
            v.parse::<u64>().map(Some).map_err(|_| malformed("status", format!("{name}: {v:?} is not a count")))
        }
    }
}

/// The counters' rows (the clock with the threads by command, the status variables): the
/// sample (with the monitor's own statements as the driver counted them when the status was
/// read), the session counts, and what the account cannot see of the threads (without `PROCESS`
/// the process list holds the account's own threads only).
pub fn counters(rows: &[Rows], my: &MySqlMonitor, own: Own) -> Result<(Sample, Vec<Gauge>, Vec<Hidden>), DecodeError> {
    let [clock, status] = rows else {
        return Err(malformed("counters", format!("{} statements, expected 2", rows.len())));
    };
    let clock = one(clock, "clock", CLOCK)?;
    let time = clock.required_time("now")?;
    let values = name_values(status, "status")?;
    let get = |name: &str| status_value(&values, name);
    // A sum of counters, `None` when the server lacks one of them (never a part taken as all).
    let sum = |names: &[&str]| -> Result<Option<u64>, DecodeError> {
        let mut total = Some(0u64);
        for name in names {
            total = match (total, get(name)?) {
                (Some(t), Some(v)) => Some(t.saturating_add(v)),
                _ => None,
            };
        }
        Ok(total)
    };
    let uptime = get("Uptime")?.ok_or_else(|| malformed("status", "no Uptime"))?;
    let mut counters = Vec::new();
    let mut push = |kind, value: Option<u64>| {
        if let Some(value) = value {
            counters.push(Counter { kind, value });
        }
    };
    push(CounterKind::Statements, get("Questions")?);
    push(CounterKind::ExplicitCommits, get("Com_commit")?);
    push(CounterKind::ExplicitRollbacks, get("Com_rollback")?);
    push(CounterKind::RowsRead, get("Innodb_rows_read")?);
    push(CounterKind::RowsWritten, sum(&["Innodb_rows_inserted", "Innodb_rows_updated", "Innodb_rows_deleted"])?);
    push(CounterKind::PageRequests, get("Innodb_buffer_pool_read_requests")?);
    push(CounterKind::PageReads, get("Innodb_buffer_pool_reads")?);
    push(CounterKind::BytesIn, get("Bytes_received")?);
    push(CounterKind::BytesOut, get("Bytes_sent")?);
    push(CounterKind::RowLockWaits, get("Innodb_row_lock_waits")?);
    push(CounterKind::SlowQueries, get("Slow_queries")?);
    push(CounterKind::TempDiskTables, get("Created_tmp_disk_tables")?);
    push(CounterKind::AbortedConnects, get("Aborted_connects")?);
    push(CounterKind::Selects, get("Com_select")?);
    push(CounterKind::Inserts, sum(&["Com_insert", "Com_insert_select", "Com_replace", "Com_replace_select"])?);
    push(CounterKind::Updates, sum(&["Com_update", "Com_update_multi"])?);
    push(CounterKind::Deletes, sum(&["Com_delete", "Com_delete_multi"])?);
    let mut gauges = Vec::new();
    if let Some(value) = get("Threads_connected")? {
        gauges.push(Gauge { kind: GaugeKind::Sessions, value });
    }
    // The monitor's own thread runs the read.
    if let Some(value) = get("Threads_running")? {
        gauges.push(Gauge { kind: GaugeKind::Active, value: value.saturating_sub(1) });
    }
    for (kind, column) in [
        (GaugeKind::CommandQuery, "cmd_query"),
        (GaugeKind::CommandSleep, "cmd_sleep"),
        (GaugeKind::CommandDaemon, "cmd_daemon"),
        (GaugeKind::CommandConnect, "cmd_connect"),
        (GaugeKind::CommandBinlogDump, "cmd_binlog_dump"),
        (GaugeKind::CommandOther, "cmd_other"),
    ] {
        gauges.push(Gauge { kind, value: clock.required_uint(column)? });
    }
    // Only a direct grant of `PROCESS` is seen: without one, no thread of another account in
    // the list means it holds the account's own only, as far as can be told.
    let mut hidden = Vec::new();
    if my.process != Readable::Yes && clock.required_uint("others")? == 0 {
        hidden.push(Hidden { part: Part::OtherSessions, need: Need::Grant("PROCESS") });
    }
    Ok((Sample { time, epoch: Epoch::Uptime(uptime), counters, own }, gauges, hidden))
}

/// The session list's rows, and what the account cannot see of it.
pub fn sessions(
    rows: &[Rows],
    my: &MySqlMonitor,
    wanted: SessionsWanted,
) -> Result<(SessionList, Vec<Hidden>), DecodeError> {
    let [rows] = rows else {
        return Err(malformed("sessions", format!("{} statements, expected 1", rows.len())));
    };
    let mut time = None;
    let (mut trx_read, mut locks_read) = (true, true);
    let mut list = Vec::with_capacity(rows.len());
    for cells in rows {
        let row = Row::new(cells, "sessions", SESSIONS)?;
        let now = row.required_time("now")?;
        if time.is_some_and(|t| t != now) {
            return Err(malformed("sessions", "rows of different snapshots"));
        }
        time = Some(now);
        trx_read = row.required_bool("trx_read")?;
        locks_read = row.required_bool("locks_read")?;
        list.push(session(&row, now, trx_read && locks_read)?);
    }
    let time = time.ok_or_else(|| malformed("sessions", "no rows"))?;
    let mut hidden = Vec::new();
    if my.process != Readable::Yes && list.iter().all(|s| s.user.as_deref() == Some(my.user.as_str())) {
        hidden.push(Hidden { part: Part::OtherSessions, need: Need::Grant("PROCESS") });
    }
    // What would tell whether an idle session holds a transaction or a lock.
    let need = if !trx_read {
        Some(Need::Grant("PROCESS"))
    } else if !locks_read && my.metadata_locks == Readable::Refused {
        Some(Need::Grant("SELECT on performance_schema"))
    } else if !locks_read {
        Some(Need::Setting("wait/lock/metadata/sql/mdl"))
    } else {
        None
    };
    if let Some(need) = need {
        hidden.push(Hidden { part: Part::TransactionState, need });
    }
    Ok((SessionList { time, rows: list, idle_left_out: !wanted.idle && trx_read && locks_read }, hidden))
}

/// A session's row; `known`: both `INNODB_TRX` and the metadata locks were read, so a session
/// with neither an InnoDB transaction nor a lock past its statement holds no transaction.
fn session(row: &Row, now: Timestamp, known: bool) -> Result<SessionRow, DecodeError> {
    let id = row.required_int("ID")?;
    let user = row.string("USER");
    let command = row.required("COMMAND")?;
    let state = row.text("STATE").filter(|s| !s.is_empty());
    let kind = if command == "Daemon" || user.as_deref() == Some("system user") {
        SessionKind::Background(user.clone().unwrap_or_else(|| command.to_owned()))
    } else if command.starts_with("Binlog Dump") {
        SessionKind::Replication
    } else {
        SessionKind::Client
    };
    let tx = match (row.time("trx_started")?, row.bool("in_tx")?, known) {
        (Some(t), _, _) => Tx::Since(t),
        // Locks held until a transaction ends, and no InnoDB transaction (one that read only
        // MyISAM tables): open, since when not told.
        (None, Some(true), _) => Tx::Open,
        (None, _, true) => Tx::None,
        (None, _, false) => Tx::Unknown,
    };
    let time = row.int("TIME")?;
    let since = time.map(|secs| Timestamp(now.0 - secs.max(0) * 1_000_000));
    let session_state = match command {
        _ if matches!(kind, SessionKind::Background(_)) => SessionState::Background,
        "Sleep" if matches!(tx, Tx::Since(_) | Tx::Open) => SessionState::IdleInTx,
        "Sleep" => SessionState::Idle,
        "Query" | "Execute" => SessionState::Active,
        other => SessionState::Other(other.to_owned()),
    };
    let wait = if row.text("trx_state") == Some("LOCK WAIT") {
        Some(Wait { class: WaitClass::Lock, name: "row lock".into() })
    } else {
        state.and_then(lock_wait)
    };
    let mut extra = vec![Extra { key: ExtraKey::Command, value: command.to_owned() }];
    if let Some(state) = state {
        extra.push(Extra { key: ExtraKey::State, value: state.to_owned() });
    }
    for (key, i) in [(ExtraKey::RowsLocked, "trx_rows_locked"), (ExtraKey::RowsModified, "trx_rows_modified")] {
        if let Some(value) = row.string(i) {
            extra.push(Extra { key, value });
        }
    }
    if row.bool("explicit_locks")? == Some(true) {
        extra.push(Extra { key: ExtraKey::ExplicitLocks, value: "yes".into() });
    }
    let thread = row.uint("THREAD_ID")?;
    let info_md5 = row.string("info_md5");
    // The running statement event where there is one; else, for a session running a statement
    // (or holding its text), its command, text and time.
    let statement = match (thread, row.uint("EVENT_ID")?) {
        (Some(thread_id), Some(event_id)) => Some(StatementStamp::MySqlEvent { thread_id, event_id }),
        _ if session_state == SessionState::Active || info_md5.is_some() => time.map(|t| StatementStamp::MySqlWeak {
            command: command.to_owned(),
            info_md5: info_md5.clone(),
            time: t.max(0).unsigned_abs(),
        }),
        _ => None,
    };
    Ok(SessionRow {
        key: SessionKey { id, started: None },
        kind,
        user,
        database: row.string("DB"),
        application: None,
        client: row.string("HOST").filter(|h| !h.is_empty()),
        query_started: if session_state == SessionState::Active { since } else { None },
        state: session_state,
        wait,
        tx,
        state_since: since,
        // INFO is the whole statement: the server does not cut it.
        query: match (row.string("INFO"), row.bool("shortened")?) {
            (Some(text), shortened) if !text.is_empty() => {
                Some(QueryText { text, shortened: shortened.unwrap_or(false), maybe_cut_by_server: false })
            }
            _ => None,
        },
        blocked_by: None,
        monitor: row.required_bool("monitor")?,
        visible: Visibility::Full,
        extra,
        thread,
        statement,
    })
}

/// A `STATE` that waits for a lock another session holds (`Waiting for table metadata lock`,
/// `Waiting for global read lock`, ...), named by what it waits for.
fn lock_wait(state: &str) -> Option<Wait> {
    // `GET_LOCK()` waiting for a user-level lock says so in its own words.
    if state == "User lock" {
        return Some(Wait { class: WaitClass::Lock, name: "user lock".into() });
    }
    let what = state.strip_prefix("Waiting for ")?;
    what.ends_with(" lock").then(|| Wait { class: WaitClass::Lock, name: what.to_owned() })
}

/// Whether a session may be waiting for a lock, from the session list's rows: one waits for a
/// row lock (`INNODB_TRX`) or says it waits for a lock (`STATE`), or the list cannot tell (it
/// lacks `INNODB_TRX`, or other accounts' sessions). Only then are the lock tables read.
pub fn may_wait(rows: &Rows, my: &MySqlMonitor) -> Result<bool, DecodeError> {
    if my.process != Readable::Yes {
        return Ok(true);
    }
    for cells in rows {
        let row = Row::new(cells, "sessions", SESSIONS)?;
        let waits = row.text("trx_state") == Some("LOCK WAIT") || row.text("STATE").and_then(lock_wait).is_some();
        if waits || !row.required_bool("trx_read")? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The lock reads' answers ([`ROW_LOCK_WAITS`], then [`METADATA_LOCKS`] when the account may
/// read them), read at `time`; what the account could not read is said. Row lock waits name
/// their blockers; a metadata lock request is paired with the granted locks of other sessions on
/// the same object that it cannot be granted beside ([`lk::mdl_conflicts`]), which may block it.
pub fn locks(answers: &[Answer], my: &MySqlMonitor, time: Timestamp) -> Result<(LockList, Vec<Hidden>), DecodeError> {
    let mut waits: Vec<LockWait> = Vec::new();
    let mut hidden = Vec::new();
    let mut more = 0u64;
    // In the order the driver sends them: the row lock waits from 8.0.38, the metadata lock
    // waits when readable.
    let expected = usize::from(my.lock_waits()) + usize::from(my.metadata_locks == Readable::Yes);
    if answers.len() != expected {
        return Err(malformed("locks", format!("{} answers, expected {expected}", answers.len())));
    }
    let mut next = answers.iter();
    let rows = if my.lock_waits() { next.next() } else { None };
    let metadata = next.next();
    match rows {
        // Not read on this version: its read stalls a busy server.
        None => hidden
            .push(Hidden { part: Part::RowLockWaits, need: Need::Version(crate::monitor::format::LOCK_WAITS_FROM) }),
        Some(Answer::Rows(rows)) => {
            if rows.len() >= lk::LOCK_ROWS {
                more += 1;
            }
            for cells in rows {
                let row = Row::new(cells, "row lock waits", ROW_LOCK_WAITS)?;
                // A request of a thread with no session (a background thread): not a session's.
                let Some(waiter) = row.int("waiter")? else { continue };
                // What the request is on is not read: the engine finds a lock by its id only by
                // going through all its locks. The wait is a row lock's.
                let object = LockObject { kind: "RECORD".into(), ..Default::default() };
                let mode = String::new();
                let holder = match row.int("blocker")? {
                    Some(id) => Holder::Session(id),
                    None => Holder::Transaction {
                        id: row.required("blocking_trx")?.to_owned(),
                        sessionless: row.bool("blocking_session_read")? == Some(true),
                    },
                };
                let held =
                    LockHolder { holder, modes: Vec::new(), waiting: false, blame: Blame::Proven, explicit: false };
                let at = waits.iter().position(|w| w.waiter.id == waiter && w.object == object && w.mode == mode);
                let w = match at {
                    Some(i) => &mut waits[i],
                    None => {
                        waits.push(LockWait {
                            waiter: SessionKey { id: waiter, started: None },
                            object,
                            mode,
                            since: None,
                            query_started: None,
                            whole_seconds: true,
                            holders: Vec::new(),
                        });
                        waits.last_mut().expect("just pushed")
                    }
                };
                if !w.holders.iter().any(|h| h.holder == held.holder) {
                    w.holders.push(held);
                }
            }
        }
        Some(refused @ Answer::Error { .. }) if readable(refused) == Readable::Refused => {
            hidden.push(Hidden { part: Part::RowLockWaits, need: Need::Grant("SELECT on performance_schema") });
        }
        Some(Answer::Error { message, .. }) => return Err(malformed("row lock waits", message.clone())),
    }
    match metadata {
        Some(Answer::Rows(rows)) => {
            if rows.len() >= lk::LOCK_ROWS {
                more += 1;
            }
            let mut cut = false;
            waits.extend(metadata_waits(rows, &mut cut)?);
            more += u64::from(cut);
        }
        Some(Answer::Error { message, .. }) => return Err(malformed("metadata locks", message.clone())),
        None => hidden.push(Hidden {
            part: Part::MetadataLockWaits,
            need: match my.metadata_locks {
                Readable::Refused => Need::Grant("SELECT on performance_schema"),
                _ => Need::Setting("wait/lock/metadata/sql/mdl"),
            },
        }),
    }
    Ok((LockList { time, waits, more, blocks: true }, hidden))
}

/// `schema.name`, or the one of them there is.
fn qualified(schema: Option<&str>, name: Option<&str>) -> Option<String> {
    match (schema.filter(|s| !s.is_empty()), name.filter(|n| !n.is_empty())) {
        (Some(s), Some(n)) => Some(format!("{s}.{n}")),
        (Some(x), None) | (None, Some(x)) => Some(x.to_owned()),
        (None, None) => None,
    }
}

/// The metadata lock waits of [`METADATA_LOCKS`] rows: each pending request with the granted
/// locks of other threads on the same object that it cannot be granted beside.
fn metadata_waits(rows: &Rows, cut: &mut bool) -> Result<Vec<LockWait>, DecodeError> {
    struct Mdl<'a> {
        id: Option<i64>,
        thread: i64,
        kind: &'a str,
        schema: Option<&'a str>,
        name: Option<&'a str>,
        lock: &'a str,
        pending: bool,
        explicit: bool,
    }
    let mut all = Vec::with_capacity(rows.len());
    for cells in rows {
        let row = Row::new(cells, "metadata locks", METADATA_LOCKS)?;
        all.push(Mdl {
            id: row.int("id")?,
            thread: row.required_int("thread")?,
            kind: row.required("OBJECT_TYPE")?,
            schema: row.text("OBJECT_SCHEMA"),
            name: row.text("OBJECT_NAME"),
            lock: row.required("LOCK_TYPE")?,
            pending: row.required("LOCK_STATUS")? == "PENDING",
            explicit: row.required("LOCK_DURATION")? == "EXPLICIT",
        });
    }
    // The granted locks of each object, so pairing does not go through every lock per request.
    type Object<'a> = (&'a str, Option<&'a str>, Option<&'a str>);
    let mut by_object: std::collections::HashMap<Object, Vec<usize>> = Default::default();
    for (i, g) in all.iter().enumerate().filter(|(_, g)| !g.pending) {
        by_object.entry((g.kind, g.schema, g.name)).or_default().push(i);
    }
    let mut waits: Vec<LockWait> = Vec::new();
    for p in all.iter().filter(|m| m.pending) {
        // A thread with no session (a background thread): not a session's wait.
        let Some(id) = p.id else { continue };
        let mut holders: Vec<LockHolder> = Vec::new();
        let mut at: std::collections::HashMap<i64, usize> = Default::default();
        let granted = by_object.get(&(p.kind, p.schema, p.name)).map(Vec::as_slice).unwrap_or_default();
        for g in granted.iter().map(|&i| &all[i]).filter(|g| g.thread != p.thread) {
            let blame = match lk::mdl_conflicts(p.kind, p.lock, g.lock) {
                Some(true) => Blame::Incompatible,
                Some(false) => continue,
                None => Blame::CannotTell,
            };
            // A holding thread with no session: not one the tree can show.
            let Some(sid) = g.id else { continue };
            match at.get(&sid) {
                Some(&i) => {
                    let h = &mut holders[i];
                    if !h.modes.iter().any(|m| m == g.lock) {
                        h.modes.push(g.lock.to_owned());
                    }
                    if blame == Blame::Incompatible {
                        h.blame = blame;
                    }
                    h.explicit |= g.explicit;
                }
                // Past the most holders a wait keeps: said as a read cut short.
                None if holders.len() >= lk::HOLDERS_MAX => *cut = true,
                None => {
                    at.insert(sid, holders.len());
                    let holder = Holder::Session(sid);
                    holders.push(LockHolder {
                        holder,
                        modes: vec![g.lock.to_owned()],
                        waiting: false,
                        blame,
                        explicit: g.explicit,
                    });
                }
            }
        }
        waits.push(LockWait {
            waiter: SessionKey { id, started: None },
            object: LockObject {
                kind: p.kind.to_owned(),
                name: qualified(p.schema, p.name),
                detail: None,
                database: None,
            },
            mode: p.lock.to_owned(),
            since: None,
            query_started: None,
            whole_seconds: true,
            holders,
        });
    }
    Ok(waits)
}

/// The rows of a tick's statements (the clock, the status, then the sessions and the server
/// when wanted), and the lock reads' answers when the Locks view asked (`None`: not asked),
/// with the monitor's own statements as they stood when the counters were read.
pub fn tick(
    my: &MySqlMonitor,
    want: &Want,
    rows: &[Rows],
    locks_read: Option<&[Answer]>,
    own: Own,
) -> Result<Snapshot, DecodeError> {
    let expected = 2 + usize::from(want.sessions.is_some()) + 2 * usize::from(want.server);
    if rows.len() != expected {
        return Err(malformed("tick", format!("{} statements, expected {expected}", rows.len())));
    }
    if locks_read.is_some() != want.locks {
        return Err(malformed("tick", "lock reads not as asked"));
    }
    let (sample, gauges, own_only) = counters(&rows[..2], my, own)?;
    let mut next = 2;
    let (sessions, hidden) = match want.sessions {
        Some(wanted) => {
            next += 1;
            let (list, hidden) = sessions(&rows[2..3], my, wanted)?;
            (Some(list), hidden)
        }
        None => (None, Vec::new()),
    };
    let server = if want.server { Some(server_info(&rows[next..next + 2])?) } else { None };
    let mut hidden = hidden;
    for h in own_only {
        if !hidden.contains(&h) {
            hidden.push(h);
        }
    }
    let locks = match locks_read {
        Some(answers) => {
            let (mut list, more) = locks(answers, my, sample.time)?;
            hidden.extend(more);
            // Metadata locks the performance schema had no room for: some holders are missing
            // (not knowing the counter is the same as not knowing they are all there).
            let status = name_values(&rows[1], "status")?;
            let lost = status_value(&status, "Performance_schema_metadata_lock_lost")?;
            // A thread the performance schema could not track has none of its metadata locks
            // tracked either.
            let threads = status_value(&status, "Performance_schema_thread_instances_lost")?;
            let untracked = lost.is_none_or(|n| n > 0) || threads.is_none_or(|n| n > 0);
            if my.metadata_locks == Readable::Yes && untracked {
                list.more += 1;
                hidden.push(Hidden {
                    part: Part::UntrackedMetadataLocks,
                    need: Need::Setting("performance_schema_max_metadata_locks"),
                });
            }
            // Each wait is of the statement its session runs in this read.
            if let Some(rows) = sessions.as_ref().map(|l| &l.rows) {
                for w in &mut list.waits {
                    w.query_started = rows.iter().find(|r| r.key.id == w.waiter.id).and_then(|r| r.query_started);
                }
            }
            Some(list)
        }
        None => None,
    };
    // `performance_schema` ran out of thread slots since the server started: its session list
    // may miss some.
    let lost = status_value(&name_values(&rows[1], "status")?, "Performance_schema_thread_instances_lost")?;
    if sessions.is_some() && my.ps_processlist() && lost.is_some_and(|n| n > 0) {
        hidden.push(Hidden {
            part: Part::UntrackedSessions,
            need: Need::Setting("performance_schema_max_thread_instances"),
        });
    }
    Ok(Snapshot { sample, gauges, sessions, locks, server, hidden })
}

/// The row read for the whole text of the statement `ask` names (none: the session has ended).
/// The server tells how long a statement has run in whole seconds only, so a start worked out
/// from it lies within a second either side of the true one. The text is taken as that
/// statement's only when the list saw it running for at least [`TEXT_MIN_AGE_US`] (a later
/// statement then starts well after the one listed), the session runs one now whose start lies
/// within [`TEXT_SAME_START_US`] of the listed one's, and it begins as the list showed it. Even
/// so a statement inside a stored program may share its program's start, so this is likely,
/// not proven ([`Proof::Likely`]).
pub fn query_text(ask: &TextAsk, rows: &Rows) -> Result<FullText, DecodeError> {
    let row = match rows.as_slice() {
        [] => return Ok(FullText::Gone),
        [row] => Row::new(row, "query text", QUERY_TEXT)?,
        _ => return Err(malformed("query text", format!("{} rows, expected at most 1", rows.len()))),
    };
    if row.required_int("ID")? != ask.session.id {
        return Err(malformed("query text", "another session's row"));
    }
    let Some(asked) = ask.query_started else { return Ok(FullText::Unknown(None)) };
    if ask.seen.0 - asked.0 < TEXT_MIN_AGE_US {
        return Ok(FullText::NotYet);
    }
    let now = row.required_time("now")?;
    let running = matches!(row.required("COMMAND")?, "Query" | "Execute");
    let apart = row.int("TIME")?.map(|secs| (now.0 - secs.max(0) * 1_000_000 - asked.0).abs());
    let text = row.string("INFO").filter(|t| t.starts_with(&ask.start));
    match (running, apart, text) {
        (true, Some(apart), Some(text)) if apart < TEXT_SAME_START_US => Ok(FullText::Text {
            text: QueryText { text, shortened: false, maybe_cut_by_server: row.bool("maybe_cut")?.unwrap_or(false) },
            proof: Proof::Likely,
        }),
        // Between the two bounds a start cannot say which.
        (true, Some(apart), Some(_)) if apart < TEXT_SAME_START_US + 1_000_000 => Ok(FullText::Unknown(None)),
        _ => Ok(FullText::Moved),
    }
}
