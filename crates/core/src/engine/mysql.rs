//! MySQL's row of the registry.

use super::mysql_family::{
    FamilyCancel, FamilyCatalogue, FamilyDdl, FamilyExport, FamilyFilter, FamilySession, FamilySort, FamilyText,
    FamilyTx, FamilyValues, family_classifier, family_filter_calls, family_read_only_calls,
};
use super::{
    EditRules, EngineFacts, EngineId, EngineSpec, Fact, FilterCalls, MYSQL_FAMILY, PlanRules, RiskRules, ServerFacts,
    Support, Transport, Version, Why,
};
use crate::driver::{EditFormat, ExplainFormat, Hierarchy, MySqlEdit};
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
    catalogue: &FamilyCatalogue,
    ddl: Fact::Known(&FamilyDdl),
    sort: &FamilySort,
    filter: Fact::Known(&FamilyFilter),
    plan: Fact::Known(&MysqlPlan),
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

/// Which plan format MySQL's servers give: the tree from 8.0.16 on (`EXPLAIN FORMAT=TREE`;
/// `EXPLAIN ANALYZE` from 8.0.18, an older server's error is its own).
struct MysqlPlan;

/// The first MySQL version whose servers give the tree.
const PLAN_SINCE: Version = Version::new(8, 0, 16);

impl PlanRules for MysqlPlan {
    /// By the server's version when it is known, else by the number of its greeting, whichever
    /// engine its version line names: a compatibility decision (a known version is the
    /// greeting's number), so a session whose reports disagree keeps the plan it had.
    fn format(&self, f: &ServerFacts) -> Fact<ExplainFormat> {
        match &f.detail {
            EngineFacts::MySql(my) | EngineFacts::MariaDb(my) => {
                let version = match &f.version {
                    Fact::Known(v) => *v,
                    Fact::No(_) | Fact::Unknown(_) => my.handshake,
                };
                match version >= PLAN_SINCE {
                    true => Fact::Known(ExplainFormat::MySqlTree),
                    false => Fact::No(Why::VersionTooOld { needed: PLAN_SINCE }),
                }
            }
            // Never the facts of a MySQL greeting.
            EngineFacts::Postgres(_) => Fact::Unknown(Why::EnginesDisagree),
        }
    }
}

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

    /// Only when both sessions said what their server is, each reports one engine at one known
    /// version, the same on both, and the applying session takes edits in `format`. The first
    /// cause that fails is the answer: a session that said nothing or whose version was not
    /// read, then sessions that differ, then a server too old, then another format.
    fn duplicates_choice(
        &self,
        structure_by: Option<&ServerFacts>,
        applied_by: Option<&ServerFacts>,
        format: EditFormat,
    ) -> Support {
        let (Some(read), Some(applies)) = (structure_by, applied_by) else {
            return Fact::Unknown(Why::NotChecked);
        };
        // Not read, as against read twice and found to differ.
        let not_read = |f: &ServerFacts| match &f.version {
            Fact::Known(_) | Fact::Unknown(Why::VersionsDisagree | Why::EnginesDisagree) => false,
            Fact::Unknown(_) | Fact::No(_) => true,
        };
        if not_read(read) || not_read(applies) {
            return Fact::Unknown(Why::NotChecked);
        }
        let one = match (&read.version, &applies.version) {
            (Fact::Known(a), Fact::Known(b)) => a == b,
            _ => false,
        };
        if !one || read.engine() != applies.engine() || read.reports_disagree() || applies.reports_disagree() {
            return Fact::No(Why::SessionsDiffer);
        }
        match self.writer(applies) {
            Fact::Known(f) if f == format => Fact::Known(()),
            Fact::Known(_) => Fact::No(Why::EditFormatDiffers),
            Fact::No(why) => Fact::No(why),
            Fact::Unknown(why) => Fact::Unknown(why),
        }
    }
}
