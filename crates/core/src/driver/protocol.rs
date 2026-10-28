//! Messages between the UI and a driver [`Session`](super::Session): commands go in on the
//! session's channels, progress and results come back as [`DbEvent`]s.

use super::ddl::{DdlObject, DdlSource, ObjectKind};
use super::keys::KeyCatalog;
use super::structure::{RelationStats, TableStructure};
use crate::fault::Fault;
use crate::sql::complete::Catalog;
use crate::sql::risk::repeat::NotRepeatable;
use crate::tls::{Encryption, TlsProblem};
use crate::transport::DialError;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// The tables and views of a schema (`DbCommand::LoadObjects`), by name, each list sorted.
/// Materialized views are listed with the views and named again in `materialized` (the
/// explorer draws them with an icon of their own). `stats` has the estimates of each relation
/// with storage (tables, partitioned tables, materialized views), by name, read with the list:
/// a view or a foreign table has none, nor do the objects of a driver that does not read them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchemaObjects {
    pub tables: Vec<String>,
    pub views: Vec<String>,
    pub materialized: BTreeSet<String>,
    pub stats: BTreeMap<String, RelationStats>,
}

impl From<(Vec<String>, Vec<String>)> for SchemaObjects {
    /// Tables and views, none of them materialized, without estimates.
    fn from((tables, views): (Vec<String>, Vec<String>)) -> Self {
        Self { tables, views, ..Self::default() }
    }
}

