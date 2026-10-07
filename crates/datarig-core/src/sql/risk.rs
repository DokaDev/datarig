//! What a statement may do to a database (the
//! policy items `read_only` and `confirm`), read from **PostgreSQL's own parse tree**. (MySQL
//! text is read by [`mysql`], into the same [`Risk`].)
//!
//! The text is parsed by libpg_query (the `pg_query` crate), which is the server's parser
//! (`gram.y` and `scan.l` of PostgreSQL 17) built as a library. So comments, strings, quoted
//! identifiers, dollar bodies, line ends, case and quoting of option names are read exactly as
//! the server reads them: `EXPLAIN ("analyze") DELETE …` is an `EXPLAIN ANALYZE`, a `--`
//! comment ends at a lone `\r`, and `x<NBSP>$$` is an identifier. A text whose plain strings
//! hold a backslash is parsed a second time as a server with `standard_conforming_strings =
//! off` would read it (each plain string becomes an `E''` string), and the worse of the two
//! readings counts.
//!
//! **Allowlist, not denylist.** A statement is a read only when its parse tree is a query
//! that neither writes nor locks, with no data-modifying statement anywhere in it. A text the
//! parser rejects is [`Danger::Unparsed`]: it always asks, and a read-only policy refuses it.
//! Anything the classifier does not know is [`Class::Unknown`], which the safety checks treat
//! like a write.
//!
//! **Never a crash.** Parsing and reading the tree recurse once per level of nesting, in C
//! (libpg_query) and in Rust (the protobuf decoder, the tree walks), and a long left-deep
//! expression (`SELECT 1 + 1 + …`) nests tens of thousands of levels. So the work runs on a
//! thread of its own with a [`STACK`] of 256 MiB (address space; only what is used is
//! committed), and a text longer than [`MAX_BYTES`] or nested deeper than [`MAX_DEPTH`] (a
//! cheap estimate from the lexer, see [`depth`]) is not parsed at all: it is
//! [`Danger::TooComplex`], which asks like a text the parser rejects and which a read-only
//! policy refuses. The caps also bound the time: libpg_query's output grows with the square of
//! the depth (about 0.1 s at the depth cap in a release build). A panic on that thread is
//! caught and reads as [`Danger::Unparsed`]. The protobuf decoder's own limit of 100 nested
//! messages is turned off (prost's `no-recursion-limit`): it made ordinary queries unreadable
//! (about 46 chained operators or 50 `JOIN`s), and the caps bound the depth instead.
//!
//! **Prepared statements** ([`Prepared`]): a session's `PREPARE name AS …` is remembered with
//! the risk of its statement, and `EXECUTE name` has that risk. `EXECUTE` of a name the session
//! did not prepare is [`Danger::UnknownPrepared`]; `DEALLOCATE` and `DISCARD ALL` forget names.
//! Code the text does not show may prepare or deallocate too (dynamic SQL in a function), so a
//! statement that may run such code ([`Risk::runs_code`]) forgets every name: `DO`, `CALL`, a
//! call of a function that is not a known built-in ([`builtin`]; the built-ins that run SQL or
//! code they are given, [`RUNS_CODE`], are not), anything that may fire a
//! trigger (every write, DDL and maintenance statement, `COMMIT`, `SET CONSTRAINTS`), `FETCH`
//! (the cursor's query runs then) and a text that is not read. Sent is not succeeded: the app
//! applies a statement to its session's names only once the server says it succeeded, and
//! [`Prepared::forget`]s the names of one whose outcome is unknown.
//!
//! **Built-ins that act on the server.** A call of a built-in that reads, writes or lists files
//! of the server ([`SERVER_FILES`]: `lo_export`, `pg_read_file`, …) is [`Danger::ServerFile`],
//! and one that acts on the server beyond the transaction ([`SERVER_ACTIONS`]:
//! `pg_terminate_backend`, `pg_reload_conf`, replication slots, …) is [`Danger::ServerAction`]:
//! they ask, and a read-only policy refuses them before they are sent, since the server's
//! read-only transaction does not stop them. The call is seen in every form the parse tree
//! names it: a bare name, one qualified with any schema or with the database as well
//! (`datarig.pg_catalog.lo_export(…)`; a function of the user's with such a name in another
//! schema asks too), and attribute notation (`('/etc/hostname'::text).pg_read_file`,
//! `t.pg_ls_dir`: see [`Call::Attribute`]; a real column with such a name asks too). Advisory
//! locks, `pg_notify`, `nextval` and the like stay plain reads (the server refuses `nextval`
//! and large object writes itself).
//!
//! **Queries given as text.** A built-in that runs a query given as text ([`query_position`]:
//! `query_to_xml`, `query_to_xmlschema`, `query_to_xml_and_xmlschema`, `ts_stat`,
//! `ts_rewrite(tsquery, text)`) runs whatever that query calls. When the call gives the text as
//! a string constant (also cast to `text`; by position or named `query`, in any form of the
//! call), the text is read as a statement ([`Prepared::query_text`]) and its risk is the
//! call's: a harmless query stays a plain read, one that calls a built-in of [`SERVER_FILES`]
//! asks and is refused under a read-only policy, and a query text in it is read in turn. When
//! it does not (a column, a parameter, a concatenation, the result of a call), when the parser
//! rejects the text and when it is over the caps, the call is [`Danger::RunsQueryText`]: it
//! asks, and a read-only policy refuses it before it is sent. The other built-ins of
//! [`RUNS_CODE`] run no query text of the caller's: `cursor_to_xml` fetches from a cursor,
//! whose `DECLARE` was checked (as for `FETCH`); `table_to_xml` and the like read tables (a
//! view's query runs, as for a `SELECT` of it); `pg_input_is_valid` and `pg_input_error_info`
//! run a type's input function (a domain's `CHECK` runs, as for a cast). They only forget the
//! prepared names, and the server's read-only transaction refuses their writes.
//!
//! **What a plain `EXPLAIN` runs.** Without `ANALYZE` a statement is only planned. The planner
//! folds a call of an immutable function with constant arguments (and of a stable one, to
//! estimate a condition), but never one of a volatile function, and every built-in of
//! [`SERVER_FILES`] and [`SERVER_ACTIONS`] and every one that runs a query given as text is
//! volatile; a function that is not a known built-in forgets the prepared names, since the
//! planner may run it. The exception is `EXECUTE`, bare or in `CREATE TABLE … AS EXECUTE`, with
//! any options (`GENERIC_PLAN` too): the server evaluates its parameters to plan the prepared
//! statement, so they are read as those of an `EXECUTE` that runs ([`Explain::Plan`]).
//!
//! **What the confirm is not.** Functions called from a query cannot be seen from the text
//! (`SELECT delete_everything()` is a read here) and asking for every function call would ask
//! for nearly every query, so it does not. Nor is code the text does not name as a function
//! call seen: a view's query, a row-level security policy, an operator, a cast, a domain's
//! `CHECK` or the input function of a user-defined type, a function of the user's called with
//! attribute notation (`SELECT t.f FROM t t`, `(t).f`, `(t.*).f`: a column reference to the
//! parser, a call only once the catalog is read; the built-ins of [`SERVER_FILES`],
//! [`SERVER_ACTIONS`] and [`RUNS_CODE`] are seen in it), and a user function that shares a
//! built-in's name without naming its schema (an overload, for any argument types, in a schema
//! of `search_path`, such as `myschema.upper(int)` for `upper(1)`; or any function of that name
//! in a schema ahead of `pg_catalog`) all run on a read without forgetting the prepared names.
//! The confirm is a guardrail against mistakes, not a security boundary. The hard guarantee is
//! a read-only policy: every transaction the driver opens for such a profile is `READ ONLY` on
//! the server, which rejects any write, whatever runs it. The classifier also refuses
//! `set_config()` of the read-only settings under such a policy, so a query cannot turn the
//! session's default off. What the server's read-only transaction does not stop (the
//! built-ins of [`SERVER_FILES`] and [`SERVER_ACTIONS`]) is refused only where the text shows
//! the call, directly or in a query given as a string constant: one made by a function of the
//! user's, a view, a policy or a trigger is not seen.
//!
//! "Effectively every row" ([`NoWhere`]): no `WHERE`; a `WHERE` recognised as always true
//! (`true`, `NOT false`, a string that reads as true, both sides of `=` the same, a top-level
//! `OR` with such an operand, an `AND` of them); and a `WHERE` that reads no column at all
//! (`1 <> 0`, `true AND true`, `random() < 2`), which matches every row or none. Other
//! conditions that happen to be always true (`id > 0`) are not recognised.

mod classifier;
pub mod mysql;
pub mod repeat;

pub use classifier::Classifier;

use super::lexer::{Tok, lex, lex_backslash_strings};
use pg_query::protobuf;
use serde_json::Value;
use std::collections::HashMap;

/// What kind of statement it is, from the harmless to the unknown (the order is the order in
/// which two classes combine: the later one wins).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// Reads only: `SELECT`, `VALUES`, `TABLE`, `SHOW`, `FETCH`, `COPY … TO STDOUT`, `EXPLAIN`
    /// without `ANALYZE`, …
    Read,
    /// Changes a setting of the session: `SET`, `RESET`, `DISCARD`, `LISTEN`, `DEALLOCATE`, …
    Session,
    /// Transaction control: `BEGIN`, `COMMIT`, `ROLLBACK`, `SAVEPOINT`, `SET TRANSACTION`, …
    Tx,
    /// Changes rows or takes row locks: `INSERT`, `UPDATE`, `DELETE`, `MERGE`, `COPY … FROM`,
    /// `SELECT … FOR UPDATE`, `LOCK`, `REFRESH MATERIALIZED VIEW`, a data-modifying `WITH`, …
    Write,
    /// Changes the schema or privileges: `CREATE`, `ALTER`, `DROP`, `TRUNCATE`, `COMMENT`,
    /// `GRANT`, `REVOKE`, `SELECT … INTO`, …
    Ddl,
    /// `VACUUM`, `ANALYZE`, `REINDEX`, `CLUSTER`, `CHECKPOINT`.
    Maintenance,
    /// Runs code the text does not show: `DO`, `CALL`.
    Procedural,
    /// Anything else (`LOAD`, an `EXECUTE` of a statement the session did not prepare, a text
    /// the parser rejects, no statement at all).
    Unknown,
}

