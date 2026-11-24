//! What is fixed per engine, known without a connection: one [`EngineSpec`] per engine.

use super::{CountInTx, EngineId, Fact, OpenVerdict, ReadOnlyProof, ServerFacts, Support, TxStatement, TxVerdict};
use crate::driver::{ColumnMeta, EditFormat, Hierarchy, ReadIn, SessionRole, SessionValues, ValueKind};
use crate::edit::{Invalid, TableColumn};
use crate::export::keyset::NotKeyset;
use crate::export::{Kind, Refused, SqlCells, SqlColumn};
use crate::profile::dsn::Scheme;
use crate::query::{CaseMode, ColumnFacts, Compares, NoEquality, Nulls, RenderError, Target};
use crate::sql::dialect::{Dialect, DollarQuotes};
use crate::sql::risk::Classifier;
use crate::sql::risk::mysql::Judged;
use std::time::Duration;

/// How datarig reaches an engine's server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// Over the network: the URL form and the default port of `scheme`, through an SSH tunnel
    /// when `ssh`.
    Network { scheme: Scheme, ssh: bool },
    /// A file opened in datarig's process.
    File,
}

/// One engine's row of the registry ([`EngineId::spec`]).
pub struct EngineSpec {
    pub id: EngineId,
    /// The driver name a profile writes for it.
    pub key: &'static str,
    /// Other driver names for it.
    pub aliases: &'static [&'static str],
    /// The engines a profile with this driver name may reach: the server decides among them.
    pub family: &'static [EngineId],
    /// The URL schemes of its connection strings.
    pub url_schemes: &'static [&'static str],
    pub transport: Transport,
    /// The `dbms` of the session presets that apply to it.
    pub preset_key: &'static str,
    /// The driver row of the profile form and its icon.
    pub form_key: &'static str,
    /// How its namespaces nest.
    pub hierarchy: Hierarchy,
    /// How its text is read.
    pub text: &'static dyn TextRules,
    /// How its statements are judged.
    pub risk: &'static dyn RiskRules,
    /// How its statements stand to the transaction open on a session.
    pub tx: &'static dyn TxRules,
    /// What its sessions may be given.
    pub session: &'static dyn SessionRules,
    /// Whether a statement can be stopped on its server.
    pub cancel: &'static dyn CancelRules,
    /// How its values are typed and shown.
    pub values: &'static dyn ValueRules,
    /// How its tables are read for an export.
    pub export: Fact<&'static dyn ExportRules>,
    /// Which edits of a result's rows its servers take.
    pub edit: Fact<&'static dyn EditRules>,
    /// How its catalogue names what a result reads.
    pub catalogue: &'static dyn CatalogueRules,
    /// How the DDL of its objects is shown and opened in a console.
    pub ddl: Fact<&'static dyn DdlRules>,
    /// How a table's rows are ordered and paged.
    pub sort: &'static dyn SortRules,
    /// How a table's filter is written.
    pub filter: Fact<&'static dyn FilterSql>,
}

/// How an engine's text is read on a server.
pub trait TextRules: Sync {
    /// Whether a session on the server of `f` reads `$$…$$` as a quote.
    fn dollar_quotes(&self, f: &ServerFacts) -> DollarQuotes;
}

/// How an engine's statements are judged.
pub trait RiskRules: Sync {
    /// A fresh classifier of text in dialect `d`, its calls judged on the engines `j` says, for
    /// a session that prepared nothing.
    fn new_classifier(&self, d: Dialect, j: Judged) -> Classifier;

    /// Whether the environment's list of functions a read-only profile may call
    /// (`read_only_functions`, `read_only_extensions`) applies to the engine's sessions.
    fn read_only_calls(&self) -> Support;

    /// Whether statement `sql`, whose first page shows values written in the session's text
    /// style, is on the engine's re-run allowlist as far as its text tells, so that the app
    /// asks the server about it (the allowlist's question, which reads the catalogue only and
    /// does not run `sql`) to learn whether that style holds: `Some(true)` when it is,
    /// `Some(false)` when it is not, `None` on an engine whose results are never checked so.
    fn rerun_style(&self, sql: &str) -> Option<bool>;

    /// The calls a table's filter may make without asking, and those it may not make.
    fn filter_calls(&self) -> FilterCalls;

    /// Whether a session of the engine judged on `j` refuses text the user wrote that holds
    /// MariaDB's own executable comment, `/*M!` ([`crate::sql::lexer::mysql::has_m_bang`]):
    /// where MariaDB may be the server that runs it, which reads what such a comment holds.
    fn refuses_m_bang(&self, j: Judged) -> bool;
}