/// Why a database operation failed. Only the server's own message is text to show as it is
/// (it is data); everything else is a kind the UI words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DbError {
    /// The server's message (severity, text, detail).
    Server(String),
    /// The server closed the connection.
    Closed,
    /// The server did not answer within this time.
    NoAnswer(Duration),
    /// The connection settings cannot be used (a DSN that does not parse, …).
    Settings(Fault),
    /// Reaching or talking to the server failed (network, TLS, protocol, the client's own
    /// checks); an io kind when the cause is one.
    Connection(Fault),
    /// The transport the session reaches the server through (a tunnel) did not give it a
    /// connection.
    Transport(DialError),
    /// The session does not do this (a statement sent to the metadata session).
    NotSupported,
    /// The server is too old for key metadata (PostgreSQL before 12 has no `attgenerated`).
    ServerTooOld,
    /// A statement expected to return no rows (DML without `RETURNING`: a rule made it return
    /// some) ran and returned more than were read; only this many were, and the rest are gone.
    RowsNotRead(u64),
    /// The statement's table (or anything else its result depends on) changed while it was
    /// being prepared and run, again after it was prepared once more: run it again.
    SchemaChanged,
    /// The driver ended the statement without a result (a bug the driver guards against: every
    /// run ends with one result, see the PostgreSQL driver's `Reply`).
    NoResult,
    /// A run of several statements was cancelled between two of them: the rest did not run.
    Cancelled,
    /// A read-only session did not send a statement that asks for a read-write transaction or
    /// turns read-only off (`SET transaction_read_only = off`, `BEGIN READ WRITE`, …): the app
    /// refuses them first; this is the driver's own check.
    ReadWriteRefused,
    /// `COPY … FROM STDIN` or `COPY … TO STDOUT` was not sent: its data would go through the
    /// connection's COPY sub-protocol, which the session does not carry. The app refuses them
    /// first; this is the driver's own check.
    CopyStdio,
    /// The server started a COPY to or from the client that no request asked for (a `COPY …
    /// TO STDOUT` or `FROM STDIN` the driver's own check missed): the connection was closed, so
    /// the server stopped it.
    CopyStarted,
    /// The server refused a statement because it ran inside a transaction: one that cannot run
    /// in a transaction block (SQLSTATE `25001`: `VACUUM`, `CREATE INDEX CONCURRENTLY`, `CREATE
    /// DATABASE`, …) or a procedure's transaction control (`2D000`, a `COMMIT` in a `CALL`), in a
    /// transaction the driver opened only to set the session's search path (a session whose
    /// server ignored the path given at connect, a pooler: [`DbEvent::ContextPerTransaction`]),
    /// or one it opened right before the statement for manual commit ([`TxRule::Manual`], rolled
    /// back since: [`DbEvent::ManualRolledBack`]). The server's message. The same failure in the
    /// user's own block is a plain `Server`.
    NeedsNoTransaction(String),
    /// A statement the app was to run again or count for the user (`Count`, `Resume`) is not
    /// on the allowlist of `sql::risk::repeat`, as the text says or as the server told when it
    /// was asked right before (a view, a function of the user's with the name of a built-in,
    /// …): it was not sent.
    NotRepeatable(NotRepeatable),
    /// A lock another session holds or asked for (an `ALTER TABLE`, a `VACUUM FULL`) was in the
    /// way of a lookup of the metadata session, which never waits for one: it gave up (at once,
    /// or after a short `lock_timeout`). Asking again later may work.
    Locked,
    /// The object a lookup names does not exist (any more): renamed or dropped since it was
    /// listed, or a name that names nothing.
    NotFound,
    /// The name a lookup was given names more than one object where it was looked up (these
    /// kinds, each once, in order): which one is meant is not known, so none was read.
    Ambiguous(Vec<ObjectKind>),
    /// The object exists, and the server does not show its definition to this user (MySQL
    /// shows a routine's only to its definer, or to a user with `SHOW_ROUTINE` or `SELECT` on
    /// every database).
    DefinitionHidden,
    /// A name the user typed to look up is not one (MySQL: `name` or `database.name`, each part
    /// bare or quoted).
    NotAName,
    /// A name the user typed has no database to be looked up in: the session has none.
    NoDatabase,
    /// The name names an object of a kind the driver does not read the DDL of (the server's word
    /// for it, MariaDB's `PACKAGE`).
    KindUnsupported(String),
    /// The server's version is older than the driver supports: `server` is what it is
    /// (`MySQL 5.7.44`), `needed` the oldest version the driver works with (`MySQL 8.0`).
    VersionUnsupported { server: String, needed: String },
    /// The server accepts only encrypted connections (MySQL's `require_secure_transport`), and
    /// the profile's TLS mode is off.
    TlsRequired,
    /// A TLS connection could not be made (or its settings cannot be used).
    Tls(TlsProblem),
    /// The server asked for an authentication method the driver does not have (its name).
    AuthUnsupported(String),
    /// MySQL's server refused the login (`ER_ACCESS_DENIED_ERROR`): a wrong user or password,
    /// or an account that requires TLS (`REQUIRE SSL`) on a connection without it. The
    /// server's message.
    AccessDenied(String),
    /// A MySQL login needed the server's public key to send the password, the profile names no
    /// key file, and the driver did not ask the server for it: the connection goes directly to
    /// another machine, unencrypted, so the key could come from anyone in between (see
    /// `ConnectionConfig::allow_public_key_retrieval`). Nothing about the password was sent.
    KeyRetrievalRefused,
    /// The profile's server public key file (its path, as the profile names it) cannot be
    /// used: unreadable (`Some`, why), or not an RSA public key in PEM of 2048 bits or more.
    ServerKeyFile { path: String, fault: Option<Fault> },
    /// The profile's server public key file is a relative path (its path): it must be absolute
    /// or start with `~/`, so it does not depend on the directory datarig was started in.
    ServerKeyPathRelative(String),
    /// A MySQL login would encrypt the password with the server's RSA public key, and the
    /// password is too long for that key: `max` bytes at most (UTF-8; 213 with a 2048-bit key,
    /// RSA-OAEP's limit less the zero byte the client ends it with). Nothing about the password
    /// was sent.
    PasswordTooLong { max: usize },
    /// The server's public key a MySQL login would encrypt the password with (the one the
    /// server sent when asked) is not an RSA public key in PEM of 2048 bits or more. Nothing
    /// about the password was sent.
    ServerKeyInvalid,
    /// A read-only session stopped being read-only on the server (a statement turned it off
    /// or opened a read-write transaction, which the app refuses to send): the session rolled
    /// back and closed.
    ReadOnlyLost,
    /// The server did not stop the statement the session runs when asked to (by the user's
    /// cancel, or by the session itself stopping a result it holds), for this reason: the
    /// session closed its connection instead of reading the rest of the result. Whether the
    /// statement stopped is not known: it may run on (and, outside a transaction block, commit
    /// what it does); the user's open transaction is rolled back once the server ends the
    /// session.
    CancelNotTaken(CancelFailure),
    /// [`DbCommand::EndTx`] found no transaction block open on the session: nothing was sent.
    NoTransaction,
    /// The server answered the transaction the driver opened for a statement
    /// ([`TxRule::Manual`]) without one open: the statement was not sent (run without a
    /// transaction, it could not be rolled back).
    TxNotOpened,
    /// The connection ended while a `COMMIT` ([`DbCommand::EndTx`] with `commit`) was on its
    /// way or being carried out: whether the server committed is not known (a server may commit
    /// and lose the connection before it answers). [`DbEvent::Lost`] follows with why.
    CommitUnknown,
    /// The server ended the session because its transaction stayed idle longer than
    /// `idle_in_transaction_session_timeout` (PostgreSQL's SQLSTATE `25P03`; the limit may be
    /// the server's own or the one [`TxRule::Manual`] set for the transaction): the server rolled
    /// the transaction back. The server's message. Comes as [`DbEvent::Lost`]'s reason.
    IdleInTransactionTimeout(String),
    /// A statement waited for a lock another session holds longer than the session's lock wait
    /// limit, and the server cancelled it (PostgreSQL's `lock_timeout`, SQLSTATE `55P03`;
    /// MySQL's `innodb_lock_wait_timeout` / `lock_wait_timeout`, error 1205). The server's
    /// message. The session stays (PostgreSQL: a block it ran in is aborted as by any failure).
    LockTimeout(String),
    /// As [`DbError::LockTimeout`], but the statement ran without the session's lock wait limit:
    /// the server connection's own cut it short (PostgreSQL behind a pooler: a statement outside
    /// the transactions that start with the limit, such as `CALL`, `DO` or `VACUUM`). How long it
    /// waited is not the app's to say.
    ServerLockTimeout(String),
    /// The server ended the session because it sat idle longer than the server's idle limit
    /// (PostgreSQL's `idle_session_timeout`, SQLSTATE `57P05`; MySQL's `wait_timeout`, error
    /// 4031). The server's message. Comes as [`DbEvent::Lost`]'s reason.
    IdleSessionTimeout(String),
    /// A statement in a transaction opened for manual commit names a savepoint whose name starts
    /// with `datarig_` (`SAVEPOINT`, `RELEASE`, `ROLLBACK TO`): those are the session's own, and
    /// using one would break how it undoes a failed run. It was not sent.
    ReservedSavepoint,
    /// MySQL: an `EXPLAIN ANALYZE` of a write names tables whose changes a rollback does not undo
    /// (an engine that is not InnoDB, such as MyISAM or MEMORY, or a view, whose tables are not
    /// known), or tables whose triggers may write elsewhere: it was not sent. The tables,
    /// `schema.table (engine)`; none when the text could not be read for its names (an
    /// executable comment, a text too long).
    NotUndoable(Vec<String>),
    /// A statement sent as a plain read ([`TxStep::ReadOnly`]) asks for a read-write transaction
    /// or turns read-only off (`SET TRANSACTION READ WRITE`, `set_config()` of it, …), as the
    /// driver's own check reads it: it was not sent. Send it as [`TxStep::Open`] instead.
    NotAPlainRead,
    /// Statement `index` (from 0) of what the query session starts with
    /// ([`super::ConnectOptions::session_sql`]), `statement`, failed: the session was closed
    /// before `Connected`. Why, as the server said it.
    SessionSql { index: usize, statement: String, error: Box<DbError> },
}