/// `EXPLAIN` around the statement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Explain {
    /// Not an `EXPLAIN`.
    #[default]
    No,
    /// `EXPLAIN` without `ANALYZE`: only planned, never run (a read, whatever it wraps). The
    /// parameters of an `EXECUTE` it wraps are the exception: the server evaluates them to plan
    /// the prepared statement, so what they call counts as if it ran.
    Plan,
    /// `EXPLAIN ANALYZE` (or `ANALYSE`, or the option in parentheses, in any quoting or case):
    /// the wrapped statement runs. The risk is the wrapped statement's.
    Analyze,
}

/// An `UPDATE` or `DELETE` that changes every row, or may.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoWhere {
    /// No `WHERE` at all.
    Missing,
    /// A `WHERE` that is always true (see the module docs for what is recognised).
    AlwaysTrue,
    /// A `WHERE` that reads no column: every row or none, whatever the table holds.
    NoColumn,
}

/// Why a statement asks before it runs under the default policy (`confirm = "destructive"`):
/// it loses data or objects in a way a later statement cannot undo once committed, or what it
/// does cannot be known from its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Danger {
    /// `DROP` of any object, or an `ALTER` that drops something that is not a column.
    Drop,
    Truncate,
    /// `DELETE` of every row.
    DeleteAll,
    /// `UPDATE` of every row.
    UpdateAll,
    /// `ALTER TABLE … DROP [COLUMN]` (or `DROP ATTRIBUTE` of a type).
    DropColumn,
    /// `ALTER TABLE … ALTER COLUMN … TYPE`: every value of the column is converted, with or
    /// without `USING`, and a conversion can lose data (`numeric` to `int` rounds 1.75 to 2).
    AlterColumnType,
    /// `MERGE` with a `WHEN … THEN UPDATE` or `THEN DELETE`: which rows it matches depends on
    /// its join, which the text cannot judge, so it always asks.
    Merge,
    /// `DO` or `CALL`: runs code the text does not show.
    Procedural,
    /// `COPY … FROM PROGRAM` or `TO PROGRAM`: runs a command of the server's operating system.
    CopyProgram,
    /// `COPY … FROM` or `TO` a file name: reads or writes a file on the server.
    CopyFile,
    /// A call of a built-in that reads, writes or lists files on the server ([`SERVER_FILES`]).
    ServerFile,
    /// A call of a built-in that acts on the server beyond the transaction, which a rollback
    /// does not undo and a read-only transaction does not stop ([`SERVER_ACTIONS`]).
    ServerAction,
    /// A call of a built-in that runs a query given as text (`query_to_xml`, `ts_stat`, …: see
    /// [`query_position`]) whose text the classifier cannot read: not a string constant (a
    /// column, a parameter, a concatenation, …), a text the parser rejects, or one over the caps
    /// of [`Prepared::query_text`]. What the query does is unknown, and a read-only transaction
    /// does not stop a built-in of [`SERVER_FILES`] or [`SERVER_ACTIONS`] it may call.
    RunsQueryText,
    /// `EXECUTE` of a name whose statement is unknown: the session did not prepare it as far
    /// as the server confirmed (never prepared here, or a `PREPARE`, `DEALLOCATE` or `DO` of
    /// it whose outcome is unknown).
    UnknownPrepared,
    /// PostgreSQL's parser rejects the text (or reading it failed): what the server would do
    /// with it is unknown.
    Unparsed,
    /// The text is longer than [`MAX_BYTES`] or nested deeper than [`MAX_DEPTH`]: it is not
    /// parsed, so what it does is unknown.
    TooComplex,
    /// MySQL: an executable comment (`/*! … */`, MariaDB's `/*M! … */`), whose code the server
    /// runs or skips depending on its version.
    ExecutableComment,
    /// MySQL: a form of statement the classifier does not know, or a text the server would read
    /// otherwise than its lexer (see [`mysql`]): what it does is unknown.
    Unrecognized,
    /// MySQL: takes locks that block other sessions until it releases them (`LOCK TABLES`,
    /// `HANDLER`, `FLUSH TABLES … WITH READ LOCK`, `LOCK INSTANCE FOR BACKUP`).
    Locks,
    /// MySQL: changes accounts or privileges (`GRANT`, `REVOKE`, `CREATE USER`, `SET
    /// PASSWORD`, …).
    Privileges,
    /// MySQL: renames a table, a column, an index or another object (`RENAME TABLE`, `ALTER …
    /// RENAME`): what uses the old name breaks.
    Rename,
    /// MySQL: changes a session setting that turns a check of the server off, changes how the
    /// server reads the text or what reaches the binary log ([`mysql::RISKY_SETTINGS`], a client
    /// character set that is not UTF-8).
    Setting,
    /// MySQL: `PREPARE` of SQL given as text, which the classifier does not read.
    DynamicSql,
    /// MySQL: a statement that acts on the whole server beyond this session (`KILL`, `SET
    /// GLOBAL`, `FLUSH`, `RESET`, `PURGE`, replication, `INSTALL`, `SHUTDOWN`, `XA`, …).
    ServerCommand,
    /// MySQL: reads or writes a file of the server or of the client (`LOAD DATA`, `SELECT …
    /// INTO OUTFILE`/`DUMPFILE`).
    FileAccess,
}

/// What a statement may do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Risk {
    pub class: Class,
    pub explain: Explain,
    /// It changes rows (a write, `TRUNCATE`, `SELECT … INTO`, `CREATE TABLE … AS`, …). A lock
    /// or a `COPY … TO` a file is a [`Class::Write`] that changes no rows.
    pub writes: bool,
    pub no_where: Option<NoWhere>,
    pub danger: Option<Danger>,
    /// The table (or object) it acts on, when the text names one: with its schema, quoted
    /// where PostgreSQL needs quotes.
    pub target: Option<String>,
    /// A session statement that a read-only policy allows: a setting on the allowlist
    /// ([`SAFE_SETTINGS`], names with a `.`, the `enable_*` planner switches), `RESET ALL`,
    /// `DISCARD`, `LISTEN`, `UNLISTEN`, `DEALLOCATE`, `PREPARE … AS`.
    pub safe_setting: bool,
    /// It asks for a read-write transaction or may turn read-only off: `BEGIN READ WRITE`,
    /// `SET TRANSACTION READ WRITE`, `SET default_transaction_read_only = off`, `RESET
    /// transaction_read_only`, a call of `set_config()` on one of those settings, …
    pub read_write: bool,
    /// `COPY … FROM STDIN` or `COPY … TO STDOUT`: the data would go through the connection's
    /// COPY protocol, which the app does not carry yet.
    pub stdio: bool,
    /// It may run code the text does not show, which may prepare or deallocate statements (see
    /// the module docs): after it, whatever its outcome, the session's prepared names are
    /// unknown.
    pub runs_code: bool,
    /// It sets `search_path` for the session, not only for its transaction: `SET
    /// search_path …` or `SET SESSION search_path …` (not `SET LOCAL`), or a call of
    /// `set_config()` of it (or of a setting it names with something other than a constant)
    /// whose third argument is not the constant `true`. Behind a pooler whose tab sets its path
    /// per transaction, the tab ignores it and it stays on the pooled connection.
    pub session_path: bool,
    /// MySQL: the server commits the open transaction before it runs (DDL, `LOCK TABLES`,
    /// `START TRANSACTION`, `SET autocommit = 1`, …: see [`mysql`]).
    pub implicit_commit: bool,
    /// MySQL: it calls an unqualified name that is not a built-in, which may be a loadable
    /// function: code of the server's that a read-only transaction does not stop, so a
    /// read-only policy refuses it.
    pub unchecked_call: bool,
}

/// Why a read-only policy refuses a statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadOnlyBlock {
    /// Its class may change data (or is unknown).
    Class(Class),
    /// It asks for a read-write transaction or turns read-only off.
    ReadWrite,
    /// It changes a setting that is not on the allowlist.
    Setting,
    /// PostgreSQL's parser rejects it ([`Danger::Unparsed`]).
    Unparsed,
    /// It is too long or too deeply nested to check ([`Danger::TooComplex`]).
    TooComplex,
    /// It reads, writes or lists files on the server ([`Danger::ServerFile`]).
    ServerFile,
    /// It acts on the server beyond the transaction ([`Danger::ServerAction`]).
    ServerAction,
    /// It runs a query given as text that cannot be checked ([`Danger::RunsQueryText`]).
    RunsQueryText,
    /// It has an executable comment ([`Danger::ExecutableComment`]).
    ExecutableComment,
    /// Its form is not one the classifier knows ([`Danger::Unrecognized`]).
    Unrecognized,
    /// It calls a function that may be a loadable one ([`Risk::unchecked_call`]).
    UnknownFunction,
    /// It prepares SQL given as text ([`Danger::DynamicSql`]).
    DynamicSql,
    /// It acts on the whole server ([`Danger::ServerCommand`]).
    ServerCommand,
}

/// Why a statement needs a confirmation before it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// It is dangerous (always asked).
    Danger(Danger),
    /// It may change something, and the policy asks for every such statement
    /// (`confirm = "writes"`): its class.
    Class(Class),
}

