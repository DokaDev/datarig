//! Checks before a statement runs: the profile's policy decides whether it
//! may run at all (`read_only`) and whether it asks first (`confirm`). Statements are
//! classified by `datarig_core::sql::risk`; nothing is sent (or queued for a connection)
//! before the checks pass.
//!
//! **Asking first**: a run with a dangerous statement (`DROP`, `TRUNCATE`,
//! `UPDATE`/`DELETE` of every row or with a `WHERE` that reads no column, `ALTER … DROP
//! COLUMN`, `ALTER … TYPE`, a `MERGE` that updates or deletes, `DO`/`CALL`, a `COPY` with a
//! program or a file on the server, an `EXECUTE` of an unknown prepared statement, a text
//! PostgreSQL's parser rejects; also inside
//! `EXPLAIN ANALYZE` or a data-modifying `WITH`), or with `confirm = "writes"` any statement
//! that may change something, opens one question listing every such statement of the run,
//! with the connection and its policy. Nothing runs before the answer; Cancel has the focus. A run that waits for
//! its connection is asked about before it is queued, and runs without asking again once the
//! connection is up.
//!
//! A read-only policy is enforced twice: here, where every statement that is not a read,
//! transaction control or a safe session setting is refused with a notice naming the policy,
//! and on the server, where every session of the profile (queries and metadata) is opened
//! read-only (`ConnectOptions::read_only`), which also stops writes the text does not show.

use super::overlay::RunConfirm;
use super::*;
use datarig_core::policy::Policy;
use datarig_core::sql::risk::{Class, Danger, NoWhere, ReadOnlyBlock, Why};

/// A statement of a run that asks before it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dangerous {
    /// Its place in the run (from 0).
    pub index: usize,
    pub why: Why,
    /// Why it changes every row (or may), for an `UPDATE` or `DELETE`.
    pub no_where: Option<NoWhere>,
    /// An `EXPLAIN ANALYZE`: it runs, then it is rolled back.
    pub explain: bool,
    /// The table (or object) it acts on, when the text names one.
    pub target: Option<String>,
    pub sql: String,
}

impl App {
    /// The policy of profile `id`: its name as the status bar says it, and its values.
    pub fn policy_of(&self, id: ProfileId) -> (String, Policy) {
        let name = self.profile(id).and_then(|p| p.policy.clone());
        let policy = self.policies.get(name.as_deref());
        (name.unwrap_or_else(|| datarig_core::policy::DEFAULT.to_string()), policy)
    }

    /// Profile `id`'s policy is read-only.
    pub fn read_only(&self, id: ProfileId) -> bool {
        self.policy_of(id).1.read_only
    }

    /// Whether a session opened for `cfg` (the profile as it connected) is read-only: when its
    /// policy was read-only then, or is now.
    pub(super) fn session_read_only(&self, cfg: &ConnectionConfig) -> bool {
        self.policies.get(cfg.policy.as_deref()).read_only || self.read_only(cfg.id)
    }

    /// Why profile `pid`'s policy refuses to run `statements` in tab `tab` (the first it
    /// refuses), or `None` when it may run. The tab's open session must have been opened
    /// read-only too.
    pub(super) fn read_only_refusal(&self, tab: TabId, pid: ProfileId, statements: &[String]) -> Option<Notice> {
        let (policy, values) = self.policy_of(pid);
        if !values.read_only {
            return None;
        }
        // The policy became read-only after this tab's session opened: that session is not
        // read-only on the server, so nothing runs on it until it connects again.
        let t = self.tabs.get(tab)?;
        if t.exec.session.is_some() && !t.exec.read_only {
            return Some(Notice::new(
                Msg::SafetyReadOnlyReconnect { policy, key: self.key_for(Action::ReconnectCurrent, Ctx::Nav) },
                Level::Error,
            ));
        }
        let mut classifier = self.classifier(tab);
        statements.iter().find_map(|sql| {
            let why = classifier.classify(sql).read_only().err()?;
            let what = self.i18n.label(blocked_label(why)).to_string();
            let sql = super::runlog::excerpt(sql, 60);
            Some(Notice::new(Msg::SafetyReadOnlyBlocked { policy: policy.clone(), what, sql }, Level::Error))
        })
    }
}

impl App {
    /// Why `statements` cannot run at all in tab `tab`, whatever the policy: `COPY … FROM
    /// STDIN` and `COPY … TO STDOUT` need the COPY protocol, which the app does not carry yet
    /// (sent, the first broke the tab's connection).
    pub(super) fn unsupported(&self, tab: TabId, statements: &[String]) -> Option<Notice> {
        let lang = self.tabs.get(tab).map_or_else(|| self.tab_language(tab), |t| t.exec.prepared.language());
        let sql = statements.iter().find(|sql| Classifier::classify_once(lang, sql).stdio)?;
        let sql = super::runlog::excerpt(sql, 60);
        Some(Notice::new(Msg::SafetyCopyStdio { sql }, Level::Error))
    }