/// Why a request to stop the running statement did not stop it ([`DbError::CancelNotTaken`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancelFailure {
    /// The server refused it (the statement runs as another account; MySQL: inside a
    /// `SQL SECURITY DEFINER` routine, where `KILL QUERY` needs `CONNECTION_ADMIN`), and the
    /// statement kept sending rows.
    Refused,
    /// It could not reach the server, for this reason (the connection to send it on did not
    /// open: the network, a tunnel, the login; or no answer in time). What lived in the session
    /// (temporary tables, settings, advisory locks, savepoints) is gone with it too.
    NotSent(Box<DbError>),
    /// The server agreed to stop the statement, and went on sending its rows.
    KeptSending,
    /// It was sent and got no answer within this time: whether the server took it is not known
    /// (it may still be carried out), and the statement runs on meanwhile.
    NotAnswered(Duration),
    /// The statement answered (its outcome was reported) while a request to stop it got no
    /// answer within this time: that request may still be carried out on whatever the
    /// connection would run next, so the session closed its connection once the statement's
    /// answer was read.
    StopPending(Duration),
}

impl DbError {
    /// The raw text (the server's message, or a fault's detail), for matching and the error
    /// log only.
    pub fn raw(&self) -> Cow<'_, str> {
        match self {
            DbError::Server(s)
            | DbError::NeedsNoTransaction(s)
            | DbError::IdleInTransactionTimeout(s)
            | DbError::LockTimeout(s)
            | DbError::ServerLockTimeout(s)
            | DbError::IdleSessionTimeout(s) => Cow::Borrowed(s),
            DbError::Settings(f) | DbError::Connection(f) => Cow::Borrowed(&f.detail),
            DbError::Transport(e) => Cow::Borrowed(e.raw()),
            DbError::CancelNotTaken(CancelFailure::NotSent(why)) => why.raw(),
            DbError::SessionSql { error, .. } => error.raw(),
            DbError::AuthUnsupported(s) | DbError::AccessDenied(s) => Cow::Borrowed(s),
            DbError::Tls(p) => Cow::Borrowed(p.raw()),
            DbError::ServerKeyFile { fault, .. } => {
                fault.as_ref().map_or(Cow::Borrowed(""), |f| Cow::Borrowed(&f.detail))
            }
            DbError::Closed
            | DbError::NoAnswer(_)
            | DbError::NotSupported
            | DbError::ServerTooOld
            | DbError::RowsNotRead(_)
            | DbError::SchemaChanged
            | DbError::NoResult
            | DbError::Cancelled
            | DbError::ReadWriteRefused
            | DbError::CopyStdio
            | DbError::CopyStarted
            | DbError::NotRepeatable(_)
            | DbError::Locked
            | DbError::NotFound
            | DbError::Ambiguous(_)
            | DbError::DefinitionHidden
            | DbError::NotAName
            | DbError::NoDatabase
            | DbError::KindUnsupported(_)
            | DbError::VersionUnsupported { .. }
            | DbError::TlsRequired
            | DbError::KeyRetrievalRefused
            | DbError::ServerKeyPathRelative(_)
            | DbError::PasswordTooLong { .. }
            | DbError::ServerKeyInvalid
            | DbError::ReadOnlyLost
            | DbError::NoTransaction
            | DbError::TxNotOpened
            | DbError::CommitUnknown
            | DbError::ReservedSavepoint
            | DbError::NotUndoable(_)
            | DbError::NotAPlainRead
            | DbError::CancelNotTaken(
                CancelFailure::Refused
                | CancelFailure::KeptSending
                | CancelFailure::NotAnswered(_)
                | CancelFailure::StopPending(_),
            ) => Cow::Borrowed(""),
        }
    }
}

/// Tests write server messages as plain text.
#[cfg(any(test, feature = "test-util"))]
impl From<&str> for DbError {
    fn from(s: &str) -> Self {
        DbError::Server(s.to_string())
    }
}

#[cfg(any(test, feature = "test-util"))]
impl From<String> for DbError {
    fn from(s: String) -> Self {
        DbError::Server(s)
    }
}

/// What happens to a result that has more rows than its first page, outside the user's own
/// transaction block (inside it a result's portal always lives in the block, and ends with it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PagingMode {
    /// The first page comes back and the session keeps nothing open: the server-side cursor and
    /// the transaction it needs end in the same request (so no lock, snapshot or transaction is
    /// left while the user reads), and [`DbEvent::Released`] comes before the page. Past it
    /// the app can only run the statement again (`Resume`).
    #[default]
    NoHold,
    /// The cursor stays open, and with it the transaction that holds it (and the locks its
    /// statement took), until the result is read to its end, `ClosePortal` closes it, or the next
    /// command ends it; `FetchMore` reads the next page from it.
    Hold,
}