/// Settings a read-only policy lets a session change: none of them can turn read-only off or
/// write anything. Names are lower case, as PostgreSQL folds them.
pub const SAFE_SETTINGS: &[&str] = &[
    "application_name",
    "bytea_output",
    "client_encoding",
    "client_min_messages",
    "constraint_exclusion",
    "cpu_index_tuple_cost",
    "cpu_operator_cost",
    "cpu_tuple_cost",
    "cursor_tuple_fraction",
    "datestyle",
    "default_statistics_target",
    "default_transaction_deferrable",
    "default_transaction_isolation",
    "effective_cache_size",
    "effective_io_concurrency",
    "extra_float_digits",
    "from_collapse_limit",
    "geqo",
    "idle_in_transaction_session_timeout",
    "idle_session_timeout",
    "intervalstyle",
    "jit",
    "join_collapse_limit",
    "lc_monetary",
    "lc_numeric",
    "lc_time",
    "lock_timeout",
    "maintenance_work_mem",
    "max_parallel_workers_per_gather",
    "parallel_setup_cost",
    "parallel_tuple_cost",
    "plan_cache_mode",
    "random_page_cost",
    "role",
    "row_security",
    "search_path",
    "seq_page_cost",
    "statement_timeout",
    "temp_buffers",
    "timezone",
    "transaction_deferrable",
    "transaction_isolation",
    "transaction_timeout",
    "work_mem",
    "xmloption",
];

/// The longest text the classifier parses, in bytes; a longer one is [`Danger::TooComplex`].
pub const MAX_BYTES: usize = 256 * 1024;

/// The deepest nesting the classifier parses, as [`depth`] estimates it; a deeper text is
/// [`Danger::TooComplex`]. About 8000 terms of `1 + 1 + …`, 8000 `JOIN`s or 8000 `UNION`s; a
/// list (`IN (…)`, `VALUES (…), (…)`, the columns of a `SELECT`) and a
/// chain of `AND`/`OR` do not nest, so their length does not count.
pub const MAX_DEPTH: usize = 16_000;

/// The stack of the thread that parses and classifies. At [`MAX_DEPTH`] a release build uses
/// under 32 MiB and a debug build (the tests, with pg_query optimized: see the workspace
/// manifest) under 96 MiB. It is address space; only the pages a parse touches are committed.
pub const STACK: usize = 256 << 20;

/// The name of that thread (the binary's panic hook leaves the terminal alone for a panic on
/// it: the panic is caught and the statement reads as [`Danger::Unparsed`]).
pub const THREAD: &str = "datarig-classify";

/// The settings that make a transaction read-only.
const READ_ONLY_SETTINGS: [&str; 2] = ["default_transaction_read_only", "transaction_read_only"];

impl Risk {
    fn of(class: Class) -> Self {
        Self {
            class,
            explain: Explain::No,
            writes: false,
            no_where: None,
            danger: None,
            target: None,
            safe_setting: false,
            read_write: false,
            stdio: false,
            runs_code: false,
            session_path: false,
            implicit_commit: false,
            unchecked_call: false,
        }
    }

    fn writing(class: Class, target: Option<String>) -> Self {
        Self { writes: true, target, ..Self::of(class) }
    }

    fn danger(class: Class, danger: Danger) -> Self {
        Self { danger: Some(danger), ..Self::of(class) }
    }

    fn session(safe: bool) -> Self {
        Self { safe_setting: safe, ..Self::of(Class::Session) }
    }

    /// The worse of two risks (a statement read two ways, or a part of it).
    fn merge(self, o: Risk) -> Risk {
        Risk {
            class: self.class.max(o.class),
            explain: if self.explain == Explain::No { o.explain } else { self.explain },
            writes: self.writes || o.writes,
            no_where: self.no_where.or(o.no_where),
            danger: self.danger.or(o.danger),
            target: self.target.or(o.target),
            safe_setting: self.safe_setting && o.safe_setting,
            read_write: self.read_write || o.read_write,
            stdio: self.stdio || o.stdio,
            runs_code: self.runs_code || o.runs_code,
            session_path: self.session_path || o.session_path,
            implicit_commit: self.implicit_commit || o.implicit_commit,
            unchecked_call: self.unchecked_call || o.unchecked_call,
        }
    }

    /// Whether a read-only policy lets it run: a read (`EXPLAIN` without `ANALYZE` of
    /// anything is one, unless the parameters of an `EXPLAIN EXECUTE` are refused), transaction
    /// control that does not ask for read-write, and the safe session statements. Whatever the
    /// class, not a call of a built-in that acts on the server, nor a query given as text that
    /// cannot be checked, since the server's read-only transaction does not stop them.
    pub fn read_only(&self) -> Result<(), ReadOnlyBlock> {
        match self.danger {
            Some(Danger::Unparsed) => return Err(ReadOnlyBlock::Unparsed),
            Some(Danger::TooComplex) => return Err(ReadOnlyBlock::TooComplex),
            // The server's read-only transaction does not stop them.
            Some(Danger::ServerFile) => return Err(ReadOnlyBlock::ServerFile),
            Some(Danger::ServerAction) => return Err(ReadOnlyBlock::ServerAction),
            Some(Danger::RunsQueryText) => return Err(ReadOnlyBlock::RunsQueryText),
            Some(Danger::ExecutableComment) => return Err(ReadOnlyBlock::ExecutableComment),
            Some(Danger::Unrecognized) => return Err(ReadOnlyBlock::Unrecognized),
            Some(Danger::DynamicSql) => return Err(ReadOnlyBlock::DynamicSql),
            Some(Danger::ServerCommand) => return Err(ReadOnlyBlock::ServerCommand),
            _ => {}
        }
        if self.unchecked_call {
            return Err(ReadOnlyBlock::UnknownFunction);
        }
        if self.read_write {
            return Err(ReadOnlyBlock::ReadWrite);
        }
        match self.class {
            Class::Read | Class::Tx => Ok(()),
            Class::Session if self.safe_setting => Ok(()),
            Class::Session => Err(ReadOnlyBlock::Setting),
            c => Err(ReadOnlyBlock::Class(c)),
        }
    }

    /// Why it needs a confirmation, if it does: a dangerous statement always; with `writes`
    /// (the policy's `confirm = "writes"`) also everything that is not a read, a session
    /// statement or transaction control (unknown and procedural statements included).
    pub fn confirm(&self, writes: bool) -> Option<Why> {
        if let Some(d) = self.danger {
            return Some(Why::Danger(d));
        }
        let harmless = matches!(self.class, Class::Read | Class::Session | Class::Tx);
        (writes && !harmless).then_some(Why::Class(self.class))
    }

    /// `EXPLAIN ANALYZE`: the statement runs to measure its plan, so the driver runs it in a
    /// transaction (or savepoint) that it rolls back, whatever it wraps.
    pub fn rolls_back(&self) -> bool {
        self.explain == Explain::Analyze
    }

    /// `EXPLAIN ANALYZE` of something that is not a plain read: the rollback undid changes,
    /// and the user is told so.
    pub fn rollback_matters(&self) -> bool {
        self.rolls_back() && self.class != Class::Read
    }
}

/// The risk of `sql` on a session that prepared nothing: one statement, or several separated
/// by `;` (their worst).
pub fn classify(sql: &str) -> Risk {
    Prepared::default().classify(sql)
}

/// The prepared statements of one session, by name, with the risk of the statement each one
/// runs. [`Prepared::classify`] reads `EXECUTE` through it and keeps it up to date with the
/// `PREPARE`, `DEALLOCATE` and `DISCARD ALL` it classifies, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Prepared(HashMap<String, Risk>);

impl Prepared {
    /// The risk of `sql` (one statement or several) on this session, and what its `PREPARE`,
    /// `DEALLOCATE` and `DISCARD ALL` do to the session's names. A text whose plain strings
    /// hold a backslash is read a second time as a server with `standard_conforming_strings =
    /// off` reads it; the worse reading counts (only the standard one changes the names).
    ///
    /// A text over the caps ([`MAX_BYTES`], [`MAX_DEPTH`]) is [`Danger::TooComplex`] without
    /// being parsed; the rest is read on a thread with a large stack (see the module docs).
    /// When the text is not read (over the caps, a failure) and may prepare or deallocate,
    /// every name is forgotten: what they hold is unknown.
    pub fn classify(&mut self, sql: &str) -> Risk {
        if sql.len() > MAX_BYTES || depth(sql) > MAX_DEPTH {
            self.unsure(sql);
            return Risk::danger(Class::Unknown, Danger::TooComplex);
        }
        let read = std::thread::scope(|scope| {
            let worker = std::thread::Builder::new().name(THREAD.to_string()).stack_size(STACK);
            let this = &mut *self;
            worker.spawn_scoped(scope, move || this.classify_here(sql)).ok().and_then(|h| h.join().ok())
        });
        read.unwrap_or_else(|| {
            self.0.clear();
            Risk::danger(Class::Unknown, Danger::Unparsed)
        })
    }

    /// Forget every name when `sql`, which was not read, may prepare, deallocate or discard, or
    /// run code that may: unless each of its words is one of [`INERT_WORDS`] (a query of
    /// constants, which names no table, function or type).
    fn unsure(&mut self, sql: &str) {
        if lex(sql).iter().any(|t| t.is_word() && !INERT_WORDS.iter().any(|w| t.text(sql).eq_ignore_ascii_case(w))) {
            self.0.clear();
        }
    }

    /// [`Prepared::classify`] on the current thread.
    fn classify_here(&mut self, sql: &str) -> Risk {
        let before = self.clone();
        let risk = self.read(sql);
        let Some(other) = non_conforming(sql) else { return risk };
        let other = before.clone().read(&other);
        if other == risk { risk } else { risk.merge(other) }
    }

