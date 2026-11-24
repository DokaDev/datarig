//! MySQL's row of the registry.

use super::mysql_family::{
    FamilyCancel, FamilyExport, FamilySession, FamilyText, FamilyTx, FamilyValues, family_classifier,
    family_filter_calls, family_read_only_calls,
};
use super::{
    EditRules, EngineFacts, EngineId, EngineSpec, Fact, FilterCalls, MYSQL_FAMILY, RiskRules, ServerFacts, Support,
    Transport, Version, Why,
};
use crate::driver::{EditFormat, Hierarchy, MySqlEdit};
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
    values: &FamilyValues { offsets_since: Some(Version::new(8, 0, 19)) },
    export: Fact::Known(&FamilyExport),
    edit: Fact::Known(&MysqlEdits),
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

/// Which edits of a result's rows MySQL's servers take: MySQL's from 8.0.17 on, where
/// `utf8mb4_0900_bin` compares text exactly and `FOR UPDATE NOWAIT` locks without waiting.
struct MysqlEdits;

/// The first MySQL version whose servers take MySQL's edits.
const EDIT_SINCE: Version = Version::new(8, 0, 17);

impl EditRules for MysqlEdits {
    /// By the number of the server's greeting, whichever engine its version line names.
    fn writer(&self, f: &ServerFacts) -> Fact<EditFormat> {
        match &f.detail {
            EngineFacts::MySql(my) | EngineFacts::MariaDb(my) => match my.handshake >= EDIT_SINCE {
                true => Fact::Known(EditFormat::MySql(MySqlEdit { nowait: true })),
                false => Fact::No(Why::VersionTooOld { needed: EDIT_SINCE }),
            },
            // Never the facts of a MySQL greeting.
            EngineFacts::Postgres(_) => Fact::Unknown(Why::EnginesDisagree),
        }
    }
}
