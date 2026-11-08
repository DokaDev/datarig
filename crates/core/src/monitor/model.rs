//! The monitor's model: one shape for every database, with what an engine does not have or the
//! account may not see said as such.

/// A moment on the server's clock, in microseconds since the Unix epoch. Every age and rate is
/// worked out on this clock, read in the same statement as what it measures, so the app's own
/// clock (and its skew) never enters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// Microseconds from `earlier` to this moment, `None` when `earlier` is later.
    pub fn since(self, earlier: Timestamp) -> Option<i64> {
        self.0.checked_sub(earlier.0).filter(|d| *d >= 0)
    }
}

/// What a server is, read when the monitor connects and when the user asks for a fresh read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerInfo {
    /// The server's own version line (`version()`, `VERSION()` and `@@version_comment`).
    pub version: String,
    /// When the server started.
    pub started: Option<Timestamp>,
    /// The server's clock when this was read.
    pub read_at: Timestamp,
    /// A standby or replica that takes no writes: PostgreSQL in recovery; MySQL
    /// `read_only` (or `super_read_only`) on.
    pub read_only: bool,
    /// The most client connections the server takes.
    pub max_connections: Option<u64>,
    /// The size of the server's page cache in bytes (PostgreSQL `shared_buffers`, MySQL
    /// `innodb_buffer_pool_size`).
    pub cache_bytes: Option<u64>,
    /// Engine facts shown as they are, in this order.
    pub extra: Vec<Extra>,
}

/// An engine-only value, shown with its engine's own name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extra {
    pub key: ExtraKey,
    pub value: String,
}

/// The engine names of [`Extra`] values. The UI shows the name as it is (it is the server's
/// word, not prose).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExtraKey {
    /// PostgreSQL `track_io_timing`.
    TrackIoTiming,
    /// MySQL `STATE`: what the thread is doing, in the server's words.
    State,
    /// MySQL `COMMAND`.
    Command,
    /// MySQL `INNODB_TRX.trx_rows_locked`.
    RowsLocked,
    /// MySQL `INNODB_TRX.trx_rows_modified`.
    RowsModified,
    /// MySQL: the session holds locks past its statements outside a transaction (`LOCK
    /// TABLES`, `GET_LOCK`), from `performance_schema.metadata_locks`.
    ExplicitLocks,
    /// MySQL `@@server_id`.
    ServerId,
    /// MySQL `@@gtid_mode`.
    GtidMode,
    /// MySQL `@@log_bin`.
    LogBin,
    /// MySQL `@@super_read_only`.
    SuperReadOnly,
    /// MySQL `@@performance_schema`.
    PerformanceSchema,
}

impl ExtraKey {
    /// The engine's name for the value.
    pub fn name(self) -> &'static str {
        match self {
            ExtraKey::TrackIoTiming => "track_io_timing",
            ExtraKey::State => "STATE",
            ExtraKey::Command => "COMMAND",
            ExtraKey::RowsLocked => "trx_rows_locked",
            ExtraKey::RowsModified => "trx_rows_modified",
            ExtraKey::ExplicitLocks => "LOCK_DURATION = EXPLICIT",
            ExtraKey::ServerId => "server_id",
            ExtraKey::GtidMode => "gtid_mode",
            ExtraKey::LogBin => "log_bin",
            ExtraKey::SuperReadOnly => "super_read_only",
            ExtraKey::PerformanceSchema => "performance_schema",
        }
    }
}

