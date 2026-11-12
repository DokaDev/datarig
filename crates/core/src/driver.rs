//! Driver abstraction: a small base interface plus a capability flag set. The UI only talks to a [`Session`] through [`DbCommand`] /
//! [`DbEvent`] channels and never sees driver types. Implementations live in their own crates
//! (`datarig-driver-postgres`); the front end maps a profile's `driver` name to one of them.

pub mod ddl;
pub mod keys;
mod protocol;
pub mod structure;

pub use keys::{KeyCatalog, KeyMarks};
pub use protocol::{
    AppliedStep, ApplyError, ApplyFailure, ApplyTx, ArrayElement, COUNT_LOCK_WAIT, CancelFailure, Cell, ColumnMeta,
    ColumnOrigin, DbCommand, DbError, DbEvent, ESTIMATE_LIMIT, EXPORT_BATCH_BYTES, ExportBatch, ExportGuard,
    ExportRead, ExportSink, GROUP_ROW_OVERHEAD, GroupBudget, GroupCounts, GroupValue, Isolation, Outcome, OwnSession,
    PagingMode, ReadIn, SchemaObjects, ServerEnd, ServerMessage, SessionValues, StatementInfo, TxOutcome, TxRule,
    TxStep, ValueKind,
};

use crate::profile::ConnectionConfig;
use crate::sql::dialect::Language;
use crate::transport::DialerRef;
use futures::future::BoxFuture;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedSender;

/// Optional capabilities a driver may provide. The UI checks these instead of the driver type.
#[derive(Clone, Copy, Debug)]
pub struct Capabilities {
    /// `ResultStream` + `RowLimiter`: results are fetched page by page on the server side.
    pub server_paging: bool,
    /// Protocol-level cancellation of a running statement.
    pub cancel: bool,
    /// `SchemaIntrospector`: schemas / relations / columns for the tree and completion.
    pub introspection: bool,
    /// Key metadata: the metadata session reports the primary, foreign and unique keys of the
    /// tables (`DbEvent::Keys`), and results name the table column each column comes from
    /// (`ColumnMeta::origin`), so the UI can mark key columns without asking per query.
    pub key_metadata: bool,
    /// Contexts: the metadata session lists the server's databases
    /// (`DbCommand::LoadDatabases`), and a session opens in a database and schema of the
    /// caller's choice ([`ConnectOptions::context`]) and says which ones it got
    /// ([`DbEvent::Context`]). The UI names them; how they are applied is the driver's.
    pub contexts: bool,
    /// Table structure: the metadata session reads one table's columns, keys, indexes,
    /// constraints, triggers and size estimate when asked (`DbCommand::LoadStructure`, answered
    /// with [`DbEvent::Structure`]); the explorer shows them under the table's node. Without
    /// it an open table shows its columns from the completion catalog.
    pub structure: bool,
    /// DDL: the metadata session reads what an object's `CREATE` statement needs from the
    /// catalog when asked (`DbCommand::LoadDdl`, answered with [`DbEvent::Ddl`]), never
    /// waiting for a lock; the UI shows it as SQL in a read-only tab. PostgreSQL answers with
    /// the catalog's parts (`ddl::DdlSource`), which `sql::ddl` writes out; MySQL with the
    /// server's own text (`DdlSource::Verbatim`).
    pub ddl: bool,
    /// The editor language of the driver's sessions: the SQL tools (lexing, quoting, the risk
    /// classifier) pick their rules from it.
    pub language: Language,
    /// How the server's namespaces nest. Nothing reads it yet: the explorer and the context
    /// picker show databases holding schemas, as PostgreSQL has them.
    pub hierarchy: Hierarchy,
    /// The plan the driver's sessions can produce for a statement, `None` for none: the plan
    /// view (`query.explain`, `results.view_as_plan`) is offered only with one. A session may
    /// say otherwise for its server once connected ([`DbEvent::Explain`]).
    pub explain: Option<ExplainFormat>,
    /// A result held open (`PagingMode::Hold`, or read inside the user's transaction block) keeps
    /// its statement running on the server, and with it the locks the statement took, until it
    /// is read to its end (MySQL has no server-side cursor of its own): the app reads such a
    /// result to its end at once, a page as soon as the one before came (into its own store, up
    /// to its spill limit), instead of a page whenever the user moves to the next one.
    pub reads_held_to_end: bool,
    /// Exports: the query session reads every row of a statement into a bounded channel
    /// ([`DbCommand::Export`]) for the app to write to a file.
    pub export: bool,
    /// Manual commit: the query session opens a transaction for a run's writes and keeps it open
    /// for the user ([`TxRule::Manual`]), and ends one when asked ([`DbCommand::EndTx`]). A driver
    /// without it answers both with [`DbError::NotSupported`] and runs nothing.
    pub manual_commit: bool,
    /// Editing a result's rows: the SQL its sessions take for a change set ([`EditFormat`]),
    /// `None` for none (every result is read-only). A session may say otherwise for its server
    /// once connected ([`DbEvent::Edit`]).
    pub edit: Option<EditFormat>,
    /// Server monitoring: the driver opens [`SessionRole::Monitor`] sessions, which answer
    /// [`DbCommand::MonitorTick`], [`DbCommand::MonitorQueryText`] and [`DbCommand::MonitorReplication`]; what the server offers comes after connecting
    /// ([`DbEvent::Monitor`]).
    pub monitor: bool,
}

