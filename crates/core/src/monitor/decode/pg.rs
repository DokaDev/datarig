//! PostgreSQL rows read into the model (the statements are the driver's: its `meta::monitor`).

pub mod replication;

use super::{DecodeError, Row, Rows, malformed, one};
use crate::monitor::action::StatementStamp;
use crate::monitor::format::{self, PG_MARKERS, PG_MIN_VERSION, PgMonitor, Probe, Route};
use crate::monitor::locks::{Blame, Holder, LockHolder, LockList, LockObject, LockWait, Prepared};
use crate::monitor::model::SessionsWanted;
use crate::monitor::model::{
    Blocker, Counter, CounterKind, Epoch, Extra, ExtraKey, FullText, Gauge, GaugeKind, Hidden, Need, Own, Part, Proof,
    QueryText, Sample, ServerInfo, SessionKey, SessionKind, SessionList, SessionRow, SessionState, Tx, Visibility,
    Wait, WaitClass,
};
use crate::monitor::model::{Snapshot, TextAsk, Timestamp, Want};

/// The columns of the probe's row: version, the account's privileges, the provider markers found (comma separated), in order.
pub const PROBE: &[&str] = &["version", "superuser", "read_all_stats", "signal_backend", "markers"];

/// The columns of the server's row (times as microseconds since the Unix epoch; `shared_buffers` in bytes; then the settings that decide what the monitor can read, as the server shows them), in order.
pub const SERVER_INFO: &[&str] = &[
    "now",
    "version",
    "started",
    "in_recovery",
    "max_connections",
    "shared_buffers",
    "track_io_timing",
    "track_activities",
    "track_counts",
    "track_activity_query_size",
    "wal_level",
];

/// The columns of the counters' row: sums over every database, then session counts (the monitor's own session not active), in order.
pub const COUNTERS: &[&str] = &[
    "now",
    "commits",
    "rollbacks",
    "blks_read",
    "blks_hit",
    "tup_returned",
    "tup_written",
    "deadlocks",
    "temp_bytes",
    "databases",
    "stats_reset",
    "sessions",
    "active",
    "idle_in_tx",
    "idle",
    "waiting",
    "hidden",
];

/// The columns of a session's row (`query` its first `QUERY_CHARS` characters, `maybe_cut` within three bytes of `track_activity_query_size`, `blocked_by` `pg_blocking_pids` as text for the longest `MAX_WAITERS` lock waiters, else NULL), in order.
pub const SESSIONS: &[&str] = &[
    "now",
    "pid",
    "leader_pid",
    "usename",
    "datname",
    "application_name",
    "client_addr",
    "client_port",
    "backend_type",
    "state",
    "wait_event_type",
    "wait_event",
    "backend_start",
    "xact_start",
    "query_start",
    "state_change",
    "query",
    "shortened",
    "maybe_cut",
    "monitor",
    "blocked_by",
];

/// The columns of a session's row read for its statement's whole text (times as microseconds since the Unix epoch; `state` to tell a session whose activity is not tracked; `maybe_cut` within three bytes of `track_activity_query_size`), in order.
pub const QUERY_TEXT: &[&str] = &["pid", "backend_start", "state", "query_start", "query", "maybe_cut"];

/// The columns of a lock's row (`role` `wait` or `hold`; `key` the same text for the same object; `pid` NULL for a prepared transaction's lock, then `gid`, `owner`, `prepared` and `prepared_database` its own; `database` the object's when another than the monitor's; times as microseconds since the Unix epoch, `query_start` the waiter's statement's; `waits` the requests not granted in all), in order.
pub const LOCKS: &[&str] = &[
    "role",
    "key",
    "pid",
    "backend_start",
    "gid",
    "owner",
    "prepared",
    "prepared_database",
    "locktype",
    "mode",
    "database",
    "relation",
    "detail",
    "since",
    "query_start",
    "waits",
];

/// What `pg_stat_activity.query` holds for a session the account may not see.
pub const HIDDEN_QUERY: &str = "<insufficient privilege>";

/// The probe's row.
pub fn probe(rows: &Rows) -> Result<PgMonitor, DecodeError> {
    let row = one(rows, "probe", PROBE)?;
    let version = row.required("version")?;
    let version = version.parse::<u32>().map_err(|_| malformed("probe", format!("server_version_num {version:?}")))?;
    if version < PG_MIN_VERSION {
        return Err(DecodeError::Unsupported(format!("PostgreSQL {}.{}", version / 10000, version % 10000 / 100)));
    }
    let names = row.required("markers")?.split(',').filter(|n| !n.is_empty()).map(str::to_owned).collect();
    Ok(PgMonitor {
        version,
        superuser: row.required_bool("superuser")?,
        read_all_stats: row.required_bool("read_all_stats")?,
        signal_backend: row.required_bool("signal_backend")?,
        flavour: format::flavour(PG_MARKERS, &Probe { names, basedir: None }),
        // The driver says how it reached the server, once it has verified it.
        route: Route::Unknown,
    })
}