/// How the statements of a run (`DbCommand::Execute`) stand to transactions outside a block the
/// session has open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TxRule {
    /// Autocommit: each statement outside a transaction block ends with its own (implicit)
    /// transaction, as the server runs it.
    #[default]
    Auto,
    /// Manual commit: `steps[i]` says how statement `i` stands to a transaction ([`TxStep`]; a
    /// statement past the end of `steps` is [`TxStep::AsIs`]). Right before a statement that is
    /// [`TxStep::Open`] while no transaction block is open on the session at that moment (the
    /// driver's own state, after the statements before it in the run), the driver opens one, in
    /// the same write as the statement ([`DbEvent::ManualBegun`] once the statement succeeded;
    /// when it fails, or the run fails later, the new transaction is rolled back at once:
    /// [`DbEvent::ManualRolledBack`]). The transaction then stays open after the run until the
    /// user ends it ([`DbCommand::EndTx`], or a `COMMIT`/`ROLLBACK` the user runs).
    ///
    /// While a block the driver opened so is open when a run starts, the run goes under a
    /// savepoint of the driver's: a run that fails or is cancelled is undone, and the transaction
    /// stays open with what it held before the run ([`DbEvent::RunRolledBack`]). This holds for
    /// any run in such a block, an [`TxRule::Auto`] one too (a rule changed while the transaction
    /// stays open). A run with transaction control of its own (`COMMIT`, `ROLLBACK`, `SAVEPOINT`,
    /// …) and a block the user opened (`BEGIN`) get none: the server's own rules apply (a failure
    /// aborts the block). Savepoint names starting with `datarig_` are the driver's
    /// ([`DbError::ReservedSavepoint`]). MySQL sets no such savepoint (it would cost a round
    /// trip of its own): a failed statement there is undone by the server alone and the
    /// statements of the run before it keep what they did; the transaction stays open unless the
    /// server rolls all of it back (a deadlock, a lock wait timeout under
    /// `innodb_rollback_on_timeout`: [`DbEvent::TxClosedBy`]).
    ///
    /// `idle_timeout`: the most the server lets the transaction sit idle (PostgreSQL's
    /// `idle_in_transaction_session_timeout`, set for that transaction only, again after a
    /// `COMMIT AND CHAIN`; PostgreSQL 9.6 or later): past it the server ends the session and
    /// rolls back ([`DbError::IdleInTransactionTimeout`]). Less than a second is taken as one
    /// second (a shorter limit could end the transaction between two statements of a run).
    /// `None` leaves the server's setting as it is. MySQL has no such limit and ignores it. The
    /// app sends a limit later than its own rollback of the idle transaction, as a backstop for
    /// when it cannot send that.
    Manual { steps: Vec<TxStep>, idle_timeout: Option<Duration> },
}

/// How a statement of a [`TxRule::Manual`] run stands to a transaction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TxStep {
    /// It may write: when no transaction block is open, the driver opens one right before it,
    /// which stays open for the user to commit or roll back.
    Open,
    /// A plain read, as far as its text tells (a view, a row-level security policy or a function
    /// of the user's with a built-in's name may still hide a write behind it): when no
    /// transaction block is open it runs in a read-only transaction of its own (PostgreSQL:
    /// `BEGIN READ ONLY` where the driver sends `BEGIN`, in the same write), so a write hidden
    /// in it fails (`25006`) instead of being committed by itself. Inside an open block it runs
    /// as it is. Drivers without read-only transactions run it as [`TxStep::AsIs`]. MySQL does
    /// so: a read-only transaction would cost a round trip of its own for every read, so a write
    /// hidden in a read there (a view's stored function) commits by itself.
    ReadOnly,
    /// It runs as it would under [`TxRule::Auto`] (transaction control, session settings,
    /// statements that cannot run in a transaction).
    #[default]
    AsIs,
}

/// How the server ended an open transaction on its own ([`DbEvent::TxClosedBy`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerEnd {
    /// It rolled the whole transaction back as a statement failed (MySQL: a deadlock, `1213`; a
    /// lock wait timeout under `innodb_rollback_on_timeout`). Nothing of it was kept.
    RolledBack,
    /// A statement committed it before running (MySQL's implicit commit: DDL, `BEGIN` inside a
    /// transaction, `LOCK TABLES`, …), also when that statement then failed (`CREATE TABLE` of
    /// a table that exists). What it did was kept.
    Committed,
    /// It ended while a statement that runs code of the server's (a procedure) failed: the code
    /// may have committed it before failing, or the server rolled it back. What it did may or
    /// may not have been kept.
    Unknown,
}

/// How [`DbCommand::EndTx`] ended the transaction ([`DbEvent::TxEnded`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxOutcome {
    /// The server committed it.
    Committed,
    /// The server rolled it back, as asked.
    RolledBack,
    /// A commit was asked for and the server rolled the transaction back instead: it was aborted
    /// (a statement in it failed), and PostgreSQL answers `COMMIT` of an aborted transaction with
    /// `ROLLBACK`. Nothing of it was kept.
    RolledBackInstead,
}

