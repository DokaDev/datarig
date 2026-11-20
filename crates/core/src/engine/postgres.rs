//! PostgreSQL's row of the registry.

use super::{EngineId, EngineSpec, RiskRules, ServerFacts, TextRules, Transport};
use crate::driver::Hierarchy;
use crate::profile::dsn::Scheme;
use crate::sql::dialect::{Dialect, DollarQuotes, Language};
use crate::sql::risk::Classifier;
use crate::sql::risk::mysql::Judged;

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
    text: &PgText,
    risk: &PgRisk,
};

/// How PostgreSQL's text is read.
struct PgText;

impl TextRules for PgText {
    /// Always: `$$…$$` and `$tag$…$tag$` are string constants on every PostgreSQL.
    fn dollar_quotes(&self, _: &ServerFacts) -> DollarQuotes {
        DollarQuotes::Read
    }
}

/// How PostgreSQL's statements are judged.
struct PgRisk;

impl RiskRules for PgRisk {
    fn new_classifier(&self, d: Dialect, j: Judged) -> Classifier {
        match j {
            // PostgreSQL has no sibling engine: its sessions are judged on its own rules.
            Judged::Own | Judged::WithSibling => Classifier::new(Language::Sql(d)),
        }
    }
}