    /// Forget every name `sql` may have prepared, deallocated or discarded, when whether it
    /// succeeded is unknown (it failed, was cancelled, or its run stopped before it): an
    /// `EXECUTE` of such a name asks ([`Danger::UnknownPrepared`]). Its new statement is never
    /// assumed.
    pub fn forget(&mut self, sql: &str) {
        let mut after = self.clone();
        after.classify(sql);
        self.0.retain(|name, risk| after.0.get(name) == Some(risk));
    }

    /// Whether the session has a statement prepared as `name` (as PostgreSQL folds it).
    pub fn knows(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }

    /// One reading of `sql`: every statement the parser finds, in order.
    fn read(&mut self, sql: &str) -> Risk {
        let Some(statements) = parse(sql) else {
            self.unsure(sql);
            return Risk::danger(Class::Unknown, Danger::Unparsed);
        };
        let each = statements.iter().map(|s| {
            let risk = self.statement(s);
            if risk.runs_code {
                self.0.clear();
            }
            risk
        });
        each.reduce(Risk::merge).unwrap_or_else(|| Risk::of(Class::Unknown))
    }

    /// The risk of one statement (a `Node` of the parse tree).
    fn statement(&mut self, n: &Value) -> Risk {
        let Some((kind, b)) = node(n) else { return Risk::of(Class::Unknown) };
        let risk = match kind {
            "SelectStmt" => Risk::of(Class::Read),
            "InsertStmt" | "UpdateStmt" | "DeleteStmt" | "MergeStmt" => dml(kind, b),
            "CopyStmt" => self.copy(b),
            "ExplainStmt" => self.explain(b),
            "DeclareCursorStmt" => self.statement(&b["query"]),
            "CreateTableAsStmt" => {
                let target = relation(&b["into"]["rel"]);
                self.statement(&b["query"]).merge(Risk::writing(Class::Ddl, target))
            }
            "VariableShowStmt" | "FetchStmt" | "ClosePortalStmt" => Risk::of(Class::Read),
            "TransactionStmt" => {
                // `COMMIT PREPARED` makes a prepared transaction's writes (any session's)
                // permanent, and `ROLLBACK PREPARED` throws them away: not for a read-only
                // policy.
                use protobuf::TransactionStmtKind as K;
                let finishes_prepared = [K::TransStmtCommitPrepared, K::TransStmtRollbackPrepared]
                    .iter()
                    .any(|k| b["kind"].as_i64() == Some(*k as i64));
                Risk { read_write: finishes_prepared || options_read_write(&b["options"]), ..Risk::of(Class::Tx) }
            }
            "ConstraintsSetStmt" => Risk::of(Class::Tx),
            "VariableSetStmt" => set(b),
            "DiscardStmt" => {
                if b["target"].as_i64() == Some(protobuf::DiscardMode::DiscardAll as i64) {
                    self.0.clear();
                }
                Risk::session(true)
            }
            "DeallocateStmt" => {
                match text(&b["name"]) {
                    Some(name) if !name.is_empty() && b["isall"] != Value::Bool(true) => {
                        self.0.remove(name);
                    }
                    _ => self.0.clear(),
                }
                Risk::session(true)
            }
            "PrepareStmt" => {
                let body = self.statement(&b["query"]);
                if let Some(name) = text(&b["name"]) {
                    self.0.insert(name.to_string(), body);
                }
                // Preparing runs nothing; `EXECUTE` has the statement's risk.
                return Risk::session(true);
            }
            "ExecuteStmt" => match text(&b["name"]).and_then(|name| self.0.get(name)) {
                Some(body) => body.clone(),
                None => Risk::danger(Class::Unknown, Danger::UnknownPrepared),
            },
            "ListenStmt" | "UnlistenStmt" => Risk::session(true),
            // Its code may prepare or deallocate (dynamic SQL): the names are unknown after it
            // (see `runs_code` below).
            "DoStmt" | "CallStmt" => Risk::danger(Class::Procedural, Danger::Procedural),
            "VacuumStmt" | "ReindexStmt" | "ClusterStmt" | "CheckPointStmt" => Risk::of(Class::Maintenance),
            "LockStmt" | "NotifyStmt" => Risk::of(Class::Write),
            "RefreshMatViewStmt" => Risk::writing(Class::Write, relation(&b["relation"])),
            "TruncateStmt" => {
                Risk { danger: Some(Danger::Truncate), ..Risk::writing(Class::Ddl, names(b["relations"].as_array())) }
            }
            "DropStmt" => drops(names(b["objects"].as_array())),
            "DropOwnedStmt" | "DropRoleStmt" => drops(names(b["roles"].as_array())),
            "DropdbStmt" => drops(text(&b["dbname"]).map(quote)),
            "DropTableSpaceStmt" => drops(text(&b["tablespacename"]).map(quote)),
            "DropSubscriptionStmt" => drops(text(&b["subname"]).map(quote)),
            "DropUserMappingStmt" => drops(None),
            "AlterTableStmt" => alter_table(b),
            "AlterPublicationStmt"
                if b["action"].as_i64() == Some(protobuf::AlterPublicationAction::ApDropObjects as i64) =>
            {
                Risk { target: text(&b["pubname"]).map(quote), ..drops(None) }
            }
            "AlterExtensionContentsStmt" if b["action"].as_i64() == Some(-1) => {
                Risk { target: text(&b["extname"]).map(quote), ..drops(None) }
            }
            "AlterOpFamilyStmt" if b["is_drop"] == Value::Bool(true) => drops(None),
            "AlterTsconfigurationStmt"
                if b["kind"].as_i64() == Some(protobuf::AlterTsConfigType::AlterTsconfigDropMapping as i64) =>
            {
                drops(None)
            }
            "RuleStmt"
            | "ViewStmt"
            | "IndexStmt"
            | "CompositeTypeStmt"
            | "CreatedbStmt"
            | "DefineStmt"
            | "CommentStmt"
            | "GrantStmt"
            | "GrantRoleStmt"
            | "SecLabelStmt"
            | "ImportForeignSchemaStmt"
            | "ReassignOwnedStmt"
            | "RenameStmt" => Risk::of(Class::Ddl),
            k if k.starts_with("Create") || k.starts_with("Alter") => Risk::of(Class::Ddl),
            _ => Risk::of(Class::Unknown),
        };
        // What runs now: every data-modifying statement, row lock and `SELECT … INTO` below a
        // query (in a `WITH` at any depth, a subquery, …), and the calls of `runs_now`. A rule's
        // actions, a view's query and a function's body do not run now, nor what `EXPLAIN` only
        // plans (`EXPLAIN ANALYZE` classifies its statement, which is scanned then; a plain
        // `EXPLAIN` scans the parameters of an `EXECUTE`, which the server evaluates).
        let runs = matches!(kind, "SelectStmt" | "InsertStmt" | "UpdateStmt" | "DeleteStmt" | "MergeStmt");
        let mut risk = if runs { risk.merge(below(b)) } else { risk };
        let now = !matches!(kind, "RuleStmt" | "ViewStmt" | "CreateFunctionStmt" | "ExplainStmt");
        if now {
            risk = self.runs_now(b, risk);
        }
        // Code the text does not show, which may prepare or deallocate: a function that is not
        // a known built-in (also one `EXPLAIN` only plans: the planner may run it), a trigger
        // (a write, DDL or maintenance may fire one; `COMMIT` and `SET CONSTRAINTS` fire the
        // deferred ones), a cursor's query (`FETCH`, `MOVE`), `DO`, `CALL`, `LOAD`, anything
        // unknown. What the statement it wraps or executes runs is already in `risk`.
        risk.runs_code |= !matches!(risk.class, Class::Read | Class::Session | Class::Tx)
            || matches!(kind, "FetchStmt" | "ConstraintsSetStmt")
            || (kind == "TransactionStmt" && !quiet_transaction(b))
            || calls_code(b);
        risk
    }

    /// `COPY t FROM …` writes; `COPY … TO STDOUT` reads (what its query does counts);
    /// `COPY … TO` a file or a program writes on the server. `FROM STDIN` and `TO STDOUT` are
    /// [`Risk::stdio`]. A program or a file on the server, in either direction, is dangerous
    /// ([`Danger::CopyProgram`], [`Danger::CopyFile`]): it runs a command or reads or writes a
    /// file as the server's operating system user, which no later statement can undo. The
    /// target of `COPY … FROM` is the table it writes; that of `COPY … TO` a file or a program
    /// is the file or the program (as a string), since the table is only read.
    fn copy(&mut self, b: &Value) -> Risk {
        let target = relation(&b["relation"]);
        let danger = if b["is_program"] == Value::Bool(true) {
            Some(Danger::CopyProgram)
        } else if text(&b["filename"]).is_some_and(|f| !f.is_empty()) {
            Some(Danger::CopyFile)
        } else {
            None
        };
        if b["is_from"] == Value::Bool(true) {
            return Risk { stdio: danger.is_none(), danger, ..Risk::writing(Class::Write, target) };
        }
        let query = if b["query"].is_null() { Risk::of(Class::Read) } else { self.statement(&b["query"]) };
        match danger {
            // The server's side first: its danger and its file are the ones said.
            Some(_) => {
                let file = text(&b["filename"]).map(|f| format!("'{}'", f.replace('\'', "''")));
                Risk { danger, target: file, ..Risk::of(Class::Write) }.merge(query)
            }
            None => Risk { stdio: true, ..query },
        }
    }

