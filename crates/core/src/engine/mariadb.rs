//! MariaDB's row of the registry. Its profiles use MySQL's URL form, session presets, form
//! row and icon.

use super::mysql_family::{FamilyCancel, FamilySession, FamilyText, FamilyTx, family_classifier};
use super::{EngineId, EngineSpec, MYSQL_FAMILY, RiskRules, Transport};
use crate::driver::Hierarchy;
use crate::profile::dsn::Scheme;
use crate::sql::dialect::Dialect;
use crate::sql::risk::Classifier;
use crate::sql::risk::mysql::Judged;

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
    text: &FamilyText,
    risk: &MariadbRisk,
    tx: &FamilyTx,
    session: &FamilySession,
    cancel: &FamilyCancel,
};

/// How MariaDB's statements are judged: the family's classifier of the text's dialect
/// ([`family_classifier`]), which reads a MariaDB session's text with its engine's lists.
struct MariadbRisk;

impl RiskRules for MariadbRisk {
    fn new_classifier(&self, d: Dialect, j: Judged) -> Classifier {
        family_classifier(d, j)
    }
}