/// A cumulative counter of the server. Each kind counts one thing only; the UI names a kind by
/// what it counts, with the engine's own names in the detail line ([`CounterKind::source`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CounterKind {
    /// Transactions committed, the monitor's own taken out ([`Own::transactions`]). PostgreSQL
    /// `xact_commit`.
    Commits,
    /// Transactions rolled back. PostgreSQL `xact_rollback`.
    Rollbacks,
    /// Explicit `COMMIT` statements: an autocommit statement is not one. MySQL `Com_commit`.
    ExplicitCommits,
    /// Explicit `ROLLBACK` statements. MySQL `Com_rollback`.
    ExplicitRollbacks,
    /// Statements clients sent, the monitor's own taken out ([`Own::statements`]). MySQL
    /// `Questions`.
    Statements,
    /// Rows the server read. PostgreSQL `tup_returned`; MySQL `Innodb_rows_read`.
    RowsRead,
    /// Rows inserted, updated and deleted. PostgreSQL `tup_inserted + tup_updated +
    /// tup_deleted`; MySQL `Innodb_rows_inserted + _updated + _deleted`.
    RowsWritten,
    /// Pages asked of the page cache, found there or not. PostgreSQL `blks_hit + blks_read`;
    /// MySQL `Innodb_buffer_pool_read_requests`.
    PageRequests,
    /// Pages the page cache did not have and read from storage (or the OS cache). PostgreSQL
    /// `blks_read`; MySQL `Innodb_buffer_pool_reads`.
    PageReads,
    /// Bytes received from clients. MySQL `Bytes_received`.
    BytesIn,
    /// Bytes sent to clients. MySQL `Bytes_sent`.
    BytesOut,
    /// Deadlocks detected. PostgreSQL `deadlocks`.
    Deadlocks,
    /// Row lock waits. MySQL `Innodb_row_lock_waits`.
    RowLockWaits,
    /// Bytes written to temporary files. PostgreSQL `temp_bytes`.
    TempBytes,
    /// Temporary tables written to disk. MySQL `Created_tmp_disk_tables`.
    TempDiskTables,
    /// Statements over the slow query time. MySQL `Slow_queries`.
    SlowQueries,
    /// Connection attempts that failed. MySQL `Aborted_connects`.
    AbortedConnects,
}

impl CounterKind {
    /// The engine's own name of what the kind counts, for the detail line.
    pub fn source(self, mysql: bool) -> &'static str {
        match (self, mysql) {
            (CounterKind::Commits, _) => "xact_commit",
            (CounterKind::Rollbacks, _) => "xact_rollback",
            (CounterKind::ExplicitCommits, _) => "Com_commit",
            (CounterKind::ExplicitRollbacks, _) => "Com_rollback",
            (CounterKind::Statements, _) => "Questions",
            (CounterKind::RowsRead, false) => "tup_returned",
            (CounterKind::RowsRead, true) => "Innodb_rows_read",
            (CounterKind::RowsWritten, false) => "tup_inserted + tup_updated + tup_deleted",
            (CounterKind::RowsWritten, true) => "Innodb_rows_inserted + Innodb_rows_updated + Innodb_rows_deleted",
            (CounterKind::PageRequests, false) => "blks_hit + blks_read",
            (CounterKind::PageRequests, true) => "Innodb_buffer_pool_read_requests",
            (CounterKind::PageReads, false) => "blks_read",
            (CounterKind::PageReads, true) => "Innodb_buffer_pool_reads",
            (CounterKind::BytesIn, _) => "Bytes_received",
            (CounterKind::BytesOut, _) => "Bytes_sent",
            (CounterKind::Deadlocks, _) => "deadlocks",
            (CounterKind::RowLockWaits, _) => "Innodb_row_lock_waits",
            (CounterKind::TempBytes, _) => "temp_bytes",
            (CounterKind::TempDiskTables, _) => "Created_tmp_disk_tables",
            (CounterKind::SlowQueries, _) => "Slow_queries",
            (CounterKind::AbortedConnects, _) => "Aborted_connects",
        }
    }
}

/// One counter's cumulative value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counter {
    pub kind: CounterKind,
    pub value: u64,
}

/// What tells two samples' counters apart as the same run of the counters or not: across a
/// change the counters started again (a restart, a reset, a database dropped), and no rate is
/// worked out across it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Epoch {
    /// Equal marks are the same run. PostgreSQL: the latest `stats_reset` of
    /// `pg_stat_database` and the number of its rows (a database dropped takes its counts out
    /// of the sums).
    Mark(i64, i64),
    /// The server's uptime in seconds: the same run while it does not go down (MySQL
    /// `Uptime`).
    Uptime(u64),
}