#[derive(Clone, Debug)]
pub enum DbCommand {
    LoadSchemas,
    LoadObjects {
        schema: String,
    },
    LoadCatalog,
    /// The key constraints of every table (`Capabilities::key_metadata`): answered with
    /// [`DbEvent::Keys`]. Sent again when the schema is refreshed.
    LoadKeys,
    /// The databases of the server the user may connect to (`Capabilities::contexts`):
    /// answered with [`DbEvent::Databases`].
    LoadDatabases,
    /// The structure of table (or view) `schema.table` (`Capabilities::structure`): answered
    /// with [`DbEvent::Structure`]. Read from the catalog only, never from the table.
    LoadStructure {
        schema: String,
        table: String,
    },
    /// The DDL of `object` (`Capabilities::ddl`): answered with [`DbEvent::Ddl`] of the same
    /// `id`. Read from the catalog only, never waiting for a lock: an object another session
    /// locks (or one of the relations reading its definition would lock) answers
    /// [`DbError::Locked`], with nothing read.
    LoadDdl {
        id: u64,
        object: DdlObject,
    },
    /// Run statements in order, stopping at the first that fails; the last one's result is
    /// the run's answer (`Page`, `Done` or `Failed`). With more than one statement the
    /// session reports each one's start ([`DbEvent::Started`]) and, before the last, its end
    /// ([`DbEvent::Finished`]); a cancel between two statements stops the rest
    /// ([`DbError::Cancelled`]). `paging`: what happens to the last statement's result when it
    /// has more rows than a page (outside the user's block). `tx`: whether statements open a
    /// transaction that stays open for the user to commit ([`TxRule`]).
    Execute {
        id: u64,
        statements: Vec<String>,
        paging: PagingMode,
        tx: TxRule,
    },
    /// End the transaction block open on the session (one the user opened, or the driver for
    /// [`TxRule::Manual`]): `COMMIT` when `commit`, else `ROLLBACK`. Answered with exactly one
    /// [`DbEvent::TxEnded`] of the same `id`, or [`DbEvent::Failed`]: no block open
    /// ([`DbError::NoTransaction`], nothing sent), the server refused the commit (a deferred
    /// constraint, a serialization failure: the server rolled back, and the block is closed),
    /// or the connection ended meanwhile ([`DbError::CommitUnknown`] for a commit). A result held
    /// open in the block ends first. Sent while a run goes on, it waits for the run's end, as
    /// other commands do. Drivers without [`Capabilities::manual_commit`](super::Capabilities)
    /// answer [`DbError::NotSupported`].
    EndTx {
        id: u64,
        commit: bool,
    },
    FetchMore {
        id: u64,
    },
    /// Stop paging result `id`: close its portal and end the transaction that held it open,
    /// unless that is the user's own block (it stays open). Nothing is reported for the portal;
    /// a transaction that ends is reported as `TxOpen(false)`.
    ClosePortal {
        id: u64,
    },
    /// Count the rows of result `id`: run `sql`, a `SELECT count(*)` the app
    /// made from the result's statement when the user asked (the allowlist of
    /// `sql::risk::repeat`, whose server side is asked first in the same transaction: a
    /// statement it refuses answers [`DbError::NotRepeatable`] and is not sent), and answer
    /// with exactly one [`DbEvent::Counted`]. While a portal is
    /// open, and inside the user's block, it runs in that transaction under a savepoint, so a
    /// failure neither aborts the user's block nor ends the portal; otherwise on its own (a
    /// read-only transaction on a read-only session). A cancel stops it.
    Count {
        id: u64,
        sql: String,
    },
    /// Ask the server only what it can tell for the allowlist of `sql::risk::repeat` about
    /// `sql` (views, names a user's function or operator shadows, …: `repeat::check_query`),
    /// as before a `Resume` or a `Count`, without running `sql`: answered with exactly one
    /// [`DbEvent::RepeatChecked`] of the same `id` (a statement whose text is off the
    /// allowlist answers [`DbError::NotRepeatable`] without asking). It reads the catalog only;
    /// inside the user's block or a portal's transaction it runs under a savepoint of the
    /// driver's own (never one of the user's names) rolled back to after it, so it changes
    /// nothing there; inside an aborted block it is not asked and answers a refusal. A cancel
    /// stops it.
    CheckRepeat {
        id: u64,
        sql: String,
    },
    /// Fetch past a result whose portal was closed: run `sql` again as run
    /// `id` (one statement, the app's allowlist of `sql::risk::repeat`, whose server side is
    /// asked first: a statement it refuses fails with [`DbError::NotRepeatable`] and is not
    /// sent), read and drop its first `skip` rows, and answer like `Execute`: the next page with
    /// its columns and `more` (an empty last page when the result now has no more than `skip`
    /// rows), `Done` or `Failed`; later pages come with `FetchMore`. Inside the user's block it
    /// runs under a savepoint, released when its portal ends and rolled back to when it fails
    /// or is cancelled. `paging` as for `Execute`.
    Resume {
        id: u64,
        sql: String,
        skip: u64,
        paging: PagingMode,
    },
    /// Read every row of `sql` (one statement that reads) for an export to a file, into `sink`
    /// a batch of at most `batch` rows (and about [`EXPORT_BATCH_BYTES`]) at a time (its columns first, [`ExportBatch`]), awaiting
    /// room in the channel before reading on: the export's memory stays bounded whatever the
    /// result's size, and the rows never go through the UI's events. Answered with exactly one
    /// [`DbEvent::Exported`] of the same `id`, after the last batch was sent and `sink` was
    /// dropped.
    ///
    /// `guard` says why it may run ([`ExportGuard`]). Outside the user's block it reads in a
    /// read-only transaction of its own (MySQL: without the session's paging limit): it never
    /// writes. Inside the user's block it reads there (seeing
    /// what the block changed), PostgreSQL under a savepoint of the driver's own released after
    /// it and rolled back to when it fails or is cancelled, so the block is never aborted; an
    /// aborted block refuses it ([`DbError::NotSupported`]) without running anything. A cancel
    /// stops it. When `sink` is closed (the file could not be written), the statement is
    /// stopped and the answer says it did not read to the end ([`ExportRead::complete`]).
    Export {
        id: u64,
        sql: String,
        guard: ExportGuard,
        sink: ExportSink,
        batch: usize,
    },
}

/// Why an export's statement may run ([`DbCommand::Export`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportGuard {
    /// On the allowlist of `sql::risk::repeat`, whose server side is asked right before (inside
    /// the user's block, in the request that sets the driver's savepoint): a statement it refuses
    /// answers [`DbError::NotRepeatable`] and is not sent.
    Repeatable,
    /// The user was asked and said to run it again (it is not on the allowlist).
    Confirmed,
    /// A chunk of a table read by its key (`export::keyset`), written by the app: a short read
    /// that gives up behind a lock (PostgreSQL: `lock_timeout` in its transaction; MySQL: the
    /// statement's own hint).
    Keyset,
}

/// What [`DbCommand::Export`] sends to its sink: the result's columns once, then its rows.
#[derive(Debug)]
pub enum ExportBatch {
    Columns(Vec<ColumnMeta>),
    Rows(Vec<Vec<Cell>>),
}

/// Where [`DbCommand::Export`] sends its rows: a bounded channel, so a reader slower than the
/// server holds the statement back instead of the rows piling up.
pub type ExportSink = tokio::sync::mpsc::Sender<ExportBatch>;

/// About the most bytes of values a batch of [`DbCommand::Export`] holds: wide rows (documents,
/// images) make smaller batches, so memory is bounded by bytes as well as by rows.
pub const EXPORT_BATCH_BYTES: usize = 8 << 20;

/// What an export's statement read ([`DbEvent::Exported`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportRead {
    /// Rows sent to the sink.
    pub rows: u64,
    /// The last row sent (a keyset's next chunk starts after it).
    pub last: Option<Vec<Cell>>,
    /// The result was read to its end; `false` when the sink closed first and the statement
    /// was stopped.
    pub complete: bool,
}

impl DbCommand {
    pub fn is_meta(&self) -> bool {
        matches!(
            self,
            DbCommand::LoadSchemas
                | DbCommand::LoadObjects { .. }
                | DbCommand::LoadCatalog
                | DbCommand::LoadKeys
                | DbCommand::LoadDatabases
                | DbCommand::LoadStructure { .. }
                | DbCommand::LoadDdl { .. }
        )
    }
}

