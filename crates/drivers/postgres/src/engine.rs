//! What the driver knows of the engine it serves, PostgreSQL: what it offers ([`CAPABILITIES`])
//! and the facts a session's connection found ([`facts`]).

use datarig_core::driver::{Capabilities, EditFormat, ExplainFormat, Hierarchy};
use datarig_core::engine::{
    EngineFacts, EngineId, Fact, PgFacts, PingIdentity, ServerFacts, SessionRules, Topology, Version, Why,
};
use datarig_core::sql::dialect::{Dialect, Language};
use std::sync::Arc;
use tokio_postgres::SharedParameters;

/// The engines the driver serves.
pub(crate) const ENGINES: &[EngineId] = &[EngineId::Postgres];

/// What the engine's sessions may be given and open on.
pub(crate) fn session_rules() -> &'static dyn SessionRules {
    EngineId::Postgres.spec().session
}

/// What the driver offers.
pub(crate) const CAPABILITIES: Capabilities = Capabilities {
    server_paging: true,
    cancel: true,
    introspection: true,
    key_metadata: true,
    contexts: true,
    structure: true,
    ddl: true,
    language: Language::Sql(Dialect::Postgres),
    hierarchy: Hierarchy::DatabaseSchema,
    explain: Some(ExplainFormat::PostgresJson),
    reads_held_to_end: false,
    export: true,
    manual_commit: true,
    edit: Some(EditFormat::Postgres),
    monitor: true,
};

/// The facts of a session's connection, once it opened: the version the server reported at
/// startup (`server_version` of `params`, no statement of its own) and whether the session
/// reaches the server directly (`direct`, the connection's own check; `None`: it did not run,
/// or failed).
pub(crate) fn facts(params: &SharedParameters, direct: Option<bool>) -> Arc<ServerFacts> {
    let text = params.get("server_version").map(|(value, _)| value);
    let topology = match direct {
        Some(true) => Fact::Known(Topology::Direct),
        Some(false) => Fact::Known(Topology::Pooled),
        None => Fact::Unknown(Why::NotChecked),
    };
    Arc::new(ServerFacts {
        version: version(text.as_deref()),
        version_text: text,
        provider: Fact::Unknown(Why::NotChecked),
        topology,
        detail: EngineFacts::Postgres(PgFacts::default()),
    })
}

/// What a test connection to a server that reported its version as `server_version` says of
/// it.
pub(crate) fn identity(server_version: &str) -> PingIdentity {
    PingIdentity { engine: Fact::Known(EngineId::Postgres), version: version(Some(server_version)) }
}

/// A reported version, parsed; unknown when there is none or it does not parse.
fn version(text: Option<&str>) -> Fact<Version> {
    match text.and_then(Version::parse) {
        Some(v) => Fact::Known(v),
        None => Fact::Unknown(Why::VersionUnknown),
    }
}
