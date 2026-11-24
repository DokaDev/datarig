//! What is fixed per engine, known without a connection: one [`EngineSpec`] per engine.

use super::{CountInTx, EngineId, Fact, OpenVerdict, ReadOnlyProof, ServerFacts, Support, TxStatement, TxVerdict};
use crate::driver::{ColumnMeta, EditFormat, Hierarchy, ReadIn, SessionRole, SessionValues, ValueKind};
use crate::edit::{Invalid, TableColumn};
use crate::export::keyset::NotKeyset;
use crate::export::{Kind, Refused, SqlCells, SqlColumn};
use crate::profile::dsn::Scheme;
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