/// How a server's namespaces nest (`Capabilities::hierarchy`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hierarchy {
    /// Databases that hold schemas, a session in one database at a time (PostgreSQL).
    DatabaseSchema,
    /// One level: a database is the schema, and a session moves between them (MySQL's
    /// `USE`).
    SchemaOnly,
}

/// The format of the plan a driver produces for a statement (`Capabilities::explain`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplainFormat {
    /// PostgreSQL's `EXPLAIN (FORMAT JSON)`: a list holding one object whose `Plan` is the tree
    /// of nodes, read by `sql::plan::pg`, asked with [`ExplainFormat::explain_sql`]. Another
    /// database's JSON plan (MySQL's `FORMAT=JSON`) is shaped otherwise and would be a format of
    /// its own.
    PostgresJson,
    /// MySQL's tree text: `EXPLAIN FORMAT=TREE` and `EXPLAIN ANALYZE` (8.0.18 on), read by
    /// `sql::plan::mysql`. Each format a server may give (MySQL's JSON, MariaDB's) is a variant
    /// of its own, read by a reader of its own: see `sql::plan` for the seam.
    MySqlTree,
}

/// The SQL a driver's sessions take for edits of a result's rows (`Capabilities::edit`): how a
/// row is found and compared, and how it is locked, changed, added and removed. The statements
/// are written by `edit` in this crate, by one writer per format; a server that differs from
/// its family's (another version, a fork) is a format, or a flag of one, of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditFormat {
    /// PostgreSQL.
    Postgres,
    /// MySQL.
    MySql(MySqlEdit),
}

/// What a MySQL server offers for edits beyond the family's ([`EditFormat::MySql`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MySqlEdit {
    /// `FOR UPDATE NOWAIT` (8.0 on): a row another session holds fails at once instead of
    /// waiting for it.
    pub nowait: bool,
}

/// Where a session works: a database and a schema of the profile's server,
/// `None` for the profile's own (its database, the server's search path). PostgreSQL: a
/// database is a connection of its own; the schema is the session's whole search path (behind
/// the implicit `pg_catalog`). A later MySQL driver: `USE db` (a schema is a database there).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SessionContext {
    pub database: Option<String>,
    pub schema: Option<String>,
}

impl SessionContext {
    /// The profile's defaults.
    pub fn is_default(&self) -> bool {
        self.database.is_none() && self.schema.is_none()
    }
}

/// What a session is for. Each session is one server connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionRole {
    /// Explorer tree and completion catalog: `LoadSchemas`, `LoadObjects`, `LoadCatalog`. One per
    /// connected profile, shared by all of its tabs. It answers `Execute` with `Failed`.
    Meta,
    /// User statements of one tab: `Execute`, `FetchMore`, cancel. One per tab.
    Query,
    /// The server monitor of a profile: its `Monitor*` reads and actions only, every read in a read-only
    /// transaction of its own with short limits, so it never disturbs or waits on the other
    /// sessions, and nothing the user typed is ever sent on it. It starts with a probe of what
    /// the server offers ([`DbEvent::Monitor`]); no start-up statements, no session settings.
    Monitor,
}

