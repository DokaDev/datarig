//! MySQL rows read into the model (the statements are the driver's: its `meta::monitor`).

use super::{Answer, DecodeError, Row, Rows, malformed, name_values, one};
use crate::monitor::format::{self, MYSQL_MARKERS, MYSQL_MIN_VERSION, MySqlMonitor, Probe, Readable};
use crate::monitor::model::{
    Counter, CounterKind, Epoch, Extra, ExtraKey, Gauge, GaugeKind, Hidden, Need, Own, Part, QueryText, Sample,
    ServerInfo, SessionKey, SessionKind, SessionList, SessionRow, SessionState, Timestamp, Tx, Visibility, Wait,
    WaitClass,
};
use crate::monitor::model::{SessionsWanted, Snapshot, Want};

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
];

/// The columns of the probe's first row: the version line, `@@performance_schema`, `CURRENT_USER()`, in order.
pub const PROBE: &[&str] = &["version", "performance_schema", "user"];

/// The columns of the probe's last row: whether the metadata lock instrument is on, and a count that only names the lock tables, in order.
pub const PROBE_LOCKS: &[&str] = &["ENABLED", "probe"];

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

/// The columns of the clock's row (microseconds since the Unix epoch), in order.
pub const CLOCK: &[&str] = &["now"];

/// The columns of a session's row (`INFO` its first `QUERY_CHARS` characters; the `trx_` columns NULL and `trx_read` 0 without `INNODB_TRX`, `in_tx` and `explicit_locks` NULL and `locks_read` 0 without the metadata lock table), in order.
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
];

/// The server's error codes that refuse a read for lack of a privilege: 1044 (database), 1142
/// (table), 1143 (column), 1227 (a privilege such as `PROCESS`).
const REFUSED: &[u32] = &[1044, 1142, 1143, 1227];

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
/// instrument ([`PROBE_LOCKS`]).
pub fn probe(answers: &[Answer]) -> Result<MySqlMonitor, DecodeError> {
    let [first, variables, grants, trx, locks] = answers else {
        return Err(malformed("probe", format!("{} answers, expected 5", answers.len())));
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
        metadata_locks: match locks {
            Answer::Rows(rows) if rows.first().and_then(|r| r.first()).is_some_and(|c| c.as_deref() == Some("YES")) => {
                Readable::Yes
            }
            Answer::Rows(_) => Readable::Unknown,
            refused => readable(refused),
        },
        flavour: format::flavour(MYSQL_MARKERS, &found),
    })
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

/// The counters' rows (the clock, the status variables): the sample (with the monitor's own
/// statements as the driver counted them when the status was read) and the session counts.
pub fn counters(rows: &[Rows], own: Own) -> Result<(Sample, Vec<Gauge>), DecodeError> {
    let [clock, status] = rows else {
        return Err(malformed("counters", format!("{} statements, expected 2", rows.len())));
    };
    let time = one(clock, "clock", CLOCK)?.required_time("now")?;
    let values = name_values(status, "status")?;
    let get = |name| status_value(&values, name);
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
    let written = [get("Innodb_rows_inserted")?, get("Innodb_rows_updated")?, get("Innodb_rows_deleted")?];
    push(CounterKind::RowsWritten, written.iter().try_fold(0u64, |sum, v| v.map(|v| sum.saturating_add(v))));
    push(CounterKind::PageRequests, get("Innodb_buffer_pool_read_requests")?);
    push(CounterKind::PageReads, get("Innodb_buffer_pool_reads")?);
    push(CounterKind::BytesIn, get("Bytes_received")?);
    push(CounterKind::BytesOut, get("Bytes_sent")?);
    push(CounterKind::RowLockWaits, get("Innodb_row_lock_waits")?);
    push(CounterKind::SlowQueries, get("Slow_queries")?);
    push(CounterKind::TempDiskTables, get("Created_tmp_disk_tables")?);
    push(CounterKind::AbortedConnects, get("Aborted_connects")?);
    let mut gauges = Vec::new();
    if let Some(value) = get("Threads_connected")? {
        gauges.push(Gauge { kind: GaugeKind::Sessions, value });
    }
    // The monitor's own thread runs the read.
    if let Some(value) = get("Threads_running")? {
        gauges.push(Gauge { kind: GaugeKind::Active, value: value.saturating_sub(1) });
    }
    Ok((Sample { time, epoch: Epoch::Uptime(uptime), counters, own }, gauges))
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
    let since = row.int("TIME")?.map(|secs| Timestamp(now.0 - secs.max(0) * 1_000_000));
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
    })
}

/// A `STATE` that waits for a lock another session holds (`Waiting for table metadata lock`,
/// `Waiting for global read lock`, ...), named by what it waits for.
fn lock_wait(state: &str) -> Option<Wait> {
    let what = state.strip_prefix("Waiting for ")?;
    what.ends_with(" lock").then(|| Wait { class: WaitClass::Lock, name: what.to_owned() })
}

/// The rows of a tick's statements (the clock, the status, then the sessions and the server
/// when wanted), with the
/// monitor's own statements as they stood when the counters were read.
pub fn tick(my: &MySqlMonitor, want: &Want, rows: &[Rows], own: Own) -> Result<Snapshot, DecodeError> {
    let expected = 2 + usize::from(want.sessions.is_some()) + 2 * usize::from(want.server);
    if rows.len() != expected {
        return Err(malformed("tick", format!("{} statements, expected {expected}", rows.len())));
    }
    let (sample, gauges) = counters(&rows[..2], own)?;
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
    // `performance_schema` ran out of thread slots since the server started: its session list
    // may miss some.
    let lost = status_value(&name_values(&rows[1], "status")?, "Performance_schema_thread_instances_lost")?;
    if sessions.is_some() && my.ps_processlist() && lost.is_some_and(|n| n > 0) {
        hidden.push(Hidden {
            part: Part::UntrackedSessions,
            need: Need::Setting("performance_schema_max_thread_instances"),
        });
    }
    Ok(Snapshot { sample, gauges, sessions, server, hidden })
}
