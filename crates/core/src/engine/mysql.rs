//! MySQL's row of the registry.

use super::mysql_family::{
    FamilyCancel, FamilyExport, FamilySession, FamilyText, FamilyTx, FamilyValues, family_classifier,
    family_filter_calls, family_read_only_calls,
};
use super::{EngineId, EngineSpec, Fact, FilterCalls, MYSQL_FAMILY, RiskRules, Support, Transport};
use crate::driver::Hierarchy;
use crate::profile::dsn::Scheme;
use crate::sql::dialect::Dialect;
use crate::sql::risk::Classifier;
use crate::sql::risk::mysql::Judged;

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
    text: &FamilyText,
    risk: &MysqlRisk,
    tx: &FamilyTx,
    session: &FamilySession,
    cancel: &FamilyCancel,
    values: &FamilyValues,
    export: Fact::Known(&FamilyExport),
};

/// How MySQL's statements are judged: the family's classifier of the text's dialect
/// ([`family_classifier`]), which reads a MySQL session's text with its engine's lists.
struct MysqlRisk;

impl RiskRules for MysqlRisk {
    fn new_classifier(&self, d: Dialect, j: Judged) -> Classifier {
        family_classifier(d, j)
    }

    fn read_only_calls(&self) -> Support {
        family_read_only_calls()
    }

    /// Never: the family's results are not checked for their text style.
    fn rerun_style(&self, _: &str) -> Option<bool> {
        None
    }

    fn filter_calls(&self) -> FilterCalls {
        family_filter_calls()
    }

    /// On a session whose reports name MariaDB too.
    fn refuses_m_bang(&self, j: Judged) -> bool {
        match j {
            Judged::Own => false,
            Judged::WithSibling => true,
        }
    }
}