/// The calls a table's filter may make without asking (`allowed`) and those it may not make
/// (`refused`), each sorted as the filter check looks them up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilterCalls {
    pub allowed: &'static [&'static str],
    pub refused: &'static [&'static str],
}

/// How an engine's statements stand to the transaction open on a session.
pub trait TxRules: Sync {
    /// What statement `s` of a run does to the transaction: refused, asked about, noted, and
    /// whether it ends the transaction open before it.
    fn statement(&self, s: &TxStatement) -> TxVerdict;

    /// How long a session may take to answer the rollback the app sends for an idle
    /// transaction before the app closes it instead.
    fn rollback_wait(&self) -> Duration;

    /// Whether a count of rows takes its time limit while a transaction is open.
    fn count_in_tx(&self) -> CountInTx;

    /// Whether a session's reads may lock rows (by its isolation level and autocommit).
    fn reads_lock_rows(&self) -> bool;
}

/// What an engine's sessions may be given.
pub trait SessionRules: Sync {
    /// Whether the server may be given an idle limit for a transaction opened for manual
    /// commit, so that it ends a session the app could not roll back itself.
    fn server_idle_limit(&self) -> Support;

    /// Whether a session of `role` of a read-only profile opens when its connection proved
    /// `proof`. Asked only when read-only applies to the session.
    fn read_only_open(&self, role: SessionRole, proof: ReadOnlyProof) -> OpenVerdict;
}

/// Whether a statement that runs can be stopped on an engine's server. Advisory, for what the
/// UI says before a cancel: every cancel still goes to the session, which stops a run between
/// its statements whatever this says, and its driver decides how to stop a running one.
pub trait CancelRules: Sync {
    /// Whether the server of `f` can stop a running statement of the session's.
    fn server_cancel(&self, f: &ServerFacts) -> Support;
}

/// How an engine's values are typed and shown.
pub trait ValueRules: Sync {
    /// Whether a value typed into a cell is a value of its column's type.
    fn input(&self) -> &'static dyn InputRules;

    /// Whether a value of `kind` is shown in the time zone of the session that read it, without
    /// an offset: its text is the same instant only in that zone.
    fn shown_in_session_zone(&self, kind: ValueKind) -> bool;

    /// What the text of a bytes value starts with in a result, before its hexadecimal digits.
    fn bytes_shown_prefix(&self) -> &'static str;

    /// Value `v` (`None`: NULL) of a column of `kind` as an SQL literal: NULL, a number or a
    /// boolean bare, anything else a literal the server reads as the value.
    fn literal(&self, kind: Kind, v: Option<&str>) -> String;

    /// How the columns `columns` of a result are copied as SQL, or the first one that cannot be:
    /// `read_in` the session's values when the result was read, `server_zone` the server's own
    /// time zone.
    fn sql_cells(
        &self,
        columns: &[SqlColumn],
        read_in: &ReadIn,
        server_zone: Option<&str>,
    ) -> Result<SqlCells, Refused>;

    /// The clause an `INSERT` writes after its column list when it writes the values of an
    /// identity column the server generates (`overriding`): empty where none is needed.
    fn insert_overriding(&self, overriding: bool) -> &'static str;

    /// Whether the server of `f` reads a value of a time with a time zone written as text with
    /// its offset after it (`'2024-01-01 10:00:00+02:00'`) as that instant, so that copy as SQL
    /// and an SQL export may write it so ([`SqlCells::literal`]).
    fn zoned_literal(&self, f: &ServerFacts) -> Support;
}

/// How an engine's tables are read for an export.
pub trait ExportRules: Sync {
    /// The kind of key column `c` as a keyset literal is written, or why it cannot be one.
    /// `text_ok`: text read in the session is the text ([`ExportRules::text_read_whole`]).
    fn key_kind(&self, c: &ColumnMeta, text_ok: bool) -> Result<Kind, NotKeyset>;

    /// The condition that reads the rows past the key whose columns are `names` (quoted) and
    /// whose values in the last row read are the literals `values` (of `kinds`); `None` when a
    /// literal would not compare back exactly.
    fn after(&self, names: &[String], values: &[String], kinds: &[Kind]) -> Option<String>;

    /// Whether text read in a session of `values` (`None`: not known) is the text as stored.
    fn text_read_whole(&self, values: Option<&SessionValues>) -> bool;

    /// Whether the server ends a session that reads a result more slowly than it sends it.
    fn drops_slow_readers(&self) -> bool;
}

/// Which edits of a result's rows an engine's servers take.
pub trait EditRules: Sync {
    /// The SQL a session on the server of `f` takes for edits of a result's rows, or why it
    /// takes none.
    fn writer(&self, f: &ServerFacts) -> Fact<EditFormat>;