    /// `EXPLAIN [(options)] <statement>`: with `ANALYZE` (also `ANALYSE`, in any case or
    /// quoting; a value that is not clearly false counts as true) the statement runs and the
    /// risk is its own.
    fn explain(&mut self, b: &Value) -> Risk {
        let analyze = b["options"].as_array().into_iter().flatten().filter_map(node).any(|(kind, o)| {
            let name = text(&o["defname"]).unwrap_or_default();
            kind == "DefElem"
                && (name.eq_ignore_ascii_case("analyze") || name.eq_ignore_ascii_case("analyse"))
                && !is_false(&o["arg"])
        });
        if !analyze {
            return self.plans(&b["query"]);
        }
        Risk { explain: Explain::Analyze, ..self.statement(&b["query"]) }
    }

    /// `EXPLAIN` without `ANALYZE` of `query`: a read, which runs nothing of it but the
    /// parameters of an `EXECUTE` (bare, or in a `CREATE TABLE … AS EXECUTE`), which the server
    /// evaluates to plan the prepared statement (PostgreSQL 17's `ExplainExecuteQuery` calls
    /// `EvaluateParams`). What they call counts as in an `EXECUTE` that runs. Planning its
    /// statement with them may run a function the planner folds (an immutable or stable one
    /// called with constants): when the statement may run code the text does not show, the
    /// prepared names are forgotten, as for a plain `EXPLAIN` of a query that calls such a
    /// function. The built-ins of [`SERVER_FILES`], [`SERVER_ACTIONS`] and those that run a
    /// query given as text are volatile, which the planner never folds.
    fn plans(&mut self, query: &Value) -> Risk {
        let mut executes = Vec::new();
        let mut visit = |o: &serde_json::Map<String, Value>| executes.extend(o.get("ExecuteStmt").cloned());
        if let Some(o) = query.as_object() {
            visit(o);
        }
        walk(query, &mut visit);
        let mut risk = Risk { explain: Explain::Plan, ..Risk::of(Class::Read) };
        for e in executes {
            let prepared = text(&e["name"]).and_then(|name| self.0.get(name));
            risk.runs_code |= prepared.is_some_and(|p| p.runs_code);
            risk = self.runs_now(&e["params"], risk);
        }
        risk
    }

    /// `risk` with what the calls in `v` do when they run: `set_config()` of the read-only
    /// settings asks for read-write; a built-in that acts on the server where a read-only
    /// transaction does not stop it is dangerous (unless `risk` is dangerous already, the
    /// function is what the confirm names); and the query a built-in runs from its text has its
    /// own risk ([`Prepared::query_text`]).
    fn runs_now(&mut self, v: &Value, mut risk: Risk) -> Risk {
        if sets_read_only(v) {
            risk.read_write = true;
        }
        if sets_session_path(v) {
            risk.session_path = true;
        }
        if let Some((danger, name)) = server_call(v).filter(|_| risk.danger.is_none()) {
            risk = Risk { danger: Some(danger), target: Some(name), ..risk };
        }
        for (name, query) in query_texts(v) {
            let query = self.query_text(&name, query.as_deref());
            // Likewise, the query's danger and what it names.
            let target = if risk.danger.is_none() && query.danger.is_some() { query.target.clone() } else { None };
            risk = risk.merge(query);
            risk.target = target.or(risk.target);
        }
        risk
    }

    /// The risk of the query that the built-in `name` runs from its text, `query` (`None`: the
    /// call does not give it as a string constant, [`string_constant`]). The text is read as a
    /// statement of this session would be (both readings of a backslash, the prepared names,
    /// query texts in it in turn), on a copy of the names: the call forgets them anyway
    /// ([`RUNS_CODE`]). Its `EXPLAIN ANALYZE` and `COPY … TO STDOUT` do not change how the app
    /// runs the call (the server runs the text as a read-only query, which refuses both).
    ///
    /// [`Danger::RunsQueryText`] when the text is not a constant, when the parser rejects it,
    /// and when it is over the caps: longer than [`MAX_BYTES`] or nested deeper than
    /// [`MAX_DEPTH`] itself, more than [`MAX_QUERY_TEXT_NESTING`] query texts deep, or past
    /// [`MAX_BYTES`] of query texts in all for the statement being classified (the thread's
    /// [`QUERY_TEXTS`]). The caps keep the work of a text in one text in one … bounded like
    /// that of the text itself.
    fn query_text(&self, name: &str, query: Option<&str>) -> Risk {
        let unknown =
            Risk { danger: Some(Danger::RunsQueryText), target: Some(name.to_string()), ..Risk::of(Class::Read) };
        let Some(query) = query else { return unknown };
        let (nesting, read) = QUERY_TEXTS.get();
        let read = read.saturating_add(query.len());
        if nesting >= MAX_QUERY_TEXT_NESTING || read > MAX_BYTES || depth(query) > MAX_DEPTH {
            return unknown;
        }
        QUERY_TEXTS.set((nesting + 1, read));
        let risk = self.clone().classify_here(query);
        QUERY_TEXTS.set((nesting, QUERY_TEXTS.get().1));
        match risk.danger {
            Some(Danger::Unparsed | Danger::TooComplex) => unknown,
            _ => Risk { explain: Explain::No, stdio: false, ..risk },
        }
    }
}

/// How many query texts deep [`Prepared::query_text`] reads: a query text in a query text in
/// … deeper than this is [`Danger::RunsQueryText`].
pub const MAX_QUERY_TEXT_NESTING: usize = 8;

thread_local! {
    /// For the caps of [`Prepared::query_text`], on the thread that classifies: how many query
    /// texts deep the reading is now, and how many bytes of query texts it has read. Each
    /// classification runs on a thread of its own ([`Prepared::classify`]), so both start at
    /// zero for each.
    static QUERY_TEXTS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// The parse tree of every statement of `sql`, as JSON (`{"node": {"<Kind>": {…}}}`), or `None`
/// when PostgreSQL's parser rejects the text.
fn parse(sql: &str) -> Option<Vec<Value>> {
    let tree = pg_query::parse(sql).ok()?;
    tree.protobuf.stmts.iter().map(|s| s.stmt.as_ref().and_then(|n| serde_json::to_value(n.as_ref()).ok())).collect()
}

/// An upper estimate of how deeply PostgreSQL's parse tree of `sql` nests, from the lexer
/// (linear time, no recursion). Along the text, each token adds a level to the chain it is in.
/// A `,`, `AND` or `OR` starts a new chain (list items and the operands of an `AND`/`OR`,
/// which the parser flattens, are siblings, not nested). `JOIN`, `UNION`, `INTERSECT` and
/// `EXCEPT` nest what comes before them deeper, whatever separates them, so they count apart
/// from the chains (two each). A group (`(…)`, `[…]`, `CASE … END`) adds its own deepest
/// chain to the chain around it; `;` starts a new statement. A text with a backslash is also
/// lexed as a server with `standard_conforming_strings = off` reads it (the second reading
/// [`Prepared::classify`] parses); the deeper estimate counts.
fn depth(sql: &str) -> usize {
    let standard = depth_of(sql, &lex(sql));
    if sql.contains('\\') { standard.max(depth_of(sql, &lex_backslash_strings(sql))) } else { standard }
}

/// What opened a group of [`depth_of`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Text,
    Paren,
    Bracket,
    Case,
}

/// A group of [`depth_of`]: what opened it, the joins and set operations so far, the current
/// chain, and the deepest nesting seen in it.
struct Level {
    group: Group,
    base: usize,
    chain: usize,
    deepest: usize,
}

impl Level {
    fn new(group: Group) -> Self {
        Self { group, base: 0, chain: 0, deepest: 0 }
    }

    fn add(&mut self, n: usize) {
        self.chain += n;
        self.deepest = self.deepest.max(self.base + self.chain);
    }
}

/// [`depth`] of one lexing.
fn depth_of(sql: &str, tokens: &[super::lexer::Token]) -> usize {
    let mut levels = vec![Level::new(Group::Text)];
    // Close the groups up to the innermost one opened by `group`; each adds its depth to the
    // chain around it. `false` when no such group is open.
    fn close(levels: &mut Vec<Level>, group: Group) -> bool {
        if !levels[1..].iter().any(|l| l.group == group) {
            return false;
        }
        while levels.len() > 1 {
            let Some(inner) = levels.pop() else { break };
            if let Some(outer) = levels.last_mut() {
                outer.add(inner.deepest);
            }
            if inner.group == group {
                break;
            }
        }
        true
    }
    for t in tokens.iter().filter(|t| !t.is_trivia()) {
        let text = t.text(sql);
        let word = |w: &str| t.is_word() && text.eq_ignore_ascii_case(w);
        let opens = match t.kind {
            Tok::LParen => Some(Group::Paren),
            Tok::Op if text == "[" => Some(Group::Bracket),
            _ if word("CASE") => Some(Group::Case),
            _ => None,
        };
        let closes = match t.kind {
            Tok::RParen => Some(Group::Paren),
            Tok::Op if text == "]" => Some(Group::Bracket),
            _ if word("END") => Some(Group::Case),
            _ => None,
        };
        if let Some(g) = closes
            && close(&mut levels, g)
        {
            continue;
        }
        let Some(top) = levels.last_mut() else { break };
        if let Some(g) = opens {
            top.add(1);
            levels.push(Level::new(g));
        } else if t.kind == Tok::Semi && top.group == Group::Text {
            (top.base, top.chain) = (0, 0);
        } else if matches!(t.kind, Tok::Comma | Tok::Semi) || word("AND") || word("OR") {
            top.chain = 0;
        } else if ["JOIN", "UNION", "INTERSECT", "EXCEPT"].iter().any(|w| word(w)) {
            // Two levels of the tree (a `Node` and its `JoinExpr` or `SelectStmt`), as two
            // tokens of a chain are (`+ 1`).
            top.base += 2;
            top.chain = 0;
            top.add(0);
        } else {
            top.add(1);
        }
    }
    while levels.len() > 1 {
        let group = levels[levels.len() - 1].group;
        close(&mut levels, group);
    }
    levels[0].deepest
}