    /// The classifier of tab `tab`, with the statements its session has prepared (none without
    /// a session): each run's statements are checked in order against a copy, so an `EXPLAIN`
    /// or `EXECUTE` sees what the run prepared before it.
    fn classifier(&self, tab: TabId) -> Classifier {
        // A tab whose language was not pushed when its binding changed would be checked by the
        // rules of another language.
        debug_assert!(self.tabs.get(tab).is_none_or(|t| {
            let lang = self.tab_language(tab);
            t.exec.prepared.language() == lang && t.editor.language() == lang
        }));
        match self.tabs.get(tab) {
            Some(t) if t.exec.session.is_some() => t.exec.prepared.clone(),
            Some(t) => Classifier::new(t.exec.prepared.language()),
            None => Classifier::new(self.tab_language(tab)),
        }
    }

    /// The statements of `statements` that ask before they run in tab `tab` under profile
    /// `pid`'s policy.
    pub(super) fn dangerous(&self, tab: TabId, pid: ProfileId, statements: &[String]) -> Vec<Dangerous> {
        let writes = self.policy_of(pid).1.confirm.writes();
        let mut classifier = self.classifier(tab);
        statements
            .iter()
            .enumerate()
            .filter_map(|(index, sql)| {
                let r = classifier.classify(sql);
                let why = r.confirm(writes)?;
                Some(Dangerous {
                    index,
                    why,
                    no_where: r.no_where,
                    explain: r.rolls_back(),
                    target: r.target,
                    sql: sql.clone(),
                })
            })
            .collect()
    }

    /// Ask before `statements` run in tab `id` (bound to profile `pid`); `items` are the ones
    /// that ask.
    pub(super) fn ask_before_run(&mut self, id: TabId, pid: ProfileId, statements: Vec<String>, items: Vec<Dangerous>) {
        let Some(binding) = self.tabs.get(id).map(|t| t.binding) else { return };
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.key_state.clear();
        self.overlays.push(Overlay::RunConfirm(RunConfirm {
            tab: id,
            profile: pid,
            binding,
            statements,
            items,
            run_focused: false,
            buttons: Default::default(),
        }));
    }

    /// Keys of the run confirmation: `y` runs, `n`/`Esc` cancel, `Enter` does what has the
    /// focus (Cancel at first), `Tab`/arrows/`h`/`l` move the focus. Held keys do nothing.
    pub(super) fn run_confirm_key(&mut self, key: KeyEvent, repeat: bool) {
        if repeat {
            return;
        }
        let Some(c) = self.overlays.run_confirm_mut() else { return };
        match key.code {
            KeyCode::Char('y') => self.run_confirm_answered(true),
            KeyCode::Char('n') | KeyCode::Esc => self.run_confirm_answered(false),
            KeyCode::Enter => {
                let run = c.run_focused;
                self.run_confirm_answered(run);
            }
            KeyCode::Tab | KeyCode::BackTab => c.run_focused = !c.run_focused,
            KeyCode::Left | KeyCode::Char('h') => c.run_focused = false,
            KeyCode::Right | KeyCode::Char('l') => c.run_focused = true,
            _ => {}
        }
    }

    /// The run confirmation was answered: run (only on the tab, profile and tab generation it
    /// was asked for), or say it did not run.
    fn run_confirm_answered(&mut self, run: bool) {
        let Some(c) = self.overlays.take_run_confirm() else { return };
        if !run {
            self.unstage_run(c.tab);
            return self.tab_status(c.tab, Notice::new(Label::SafetyConfirmCancelled, Level::Warning));
        }
        match self.tabs.get(c.tab).map(|t| (t.profile, t.binding)) {
            Some((Some(p), b)) if p == c.profile && b == c.binding => self.run_approved(c.tab, c.statements),
            Some(_) => {
                self.unstage_run(c.tab);
                self.tab_status(c.tab, Notice::new(Label::SafetyConfirmMoved, Level::Warning))
            }
            None => {}
        }
    }
}