    /// Whether a choice among rows the same in every column (change one of them, or all) may
    /// be offered when the table's structure was read by the session of `structure_by`, the
    /// change is applied by the session of `applied_by`, and it is written in `format`. `None`:
    /// that session has not said what its server is.
    fn duplicates_choice(
        &self,
        structure_by: Option<&ServerFacts>,
        applied_by: Option<&ServerFacts>,
        format: EditFormat,
    ) -> Support;
}

/// How an engine's catalogue names what a result reads.
pub trait CatalogueRules: Sync {
    /// How a result names the table its columns come from: by the table's number, or by its
    /// name, found only in the cache of keys read when the profile connected.
    fn result_tables(&self) -> ResultTables;
}

/// How a result names the table its columns come from ([`CatalogueRules::result_tables`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultTables {
    /// By the table's number in the catalogue.
    ByNumber,
    /// By its name, looked up in the cache of keys read at connect: a table created later, or
    /// one the user may not see, is not there.
    ByName,
}

/// How the DDL of an engine's objects is shown and opened in a console.
pub trait DdlRules: Sync {
    /// What the DDL of an index is: its own statement, or its table's (where the table's
    /// `CREATE TABLE` defines the index).
    fn index_ddl(&self) -> DdlOfIndex;

    /// How the text of a DDL tab opens in a console.
    fn to_console(&self) -> DdlToConsole;
}

/// What the DDL of an index is ([`DdlRules::index_ddl`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DdlOfIndex {
    /// The index's own statement.
    Own,
    /// The statement of its table, which defines it.
    Table,
}

/// How the text of a DDL tab opens in a console ([`DdlRules::to_console`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DdlToConsole {
    /// As read, in the tab's context.
    AsRead,
    /// The server's own text with its strings written as the console's sql mode reads them, in
    /// the object's database.
    Requoted,
}

/// Whether a value typed into a cell is a value of its column's type, in the form the result
/// shows that type's values in, for the kinds whose check differs by engine. Each check reads
/// the column's type name itself.
pub trait InputRules: Sync {
    /// A whole number, in the range of the column's type when the type is known.
    fn integer(&self, c: &TableColumn, s: &str) -> Result<(), Invalid>;

    /// A decimal number, with as many digits as the column's type holds.
    fn decimal(&self, c: &TableColumn, s: &str) -> Result<(), Invalid>;

    /// A floating-point number the column's type holds exactly as typed.
    fn float(&self, c: &TableColumn, s: &str) -> Result<(), Invalid>;

    /// A date as the result shows it.
    fn date(&self, s: &str) -> Result<(), Invalid>;

    /// A date and time as the result shows it; `zoned` for a type with a time zone.
    fn timestamp(&self, c: &TableColumn, zoned: bool, s: &str) -> Result<(), Invalid>;

    /// Bytes as the result shows them.
    fn bytes(&self, s: &str) -> Result<(), Invalid>;

    /// Bits as the result shows them, as many as the column's type takes.
    fn bits(&self, c: &TableColumn, s: &str) -> Result<(), Invalid>;

    /// Text the column's type holds as typed.
    fn text(&self, c: &TableColumn, s: &str) -> Result<(), Invalid>;

    /// A value of a kind not otherwise checked.
    fn other(&self, c: &TableColumn, s: &str) -> Result<(), Invalid>;

    /// A span of time.
    fn interval(&self, c: &TableColumn, s: &str) -> Result<(), Invalid>;
}

/// How an engine orders and pages a table's rows.
pub trait SortRules: Sync {
    /// Whether NULL sorts as smaller than any value (else as larger), where a key does not say
    /// where its NULLs go.
    fn nulls_low(&self) -> bool;

    /// The sort key on column `col` (quoted) in direction `dir` (`ASC` or `DESC`), its NULLs
    /// where `n` says.
    fn nulls_key(&self, col: &str, dir: &str, n: Nulls) -> String;

    /// Whether an order may say where its NULLs go (`NULLS FIRST`, `NULLS LAST`).
    fn typed_nulls(&self) -> bool;

    /// How the server matches an unquoted name with a column.
    fn names(&self) -> NameMatch;

    /// The clause (with a leading space) that skips the first `offset` rows and reads every
    /// row after them.
    fn offset_alone(&self, offset: u64) -> String;
}

/// How a server matches an unquoted name with a column ([`SortRules::names`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameMatch {
    /// The name as it folds it, exactly.
    Exact,
    /// Any column whose name differs from it only in case.
    AnyCase,
}

/// How a table's filter is written on an engine. Each answer takes what is known of the table
/// and its session ([`Target`]) where it reads it.
pub trait FilterSql: Sync {
    /// What the engine's filter text means, as data.
    fn syntax(&self) -> &'static FilterSyntax;