impl SessionRole {
    /// Short tag used in the server-side `application_name`.
    pub fn tag(self) -> &'static str {
        match self {
            SessionRole::Meta => "meta",
            SessionRole::Query => "q",
            SessionRole::Monitor => "mon",
        }
    }
}

/// Per-session connection options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectOptions {
    /// Rows per page of a result (`FetchMore` asks for the next page).
    pub page_size: usize,
    /// What the server shows in `pg_stat_activity.application_name` (or the driver's
    /// equivalent).
    pub application_name: String,
    /// The profile's policy is read-only: the server must refuse every write
    /// of the session. PostgreSQL: every transaction the query session opens is `READ ONLY`
    /// (its own `BEGIN READ ONLY`, or `SET TRANSACTION READ ONLY` first in the user's block),
    /// and the session starts with `default_transaction_read_only = on` as a second layer;
    /// when the server ignores that (a pooler), [`DbEvent::ReadOnlyPerTransaction`] follows
    /// `Connected`.
    pub read_only: bool,
    /// The database and schema the session works in (`Capabilities::contexts`).
    pub context: SessionContext,
    /// Keep prepared statements between transactions (the profile's "server-side statement
    /// cache", on by default). Off, no statement outlives the transaction it runs in, which a
    /// pooler in transaction mode without prepared statement support needs; the driver also
    /// turns it off by itself when the server keeps losing them ([`DbEvent::StatementCacheOff`]).
    pub statement_cache: bool,
    /// Reach the server through this transport (an SSH tunnel) instead of
    /// connecting to it directly: the session's connection and its cancel requests each dial
    /// the profile's host and port through it.
    pub dialer: Option<DialerRef>,
    /// The query session's lock wait limit: how long a statement waits for a lock another
    /// session holds before the server cancels it ([`DbError::LockTimeout`]); `None` keeps the
    /// server's setting. PostgreSQL: `lock_timeout`, for the session on a direct connection and
    /// `SET LOCAL` in each transaction behind a pooler (nothing stays on a pooled server
    /// connection); MySQL: `lock_wait_timeout` and `innodb_lock_wait_timeout` for the session.
    /// Metadata sessions keep their own, shorter limit.
    pub lock_wait: Option<std::time::Duration>,
    /// The server's own idle limit for the session, a safety net behind the app's idle
    /// disconnect: past it the server ends the session ([`DbError::IdleSessionTimeout`]).
    /// PostgreSQL: `idle_session_timeout`, only from version 14 on and only on a direct
    /// connection (behind a pooler it would end server connections other clients share);
    /// MySQL: `wait_timeout`. `None` keeps the server's setting.
    pub server_idle: Option<std::time::Duration>,
    /// What a query session starts with after the limits (its profile's session preset and own
    /// statements, `profile::session::plan`), each statement alone and in order; any failure
    /// fails the connect ([`DbError::SessionSql`]), so `Connected` means every one succeeded.
    /// Metadata sessions never get them (the drivers also leave them out there). PostgreSQL
    /// behind a pooler (a connection not known to be direct): each statement's
    /// [`SessionStatement::per_transaction`] goes first in every transaction instead (nothing
    /// stays on a pooled server connection), one without such a form is not sent, and
    /// [`DbEvent::SessionPerTransaction`] comes before `Connected`.
    pub session_sql: Vec<SessionStatement>,
}

/// A statement a query session starts with ([`ConnectOptions::session_sql`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionStatement {
    /// As written, sent once for the session.
    pub sql: String,
    /// PostgreSQL: the same setting for one transaction (`SET LOCAL …`), what a session behind
    /// a pooler sends first in each transaction instead; `None`: there is none (it is not sent
    /// there), or a driver that does not need one.
    pub per_transaction: Option<String>,
}

impl ConnectOptions {
    /// `datarig-<role>-<instance>`: the role tag and an instance tag (the process id in the
    /// binary, a unique tag per test), so connections can be told apart on the server.
    pub fn new(page_size: usize, role: SessionRole, instance: &str) -> Self {
        Self {
            page_size,
            application_name: format!("datarig-{}-{instance}", role.tag()),
            read_only: false,
            context: SessionContext::default(),
            statement_cache: true,
            dialer: None,
            lock_wait: None,
            server_idle: None,
            session_sql: Vec::new(),
        }
    }

    /// The same options, with the statements a query session starts with.
    pub fn session_sql(self, session_sql: Vec<SessionStatement>) -> Self {
        Self { session_sql, ..self }
    }

