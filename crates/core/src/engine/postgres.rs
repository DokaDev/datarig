//! PostgreSQL's row of the registry.

use super::{EngineId, EngineSpec, Transport};
use crate::driver::Hierarchy;
use crate::profile::dsn::Scheme;

pub static SPEC: EngineSpec = EngineSpec {
    id: EngineId::Postgres,
    key: "postgres",
    aliases: &["postgresql", "pg"],
    family: &[EngineId::Postgres],
    url_schemes: &["postgres", "postgresql"],
    transport: Transport::Network { scheme: Scheme::Postgres, ssh: true },
    preset_key: "postgres",
    form_key: "postgres",
    hierarchy: Hierarchy::DatabaseSchema,
};
