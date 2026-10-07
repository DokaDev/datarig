//! Driver abstraction: a small base interface plus a capability flag set. The UI only talks to a [`Session`] through [`DbCommand`] /
//! [`DbEvent`] channels and never sees driver types. Implementations live in their own crates
//! (`datarig-driver-postgres`); the front end maps a profile's `driver` name to one of them.

pub mod ddl;
pub mod keys;
mod protocol;
pub mod structure;

pub use keys::{KeyCatalog, KeyMarks};
pub use protocol::{
    ArrayElement, Cell, ColumnMeta, ColumnOrigin, DbCommand, DbError, DbEvent, Outcome, PagingMode, SchemaObjects,
    ValueKind,
};

use crate::profile::ConnectionConfig;
use crate::sql::dialect::Language;
use crate::transport::DialerRef;
use futures::future::BoxFuture;
use std::sync::Arc;
use std::time::Duration;
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
    /// the catalog's parts (`ddl::DdlSource`), which `sql::ddl` writes out.
    pub ddl: bool,
    /// The editor language of the driver's sessions: the SQL tools (lexing, quoting, the risk
    /// classifier) pick their rules from it.
    pub language: Language,
    /// How the server's namespaces nest. Nothing reads it yet: the explorer and the context
    /// picker show databases holding schemas, as PostgreSQL has them.
    pub hierarchy: Hierarchy,
    /// The plan the driver's sessions can produce for a statement, `None` for none: the plan
    /// view (`query.explain`, `results.view_as_plan`) is offered only with one.
    pub explain: Option<ExplainFormat>,
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
    /// PostgreSQL's `EXPLAIN (FORMAT JSON)` (`sql::plan::pg`), asked with
    /// [`Dialect::explain_sql`](crate::sql::dialect::Dialect::explain_sql).
    Json,
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
}

impl SessionRole {
    /// Short tag used in the server-side `application_name`.
    pub fn tag(self) -> &'static str {
        match self {
            SessionRole::Meta => "meta",
            SessionRole::Query => "q",
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
        }
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PingError {
    Timeout(Duration),
    Failed(DbError),
}

/// Cancels whatever statement the session is currently running.
pub trait Canceller: Send + Sync {
    fn cancel(&self);
}

/// One connection of a given [`SessionRole`]: a command channel and a canceller.
pub struct Session {
    pub caps: Capabilities,
    pub role: SessionRole,
    tx: UnboundedSender<DbCommand>,
    canceller: Arc<dyn Canceller>,
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
        Self { caps, role, tx, canceller }
    }

    pub fn send(&self, cmd: DbCommand) {
        // A closed channel means the connection task ended; the UI already got the reason.
        let _ = self.tx.send(cmd);
    }

    pub fn cancel(&self) {
        self.canceller.cancel();
    }

    /// Close the session: the command channel closes, and the driver cancels a statement that
    /// is still running and closes the connection (the server rolls back an open transaction).
    /// No further events are sent for it.
    pub fn close(self) {
        drop(self);
    }
}