impl Epoch {
    /// Whether the counters of a sample with this epoch continue those of one with `before`.
    pub fn continues(self, before: Epoch) -> bool {
        match (before, self) {
            (Epoch::Mark(a, b), Epoch::Mark(c, d)) => a == c && b == d,
            (Epoch::Uptime(a), Epoch::Uptime(b)) => b >= a,
            _ => false,
        }
    }
}

/// The monitor connection's own traffic, counted by the driver as it sends it, cumulative for
/// the life of the connection, as it stood when the counters were read. Taken out of the
/// counters that count it, so a quiet server shows no load of ours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Own {
    /// Statements the server counted in [`CounterKind::Statements`] when the counters were
    /// read (MySQL counts the reading statement itself).
    pub statements: u64,
    /// Transactions committed before the counters were read ([`CounterKind::Commits`]).
    pub transactions: u64,
}

/// The counters of one tick, on the server's clock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    pub time: Timestamp,
    pub epoch: Epoch,
    pub counters: Vec<Counter>,
    pub own: Own,
}

impl Sample {
    /// The value of a counter, `None` when the format does not have it.
    pub fn get(&self, kind: CounterKind) -> Option<u64> {
        self.counters.iter().find(|c| c.kind == kind).map(|c| c.value)
    }
}

/// A value as it stands now (not cumulative).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GaugeKind {
    /// Client sessions connected that the account can tell are clients (PostgreSQL client
    /// backends it may see, the [`GaugeKind::Hidden`] ones not counted; MySQL
    /// `Threads_connected`).
    Sessions,
    /// Sessions running a statement, the monitor's own left out (PostgreSQL `active`; MySQL
    /// `Threads_running` less one).
    Active,
    /// Sessions idle with a transaction open (PostgreSQL `idle in transaction`, aborted or
    /// not).
    IdleInTx,
    /// Sessions idle (PostgreSQL `idle`).
    Idle,
    /// Sessions waiting for a lock (PostgreSQL `wait_event_type = 'Lock'`).
    WaitingForLock,
    /// Sessions and server processes whose kind and state the account cannot see (PostgreSQL
    /// rows of other roles without `pg_read_all_stats`).
    Hidden,
}

/// One gauge's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gauge {
    pub kind: GaugeKind,
    pub value: u64,
}

/// A server session as the monitor tells it apart: an id and, where the server reuses ids,
/// when the session started (PostgreSQL pid and `backend_start`; MySQL connection ids are not
/// reused while the server runs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub id: i64,
    pub started: Option<Timestamp>,
}

/// What a session is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionKind {
    /// A client's connection.
    Client,
    /// One of the server's own processes or threads, with the server's name for it.
    Background(String),
    /// A connection streaming changes to a replica.
    Replication,
    /// A parallel worker of the session `leader` (PostgreSQL; `None` before version 13, which
    /// does not say).
    ParallelWorker { leader: Option<i64> },
    /// Not known: the account may not see it (PostgreSQL hides `backend_type` with the rest).
    Unknown,
}

/// What a session is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// Running a statement.
    Active,
    /// Waiting for the client, no transaction open (or none known: see [`Tx`]).
    Idle,
    /// Waiting for the client inside a transaction.
    IdleInTx,
    /// Waiting for the client inside a transaction that failed and must be rolled back.
    IdleInTxAborted,
    /// One of the server's own processes, which serves no client and so has no client state.
    Background,
    /// The account may not see it ([`Visibility::Hidden`]).
    Unknown,
    /// Another state, in the server's words.
    Other(String),
}

/// A session's transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tx {
    /// None open.
    None,
    /// Open since then.
    Since(Timestamp),
    /// Open, since a time the server does not say (MySQL: a transaction holding metadata locks
    /// with no InnoDB transaction, such as one that read only MyISAM tables).
    Open,
    /// Not known: the account may not see it, or the format did not read it (MySQL without
    /// `INNODB_TRX`).
    Unknown,
}

