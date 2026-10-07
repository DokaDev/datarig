//! Messages between the UI and a driver [`Session`](super::Session): commands go in on the
//! session's channels, progress and results come back as [`DbEvent`]s.

use super::ddl::{DdlObject, DdlSource};
use super::keys::KeyCatalog;
use super::structure::{RelationStats, TableStructure};
use crate::fault::Fault;
use crate::sql::complete::Catalog;
use crate::sql::risk::repeat::NotRepeatable;
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
    /// The server refused a statement because it ran inside a transaction: one that cannot run
    /// in a transaction block (SQLSTATE `25001`: `VACUUM`, `CREATE INDEX CONCURRENTLY`, `CREATE
    /// DATABASE`, …) or a procedure's transaction control (`2D000`, a `COMMIT` in a `CALL`), in a
    /// transaction the driver opened only to set the session's search path (a session whose
    /// server ignored the path given at connect, a pooler: [`DbEvent::ContextPerTransaction`]).
    /// The server's message. The same failure in the user's own block is a plain `Server`.
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
}

impl DbError {
    /// The raw text (the server's message, or a fault's detail), for matching and the error
    /// log only.
    pub fn raw(&self) -> Cow<'_, str> {
        match self {
            DbError::Server(s) | DbError::NeedsNoTransaction(s) => Cow::Borrowed(s),
            DbError::Settings(f) | DbError::Connection(f) => Cow::Borrowed(&f.detail),
            DbError::Transport(e) => Cow::Borrowed(e.raw()),
            DbError::Closed
            | DbError::NoAnswer(_)
            | DbError::NotSupported
            | DbError::ServerTooOld
            | DbError::RowsNotRead(_)
            | DbError::SchemaChanged
            | DbError::NoResult
            | DbError::Cancelled
            | DbError::ReadWriteRefused
            | DbError::NotRepeatable(_)
            | DbError::Locked
            | DbError::NotFound => Cow::Borrowed(""),
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
    /// has more rows than a page (outside the user's block).
    Execute {
        id: u64,
        statements: Vec<String>,
        paging: PagingMode,
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

#[derive(Debug)]
pub enum DbEvent {
    Connected,
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
    /// The connection ended after `Connected` without the UI closing the session (network,
    /// server restart, `pg_terminate_backend`). Sent once, after the `Failed` of a statement
    /// that was running. Never sent before `Connected` (that is `ConnectFailed`) nor after
    /// `Session::close`.
    Lost {
        error: DbError,
    },
}
