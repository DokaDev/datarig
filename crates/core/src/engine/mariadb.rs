//! MariaDB's row of the registry. Its profiles use MySQL's URL form, session presets, form
//! row and icon.

use super::{EngineId, EngineSpec, MYSQL_FAMILY, Transport};
use crate::driver::Hierarchy;
use crate::profile::dsn::Scheme;

pub static SPEC: EngineSpec = EngineSpec {
    id: EngineId::MariaDb,
    key: "mariadb",
    aliases: &[],
    family: MYSQL_FAMILY,
    url_schemes: &["mariadb"],
    transport: Transport::Network { scheme: Scheme::MySql, ssh: true },
    preset_key: "mysql",
    form_key: "mysql",
    hierarchy: Hierarchy::SchemaOnly,
};