/// `sql` as a server with `standard_conforming_strings = off` reads it, spelled for one with
/// it on (each plain `'…'` string, which such a server ends at an unescaped quote, becomes an
/// `E'…'` string), when a plain string holds a backslash; `None` otherwise (both read the
/// same).
fn non_conforming(sql: &str) -> Option<String> {
    let plain = |t: &super::lexer::Token| t.kind == Tok::Str && t.text(sql).starts_with('\'');
    if !lex(sql).iter().any(|t| plain(t) && t.text(sql).contains('\\')) {
        return None;
    }
    let mut out = String::with_capacity(sql.len() + 8);
    for t in lex_backslash_strings(sql) {
        if plain(&t) {
            // `E` glued to an identifier before it would extend the identifier.
            if out.chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$') {
                out.push(' ');
            }
            out.push('E');
        }
        out.push_str(t.text(sql));
    }
    Some(out)
}

/// The kind and body of a parse-tree `Node` (`{"node": {"<Kind>": body}}`) or of the tagged
/// object inside one (`{"<Kind>": body}`).
fn node(n: &Value) -> Option<(&str, &Value)> {
    let inner = n.get("node").unwrap_or(n);
    let obj = inner.as_object()?;
    let (kind, body) = obj.iter().next()?;
    (obj.len() == 1).then_some((kind.as_str(), body))
}

/// A string field, or the value of a `String` node.
fn text(v: &Value) -> Option<&str> {
    v.as_str().or_else(|| match node(v) {
        Some(("String", s)) => s["sval"].as_str(),
        _ => None,
    })
}

/// An identifier as PostgreSQL would need it written: plain when it is lower case ASCII
/// letters, digits, `_` and `$` (not starting with a digit or `$`), quoted otherwise.
fn quote(name: &str) -> String {
    let plain = name.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '$');
    if plain { name.to_string() } else { format!("\"{}\"", name.replace('"', "\"\"")) }
}

/// A `RangeVar` (plain or in a `Node`) as `schema.table`.
fn relation(v: &Value) -> Option<String> {
    let r = match node(v) {
        Some(("RangeVar", r)) => r,
        _ => v,
    };
    let name = text(&r["relname"]).filter(|n| !n.is_empty())?;
    Some(match text(&r["schemaname"]).filter(|s| !s.is_empty()) {
        Some(schema) => format!("{}.{}", quote(schema), quote(name)),
        None => quote(name),
    })
}

/// The name of an object in a `DROP`-like list: a name, a qualified name (`List` of names), a
/// table, a function with its arguments, a type, or a role.
fn object_name(v: &Value) -> Option<String> {
    let dotted = |items: &Value| -> Option<String> {
        let parts: Vec<String> = items.as_array()?.iter().filter_map(text).map(quote).collect();
        (!parts.is_empty()).then(|| parts.join("."))
    };
    match node(v)? {
        ("String", s) => s["sval"].as_str().map(quote),
        ("List", l) => dotted(&l["items"]),
        ("RangeVar", _) => relation(v),
        ("ObjectWithArgs", o) => dotted(&o["objname"]),
        ("TypeName", t) => dotted(&t["names"]),
        ("RoleSpec", r) => text(&r["rolename"]).filter(|n| !n.is_empty()).map(quote),
        _ => None,
    }
}

/// The names of a list of objects, separated by commas.
fn names(list: Option<&Vec<Value>>) -> Option<String> {
    let names: Vec<String> = list?.iter().filter_map(object_name).collect();
    (!names.is_empty()).then(|| names.join(", "))
}

fn drops(target: Option<String>) -> Risk {
    Risk { danger: Some(Danger::Drop), target, ..Risk::of(Class::Ddl) }
}

/// `INSERT`, `UPDATE`, `DELETE` or `MERGE` (its body `b`), without what is nested in it.
fn dml(kind: &str, b: &Value) -> Risk {
    let target = relation(&b["relation"]);
    let risk = Risk::writing(Class::Write, target);
    match kind {
        "UpdateStmt" | "DeleteStmt" => {
            let no_where = filter(&b["where_clause"]);
            let all = if kind == "UpdateStmt" { Danger::UpdateAll } else { Danger::DeleteAll };
            Risk { no_where, danger: no_where.map(|_| all), ..risk }
        }
        "MergeStmt" => {
            let changes = b["merge_when_clauses"].as_array().into_iter().flatten().filter_map(node).any(|(_, w)| {
                let cmd = w["command_type"].as_i64();
                cmd == Some(protobuf::CmdType::CmdUpdate as i64) || cmd == Some(protobuf::CmdType::CmdDelete as i64)
            });
            Risk { danger: changes.then_some(Danger::Merge), ..risk }
        }
        _ => risk,
    }
}

/// Everything in a query (its body `b` and below) that runs with it: data-modifying
/// statements anywhere (a `WITH` at any depth, a subquery), row locks (`FOR UPDATE`, …) and
/// `SELECT … INTO`.
fn below(b: &Value) -> Risk {
    let mut risk = Risk::of(Class::Read);
    let mut visit = |obj: &serde_json::Map<String, Value>| {
        for (key, v) in obj {
            if matches!(key.as_str(), "InsertStmt" | "UpdateStmt" | "DeleteStmt" | "MergeStmt") {
                risk = std::mem::replace(&mut risk, Risk::of(Class::Read)).merge(dml(key, v));
            }
        }
        // A `SelectStmt` (tagged or as a set operation's operand).
        if obj.contains_key("target_list") && obj.contains_key("locking_clause") {
            if obj["locking_clause"].as_array().is_some_and(|l| !l.is_empty()) {
                risk = std::mem::replace(&mut risk, Risk::of(Class::Read)).merge(Risk::of(Class::Write));
            }
            if let Some(into) = obj["into_clause"].as_object() {
                let target = into.get("rel").and_then(relation);
                risk = std::mem::replace(&mut risk, Risk::of(Class::Read)).merge(Risk::writing(Class::Ddl, target));
            }
        }
    };
    if let Some(obj) = b.as_object() {
        visit(obj);
    }
    walk(b, &mut visit);
    risk
}

/// Calls `f` on every JSON object below `v` (not on `v` itself).
fn walk(v: &Value, f: &mut impl FnMut(&serde_json::Map<String, Value>)) {
    let children: Box<dyn Iterator<Item = &Value>> = match v {
        Value::Object(o) => Box::new(o.values()),
        Value::Array(a) => Box::new(a.iter()),
        _ => return,
    };
    for c in children {
        if let Value::Object(o) = c {
            f(o);
        }
        walk(c, f);
    }
}

/// Whether some object below `v` has the key `key` (a node of that kind).
fn has(v: &Value, key: &str) -> bool {
    let mut found = false;
    walk(v, &mut |o| found |= o.contains_key(key));
    found || v.as_object().is_some_and(|o| o.contains_key(key))
}

/// Whether `v` calls `set_config()` on a read-only setting, or on a setting it names with
/// something other than a constant (which could be one).
fn sets_read_only(v: &Value) -> bool {
    let mut found = false;
    walk(v, &mut |o| {
        let Some(call) = o.get("FuncCall") else { return };
        let is_set_config = call["funcname"]
            .as_array()
            .and_then(|n| n.last())
            .and_then(text)
            .is_some_and(|f| f.eq_ignore_ascii_case("set_config"));
        if !is_set_config {
            return;
        }
        let first = call["args"].as_array().and_then(|a| a.first());
        found |= match first.and_then(node) {
            Some(("AConst", c)) => c["val"]["Sval"]["sval"]
                .as_str()
                .is_none_or(|name| READ_ONLY_SETTINGS.contains(&name.to_ascii_lowercase().as_str())),
            _ => true,
        };
    });
    found
}

/// Whether `v` calls `set_config()` on `search_path` (or on a setting it names with something
/// other than a constant, which could be it) for the session: its third argument (`is_local`)
/// is not the constant `true`.
fn sets_session_path(v: &Value) -> bool {
    let mut found = false;
    walk(v, &mut |o| {
        let Some(call) = o.get("FuncCall") else { return };
        let is_set_config = call["funcname"]
            .as_array()
            .and_then(|n| n.last())
            .and_then(text)
            .is_some_and(|f| f.eq_ignore_ascii_case("set_config"));
        let args = call["args"].as_array().map(Vec::as_slice).unwrap_or_default();
        if !is_set_config || args.len() != 3 {
            return;
        }
        let path = match node(&args[0]) {
            Some(("AConst", c)) => {
                c["val"]["Sval"]["sval"].as_str().is_none_or(|n| n.eq_ignore_ascii_case("search_path"))
            }
            _ => true,
        };
        found |= path && constant(&args[2]) != Some(true);
    });
    found
}

/// A function an object of the parse tree may call, by name.
enum Call<'a> {
    /// A `FuncCall`: the names it is written with (`f`, `schema.f`, `database.schema.f`), and
    /// the call (its arguments).
    Func(Vec<&'a str>, &'a Value),
    /// Attribute notation, `t.f` or `(expr).f`: a column reference or an indirection to the
    /// parser, which the server runs as `f(t)` when `t` has no column or field `f`. Every name
    /// of a column reference but the first (a table, or a column of one) and every name of an
    /// indirection may be one, so each is taken as a call: a column that happens to have such a
    /// name counts too. With the expression it is called on when the text shows it: `expr` for
    /// the first name after `(expr)`; not for `t.f`, whose `t` is a column or a row, nor for a
    /// later name (`(expr).a.f`) or one after a subscript.
    Attribute(&'a str, Option<&'a Value>),
}