/// What a waiting session waits for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wait {
    pub class: WaitClass,
    /// The server's name for the wait (PostgreSQL `wait_event`; MySQL a short name).
    pub name: String,
}

/// The kind of a wait. Only [`WaitClass::Lock`] is a wait for another session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitClass {
    /// A lock another session holds (PostgreSQL `Lock`; MySQL a metadata or row lock).
    Lock,
    /// A short internal lock (PostgreSQL `LWLock`).
    LightweightLock,
    /// Storage.
    Io,
    /// The client.
    Client,
    /// Another process of the server.
    Ipc,
    /// A timer (`pg_sleep`, a delay).
    Timeout,
    /// A background process with nothing to do.
    Activity,
    /// Another class, in the server's words.
    Other(String),
}

/// A statement's text as the list holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryText {
    pub text: String,
    /// The list holds only its start; the whole of it is read when asked for (by the
    /// session's key and statement).
    pub shortened: bool,
    /// The server may have cut it where it keeps it (PostgreSQL `track_activity_query_size`):
    /// what it holds may not be the whole statement, so it is never explained.
    pub maybe_cut_by_server: bool,
}

/// The statement a row of the list showed, to read its whole text
/// ([`crate::driver::DbCommand::MonitorQueryText`]): the session, when that statement started,
/// and its text as the list holds it. The text read is shown only when it is still that
/// statement's ([`FullText`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextAsk {
    pub session: SessionKey,
    /// When the statement started, as the list read it (`None`: the list did not tell).
    pub query_started: Option<Timestamp>,
    /// The text as the list holds it: its first [`QUERY_CHARS`] characters.
    pub start: String,
    /// The server's clock when the list was read ([`SessionList::time`]).
    pub seen: Timestamp,
}

/// How long a MySQL statement must have run when the list read it for its whole text to be
/// told apart from a later one's: MySQL tells a start in whole seconds only.
pub const TEXT_MIN_AGE_US: i64 = 5_000_000;

/// How far two starts worked out from MySQL's whole seconds may lie apart and still be one
/// statement's.
pub const TEXT_SAME_START_US: i64 = 2_000_000;

impl TextAsk {
    /// Whether `later` (built from a later read of the list) names the statement this one does:
    /// the same session, beginning and start (`whole_seconds`: a start worked out from whole
    /// seconds, MySQL, within [`TEXT_SAME_START_US`]).
    pub fn same_statement(&self, later: &TextAsk, whole_seconds: bool) -> bool {
        let start = match (self.query_started, later.query_started) {
            (Some(a), Some(b)) if whole_seconds => (a.0 - b.0).abs() < TEXT_SAME_START_US,
            (a, b) => a == b,
        };
        self.session == later.session && self.start == later.start && start
    }
}

/// The whole text of the statement a [`TextAsk`] names, or why it is not shown. Another
/// statement's text is never given as this one's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FullText {
    /// The statement's whole text (as the server keeps it: it may still be cut by the server).
    Text { text: QueryText, proof: Proof },
    /// The session has ended (its id is free, or another session has it now).
    Gone,
    /// The session runs another statement now, or none.
    Moved,
    /// What tells the statement apart cannot be read: the text is not shown. `Some`: what
    /// would show it.
    Unknown(Option<Need>),
    /// The statement had run too briefly when the list read it to be told apart from a later
    /// run of the same text (MySQL, under [`TEXT_MIN_AGE_US`]): asked again once a later list
    /// shows it older.
    NotYet,
}

/// How sure [`FullText::Text`] is to be the statement the list showed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proof {
    /// The same session and the same start of the statement, to the microsecond (PostgreSQL).
    Same,
    /// The same session, a start within the same second and the same beginning (MySQL, which
    /// tells the start in whole seconds): a new run of the same text started in that second
    /// cannot be told apart.
    Likely,
}

/// What a waiting session waits for, as the server tells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocker {
    /// This session (PostgreSQL `pg_blocking_pids`).
    Session(i64),
    /// A prepared transaction, which has no session (PostgreSQL pid 0).
    PreparedTransaction,
}

