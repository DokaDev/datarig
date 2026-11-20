//! What is fixed per engine, known without a connection: one [`EngineSpec`] per engine.

use super::EngineId;
use crate::driver::Hierarchy;
use crate::profile::dsn::Scheme;

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
}