/// The calls of one object of the parse tree ([`Call`]).
fn calls(o: &serde_json::Map<String, Value>) -> Vec<Call<'_>> {
    if let Some(call) = o.get("FuncCall") {
        return vec![Call::Func(call["funcname"].as_array().into_iter().flatten().filter_map(text).collect(), call)];
    }
    let (names, skip, on) = match (o.get("ColumnRef"), o.get("AIndirection")) {
        (Some(c), _) => (&c["fields"], 1, None),
        (_, Some(i)) => (&i["indirection"], 0, Some(&i["arg"])),
        _ => return Vec::new(),
    };
    let names = names.as_array().and_then(|n| n.get(skip..)).unwrap_or_default();
    let on = |at: usize| on.filter(|_| at == 0);
    names.iter().enumerate().filter_map(|(at, n)| text(n).map(|f| Call::Attribute(f, on(at)))).collect()
}

/// Which argument of a call of the built-in `f` with `n` arguments holds the query it runs,
/// when it runs one given as text (all of them are in [`RUNS_CODE`]): `query_to_xml`,
/// `query_to_xmlschema` and `query_to_xml_and_xmlschema` (the first, named `query`),
/// `ts_stat` (the first) and `ts_rewrite(tsquery, text)` (the second; the one of three
/// `tsquery` values runs none). `cursor_to_xml` runs the query of a cursor, which its `DECLARE`
/// was checked for, as a `FETCH` does.
fn query_position(f: &str, n: usize) -> Option<usize> {
    match f {
        "query_to_xml" | "query_to_xml_and_xmlschema" | "query_to_xmlschema" | "ts_stat" => Some(0),
        "ts_rewrite" if n == 2 => Some(1),
        _ => None,
    }
}

/// The calls in `v` of a built-in that runs a query given as text ([`query_position`]), however
/// the call is written ([`Call`]): the function's name, and the text when the call gives it as
/// a string constant ([`string_constant`]; by position or, in named notation, as `query`).
/// Any argument that is not one (a column, a parameter, a concatenation, the result of a call,
/// an argument spread with `VARIADIC`, one the text does not show) is `None`.
fn query_texts(v: &Value) -> Vec<(String, Option<String>)> {
    let mut found = Vec::new();
    walk(v, &mut |o| {
        for call in calls(o) {
            let (f, query) = match call {
                Call::Func(name, call) => {
                    let Some(f) = name.last().copied() else { continue };
                    let args = call["args"].as_array().map(Vec::as_slice).unwrap_or_default();
                    let Some(at) = query_position(f, args.len()) else { continue };
                    let named = |a: &&Value| matches!(node(a), Some(("NamedArgExpr", _)));
                    let arg = match args.get(at) {
                        _ if call["func_variadic"] == Value::Bool(true) => None,
                        Some(a) if !named(&a) => Some(a),
                        _ => args.iter().filter_map(node).find_map(|(kind, a)| {
                            (kind == "NamedArgExpr" && text(&a["name"]) == Some("query")).then_some(&a["arg"])
                        }),
                    };
                    (f, arg)
                }
                Call::Attribute(f, on) => match query_position(f, 1) {
                    Some(0) => (f, on),
                    Some(_) => (f, None),
                    None => continue,
                },
            };
            found.push((f.to_string(), query.and_then(string_constant)));
        }
    });
    found
}

/// The text of a string constant (`'…'`, `E'…'`, `$$…$$`, `U&'…'`), also cast to `text`: the
/// value the server gets, whatever its spelling. A cast to another type is not one: it may
/// change the value (`varchar(n)` and `name` cut it short, a type's input function may do
/// anything).
fn string_constant(n: &Value) -> Option<String> {
    match node(n)? {
        ("AConst", c) => c["val"]["Sval"]["sval"].as_str().map(str::to_string),
        ("TypeCast", c) => {
            let t = &c["type_name"];
            let names: Vec<&str> = t["names"].as_array()?.iter().filter_map(text).collect();
            let empty = |k: &str| t[k].as_array().is_none_or(Vec::is_empty);
            let plain = empty("typmods") && empty("array_bounds") && t["setof"] != Value::Bool(true);
            let to_text = matches!(names.as_slice(), ["text"] | ["pg_catalog", "text"]);
            if plain && to_text { string_constant(&c["arg"]) } else { None }
        }
        _ => None,
    }
}

/// Whether `v` calls a function that is not a known built-in ([`builtin`]): a name with a
/// schema other than `pg_catalog`, a quoted name in another case, any name not in the list; or
/// one of [`RUNS_CODE`] in attribute notation ([`Call::Attribute`]). A function of the user's
/// in attribute notation is not seen (see the module docs).
fn calls_code(v: &Value) -> bool {
    let mut found = false;
    walk(v, &mut |o| {
        found |= calls(o).iter().any(|c| match c {
            Call::Func(name, _) => {
                !matches!(name.as_slice(), [f] | ["pg_catalog", f] | [_, "pg_catalog", f] if builtin(f))
            }
            Call::Attribute(f, _) => RUNS_CODE.contains(f),
        });
    });
    found
}

/// The first call in `v` of a built-in that acts on the server ([`SERVER_FILES`],
/// [`SERVER_ACTIONS`]): its danger and name. Any call whose last name is one counts, however
/// it is qualified (`f`, `pg_catalog.f`, `database.pg_catalog.f`; a function of the user's in
/// another schema with that name asks too) and in attribute notation ([`Call::Attribute`]).
fn server_call(v: &Value) -> Option<(Danger, String)> {
    let mut found = None;
    walk(v, &mut |o| {
        for call in calls(o) {
            let f = match call {
                Call::Func(name, _) => name.last().copied(),
                Call::Attribute(f, _) => Some(f),
            };
            let Some(f) = f else { continue };
            let danger = if SERVER_FILES.contains(&f) {
                Danger::ServerFile
            } else if SERVER_ACTIONS.contains(&f) {
                Danger::ServerAction
            } else {
                continue;
            };
            found = found.take().or(Some((danger, f.to_string())));
        }
    });
    found
}

/// Whether a `TransactionStmt` (its body `b`) runs no code: `BEGIN`, `START TRANSACTION`,
/// `SAVEPOINT`, `RELEASE`, `ROLLBACK [TO]`. A commit fires the deferred triggers.
fn quiet_transaction(b: &Value) -> bool {
    use protobuf::TransactionStmtKind as K;
    [
        K::TransStmtBegin,
        K::TransStmtStart,
        K::TransStmtSavepoint,
        K::TransStmtRelease,
        K::TransStmtRollback,
        K::TransStmtRollbackTo,
    ]
    .iter()
    .any(|k| b["kind"].as_i64() == Some(*k as i64))
}

/// The names of PostgreSQL 17's built-in functions (the `pg_proc` rows of `pg_catalog` that
/// initdb creates, `oid < 10000`), one per line in byte order, less [`RUNS_CODE`]. Generated
/// with `SELECT DISTINCT proname COLLATE "C" FROM pg_proc WHERE pronamespace =
/// 'pg_catalog'::regnamespace AND oid < 10000 AND proname <> ALL (<RUNS_CODE>) ORDER BY 1`.
pub const BUILTINS: &str = include_str!("risk/builtins.txt");

/// Built-in functions that run SQL or code they are given, by name: not in [`BUILTINS`], so a
/// call of one forgets the prepared names like a call of a user's function. Every overload of
/// the name counts (`ts_rewrite` of three `tsquery` values runs nothing, but forgets too). Those
/// that run a query given as text also have its risk ([`query_position`]).
/// Found by reading PostgreSQL 17's source: every function of
/// `pg_proc` whose C code executes a query through SPI or opens a cursor, and those that run
/// code a string argument names.
pub const RUNS_CODE: &[&str] = &[
    // The query they are given, or a cursor's query (what `FETCH` runs).
    "cursor_to_xml",
    "cursor_to_xmlschema",
    "query_to_xml",
    "query_to_xml_and_xmlschema",
    "query_to_xmlschema",
    "ts_rewrite",
    "ts_stat",
    // Every table of a database or a schema, or a table: their views and policies may call
    // anything.
    "database_to_xml",
    "database_to_xml_and_xmlschema",
    "database_to_xmlschema",
    "schema_to_xml",
    "schema_to_xml_and_xmlschema",
    "schema_to_xmlschema",
    "table_to_xml",
    "table_to_xml_and_xmlschema",
    "table_to_xmlschema",
    // The input function of a type named by a string: a domain's `CHECK` may call anything.
    "pg_input_error_info",
    "pg_input_is_valid",
    // An index's expressions and predicate (which may call any function), for each row.
    "brin_summarize_new_values",
    "brin_summarize_range",
];

/// Built-in functions that read, write or list files of the server, as its operating system
/// user, by a name the call gives ([`Danger::ServerFile`]). A read-only transaction does not
/// stop them (`lo_export` writes a file). Those that read one fixed file of the server
/// (`pg_hba_file_rules`, `pg_current_logfile`, `pg_control_*`, …) are not here.
pub const SERVER_FILES: &[&str] = &[
    "lo_export",
    "lo_import",
    "pg_ls_archive_statusdir",
    "pg_ls_dir",
    "pg_ls_logdir",
    "pg_ls_logicalmapdir",
    "pg_ls_logicalsnapdir",
    "pg_ls_replslotdir",
    "pg_ls_tmpdir",
    "pg_ls_waldir",
    "pg_read_binary_file",
    "pg_read_file",
    "pg_stat_file",
];

