//! MySQL's row of the registry.

use super::mysql_family::{
    FamilyCancel, FamilyCatalogue, FamilyDdl, FamilyExport, FamilyFilter, FamilySession, FamilySort, FamilyText,
    FamilyTx, FamilyValues, family_classifier, family_filter_calls, family_read_only_calls,
};
use super::{
    ApplierName, ChannelWords, Charts, LagBytes, LastMessage, ReceiverName, ReplicaWho, ReplicationTerms, RoleWords,
    SessionIdName, VersionLine, WaitsAsked,
};
use super::{
    EditRules, EngineFacts, EngineId, EngineSpec, Fact, FilterCalls, MYSQL_FAMILY, MonitorRules, MonitorTerms,
    PlanRules, Provider, RiskRules, Routes, ServerFacts, Support, Transport, Version, Why,
};
use crate::driver::{EditFormat, ExplainFormat, Hierarchy, MySqlEdit};
use crate::monitor::action::{
    ActionKind, ActionRoute, ActionTarget, Ask, Denied, Fallback, Guidance, MYSQL_EXECUTE_DENIED, MYSQL_KILL_DENIED,
    MYSQL_NEEDS_PRIVILEGE, NativeError, StatementStamp, Via, Warning,
};
use crate::monitor::{MonitorFormat, SessionRow};
use crate::profile::dsn::Scheme;
use crate::sql::dialect::{Dialect, MySqlMode};
use crate::sql::risk::mysql::{Judge, Judged};
use crate::sql::risk::{Class, Classifier, Explain};

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
    monitor: Fact::Known(&MysqlMonitorRules),
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

/// What a monitor may do to the other sessions of MySQL's servers.
struct MysqlMonitorRules;

/// What the monitor's views call things on MySQL's servers: connections by id, the role by
/// `read_only`, the version with its product's name, starts in whole seconds, channels with
/// their receiver and applier threads; its lock read lists every wait, and its own warning
/// says what an end keeps.
const MYSQL_TERMS: MonitorTerms = MonitorTerms {
    session_id: SessionIdName::Id,
    role: RoleWords::ReadOnlyFlag,
    version_line: VersionLine::NumberThenProduct,
    waits_asked: WaitsAsked::All,
    charts: Charts::StatementsAndCommands,
    start_in_seconds: true,
    end_says_rollback: false,
    replication: ReplicationTerms {
        channels: ChannelWords::Named,
        receiver: ReceiverName::Receiver,
        last_message: LastMessage::Heartbeat,
        receiver_line_always: true,
        applier: ApplierName::Applier,
        applied_on_applier_line: true,
        zero_lag_note: true,
        lag_bytes: LagBytes::Fetched,
        replica: ReplicaWho::ClientUserState,
    },
};

const MYSQL_STANDARD: Guidance = Guidance { grants: &["CONNECTION_ADMIN"], service_users: &[] };
const RDS_MYSQL: Fallback =
    Fallback { cancel: "mysql.rds_kill_query", terminate: "mysql.rds_kill", on: &[MYSQL_KILL_DENIED] };
const AZURE_MYSQL: Fallback =
    Fallback { cancel: "mysql.az_kill_query", terminate: "mysql.az_kill", on: &[MYSQL_KILL_DENIED] };

/// MySQL's action routes: `KILL`, and on the managed services that refuse it for other
/// accounts' sessions, their own procedures.
static MYSQL_ROUTES: Routes = Routes {
    standard: ActionRoute { flavour: Provider::Standard, fallback: None, guidance: MYSQL_STANDARD },
    providers: &[
        ActionRoute {
            flavour: Provider::AmazonRds,
            fallback: Some(RDS_MYSQL),
            guidance: Guidance { grants: MYSQL_STANDARD.grants, service_users: &["rdsadmin"] },
        },
        ActionRoute {
            flavour: Provider::AmazonAurora,
            fallback: Some(RDS_MYSQL),
            guidance: Guidance { grants: MYSQL_STANDARD.grants, service_users: &["rdsadmin"] },
        },
        ActionRoute {
            flavour: Provider::AzureDatabase,
            fallback: Some(AZURE_MYSQL),
            guidance: Guidance { grants: MYSQL_STANDARD.grants, service_users: &["azure_superuser"] },
        },
    ],
};

/// The last MySQL version of Amazon RDS whose kill procedures fail for long target user names.
const RDS_LONG_USER_FIXED_AFTER: u32 = 80032;

/// The longest target user name those procedures take.
const RDS_LONG_USER_CHARS: usize = 16;

impl MonitorRules for MysqlMonitorRules {
    fn reads(&self, f: &MonitorFormat) -> bool {
        match f {
            MonitorFormat::MySql(_) => true,
            MonitorFormat::Postgres(_) => false,
        }
    }

    /// When the connection's marker was not found on its own id when it opened
    /// ([`crate::monitor::MySqlMonitor::unconfirmed`]).
    fn unconfirmed(&self, f: &MonitorFormat) -> bool {
        match f {
            MonitorFormat::MySql(my) => my.unconfirmed,
            MonitorFormat::Postgres(_) => true,
        }
    }

    /// Always: the id, with the thread where it is read (the driver refuses a target with
    /// neither its start nor its thread).
    fn identifies(&self, f: &MonitorFormat, _: &SessionRow) -> bool {
        match f {
            MonitorFormat::MySql(_) => true,
            MonitorFormat::Postgres(_) => false,
        }
    }