/// The server's row.
pub fn server_info(rows: &Rows) -> Result<ServerInfo, DecodeError> {
    let row = one(rows, "server info", SERVER_INFO)?;
    Ok(ServerInfo {
        read_at: row.required_time("now")?,
        version: row.required("version")?.to_owned(),
        started: row.time("started")?,
        read_only: row.required_bool("in_recovery")?,
        max_connections: row.uint("max_connections")?,
        cache_bytes: row.uint("shared_buffers")?,
        extra: [
            (ExtraKey::TrackActivities, "track_activities"),
            (ExtraKey::TrackCounts, "track_counts"),
            (ExtraKey::QuerySize, "track_activity_query_size"),
            (ExtraKey::TrackIoTiming, "track_io_timing"),
            (ExtraKey::WalLevel, "wal_level"),
        ]
        .into_iter()
        .filter_map(|(key, column)| row.string(column).map(|value| Extra { key, value }))
        .collect(),
    })
}

/// The counters' row: the sample (with the monitor's own traffic as the driver counted it when
/// it read them) and the session counts.
pub fn counters(rows: &Rows, own: Own) -> Result<(Sample, Vec<Gauge>), DecodeError> {
    let row = one(rows, "counters", COUNTERS)?;
    let counter = |kind, i| Ok::<_, DecodeError>(Counter { kind, value: row.required_uint(i)? });
    let hit = row.required_uint("blks_hit")?;
    let read = row.required_uint("blks_read")?;
    let counters = vec![
        counter(CounterKind::Commits, "commits")?,
        counter(CounterKind::Rollbacks, "rollbacks")?,
        Counter { kind: CounterKind::PageReads, value: read },
        Counter { kind: CounterKind::PageRequests, value: hit.saturating_add(read) },
        counter(CounterKind::RowsRead, "tup_returned")?,
        counter(CounterKind::RowsWritten, "tup_written")?,
        counter(CounterKind::Deadlocks, "deadlocks")?,
        counter(CounterKind::TempBytes, "temp_bytes")?,
    ];
    // NULL `stats_reset` everywhere: never reset.
    let epoch = Epoch::Mark(row.int("stats_reset")?.unwrap_or(0), row.required_int("databases")?);
    let gauge = |kind, i| Ok::<_, DecodeError>(Gauge { kind, value: row.required_uint(i)? });
    let gauges = vec![
        gauge(GaugeKind::Sessions, "sessions")?,
        gauge(GaugeKind::Active, "active")?,
        gauge(GaugeKind::IdleInTx, "idle_in_tx")?,
        gauge(GaugeKind::Idle, "idle")?,
        gauge(GaugeKind::WaitingForLock, "waiting")?,
        gauge(GaugeKind::Hidden, "hidden")?,
    ];
    Ok((Sample { time: row.required_time("now")?, epoch, counters, own }, gauges))
}

/// The session list's rows, and what the account cannot see of it.
pub fn sessions(rows: &Rows, wanted: SessionsWanted) -> Result<(SessionList, Vec<Hidden>), DecodeError> {
    let mut time = None;
    let mut list = Vec::with_capacity(rows.len());
    for cells in rows {
        let row = Row::new(cells, "sessions", SESSIONS)?;
        let now = row.required_time("now")?;
        if time.is_some_and(|t| t != now) {
            return Err(malformed("sessions", "rows of different snapshots"));
        }
        time = Some(now);
        list.push(session(&row)?);
    }
    let hidden = if list.iter().any(|s| s.visible == Visibility::Hidden) {
        vec![Hidden { part: Part::OtherSessionDetails, need: Need::Grant("pg_read_all_stats") }]
    } else {
        Vec::new()
    };
    // No row at all cannot be: the reading session is always one of them.
    let time = time.ok_or_else(|| malformed("sessions", "no rows"))?;
    Ok((SessionList { time, rows: list, idle_left_out: !wanted.idle }, hidden))
}