#[derive(Clone, Debug)]
pub struct ColumnMeta {
    /// The column's name in the result (an alias when the query gives one).
    pub name: String,
    /// The type as the server names it, shown as it is (PostgreSQL: `int4`, `text[]`).
    pub type_name: String,
    /// A number type: the same as [`ValueKind::is_number`] of `kind`.
    pub numeric: bool,
    /// A JSON type: the same as `kind` being [`ValueKind::Json`].
    pub json: bool,
    /// What kind of value the column holds, whatever the server calls its type: the tools that
    /// treat values by kind (copy as SQL or JSON, the chart) read this, never `type_name`.
    pub kind: ValueKind,
    /// The table column this result column comes from, when the driver knows it (a plain
    /// column of a table; `None` for an expression, a function result, a literal).
    pub origin: Option<ColumnOrigin>,
}

impl ColumnMeta {
    /// A result column whose `numeric` and `json` follow from `kind`, so they cannot disagree
    /// with it (what a driver builds; a literal must keep them in step itself).
    pub fn new(name: String, type_name: String, kind: ValueKind, origin: Option<ColumnOrigin>) -> Self {
        Self { name, type_name, numeric: kind.is_number(), json: kind == ValueKind::Json, kind, origin }
    }
}

/// What kind of value a result column holds, the same for every database. A driver maps its
/// own types to it; a type that fits none of these is [`ValueKind::Other`] (its values are
/// still shown and copied as the text the server gives).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueKind {
    /// Character data (`text`, `varchar`, `char`).
    Text,
    /// A whole number (`int2`/`int4`/`int8`, `oid`).
    Integer,
    /// An exact decimal number (`numeric`, `money`).
    Decimal,
    /// A floating-point number (`float4`, `float8`).
    Float,
    /// A boolean.
    Bool,
    /// JSON (`json`, `jsonb`).
    Json,
    /// Binary data (`bytea`).
    Bytes,
    /// A bit string (`bit`, `varbit`).
    Bit,
    /// A calendar date.
    Date,
    /// A time of day (within a day), with or without a time zone. A type that can be negative
    /// or longer than a day (MySQL's `TIME`) is not one: it is an [`ValueKind::Interval`].
    Time,
    /// A date and time without a time zone.
    Timestamp,
    /// A date and time with a time zone (a point in time).
    TimestampTz,
    /// A span of time (`interval`; MySQL's `TIME`, which can be negative or over 24 hours).
    Interval,
    /// An array of any number of dimensions; its elements are of the [`ArrayElement`] kind.
    Array(ArrayElement),
    /// Anything else (`uuid`, network addresses, ranges, geometry, enums, composites, a domain
    /// over a type other than a number, extension types, an array whose elements are none of
    /// the built-in types). The tools that read kinds treat such a column by its `type_name`, as
    /// they did before kinds: a type of the database's own may be named like a built-in one.
    Other,
}

/// The kind of an array's elements, as far as the tools read array values (the array's text,
/// `{1,2}`, split into its elements at commas; an array whose text is not split so is
/// [`ValueKind::Other`]). Kept apart from [`ValueKind`]: an array only needs to say
/// how its elements are written, and an array of arrays is just more dimensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArrayElement {
    /// Numbers (integers, decimals and floats).
    Number,
    /// Booleans (`t`/`f`).
    Bool,
    /// JSON values.
    Json,
    /// Any other element, read as text.
    Text,
}

impl ValueKind {
    /// A number: [`ValueKind::Integer`], [`ValueKind::Decimal`] or [`ValueKind::Float`].
    pub fn is_number(self) -> bool {
        matches!(self, Self::Integer | Self::Decimal | Self::Float)
    }
}

/// A column of a table, as the driver names it. [`KeyCatalog`](super::KeyCatalog) maps it to
/// the table's name and the column's keys.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ColumnOrigin {
    /// By number: the table's id and the column's number in it (PostgreSQL: the table's `oid`
    /// and the column's `attnum`, from the RowDescription of a result).
    Pg { table: u32, column: i16 },
    /// By name: a driver that names the column a result column comes from (MySQL's column
    /// definition: `schema`, `org_table`, `org_name`).
    Named { schema: String, table: String, column: String },
}

pub type Cell = Option<String>;

#[derive(Clone, Debug)]
pub enum Outcome {
    Affected(u64),
    Command(String),
}

/// What the server said of a statement beyond its outcome (MySQL's OK packet), for the run's
/// Messages. The default says nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatementInfo {
    /// The value an `AUTO_INCREMENT` column got from the statement (`LAST_INSERT_ID()`).
    pub insert_id: Option<u64>,
    /// How many warnings it raised (`SHOW WARNINGS` lists them, until the next statement).
    pub warnings: u16,
    /// The server's own words about it (`Rows matched: 1  Changed: 1  Warnings: 0`), shown as
    /// they are.
    pub message: Option<String>,
    /// Result sets after its first one (a procedure's) that were read and dropped, with rows.
    pub more_results: u32,
    /// MySQL: an `EXPLAIN ANALYZE` of a write ran on these tables, whose changes the rollback did
    /// not undo (`schema.table (engine)`), when the driver lets such a statement run.
    pub not_undone: Vec<String>,
}

/// What a MySQL query session's values were read in, as copying them as SQL needs it: the
/// session's time zone (a `TIMESTAMP` is shown in it) and the character set of its results'
/// text, as the server reports them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionValues {
    /// `@@time_zone` (`SYSTEM`, `+00:00`, `Europe/Berlin`).
    pub time_zone: String,
    /// `character_set_results`, or `character_set_connection` when that one is not UTF-8.
    pub charset: String,
}

