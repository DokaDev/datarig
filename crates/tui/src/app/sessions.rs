//! Session presets in the app: what a profile's query sessions start with
//! ([`App::session_plan_of`], checked before a profile connects and again before each query
//! session opens), and how a failure of it is said.
//!
//! A profile whose plan has an error (a preset that is not there, one for the other database, a
//! statement that is not a setting or that its read-only environment refuses) does not connect:
//! nothing is asked or sent, never without the preset instead.

use super::*;
use datarig_core::profile::session::{self, Dbms, Defaults, PickedBy, SessionError, SessionPlan, Source};
use datarig_core::sql::risk::{Owner, SetProblem};

impl App {
    /// The session presets (`[sessions.<name>]`), by name.
    pub fn session_presets(&self) -> &[datarig_core::profile::session::SessionPreset] {
        &self.session_presets
    }

    /// What the query sessions of `cfg` (a profile as it connects) start with, by its rules now.
    pub fn session_plan_of(&self, cfg: &ConnectionConfig) -> Result<SessionPlan, SessionError> {
        // An environment that is not defined: the strictest rules (the connect refuses it first).
        let rules = self.policies.get(cfg.policy.as_deref()).unwrap_or_else(|_| self.rules_of(cfg.id));
        let defaults = Defaults { lock_wait: self.prefs.lock_wait, idle_disconnect: self.prefs.idle_disconnect };
        session::plan(cfg, &rules, defaults, &self.session_presets)
    }

    /// Where a plan's statement comes from, in words.
    pub(super) fn session_source_text(&self, source: &Source) -> String {
        let m = match source {
            Source::Preset { name, by: PickedBy::Profile } => Msg::SessionSourceProfilePreset { preset: name.clone() },
            Source::Preset { name, by: PickedBy::Environment(Some(e)) } => {
                Msg::SessionSourceEnvironmentPreset { preset: name.clone(), environment: e.clone() }
            }
            Source::Preset { name, by: PickedBy::Environment(None) } => {
                Msg::SessionSourceDefaultPreset { preset: name.clone() }
            }
            _ => return self.i18n.label(Label::SessionSourceProfile).to_string(),
        };
        self.i18n.msg(&m).to_string()
    }

    /// "statement `index` of `source`", in words.
    pub(super) fn statement_of(&self, source: &Source, index: usize) -> String {
        let m = Msg::SessionStatementOf { index: index.to_string(), source: self.session_source_text(source) };
        self.i18n.msg(&m).to_string()
    }

    /// Why a statement cannot be a start-up setting, in words.
    pub(super) fn set_problem_text(&self, p: &SetProblem) -> String {
        let label = match p {
            SetProblem::NotOne => Label::SetNotOne,
            SetProblem::Unread => Label::SetUnread,
            SetProblem::NotSet => Label::SetNotSet,
            SetProblem::Local => Label::SetLocal,
            SetProblem::Writes => Label::SetWrites,
            SetProblem::Danger(_) => Label::SetDanger,
            SetProblem::AppOwned { name, owner } => {
                let owner = self.i18n.label(match owner {
                    Owner::LockWait => Label::SetOwnerLockWait,
                    Owner::IdleDisconnect => Label::SetOwnerIdleDisconnect,
                    Owner::TxIdleRollback => Label::SetOwnerTxIdleRollback,
                    Owner::ReadOnly => Label::SetOwnerReadOnly,
                    Owner::Commit => Label::SetOwnerCommit,
                    Owner::Schema => Label::SetOwnerSchema,
                    Owner::App => Label::SetOwnerApp,
                });
                let m = Msg::SetAppOwned { setting: name.clone(), owner: owner.to_string() };
                return self.i18n.msg(&m).to_string();
            }
        };
        self.i18n.label(label).to_string()
    }