    /// No: an unconfirmed connection sends nothing at all ([`MonitorRules::unconfirmed`]).
    fn end_needs_start(&self) -> bool {
        false
    }

    /// Never: a connection not confirmed direct is [`MonitorRules::unconfirmed`].
    fn pooled(&self, f: &MonitorFormat) -> bool {
        match f {
            MonitorFormat::MySql(_) => false,
            MonitorFormat::Postgres(_) => true,
        }
    }

    /// A cancel: a line for a plain read whose whole text is shown, else a dialog that warns of
    /// a weak stamp first, then of table maintenance or of changes left half done. An end: a
    /// dialog that says non-transactional changes stay.
    fn ask(&self, f: &MonitorFormat, row: &SessionRow, kind: ActionKind) -> Ask {
        match (f, kind) {
            (MonitorFormat::MySql(_), ActionKind::Cancel) => {
                let mut warnings = Vec::new();
                if row.statement.as_ref().is_some_and(StatementStamp::weak) {
                    warnings.push(Warning::WeakIdentity);
                }
                let text = row.query.as_ref();
                if text.is_some_and(|q| maintenance(&q.text)) {
                    warnings.push(Warning::MySqlMaintenance);
                } else if !text.is_some_and(|q| !q.shortened && plain_read(&q.text)) {
                    warnings.push(Warning::MySqlMayLeaveHalfDone);
                }
                if warnings.is_empty() { Ask::Line } else { Ask::Modal { warnings } }
            }
            (MonitorFormat::MySql(_), ActionKind::Terminate) => {
                Ask::Modal { warnings: vec![Warning::MySqlTerminateKeepsNonTx] }
            }
            (MonitorFormat::Postgres(_), ActionKind::Cancel | ActionKind::Terminate) => Ask::Modal {
                warnings: vec![
                    Warning::WeakIdentity,
                    Warning::MySqlMaintenance,
                    Warning::MySqlMayLeaveHalfDone,
                    Warning::MySqlTerminateKeepsNonTx,
                ],
            },
        }
    }

    fn routes(&self) -> &'static Routes {
        &MYSQL_ROUTES
    }

    fn terms(&self) -> &'static MonitorTerms {
        &MYSQL_TERMS
    }

    /// Amazon RDS up to 8.0.32 refusing a procedure for a long target user name; the
    /// provider's own session; the grants that allow `KILL` (or a target that holds
    /// `SYSTEM_USER`); the provider's procedure the account may not call.
    fn denied(&self, f: &MonitorFormat, target: &ActionTarget, via: Via, e: NativeError) -> Option<Denied> {
        let my = match f {
            MonitorFormat::MySql(my) => my,
            MonitorFormat::Postgres(_) => return None,
        };
        let errno = match e {
            NativeError::MySql { errno } => errno,
            NativeError::Pg { .. } => return None,
        };
        let guidance = self.routes().of(my.flavour).guidance;
        let service = target.user.as_deref().is_some_and(|u| guidance.service_users.contains(&u));
        let long_user = target.user.as_deref().is_some_and(|u| u.chars().count() > RDS_LONG_USER_CHARS);
        match errno {
            _ if matches!(via, Via::Procedure(_))
                && my.flavour == Provider::AmazonRds
                && my.version <= RDS_LONG_USER_FIXED_AFTER
                && long_user
                && [MYSQL_KILL_DENIED, MYSQL_NEEDS_PRIVILEGE].contains(&errno) =>
            {
                Some(Denied::RdsLongUserName)
            }
            MYSQL_KILL_DENIED | MYSQL_NEEDS_PRIVILEGE if service => Some(Denied::ServiceSession),
            MYSQL_KILL_DENIED => Some(Denied::GrantOrSystemUser(guidance.grants)),
            MYSQL_NEEDS_PRIVILEGE => Some(Denied::SystemUserTarget),
            MYSQL_EXECUTE_DENIED => match via {
                Via::Procedure(name) => Some(Denied::ProcedureDenied(name)),
                Via::Standard => None,
            },
            _ => None,
        }
    }
}

/// The first keywords of MySQL statements that maintain a table.
const MAINTENANCE: &[&str] = &["ALTER", "OPTIMIZE", "REPAIR", "CHECK", "ANALYZE", "LOAD"];

/// The statement's first word is one of [`MAINTENANCE`] (read from a text that may be cut).
fn maintenance(text: &str) -> bool {
    let word: String = text
        .trim_start_matches(|c: char| c.is_whitespace() || c == '(')
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    MAINTENANCE.iter().any(|k| word.eq_ignore_ascii_case(k))
}

/// The MySQL classifier calls the whole text a read with no effect beyond it: no function that
/// is not a built-in, nothing that runs code, no danger, no `EXPLAIN ANALYZE`. The text is a
/// family server's (its `performance_schema`), judged on MySQL's lists; which engine it is is
/// not known here, so a text either engine's lists call a write (on MariaDB a sequence's next
/// value) is no plain read either. The session's sql mode is not known; the classifier also reads the text as the
/// other modes would and keeps the worse reading.
fn plain_read(text: &str) -> bool {
    let mode = MySqlMode::default();
    if crate::sql::risk::mysql::classify(text, Judge::mysql(mode).with_sibling()).writes {
        return false;
    }
    let risk = crate::sql::risk::mysql::classify(text, Judge::mysql(mode));
    risk.class == Class::Read
        && risk.danger.is_none()
        && !risk.writes
        && risk.unchecked_call.is_none()
        && !risk.runs_code
        && risk.explain != Explain::Analyze
}
