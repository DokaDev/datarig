//! MySQL's row of the registry.

use super::{EngineId, EngineSpec, MYSQL_FAMILY, Transport};
use crate::driver::Hierarchy;
use crate::profile::dsn::Scheme;

pub static SPEC: EngineSpec = EngineSpec {
    id: EngineId::MySql,
    key: "mysql",
    aliases: &[],
    family: MYSQL_FAMILY,
    url_schemes: &["mysql"],
    transport: Transport::Network { scheme: Scheme::MySql, ssh: true },
    preset_key: "mysql",
    form_key: "mysql",
    hierarchy: Hierarchy::SchemaOnly,
};