/// What a result's values were read in: the session's values when its first page came, kept
/// for its later pages.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ReadIn {
    /// Not known (the session had not said, its driver does not, or the statement ran code of
    /// the server's that may change them while its result is read).
    #[default]
    Unknown,
    Values(SessionValues),
    /// The session's values changed between two pages of the result: its rows are not all read
    /// in the same.
    Changed,
}

impl ReadIn {
    /// What the first page of a result was read in: the session's `values` then.
    pub fn first(values: Option<&SessionValues>) -> Self {
        values.map_or(ReadIn::Unknown, |v| ReadIn::Values(v.clone()))
    }

    /// Another page of the result was read in `values`.
    pub fn page(&mut self, values: Option<&SessionValues>) {
        if let ReadIn::Values(v) = self
            && values != Some(&*v)
        {
            *self = ReadIn::Changed;
        }
    }
}

#[derive(Debug)]
pub enum DbEvent {
    /// How the connection's traffic goes (encrypted with TLS or not): sent right before
    /// `Connected`.
    Encryption(Encryption),
    Connected,
    /// The next result of run `id` (its first `Page` or `StepRows`) is read while code of the
    /// server's runs (a procedure, a prepared statement, an executable comment), which may change
    /// the session's values without saying: what they were is not known ([`ReadIn::Unknown`]).
    ValuesNotKnown {
        id: u64,
    },
    /// What a query session's values are read in from now on ([`SessionValues`]): sent right
    /// after `Connected` by a driver whose results depend on it (MySQL), and again when it
    /// changes (a `SET time_zone`, `SET NAMES` the user ran).
    SessionValues(SessionValues),
    /// The language the query session's text is read in from now on, where it is the
    /// session's own and not the driver's ([`Capabilities::language`](super::Capabilities)):
    /// MySQL's sql mode (`ANSI_QUOTES`, `NO_BACKSLASH_ESCAPES`) and whether its server reads
    /// dollar quotes. Sent right after `Connected`, and again when it changes (a `SET sql_mode`
    /// the user ran), as the server confirmed it.
    Language(crate::sql::dialect::Language),
    /// Sent right after `Connected` of a read-only session (a read-only policy) when the
    /// server did not apply the read-only default (a pooler that drops startup options): the
    /// session runs anyway, because the driver makes each transaction it opens read-only.
    ReadOnlyPerTransaction,
    /// Sent right after `Connected` of a query session opened in a schema of its own (before
    /// its `Context`) when the server did not apply the search path given as a startup option
    /// (a pooler that drops startup options): the driver sets it in each transaction the
    /// session runs instead (never for the session, which could outlive the transaction on a
    /// pooled connection).
    ContextPerTransaction,
    /// Sent right before `Encryption` and `Connected` of a PostgreSQL query session with
    /// start-up statements ([`super::ConnectOptions::session_sql`]) whose connection is not
    /// known to be direct (a pooler): they go first in each transaction as `SET LOCAL` instead
    /// of once for the session; `refused` are the indexes (from 0) of those that have no such
    /// form and were not sent at all.
    SessionPerTransaction {
        refused: Vec<usize>,
    },
    /// The query session stopped keeping prepared statements between transactions:
    /// the server lost them more than once (`26000`, invalid_sql_statement_name) or
    /// had one of the same name already (`42P05`), as behind a pooler in transaction mode that
    /// does not support them. Sent once per session, before the answer of the run that found
    /// out, which went through without them. Never sent when the profile turned the cache off.
    StatementCacheOff,
    /// `auth`: the server rejected or asked for a password (the UI may prompt for one).
    ConnectFailed {
        error: DbError,
        auth: bool,
    },
    Schemas(Result<Vec<String>, DbError>),
    Objects {
        schema: String,
        result: Result<SchemaObjects, DbError>,
    },
    /// The completion catalog (schemas, relations, columns), or why it could not be read.
    Catalog(Result<Catalog, DbError>),
    /// The key constraints of the tables (`DbCommand::LoadKeys`), or why they could not be
    /// read. Sent once after `Catalog` when the session connects, and again on request.
    Keys(Result<KeyCatalog, DbError>),
    /// The answer to `DbCommand::LoadDatabases`: the databases the user may connect to, by name.
    Databases(Result<Vec<String>, DbError>),
    /// The answer to `DbCommand::LoadStructure`: the structure of `schema.table`, or why it
    /// could not be read (a table that no longer exists is the server's error).
    Structure {
        schema: String,
        table: String,
        result: Result<Box<TableStructure>, DbError>,
    },
    /// The answer to `DbCommand::LoadDdl` `id`: the object's DDL as the catalog has it, or why
    /// it could not be read.
    Ddl {
        id: u64,
        result: Result<DdlSource, DbError>,
    },
    /// Where the session works, as the server says right after `Connected` of a session opened
    /// in a context of its own (`Capabilities::contexts`; none for the profile's
    /// defaults): its database and the schemas of its search path that exist
    /// (a chosen schema missing here does not exist or cannot be used).
    Context {
        database: String,
        schemas: Vec<String>,
    },
    /// Sent right before the first `Page` of result `id` (a run's answer, or a `Resume`'s) when
    /// that page has more rows after it and the session did not keep them ([`PagingMode::NoHold`],
    /// outside the user's block): its server-side cursor and the transaction it needed have
    /// ended already. Nothing can be fetched from it (a `FetchMore` would get an empty last
    /// page); past the page the statement can only run again. Never sent for a result held open.
    Released {
        id: u64,
    },
    /// `columns` is `Some` for the first page of a result set.
    Page {
        id: u64,
        columns: Option<Vec<ColumnMeta>>,
        rows: Vec<Vec<Cell>>,
        more: bool,
        elapsed: Duration,
    },
    Done {
        id: u64,
        outcome: Outcome,
        elapsed: Duration,
    },
    Failed {
        id: u64,
        error: DbError,
        cancelled: bool,
    },
    /// What the server said of statement `index` (from 0) of run `id` beyond its outcome; sent
    /// before the statement's `Finished` or the run's answer (for a result read to its end
    /// later, once it ended).
    Info {
        id: u64,
        index: usize,
        info: StatementInfo,
    },
    /// A run of several statements (`Execute`) starts its statement `index` (from 0).
    Started {
        id: u64,
        index: usize,
    },
    /// Rows of statement `index`, a statement before the last of a run of several: it runs to
    /// its end (every row is fetched, as psql does, so volatile functions and row locks take
    /// effect for all of them) and its rows come page by page, `columns` with the first page,
    /// `more` until the last. Its `Finished` follows.
    StepRows {
        id: u64,
        index: usize,
        columns: Option<Vec<ColumnMeta>>,
        rows: Vec<Vec<Cell>>,
        more: bool,
    },
    /// A statement before the last of a run of several ended well: what it did (a statement
    /// with rows reports its command after its `StepRows`). The last one answers the run as
    /// usual.
    Finished {
        id: u64,
        index: usize,
        outcome: Outcome,
        elapsed: Duration,
    },
    /// Whether a transaction is open on the session's connection (an explicit `BEGIN` block,
    /// or the implicit one that holds a result's portal open).
    TxOpen(bool),
    /// Whether the user's own transaction block (a `BEGIN` the user ran) is open: `TxOpen`
    /// also counts the transaction that holds a result's portal open, this does not
    /// (a result read inside the user's block is labelled so, and its portal is never
    /// closed for being idle). Sent when that changes, before the `TxOpen` of the same change.
    Block(bool),
    /// The answer to `DbCommand::Count` for result `id`: the number of rows, or why it could
    /// not be counted (a cancel is `DbError::Cancelled`). `snapshot`: it counted in a
    /// transaction with one snapshot for its whole life (the server said `REPEATABLE READ` or
    /// `SERIALIZABLE` in the count's own request); otherwise it counted the rows committed when
    /// it ran, which may differ from the rows paged.
    Counted {
        id: u64,
        result: Result<u64, DbError>,
        snapshot: bool,
    },
    /// The answer to [`DbCommand::CheckRepeat`] `id`: `Ok` when the server has nothing against
    /// running the statement again.
    RepeatChecked {
        id: u64,
        result: Result<(), DbError>,
    },
    /// Whether the open transaction block is aborted: a statement in it failed, so the server
    /// refuses everything but `ROLLBACK` (or `ROLLBACK TO SAVEPOINT`) until it ends. Sent when
    /// that changes; `TxOpen(false)` ends it too.
    TxAborted(bool),
    /// Whether the session runs with autocommit, as the server says (MySQL's
    /// `SERVER_STATUS_AUTOCOMMIT`), when that changed: sessions start with it on; a statement
    /// of the user's (`SET autocommit = 0`) turns it off for the session, and every statement
    /// then opens a transaction (`Block`, `TxOpen`) that stays open until it is committed or
    /// rolled back.
    Autocommit(bool),
    /// The connection ended after `Connected` without the UI closing the session (network,
    /// server restart, `pg_terminate_backend`). Sent once, after the `Failed` of a statement
    /// that was running. Never sent before `Connected` (that is `ConnectFailed`) nor after
    /// `Session::close`.
    Lost {
        error: DbError,
    },
    /// Run (or count) `id` was cancelled, the server took the cancel, and the session is still
    /// waiting for it to end its statement (a large write rolls back for a long time, saying
    /// nothing meanwhile), the server having just said the statement is still there. Sent every
    /// few seconds while that lasts: the session is waiting on the server, not stuck, and the UI
    /// keeps it (only the driver decides to give a statement up).
    Stopping {
        id: u64,
    },
    /// Run (or count) `id` was cancelled and the server refused the cancel: the statement runs
    /// as another account (MySQL: inside a `SQL SECURITY DEFINER` routine), sends nothing, and
    /// runs on to its end; closing the session would not stop it (the server does not look at
    /// its connection meanwhile), so the session keeps waiting for its answer. Sent every few
    /// seconds while that lasts: the session is waiting on the server, not stuck.
    CancelRefused {
        id: u64,
    },
    /// The answer to [`DbCommand::Export`] `id`: what it read, or why it stopped (a cancel is
    /// `cancelled`, with the error the server gave).
    Exported {
        id: u64,
        result: Result<ExportRead, DbError>,
        cancelled: bool,
    },
    /// Statement `index` of run `id` ran in a transaction block the driver opened right before
    /// it ([`TxRule::Manual`]), and succeeded: the block stays open for the user to commit or
    /// roll back. Sent right before the [`DbEvent::Block`] (`true`) of that block, so a block
    /// without it before is one the user opened.
    ManualBegun {
        id: u64,
        index: usize,
    },
    /// Run `id` failed or was cancelled in a transaction block the driver opened for it
    /// ([`TxRule::Manual`], at statement `index`): the driver rolled the new transaction back at
    /// once, so nothing the run did in it is kept and no block is left open. Sent after the
    /// run's answer; the `Block`/`TxOpen` changes follow when the block had been reported open.
    ManualRolledBack {
        id: u64,
        index: usize,
    },
    /// Run `id` failed or was cancelled in a transaction the driver opened for manual commit
    /// before the run ([`TxRule::Manual`]): everything the run did was rolled back to the
    /// driver's savepoint taken at its start, and the transaction stays open (not aborted) with
    /// what earlier runs did in it. Sent after the run's answer.
    RunRolledBack {
        id: u64,
    },
    /// The transaction open on the session (said with `Block(true)`) was ended by the server, not
    /// by the user's `COMMIT`/`ROLLBACK` nor [`DbCommand::EndTx`], during request `id` (a run's
    /// statement, an `EndTx` the server refused): `how` says whether its work was kept. Sent
    /// before the `Block(false)` of that change (a statement that opens another transaction at
    /// once: before the `Block(false)`, `Block(true)` pair).
    TxClosedBy {
        id: u64,
        how: ServerEnd,
    },
    /// The answer to [`DbCommand::EndTx`] `id`: how the transaction ended. The `Block` and
    /// `TxOpen` changes follow.
    TxEnded {
        id: u64,
        outcome: TxOutcome,
    },
}