    /// The same options, with lock wait limit `lock_wait` (or the server's).
    pub fn lock_wait(self, lock_wait: Option<std::time::Duration>) -> Self {
        Self { lock_wait, ..self }
    }

    /// The same options, with the server's idle limit `server_idle` (or the server's own).
    pub fn server_idle(self, server_idle: Option<std::time::Duration>) -> Self {
        Self { server_idle, ..self }
    }

    /// The same options, read-only or not.
    pub fn read_only(self, read_only: bool) -> Self {
        Self { read_only, ..self }
    }

    /// The same options, in context `context`.
    pub fn context(self, context: SessionContext) -> Self {
        Self { context, ..self }
    }

    /// The same options, through `dialer` (or directly).
    pub fn dialer(self, dialer: Option<DialerRef>) -> Self {
        Self { dialer, ..self }
    }

    /// The same options, with the statement cache on or off.
    pub fn statement_cache(self, statement_cache: bool) -> Self {
        Self { statement_cache, ..self }
    }
}

pub trait Driver: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    /// Start opening one connection for `role` in the background. Progress and results arrive
    /// as [`DbEvent`]s: `Connected` or `ConnectFailed` first. Commands sent before `Connected`
    /// wait for the connection.
    fn connect(
        &self,
        cfg: &ConnectionConfig,
        role: SessionRole,
        opts: ConnectOptions,
        events: UnboundedSender<DbEvent>,
    ) -> Session;
    /// "Test connection": open a throwaway connection, ask for the
    /// server version and close it, through `dialer` when there is one. Gives up after
    /// `timeout`. Dropping or aborting the future cancels the attempt.
    fn ping(
        &self,
        cfg: &ConnectionConfig,
        timeout: Duration,
        dialer: Option<DialerRef>,
    ) -> BoxFuture<'static, Result<PingInfo, PingError>>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PingInfo {
    pub server_version: String,
    pub latency: Duration,
    /// How the test connection's traffic went.
    pub encryption: crate::tls::Encryption,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PingError {
    Timeout(Duration),
    Failed(DbError),
}

/// Cancels run number `run` of the session (runs are numbered as they are sent, from 1): the
/// one running now, or one that has not started yet, which then never runs.
pub trait Canceller: Send + Sync {
    fn cancel(&self, run: u64);
}

/// Which run of a session the user asked to cancel, shared by its canceller and its loop. A
/// cancel belongs to the run it was pressed for: asked while that run is still waiting, it
/// stops that run when it starts; asked late, it stops nothing that comes after. A run's number
/// is its place among the runs of the session, and not the `id` of its command, which a count
/// asked again or the next chunk of an export repeats.
#[derive(Debug, Default)]
pub struct CancelMark {
    marks: Mutex<Marks>,
    /// A cancel was asked: wakes a session waiting with something queued.
    asked_now: Notify,
}

#[derive(Debug, Default)]
struct Marks {
    /// The runs asked to cancel that are not over yet.
    asked: BTreeSet<u64>,
    /// The run the session is at (0: none yet).
    current: u64,
}

impl CancelMark {
    fn marks(&self) -> MutexGuard<'_, Marks> {
        self.marks.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The user asked to cancel run number `run`. Whether it is the run the session is at, so
    /// that there is something to stop on the server.
    pub fn ask(&self, run: u64) -> bool {
        let mut m = self.marks();
        if run >= m.current {
            m.asked.insert(run);
        }
        let at_it = m.current == run;
        drop(m);
        self.asked_now.notify_one();
        at_it
    }

    /// The session starts its next run. Whether a cancel for it came first (it must not run).
    /// Cancels of earlier runs are dropped; those of later runs stay for them.
    pub fn begin(&self) -> bool {
        let mut m = self.marks();
        m.current += 1;
        let current = m.current;
        m.asked = m.asked.split_off(&current);
        m.asked.contains(&current)
    }

    /// Whether the run the session is at was asked to cancel.
    pub fn asked(&self) -> bool {
        let m = self.marks();
        m.asked.contains(&m.current)
    }

    /// Whether the cancel of run number `run` still stands and it is the run the session is at.
    pub fn stands(&self, run: u64) -> bool {
        let m = self.marks();
        m.current == run && m.asked.contains(&run)
    }