    /// What profile `name` says when its plan has error `e`.
    pub(super) fn session_error_notice(&self, name: &str, e: &SessionError) -> Notice {
        let name = name.to_string();
        let sql = |s: &str| super::runlog::excerpt(s, 60);
        let m = match e {
            SessionError::NotFound { preset, by: PickedBy::Profile } => {
                Msg::SessionNotFoundProfile { name, preset: preset.clone() }
            }
            SessionError::NotFound { preset, by: PickedBy::Environment(env) } => {
                let dbms = self
                    .profiles
                    .iter()
                    .find(|c| c.name == name)
                    .and_then(|c| Dbms::of_driver(&c.driver))
                    .map_or_else(String::new, |d| d.display().to_string());
                match env {
                    Some(e) => {
                        Msg::SessionNotFoundEnvironment { name, preset: preset.clone(), environment: e.clone(), dbms }
                    }
                    None => Msg::SessionNotFoundDefault { name, preset: preset.clone(), dbms },
                }
            }
            SessionError::WrongDbms { preset, by, dbms, driver } => {
                let source = self.session_source_text(&Source::Preset { name: preset.clone(), by: by.clone() });
                let driver = Dbms::of_driver(driver).map_or(driver.clone(), |d| d.display().to_string());
                Msg::SessionWrongDbms { name, source, dbms: dbms.display().to_string(), driver }
            }
            SessionError::Statement { source, index, statement, problem } => Msg::SessionStatement {
                name,
                statement: self.statement_of(source, *index),
                problem: self.set_problem_text(problem),
                sql: sql(statement),
            },
            SessionError::ReadOnly { source, index, statement, block } => Msg::SessionReadOnly {
                name,
                statement: self.statement_of(source, *index),
                what: self.i18n.label(super::safety::blocked_label(*block)).to_string(),
                sql: sql(statement),
            },
            SessionError::Driver(driver) => Msg::SessionDriver { name, driver: driver.clone() },
        };
        Notice::new(m, Level::Error)
    }

    /// A start-up statement of profile `pid`'s query session failed (`index` from 0, as the
    /// driver counts): which one, from where, and the server's words.
    pub(super) fn session_failed_text(&self, pid: Option<ProfileId>, index: usize, sql: &str, error: &str) -> String {
        let cfg = pid.and_then(|p| self.conns.get(p).and_then(|c| c.resolved.as_ref()).or(self.profile(p)));
        let name = cfg.map(|c| c.name.clone()).unwrap_or_default();
        let plan = cfg.and_then(|c| self.session_plan_of(c).ok());
        let statement = match plan.as_ref().and_then(|p| p.statement_source(index)) {
            Some((source, n)) => self.statement_of(source, n),
            None => self.statement_of(&Source::Profile, index + 1),
        };
        let m = Msg::SessionFailed { name, statement, error: error.to_string(), sql: super::runlog::excerpt(sql, 60) };
        self.i18n.msg(&m).to_string()
    }

    /// A PostgreSQL query session behind a pooler sets its start-up statements per transaction;
    /// `refused` (indexes from 0) were not sent at all.
    pub(super) fn session_per_transaction_notice(&self, pid: Option<ProfileId>, refused: &[usize]) -> Notice {
        let cfg = pid.and_then(|p| self.conns.get(p).and_then(|c| c.resolved.as_ref()).or(self.profile(p)));
        let name = cfg.map(|c| c.name.clone()).unwrap_or_default();
        if refused.is_empty() {
            return Notice::new(Msg::SessionPerTransaction { name }, Level::Info);
        }
        let statements = cfg.and_then(|c| self.session_plan_of(c).ok()).map(|p| p.statements()).unwrap_or_default();
        let sql: Vec<String> =
            refused.iter().filter_map(|i| statements.get(*i)).map(|s| super::runlog::excerpt(&s.sql, 40)).collect();
        Notice::new(
            Msg::SessionPooledRefused { name, count: refused.len() as u64, sql: sql.join("; ") },
            Level::Warning,
        )
    }
}
