//! What is fixed per engine, known without a connection: one [`EngineSpec`] per engine.

use super::{EngineId, ServerFacts};
use crate::driver::Hierarchy;
use crate::profile::dsn::Scheme;
use crate::sql::dialect::{Dialect, DollarQuotes};
use crate::sql::risk::Classifier;
use crate::sql::risk::mysql::Judged;

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