    /// Resolves once the next run, not started yet, is asked to cancel: for a session that
    /// holds that run back and would otherwise not look until its turn. One waiter at a time.
    pub async fn next_asked(&self) {
        loop {
            {
                let m = self.marks();
                if m.asked.contains(&(m.current + 1)) {
                    return;
                }
            }
            self.asked_now.notified().await;
        }
    }

    /// The run the session is at was answered cancelled: its cancel is spent, and what the
    /// session goes on with (a held result) is not taken for cancelled.
    fn spent(&self) {
        let mut m = self.marks();
        let current = m.current;
        m.asked.remove(&current);
    }
}

impl DbCommand {
    /// Whether this command starts a run; one that is part of a run, or that a cancel never
    /// stops, does not.
    pub fn starts_run(&self) -> bool {
        matches!(
            self,
            DbCommand::Execute { .. }
                | DbCommand::Resume { .. }
                | DbCommand::Count { .. }
                | DbCommand::GroupCount { .. }
                | DbCommand::Estimate { .. }
                | DbCommand::CheckRepeat { .. }
                | DbCommand::Export { .. }
                | DbCommand::ApplyEdits { .. }
        )
    }

    /// The answer of this run when it is cancelled before it starts; `None` for a command that
    /// starts no run.
    pub fn cancelled(&self) -> Option<DbEvent> {
        let error = DbError::Cancelled;
        Some(match self {
            DbCommand::Execute { id, .. } | DbCommand::Resume { id, .. } => {
                DbEvent::Failed { id: *id, error, cancelled: true }
            }
            DbCommand::Count { id, .. } => DbEvent::Counted { id: *id, result: Err(error), snapshot: false },
            DbCommand::GroupCount { id, .. } => DbEvent::GroupCounted { id: *id, result: Err(error), snapshot: false },
            DbCommand::Estimate { id, .. } => DbEvent::Estimated { id: *id, result: Err(error) },
            DbCommand::CheckRepeat { id, .. } => DbEvent::RepeatChecked { id: *id, result: Err(error) },
            DbCommand::Export { id, .. } => DbEvent::Exported { id: *id, result: Err(error), cancelled: true },
            DbCommand::ApplyEdits { id, .. } => {
                DbEvent::Applied { id: *id, result: Err(ApplyFailure { step: None, why: ApplyError::Db(error) }) }
            }
            _ => return None,
        })
    }

    /// Start this command under `mark`: `true` when it is a run that was cancelled before it
    /// started, whose one answer went to `events` (it must not run).
    pub fn cancelled_before_start(&self, mark: &CancelMark, events: &UnboundedSender<DbEvent>) -> bool {
        if !self.starts_run() || !mark.begin() {
            return false;
        }
        if let Some(answer) = self.cancelled() {
            let _ = events.send(answer);
        }
        mark.spent();
        true
    }
}

/// One connection of a given [`SessionRole`]: a command channel and a canceller.
pub struct Session {
    pub caps: Capabilities,
    pub role: SessionRole,
    tx: UnboundedSender<DbCommand>,
    canceller: Arc<dyn Canceller>,
    /// How many `LoadStructure` were sent, and how many of their answers came: a session
    /// answers its commands one by one, in order, so the n-th answer is the n-th request's.
    structures: AtomicU64,
    structures_answered: AtomicU64,
    /// How many runs were sent: the last one is what a cancel is for.
    runs: AtomicU64,
}

impl Session {
    /// Build a session from a raw channel. Drivers use this; tests use it to observe the
    /// commands the UI sends without a database.
    pub fn new(
        caps: Capabilities,
        role: SessionRole,
        tx: UnboundedSender<DbCommand>,
        canceller: Arc<dyn Canceller>,
    ) -> Self {
        Self {
            caps,
            role,
            tx,
            canceller,
            structures: AtomicU64::new(0),
            structures_answered: AtomicU64::new(0),
            runs: AtomicU64::new(0),
        }
    }

    pub fn send(&self, cmd: DbCommand) {
        if matches!(cmd, DbCommand::LoadStructure { .. }) {
            self.structures.fetch_add(1, Ordering::Relaxed);
        }
        if cmd.starts_run() {
            self.runs.fetch_add(1, Ordering::Relaxed);
        }
        // A closed channel means the connection task ended; the UI already got the reason.
        let _ = self.tx.send(cmd);
    }