fn session(row: &Row) -> Result<SessionRow, DecodeError> {
    let pid = row.required_int("pid")?;
    let state = row.text("state");
    let query = row.text("query");
    let hidden = state.is_none() && query == Some(HIDDEN_QUERY);
    let kind = match row.text("backend_type") {
        Some("client backend") => SessionKind::Client,
        Some("walsender") => SessionKind::Replication,
        Some("parallel worker") => SessionKind::ParallelWorker { leader: row.int("leader_pid")? },
        Some(other) => SessionKind::Background(other.to_owned()),
        // Hidden with the rest from an account without `pg_read_all_stats`.
        None => SessionKind::Unknown,
    };
    let state = match (hidden, state) {
        (true, _) => SessionState::Unknown,
        (false, Some("active")) => SessionState::Active,
        (false, Some("idle")) => SessionState::Idle,
        (false, Some("idle in transaction")) => SessionState::IdleInTx,
        (false, Some("idle in transaction (aborted)")) => SessionState::IdleInTxAborted,
        (false, Some(other)) => SessionState::Other(other.to_owned()),
        (false, None) => SessionState::Background,
    };
    let wait = row.text("wait_event_type").map(|class| Wait {
        class: match class {
            "Lock" => WaitClass::Lock,
            "LWLock" => WaitClass::LightweightLock,
            "IO" => WaitClass::Io,
            "Client" => WaitClass::Client,
            "IPC" => WaitClass::Ipc,
            "Timeout" => WaitClass::Timeout,
            "Activity" => WaitClass::Activity,
            other => WaitClass::Other(other.to_owned()),
        },
        name: row.string("wait_event").unwrap_or_default(),
    });
    // `track_activities` off for that session: its state is `disabled` and its transaction is
    // not told.
    let untracked = state == SessionState::Other("disabled".into());
    let tx = match (hidden || untracked, row.time("xact_start")?) {
        (true, _) => Tx::Unknown,
        (false, None) => Tx::None,
        (false, Some(t)) => Tx::Since(t),
    };
    // A local socket has port -1 and no address.
    let client = match row.int("client_port")? {
        Some(-1) => None,
        _ => row.string("client_addr"),
    };
    let query = match (hidden, query) {
        (false, Some(text)) if !text.is_empty() => Some(QueryText {
            text: text.to_owned(),
            shortened: row.bool("shortened")?.unwrap_or(false),
            maybe_cut_by_server: row.bool("maybe_cut")?.unwrap_or(false),
        }),
        _ => None,
    };
    let query_started = row.time("query_start")?;
    Ok(SessionRow {
        key: SessionKey { id: pid, started: row.time("backend_start")? },
        kind,
        user: row.string("usename"),
        database: row.string("datname"),
        application: row.string("application_name").filter(|a| !a.is_empty()),
        client,
        state,
        wait,
        tx,
        query_started,
        state_since: row.time("state_change")?,
        query,
        blocked_by: row.text("blocked_by").map(blockers).transpose()?,
        monitor: row.required_bool("monitor")?,
        visible: if hidden { Visibility::Hidden } else { Visibility::Full },
        extra: Vec::new(),
        thread: None,
        statement: (!hidden).then_some(StatementStamp::Pg { query_start: query_started }),
    })
}

/// `pg_blocking_pids` as text (`{4211,3907}`); pid 0 is a prepared transaction.
fn blockers(text: &str) -> Result<Vec<Blocker>, DecodeError> {
    let inner = text
        .strip_prefix('{')
        .and_then(|t| t.strip_suffix('}'))
        .ok_or_else(|| malformed("sessions", format!("blocked_by {text:?}")))?;
    inner
        .split(',')
        .filter(|p| !p.is_empty())
        .map(|p| match p.parse::<i64>() {
            Ok(0) => Ok(Blocker::PreparedTransaction),
            Ok(pid) => Ok(Blocker::Session(pid)),
            Err(_) => Err(malformed("sessions", format!("blocked_by {text:?}"))),
        })
        .collect()
}