/// Built-in functions that act on the server beyond the transaction ([`Danger::ServerAction`]):
/// a rollback does not undo them and a read-only transaction does not stop them. The volatile
/// built-ins of PostgreSQL 17 that are neither here nor in [`SERVER_FILES`] or [`RUNS_CODE`]
/// were read one by one (the integration test lists them): they read, change the session only
/// (`set_config`, `setseed`, advisory locks), are refused by a read-only transaction (`nextval`,
/// `setval`, large object writes), send a notification (`pg_notify`), or fail outside initdb,
/// `pg_upgrade` and extension scripts.
pub const SERVER_ACTIONS: &[&str] = &[
    // Indexes, written outside the checks of a read-only transaction.
    "brin_desummarize_range",
    "brin_summarize_new_values",
    "brin_summarize_range",
    "gin_clean_pending_list",
    // Backups, the write-ahead log and recovery.
    "pg_backup_start",
    "pg_backup_stop",
    "pg_create_restore_point",
    "pg_log_standby_snapshot",
    "pg_logical_emit_message",
    "pg_promote",
    "pg_switch_wal",
    "pg_wal_replay_pause",
    "pg_wal_replay_resume",
    // Other sessions, the configuration and the server's log.
    "pg_cancel_backend",
    "pg_log_backend_memory_contexts",
    "pg_reload_conf",
    "pg_rotate_logfile",
    "pg_terminate_backend",
    // Collations imported into the catalog.
    "pg_import_system_collations",
    // Replication slots and origins.
    "pg_copy_logical_replication_slot",
    "pg_copy_physical_replication_slot",
    "pg_create_logical_replication_slot",
    "pg_create_physical_replication_slot",
    "pg_drop_replication_slot",
    "pg_logical_slot_get_binary_changes",
    "pg_logical_slot_get_changes",
    "pg_replication_origin_advance",
    "pg_replication_origin_create",
    "pg_replication_origin_drop",
    "pg_replication_origin_session_reset",
    "pg_replication_origin_session_setup",
    "pg_replication_origin_xact_reset",
    "pg_replication_origin_xact_setup",
    "pg_replication_slot_advance",
    "pg_sync_replication_slots",
    // Statistics.
    "pg_stat_reset",
    "pg_stat_reset_replication_slot",
    "pg_stat_reset_shared",
    "pg_stat_reset_single_function_counters",
    "pg_stat_reset_single_table_counters",
    "pg_stat_reset_slru",
    "pg_stat_reset_subscription_stats",
];

/// Whether `name` (as PostgreSQL folds it) is a built-in function in [`BUILTINS`].
pub fn builtin(name: &str) -> bool {
    static SORTED: std::sync::OnceLock<Vec<&str>> = std::sync::OnceLock::new();
    SORTED.get_or_init(|| BUILTINS.lines().collect()).binary_search(&name).is_ok()
}

/// The words of a text that was not read that leave the prepared names alone: they make a
/// query of constants, which names no table, function or type (so no code of its own runs).
const INERT_WORDS: &[&str] = &[
    "SELECT",
    "VALUES",
    "AS",
    "AND",
    "OR",
    "NOT",
    "IS",
    "TRUE",
    "FALSE",
    "NULL",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "ALL",
];

/// The `WHERE` of an `UPDATE` or `DELETE`, and whether it matches every row (or may).
fn filter(w: &Value) -> Option<NoWhere> {
    if w.is_null() {
        return Some(NoWhere::Missing);
    }
    if always_true(w) {
        return Some(NoWhere::AlwaysTrue);
    }
    (!has(w, "ColumnRef") && !has(w, "CurrentOfExpr")).then_some(NoWhere::NoColumn)
}

/// A constant that reads as true (`true`, `'t'`, `'on'`, …) or false.
fn constant(n: &Value) -> Option<bool> {
    let (kind, c) = node(n)?;
    match kind {
        "AConst" => {
            if let Some(b) = c["val"]["Boolval"]["boolval"].as_bool() {
                return Some(b);
            }
            // An absent `boolval` is false (the default is left out).
            if c["val"].get("Boolval").is_some() {
                return Some(false);
            }
            let s = c["val"]["Sval"]["sval"].as_str()?.to_ascii_lowercase();
            match s.as_str() {
                "t" | "true" | "y" | "yes" | "on" | "1" => Some(true),
                "f" | "false" | "n" | "no" | "off" | "0" => Some(false),
                _ => None,
            }
        }
        "TypeCast" => constant(&c["arg"]),
        _ => None,
    }
}

/// Whether condition `n` is recognised as always true.
fn always_true(n: &Value) -> bool {
    if constant(n) == Some(true) {
        return true;
    }
    let Some((kind, b)) = node(n) else { return false };
    match kind {
        "BoolExpr" => {
            let args = b["args"].as_array().map(Vec::as_slice).unwrap_or_default();
            match b["boolop"].as_i64() {
                Some(op) if op == protobuf::BoolExprType::OrExpr as i64 => args.iter().any(always_true),
                Some(op) if op == protobuf::BoolExprType::AndExpr as i64 => {
                    !args.is_empty() && args.iter().all(always_true)
                }
                Some(op) if op == protobuf::BoolExprType::NotExpr as i64 => {
                    args.first().and_then(constant) == Some(false)
                }
                _ => false,
            }
        }
        // `a = a`: the same expression on both sides.
        "AExpr" => {
            let eq = b["kind"].as_i64() == Some(protobuf::AExprKind::AexprOp as i64)
                && b["name"].as_array().is_some_and(|n| n.len() == 1 && text(&n[0]) == Some("="));
            eq && !b["lexpr"].is_null() && without_locations(&b["lexpr"]) == without_locations(&b["rexpr"])
        }
        _ => false,
    }
}

/// `v` without its `location` fields (byte offsets in the text), to compare two expressions.
fn without_locations(v: &Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(
            o.iter().filter(|(k, _)| *k != "location").map(|(k, v)| (k.clone(), without_locations(v))).collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(without_locations).collect()),
        other => other.clone(),
    }
}

/// An option value that is clearly false (as PostgreSQL's `parse_bool` reads it): `false`,
/// `off`, `no`, `0` and their prefixes. No value, and anything else, is true.
fn is_false(arg: &Value) -> bool {
    match node(arg) {
        Some(("Integer", i)) => i["ival"].as_i64().unwrap_or(0) == 0,
        Some(("Boolean", b)) => b["boolval"].as_bool() != Some(true),
        Some(("String", s)) => {
            let v = s["sval"].as_str().unwrap_or_default().to_ascii_lowercase();
            (!v.is_empty() && "false".starts_with(&v))
                || v == "n"
                || v == "no"
                || (v.len() >= 2 && "off".starts_with(&v))
                || v == "0"
        }
        _ => false,
    }
}

/// Whether transaction options (`BEGIN …`, `SET TRANSACTION …`) ask for `READ WRITE`.
fn options_read_write(options: &Value) -> bool {
    options.as_array().into_iter().flatten().filter_map(node).any(|(kind, o)| {
        kind == "DefElem" && text(&o["defname"]) == Some("transaction_read_only") && constant_int(&o["arg"]) == Some(0)
    })
}

/// An integer constant.
fn constant_int(n: &Value) -> Option<i64> {
    match node(n)? {
        ("AConst", c) => c["val"].get("Ival").map(|i| i["ival"].as_i64().unwrap_or(0)),
        ("Integer", i) => Some(i["ival"].as_i64().unwrap_or(0)),
        _ => None,
    }
}

/// Whether a setting's name is one a read-only policy lets a session change.
fn safe_setting(name: &str) -> bool {
    SAFE_SETTINGS.contains(&name) || name.starts_with("enable_") || name.contains('.')
}

/// `SET …` and `RESET …`: transaction characteristics are transaction control; the rest
/// changes a session setting.
fn set(b: &Value) -> Risk {
    use protobuf::VariableSetKind as K;
    let kind = b["kind"].as_i64().unwrap_or(0);
    let name = text(&b["name"]).unwrap_or_default().to_ascii_lowercase();
    if kind == K::VarSetMulti as i64 {
        // `SET TRANSACTION …`, `SET SESSION CHARACTERISTICS AS TRANSACTION …`.
        return Risk { read_write: options_read_write(&b["args"]), ..Risk::of(Class::Tx) };
    }
    if kind == K::VarResetAll as i64 {
        return Risk::session(true);
    }
    if name == "search_path" {
        // For the session unless `LOCAL` (`RESET` goes back to the session's default).
        let set = [K::VarSetValue, K::VarSetDefault, K::VarSetCurrent].iter().any(|k| *k as i64 == kind);
        let local = b["is_local"].as_bool().unwrap_or(false);
        return Risk { session_path: set && !local, ..Risk::session(true) };
    }
    if READ_ONLY_SETTINGS.contains(&name.as_str()) {
        // Only turning it on is harmless.
        let args = b["args"].as_array().map(Vec::as_slice).unwrap_or_default();
        let on = kind == K::VarSetValue as i64
            && args.len() == 1
            && (constant(&args[0]) == Some(true) || constant_int(&args[0]) == Some(1));
        return Risk { read_write: !on, ..Risk::session(on) };
    }
    Risk::session(safe_setting(&name))
}

/// `ALTER TABLE` (also of a foreign table, a view, a type's attributes): DDL; dropping a
/// column, or changing its type (which converts every value, and may lose data), is dangerous.
fn alter_table(b: &Value) -> Risk {
    use protobuf::AlterTableType as T;
    let target = relation(&b["relation"]);
    let cmds: Vec<&Value> = b["cmds"].as_array().into_iter().flatten().filter_map(node).map(|(_, c)| c).collect();
    let subtype = |c: &&Value, t: T| c["subtype"].as_i64() == Some(t as i64);
    let danger = if cmds.iter().any(|c| subtype(c, T::AtDropColumn)) {
        Some(Danger::DropColumn)
    } else if cmds.iter().any(|c| subtype(c, T::AtAlterColumnType)) {
        Some(Danger::AlterColumnType)
    } else {
        None
    };
    Risk { danger, target, ..Risk::of(Class::Ddl) }
}

#[cfg(test)]
mod tests;