/// Whether the account sees a session in full.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Full,
    /// Its state, statement and times are hidden from the account ([`Hidden`] says what would
    /// show them).
    Hidden,
}

/// One session of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRow {
    pub key: SessionKey,
    pub kind: SessionKind,
    pub user: Option<String>,
    pub database: Option<String>,
    pub application: Option<String>,
    /// The client's address (and port where the server says), `None` for none (a local socket,
    /// a background process) or hidden ([`SessionRow::visible`]).
    pub client: Option<String>,
    pub state: SessionState,
    pub wait: Option<Wait>,
    pub tx: Tx,
    /// When the running (or, for an idle session, the last) statement started.
    pub query_started: Option<Timestamp>,
    /// When the session entered its state.
    pub state_since: Option<Timestamp>,
    pub query: Option<QueryText>,
    /// What it waits for, `None` when not asked: it does not wait for a lock, or it is past
    /// the most waiters a tick asks about (the longest waiters are asked first).
    pub blocked_by: Option<Vec<Blocker>>,
    /// The monitor's own connection: listed dim, with no actions.
    pub monitor: bool,
    pub visible: Visibility,
    pub extra: Vec<Extra>,
}

impl SessionRow {
    /// Whether the session waits for a lock another session holds.
    pub fn waits_for_lock(&self) -> bool {
        self.wait.as_ref().is_some_and(|w| w.class == WaitClass::Lock)
    }
}

/// The sessions a tick read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionList {
    /// The server's clock when they were read: every age is worked out on it.
    pub time: Timestamp,
    pub rows: Vec<SessionRow>,
    /// Idle sessions were left out of [`SessionList::rows`] (counted by [`GaugeKind::Idle`]
    /// when the format has it).
    pub idle_left_out: bool,
}

/// Something a view cannot show, and what would show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hidden {
    pub part: Part,
    pub need: Need,
}

/// The part of a view that is hidden or incomplete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The state, statement and times of other accounts' sessions (they are listed).
    OtherSessionDetails,
    /// Other accounts' sessions (they are not listed at all).
    OtherSessions,
    /// Whether an idle session holds a transaction open.
    TransactionState,
    /// Sessions the server could not track (MySQL `performance_schema` ran out of thread
    /// slots): the list may miss some.
    UntrackedSessions,
}

/// What would show a [`Hidden`] part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Need {
    /// A privilege or role, by its server name (shown as it is: `pg_read_all_stats`,
    /// `PROCESS`).
    Grant(&'static str),
    /// A server setting raised, by its name (`performance_schema_max_thread_instances`).
    Setting(&'static str),
}

/// What a monitor tick reads ([`crate::driver::DbCommand::MonitorTick`]): the counters always,
/// the rest when the view showing them is visible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Want {
    /// The session list, and which of them.
    pub sessions: Option<SessionsWanted>,
    /// What the server is ([`ServerInfo`]).
    pub server: bool,
    /// The longest each statement may run on the server; the driver gives the whole tick a
    /// little more before it gives up on the connection.
    pub limit: std::time::Duration,
}

/// What one tick read ([`crate::driver::DbEvent::MonitorSnapshot`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub sample: Sample,
    pub gauges: Vec<Gauge>,
    /// `None`: not asked for.
    pub sessions: Option<SessionList>,
    /// `None`: not asked for.
    pub server: Option<ServerInfo>,
    /// What the account could not see of what was read.
    pub hidden: Vec<Hidden>,
}

/// The longest statement text the session list holds, in characters, in every format; the
/// whole text of one session is read when asked for.
pub const QUERY_CHARS: usize = 256;

/// The most waiting sessions a tick asks what they wait for (the longest waits first): asking
/// reads the whole lock table under its partition locks, so the cost is bounded on a server with
/// many waiters.
pub const MAX_WAITERS: usize = 50;

/// What the session list should hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionsWanted {
    /// Idle sessions too. Without, the server leaves them out (they are counted all the same),
    /// except where it cannot tell an idle session holding a transaction open from one that
    /// does not (MySQL without `INNODB_TRX`): those are listed.
    pub idle: bool,
}
