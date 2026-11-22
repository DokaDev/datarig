//! What is fixed per engine, known without a connection: one [`EngineSpec`] per engine.

use super::{CountInTx, EngineId, OpenVerdict, ReadOnlyProof, ServerFacts, Support, TxStatement, TxVerdict};
use crate::driver::{Hierarchy, SessionRole};
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