/// Why a statement asks, in words: what it does, and what makes it worse.
pub(crate) fn why_text(i18n: &I18n, d: &Dangerous) -> String {
    let what = match d.why {
        Why::Danger(Danger::Drop) => Label::SafetyWhyDrop,
        Why::Danger(Danger::Truncate) => Label::SafetyWhyTruncate,
        Why::Danger(Danger::DeleteAll) => Label::SafetyWhyDeleteAll,
        Why::Danger(Danger::UpdateAll) => Label::SafetyWhyUpdateAll,
        Why::Danger(Danger::DropColumn) => Label::SafetyWhyDropColumn,
        Why::Danger(Danger::AlterColumnType) => Label::SafetyWhyAlterType,
        Why::Danger(Danger::Merge) => Label::SafetyWhyMerge,
        Why::Danger(Danger::Procedural) => Label::SafetyWhyProcedural,
        Why::Danger(Danger::UnknownPrepared) => Label::SafetyWhyUnknownPrepared,
        Why::Danger(Danger::Unparsed) => Label::SafetyWhyUnparsed,
        Why::Danger(Danger::TooComplex) => Label::SafetyWhyTooComplex,
        Why::Danger(Danger::CopyProgram) => Label::SafetyWhyCopyProgram,
        Why::Danger(Danger::CopyFile) => Label::SafetyWhyCopyFile,
        Why::Danger(Danger::ServerFile) => Label::SafetyWhyServerFile,
        Why::Danger(Danger::ServerAction) => Label::SafetyWhyServerAction,
        Why::Danger(Danger::RunsQueryText) => Label::SafetyWhyRunsQueryText,
        Why::Danger(Danger::ExecutableComment) => Label::SafetyWhyExecComment,
        Why::Danger(Danger::Unrecognized) => Label::SafetyWhyUnrecognized,
        Why::Danger(Danger::Locks) => Label::SafetyWhyLocks,
        Why::Danger(Danger::Privileges) => Label::SafetyWhyPrivileges,
        Why::Danger(Danger::Rename) => Label::SafetyWhyRename,
        Why::Danger(Danger::Setting) => Label::SafetyWhySetting,
        Why::Danger(Danger::DynamicSql) => Label::SafetyWhyDynamicSql,
        Why::Danger(Danger::ServerCommand) => Label::SafetyWhyServerCommand,
        Why::Danger(Danger::FileAccess) => Label::SafetyWhyFileAccess,
        Why::Class(Class::Write) => Label::SafetyWhyWrite,
        Why::Class(Class::Ddl) => Label::SafetyWhyDdl,
        Why::Class(Class::Maintenance) => Label::SafetyWhyMaintenance,
        Why::Class(Class::Procedural) => Label::SafetyWhyProcedural,
        Why::Class(_) => Label::SafetyWhyUnknown,
    };
    let mut parts = vec![i18n.label(what).to_string()];
    if let Why::Danger(Danger::DeleteAll | Danger::UpdateAll) = d.why {
        let how = match d.no_where {
            Some(NoWhere::AlwaysTrue) => Label::SafetyWhyAlwaysTrue,
            Some(NoWhere::NoColumn) => Label::SafetyWhyNoColumn,
            Some(NoWhere::OtherTables) => Label::SafetyWhyOtherTables,
            _ => Label::SafetyWhyNoWhere,
        };
        parts.push(i18n.label(how).to_string());
    }
    if d.explain {
        parts.push(i18n.label(Label::SafetyWhyExplain).to_string());
    }
    parts.join(" · ")
}

/// What a read-only policy says about a statement it refuses.
fn blocked_label(why: ReadOnlyBlock) -> Label {
    match why {
        ReadOnlyBlock::ReadWrite => Label::SafetyBlockedReadWrite,
        ReadOnlyBlock::Setting => Label::SafetyBlockedSetting,
        ReadOnlyBlock::Unparsed => Label::SafetyBlockedUnparsed,
        ReadOnlyBlock::TooComplex => Label::SafetyBlockedTooComplex,
        ReadOnlyBlock::ServerFile => Label::SafetyBlockedServerFile,
        ReadOnlyBlock::ServerAction => Label::SafetyBlockedServerAction,
        ReadOnlyBlock::RunsQueryText => Label::SafetyBlockedRunsQueryText,
        ReadOnlyBlock::ExecutableComment => Label::SafetyBlockedExecComment,
        ReadOnlyBlock::Unrecognized => Label::SafetyBlockedUnrecognized,
        ReadOnlyBlock::UnknownFunction => Label::SafetyBlockedUnknownFunction,
        ReadOnlyBlock::DynamicSql => Label::SafetyBlockedDynamicSql,
        ReadOnlyBlock::ServerCommand => Label::SafetyBlockedServerCommand,
        ReadOnlyBlock::Locks => Label::SafetyBlockedLocks,
        ReadOnlyBlock::FileAccess => Label::SafetyBlockedFileAccess,
        ReadOnlyBlock::Class(Class::Write) => Label::SafetyBlockedWrite,
        ReadOnlyBlock::Class(Class::Ddl) => Label::SafetyBlockedDdl,
        ReadOnlyBlock::Class(Class::Maintenance) => Label::SafetyBlockedMaintenance,
        ReadOnlyBlock::Class(Class::Procedural) => Label::SafetyBlockedProcedural,
        ReadOnlyBlock::Class(_) => Label::SafetyBlockedUnknown,
    }
}