    /// `s` as a string literal the server reads as `s`, on one line.
    fn text_literal(&self, t: &Target, s: &str) -> String;

    /// Bytes `b` as a literal the server reads as them.
    fn bytes_literal(&self, t: &Target, b: &[u8]) -> String;

    /// Column `col` (quoted) compared with literal `lit` so that NULL equals NULL; `negated`:
    /// that they differ so.
    fn null_safe(&self, col: &str, lit: &str, negated: bool) -> String;

    /// The pattern `c`, or why the server would not read it as shown.
    fn like(&self, t: &Target, c: &Like<'_>) -> Result<String, RenderError>;

    /// How a filter compares the values of a result column `m`, as its type says: with an
    /// order, for equality only, or not at all. `enum_known`: the column's type is known to be
    /// an enum of the engine that has no kind of its own.
    fn compares(&self, m: &ColumnMeta, enum_known: bool) -> Result<Compares, NoEquality>;

    /// The three items a group count reads of `column` (`quoted`, its name as written).
    fn group_parts(&self, column: &ColumnMeta, quoted: &str) -> GroupParts;
}

/// What an engine's filter text means, as data: each engine writes every field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilterSyntax {
    /// The server compares a number column with a string as two doubles: a number for such a
    /// column is written as its exact decimal, a single-precision float bare, and values that
    /// would compare otherwise than shown are refused.
    pub numbers_as_doubles: bool,
    /// A value with the offset of the zone it was read in, compared with a column shown in the
    /// session's zone, is written as that instant in the session's zone.
    pub zoned_instants: bool,
    /// How a NULL-safe equality is written, and so read back.
    pub null_safe: NullSafe,
    /// `ILIKE` is the server's case-insensitive pattern.
    pub ilike: bool,
    /// A string may be written as hexadecimal digits: `X'…'` bytes, and `_utf8mb4 X'…'` text.
    pub hex_strings: bool,
    /// A plain string with a backslash reads as the server's `standard_conforming_strings`
    /// says (only `E'…'` reads it the same either way), and `'\x…'::bytea` is bytes.
    pub pg_strings: bool,
    /// Operators the reader does not model may stand beside `AND` and `NOT` at another
    /// precedence: `XOR`, `&`, `|` (`||` may be `OR`), `!` and `:=`. A text holding one at its
    /// level is kept whole.
    pub loose_ops: bool,
    /// A `NOT` before a bare comparison may bind otherwise under a server setting
    /// (`HIGH_NOT_PRECEDENCE` reads `NOT a BETWEEN …` as `(NOT a) BETWEEN …`): only a `NOT`
    /// over one parenthesised group is read.
    pub not_caution: bool,
    /// `INTERVAL expr unit` takes an expression that may hold an `AND` of its own.
    pub interval_and: bool,
    /// The planner may run a subquery while it plans a statement: no estimate of a filter that
    /// may hold one.
    pub plans_subqueries: bool,
    /// The filter refuses `/*M! … */` as an executable comment, not as a plain one. MariaDB runs
    /// what it holds; MySQL reads it as a plain comment, and its filter says the same as
    /// MariaDB's.
    pub m_bang: bool,
    /// How the shape of a filter's statement is checked.
    pub shape: FilterShape,
}

/// How a NULL-safe equality is written ([`FilterSyntax::null_safe`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NullSafe {
    /// `a IS NOT DISTINCT FROM v`, and `a IS DISTINCT FROM v` for the inequality.
    DistinctFrom,
    /// `a <=> v`, and `NOT (a <=> v)` for the inequality (also read as `IS [NOT] DISTINCT
    /// FROM`).
    Spaceship,
}

/// How the statement a filter is put in is checked to be one read of the table
/// ([`FilterSyntax::shape`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterShape {
    /// On the server's parse tree of the statement.
    ParseTree,
    /// On the tokens of each reading of the text.
    Tokens,
}

/// A pattern of a filter ([`FilterSql::like`]): column `col` (its name, not quoted) matched with
/// `pattern` (escaped with `escape`), as `case` says, `negated` or not. `facts`: what is known
/// of the column.
#[derive(Clone, Copy, Debug)]
pub struct Like<'a> {
    pub col: &'a str,
    pub pattern: &'a str,
    pub escape: char,
    pub negated: bool,
    pub case: CaseMode,
    pub facts: Option<&'a ColumnFacts>,
}

/// The items a group count reads ([`FilterSql::group_parts`]): the value, sent only when it is
/// short; its length, or NULL for a type whose values are short; and the count of its rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupParts {
    pub preview: String,
    pub length: String,
    pub count: &'static str,
}