/// The lock read's rows, read at `time` (the transaction's: the counters' clock): each wait,
/// with the locks other sessions and prepared transactions hold on what it waits for. Who of
/// them blocks it is the session list's to say (`pg_blocking_pids`): these only tell what they
/// hold ([`Blame::Holds`]).
pub fn locks(rows: &Rows, time: Timestamp) -> Result<LockList, DecodeError> {
    let mut waits: Vec<(&str, LockWait)> = Vec::new();
    let mut holds: Vec<(&str, Holder, String)> = Vec::new();
    let mut total = 0u64;
    for cells in rows {
        let row = Row::new(cells, "locks", LOCKS)?;
        total = row.required_uint("waits")?;
        let key = row.required("key")?;
        let mode = row.required("mode")?.to_owned();
        match row.required("role")? {
            "wait" => waits.push((
                key,
                LockWait {
                    waiter: SessionKey { id: row.required_int("pid")?, started: row.time("backend_start")? },
                    object: LockObject {
                        kind: row.required("locktype")?.to_owned(),
                        name: row.string("relation"),
                        detail: row.string("detail"),
                        database: row.string("database"),
                    },
                    mode,
                    since: row.time("since")?,
                    query_started: row.time("query_start")?,
                    whole_seconds: false,
                    holders: Vec::new(),
                },
            )),
            "hold" => {
                let holder = match row.int("pid")? {
                    Some(pid) => Holder::Session(pid),
                    None => Holder::Prepared(row.string("gid").map(|gid| Prepared {
                        gid,
                        owner: row.string("owner"),
                        database: row.string("prepared_database"),
                        since: None,
                    })),
                };
                let holder = match (holder, row.time("prepared")?) {
                    (Holder::Prepared(Some(p)), since) => Holder::Prepared(Some(Prepared { since, ..p })),
                    (h, _) => h,
                };
                holds.push((key, holder, mode));
            }
            other => return Err(malformed("locks", format!("role {other:?}"))),
        }
    }
    let read = waits.len() as u64;
    let waits = waits
        .into_iter()
        .map(|(key, mut w)| {
            for (_, holder, mode) in holds.iter().filter(|(k, h, _)| *k == key && *h != Holder::Session(w.waiter.id)) {
                match w.holders.iter_mut().find(|h| h.holder == *holder) {
                    Some(h) if !h.modes.contains(mode) => h.modes.push(mode.clone()),
                    Some(_) => {}
                    None => w.holders.push(LockHolder {
                        holder: holder.clone(),
                        modes: vec![mode.clone()],
                        waiting: false,
                        blame: Blame::Holds,
                        explicit: false,
                    }),
                }
            }
            w
        })
        .collect();
    Ok(LockList { time, waits, more: total.saturating_sub(read), blocks: false })
}

/// The rows of a tick's statements (the counters, then the sessions, the locks and the server
/// when wanted), with the monitor's own traffic as it stood when the counters were read.
pub fn tick(want: &Want, rows: &[Rows], own: Own) -> Result<Snapshot, DecodeError> {
    let expected = 1 + usize::from(want.sessions.is_some()) + usize::from(want.locks) + usize::from(want.server);
    if rows.len() != expected {
        return Err(malformed("tick", format!("{} statements, expected {expected}", rows.len())));
    }
    let (sample, gauges) = counters(&rows[0], own)?;
    let mut next = 1;
    let (sessions, hidden) = match want.sessions {
        Some(wanted) => {
            next += 1;
            let (list, hidden) = sessions(&rows[1], wanted)?;
            (Some(list), hidden)
        }
        None => (None, Vec::new()),
    };
    let locks = match want.locks {
        true => {
            next += 1;
            Some(locks(&rows[next - 1], sample.time)?)
        }
        false => None,
    };
    let server = if want.server { Some(server_info(&rows[next])?) } else { None };
    Ok(Snapshot { sample, gauges, sessions, locks, server, hidden })
}

/// The row read for the whole text of the statement `ask` names (none: no session has its pid
/// now). The text is that statement's only when the session started when the list said (a pid
/// is reused) and so did the statement, to the microsecond; then it must also begin as the list
/// showed it.
pub fn query_text(ask: &TextAsk, rows: &Rows) -> Result<FullText, DecodeError> {
    let row = match rows.as_slice() {
        [] => return Ok(FullText::Gone),
        [row] => Row::new(row, "query text", QUERY_TEXT)?,
        _ => return Err(malformed("query text", format!("{} rows, expected at most 1", rows.len()))),
    };
    if row.required_int("pid")? != ask.session.id {
        return Err(malformed("query text", "another session's row"));
    }
    let query = row.text("query");
    let started = row.time("backend_start")?;
    // Without `pg_read_all_stats`, another account's session shows neither its start nor its
    // statement.
    if started.is_none() && query == Some(HIDDEN_QUERY) {
        return Ok(FullText::Unknown(Some(Need::Grant("pg_read_all_stats"))));
    }
    match (ask.session.started, started) {
        (Some(asked), Some(now)) if asked != now => return Ok(FullText::Gone),
        (Some(_), Some(_)) => {}
        _ => return Ok(FullText::Unknown(None)),
    }
    // `track_activities` off for the session now: its statement and start are not told.
    let untracked = row.text("state") == Some("disabled");
    let (Some(asked), Some(now), false) = (ask.query_started, row.time("query_start")?, untracked) else {
        return Ok(FullText::Unknown(None));
    };
    // An idle session still shows its last statement: the one the list showed, while it
    // started then.
    match (Some(now), query) {
        (Some(now), Some(text)) if now == asked && text.starts_with(&ask.start) => Ok(FullText::Text {
            text: QueryText {
                text: text.to_owned(),
                shortened: false,
                maybe_cut_by_server: row.bool("maybe_cut")?.unwrap_or(false),
            },
            proof: Proof::Same,
        }),
        _ => Ok(FullText::Moved),
    }
}