    /// Cancel the last run sent, whether it runs by now or still waits its turn (the session
    /// may still be connecting). Nothing sent yet: nothing to cancel.
    pub fn cancel(&self) {
        match self.runs.load(Ordering::Relaxed) {
            0 => {}
            run => self.canceller.cancel(run),
        }
    }

    /// How many `LoadStructure` were sent on this session so far.
    pub fn structures_asked(&self) -> u64 {
        self.structures.load(Ordering::Relaxed)
    }

    /// A `DbEvent::Structure` of this session came: which request's answer it is (1 for the
    /// first `LoadStructure` sent on it).
    pub fn structure_answered(&self) -> u64 {
        self.structures_answered.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Close the session: the command channel closes, and the driver cancels a statement that
    /// is still running and closes the connection (the server rolls back an open transaction).
    /// No further events are sent for it.
    pub fn close(self) {
        drop(self);
    }
}

#[cfg(test)]
mod cancel_tests {
    use super::*;

    #[test]
    fn a_cancel_asked_before_a_run_starts_stops_that_run() {
        let mark = CancelMark::default();
        assert!(!mark.ask(1), "not started: nothing to stop on the server");
        assert!(mark.begin());
    }

    #[test]
    fn a_cancel_of_a_later_run_waits_for_it_and_an_earlier_one_is_dropped() {
        let mark = CancelMark::default();
        mark.ask(2);
        assert!(!mark.begin(), "a run before the one cancelled runs");
        assert!(!mark.asked());
        assert!(mark.begin(), "the one cancelled does not");
        assert!(!mark.begin(), "and the cancel ends with it");
        assert!(!mark.asked());
    }

    #[test]
    fn each_waiting_run_keeps_its_own_cancel() {
        let mark = CancelMark::default();
        mark.ask(1);
        mark.ask(2);
        assert!(mark.begin(), "the first");
        assert!(mark.begin(), "and the second");
        assert!(!mark.begin());
    }

    #[tokio::test]
    async fn the_next_run_being_cancelled_is_said_at_once() {
        let mark = CancelMark::default();
        assert!(!mark.begin());
        let no = tokio::time::timeout(std::time::Duration::from_millis(50), mark.next_asked()).await;
        assert!(no.is_err(), "nothing asked");
        mark.ask(2);
        tokio::time::timeout(std::time::Duration::from_secs(5), mark.next_asked()).await.expect("said");
    }

    #[test]
    fn a_late_cancel_stops_nothing_that_comes_after() {
        let mark = CancelMark::default();
        assert!(!mark.begin());
        assert!(mark.ask(1), "the run under way");
        assert!(mark.asked() && mark.stands(1));
        assert!(!mark.begin());
        assert!(!mark.asked() && !mark.stands(1));
        assert!(!mark.ask(1), "a run that is over");
        assert!(!mark.asked());
    }

    #[derive(Default)]
    struct Recorded(Mutex<Vec<u64>>);

    impl Canceller for Recorded {
        fn cancel(&self, run: u64) {
            self.0.lock().unwrap().push(run);
        }
    }

    #[test]
    fn a_session_numbers_its_runs_and_cancels_the_last_one() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let recorded = Arc::new(Recorded::default());
        let caps = Capabilities {
            server_paging: false,
            cancel: true,
            introspection: false,
            key_metadata: false,
            contexts: false,
            structure: false,
            ddl: false,
            language: Language::Sql(crate::sql::dialect::Dialect::Postgres),
            hierarchy: Hierarchy::DatabaseSchema,
            explain: None,
            reads_held_to_end: false,
            export: false,
            manual_commit: false,
            edit: None,
            monitor: false,
        };
        let session = Session::new(caps, SessionRole::Query, tx, recorded.clone());
        session.cancel();
        assert!(recorded.0.lock().unwrap().is_empty(), "nothing sent, nothing to cancel");
        session.send(DbCommand::Count { id: 4, sql: "x".into(), limit: None });
        session.send(DbCommand::FetchMore { id: 2 });
        session.cancel();
        assert_eq!(*recorded.0.lock().unwrap(), [1], "a fetch is part of a run");
        // A count asked again has the same id, and is another run.
        session.send(DbCommand::Count { id: 4, sql: "x".into(), limit: None });
        session.cancel();
        assert_eq!(*recorded.0.lock().unwrap(), [1, 2]);
    }
}
