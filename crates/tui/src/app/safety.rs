//! Checks before a statement runs: the profile's environment (its rules, or the default rules
//! without one) decides whether it may run at all (`read_only`) and whether it asks first
//! (`confirm`). A profile naming an environment that is not defined runs nothing: every
//! statement is refused, never run under the default rules instead. Statements are
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
//! with the connection and its environment. Nothing runs before the answer; Cancel has the focus. A run that waits for
//! its connection is asked about before it is queued, and runs without asking again once the
//! connection is up.
//!
//! A read-only environment is enforced twice: here, where every statement that is not a read,
//! transaction control or a safe session setting is refused with a notice naming the
//! environment,
//! and on the server, where every session of the profile (queries and metadata) is opened
//! read-only (`ConnectOptions::read_only`), which also stops writes the text does not show.

use super::conn::{ExtensionRead, ExtensionState};
use super::overlay::RunConfirm;
use super::*;
use datarig_core::engine::Fact;
use datarig_core::policy::{Confirm, Policy, UnknownEnvironment};
use datarig_core::sql::risk::{CallHint, Class, Danger, ExtensionLookup, NoWhere, ReadOnlyBlock, ReadOnlyContext, Why};

/// A statement of a run that asks before it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dangerous {
    /// Its place in the run (from 0).
    pub index: usize,
    /// Why its risk asks (`None`: only what it does to the open transaction asks).
    pub why: Option<Why>,
    /// What it does to the open transaction that asks ([`super::tx::TxReason`]).
    pub tx: Option<super::tx::TxReason>,
    /// Why it changes every row (or may), for an `UPDATE` or `DELETE`.
    pub no_where: Option<NoWhere>,
    /// An `EXPLAIN ANALYZE`: it runs, then it is rolled back.
    pub explain: bool,
    /// The table (or object) it acts on, when the text names one.
    pub target: Option<String>,
    pub sql: String,
}

/// Whether the user's yes to `approved` covers `item`, a statement's verdict judged again: the
/// same statement was asked about for each reason it asks for now (its risk, what it does to
/// the open transaction).
pub(super) fn covers(approved: &[Dangerous], item: &Dangerous) -> bool {
    let at = |a: &&Dangerous| a.index == item.index;
    item.why.as_ref().is_none_or(|w| approved.iter().filter(at).any(|a| a.why.as_ref() == Some(w)))
        && item.tx.as_ref().is_none_or(|t| approved.iter().filter(at).any(|a| a.tx.as_ref() == Some(t)))
}

impl App {
    /// The rules of profile `id`: its environment's, the default rules without one, or the
    /// name of the environment it names that is not defined.
    pub fn policy_of(&self, id: ProfileId) -> Result<Policy, UnknownEnvironment> {
        self.policies.get(self.profile(id).and_then(|p| p.policy.as_deref()))
    }

    /// The rules profile `id` is held to: [`App::policy_of`], and for an environment that is not
    /// defined the strictest (read-only, every write asks), so that nothing runs under looser
    /// ones by mistake ([`App::read_only_refusal`] refuses everything then anyway).
    pub fn rules_of(&self, id: ProfileId) -> Policy {
        self.policy_of(id).unwrap_or(Policy { read_only: true, confirm: Confirm::Writes, ..Policy::default() })
    }

    /// The name of profile `id`'s environment, as the status bar and the dialogs say it
    /// (`None`: the default rules).
    pub fn environment_name(&self, id: ProfileId) -> Option<String> {
        self.profile(id)?.policy.clone().filter(|n| n != datarig_core::policy::DEFAULT)
    }

    /// Profile `id`'s environment is read-only.
    pub fn read_only(&self, id: ProfileId) -> bool {
        self.rules_of(id).read_only
    }

    /// What profile `name` says when its environment `environment` is not defined.
    pub(super) fn unknown_environment_notice(&self, name: &str, e: &UnknownEnvironment) -> Notice {
        Notice::new(Msg::EnvironmentNotFound { name: name.to_string(), environment: e.0.clone() }, Level::Error)
    }

    /// Whether a session opened for `cfg` (the profile as it connected) is read-only: when its
    /// environment was read-only then, or is now (one that is not defined counts as read-only).
    pub(super) fn session_read_only(&self, cfg: &ConnectionConfig) -> bool {
        !self.policies.get(cfg.policy.as_deref()).is_ok_and(|p| !p.read_only) || self.read_only(cfg.id)
    }

    /// Why profile `pid`'s environment refuses to run `statements` in tab `tab` (the first it
    /// refuses), or `None` when it may run. The tab's open session must have been opened
    /// read-only too.
    pub(super) fn read_only_refusal(&self, tab: TabId, pid: ProfileId, statements: &[String]) -> Option<Notice> {
        let values = match self.policy_of(pid) {
            Ok(v) => v,
            // Its environment was deleted or renamed away since it connected: nothing runs.
            Err(e) => {
                let name = self.profile(pid).map(|p| p.name.clone()).unwrap_or_default();
                return Some(self.unknown_environment_notice(&name, &e));
            }
        };
        if !values.read_only {
            return None;
        }
        let environment = self.environment_name(pid).unwrap_or_else(|| datarig_core::policy::DEFAULT.to_string());
        // The environment became read-only after this tab's session opened: that session is not
        // read-only on the server, so nothing runs on it until it connects again.
        let t = self.tabs.get(tab)?;
        if t.exec.session.is_some() && !t.exec.read_only {
            return Some(Notice::new(
                Msg::SafetyReadOnlyReconnect { environment, key: self.key_for(Action::ReconnectCurrent, Ctx::Nav) },
                Level::Error,
            ));
        }
        let cx = self.read_only_context(tab, pid);
        let mut classifier = self.classifier(tab);
        statements.iter().find_map(|sql| {
            let risk = classifier.classify(sql);
            let why = risk.read_only_in(&cx).err()?;
            let mut what = self.i18n.msg(&blocked_msg(&why)).to_string();
            // A call the context could let run: what would.
            if let (ReadOnlyBlock::UnknownFunction(_), Some(name)) = (&why, risk.refused_call_name(&cx)) {
                let hint = hint_msg(&cx.hint(name), name, self.environment_name(pid));
                let hint = self.i18n.msg(&hint).to_string();
                what = self.i18n.msg(&Msg::SafetyBlockedWithHint { what, hint }).to_string();
            }
            let sql = super::runlog::excerpt(sql, 60);
            Some(Notice::new(Msg::SafetyReadOnlyBlocked { environment: environment.clone(), what, sql }, Level::Error))
        })
    }
}

impl App {
    /// What a read-only policy lets run in tab `tab` of profile `pid` beyond PostgreSQL 17's
    /// built-ins: what the environment allows (`read_only_functions`, `read_only_extensions`),
    /// and PostgreSQL 18's built-ins called in `pg_catalog`, which need no server version.
    /// Nothing more on a profile that is not PostgreSQL's.
    pub(super) fn read_only_context(&self, tab: TabId, pid: ProfileId) -> ReadOnlyContext {
        let applies = self.profile(pid).is_some_and(|p| p.expected().read_only_calls() == Fact::Known(()));
        if !applies {
            return ReadOnlyContext::default();
        }
        let rules = self.rules_of(pid);
        ReadOnlyContext {
            extensions: self.extension_lookup(tab, pid, &rules.read_only_extensions),
            allowed: rules.read_only_functions,
            wanted_extensions: rules.read_only_extensions,
        }
    }

    /// What the catalog of tab `tab`'s database says of the extensions `wanted` that profile
    /// `pid`'s environment allows on read-only, as the current connection's metadata session
    /// read them ([`App::ask_extension_functions`]): pending until it answers for this list.
    /// The answer holds only for the database the metadata session said it read it in: failed
    /// for a tab working in another one, and when the profile's settings name another (changed
    /// since it connected, not reconnected).
    fn extension_lookup(&self, tab: TabId, pid: ProfileId, wanted: &[String]) -> ExtensionLookup {
        if wanted.is_empty() {
            return ExtensionLookup::None;
        }
        let Some(t) = self.tabs.get(tab) else { return ExtensionLookup::Failed };
        if self.other_database(t).is_some() {
            return ExtensionLookup::Failed;
        }
        match self.conns.get(pid).and_then(|c| c.extension_functions.as_ref()) {
            Some(r) if r.extensions == wanted => match &r.state {
                ExtensionState::Pending => ExtensionLookup::Pending,
                ExtensionState::Failed => ExtensionLookup::Failed,
                ExtensionState::Read { database, functions } => {
                    // The profile's settings (none: the server's default, the one it read in).
                    let configured = self.profile(pid).map(|p| p.endpoint().1).unwrap_or_default();
                    let own = if configured.is_empty() { database.as_str() } else { configured.as_str() };
                    let tab_db = t.context.database.as_deref().unwrap_or(own);
                    match own == database && tab_db == database {
                        true => ExtensionLookup::Read(functions.clone()),
                        false => ExtensionLookup::Failed,
                    }
                }
            },
            _ => ExtensionLookup::Pending,
        }
    }

    /// Ask profile `pid`'s metadata session for the functions of the extensions its
    /// environment allows on read-only (`read_only_extensions`), in the database it works in:
    /// on PostgreSQL, once connected, when the environment is read-only and lists any, unless
    /// this connection asked for that list already (a failed read is asked again on an
    /// explorer refresh, [`App::retry_extension_functions`], or a reconnect). A metadata
    /// session lost before it answered for this list reopens, to ask again once connected
    /// (the run that asks is still refused as pending).
    pub(super) fn ask_extension_functions(&mut self, pid: ProfileId) {
        let applies = self.profile(pid).is_some_and(|p| p.expected().read_only_calls() == Fact::Known(()));
        let rules = self.rules_of(pid);
        if !applies || !rules.read_only || rules.read_only_extensions.is_empty() {
            return;
        }
        let Some(c) = self.conns.get(pid) else { return };
        let extensions = rules.read_only_extensions;
        let answered = c
            .extension_functions
            .as_ref()
            .is_some_and(|r| r.extensions == extensions && !matches!(r.state, ExtensionState::Pending));
        if c.meta.is_none() && c.connecting.is_none() && c.idle_closed.is_none() && !answered {
            return self.reopen_meta(pid);
        }
        if c.meta.is_none() || !c.connected {
            return;
        }
        if c.extension_functions.as_ref().is_some_and(|r| r.extensions == extensions) {
            return;
        }
        let state = ExtensionState::Pending;
        self.conns.entry(pid).extension_functions = Some(ExtensionRead { extensions: extensions.clone(), state });
        self.send_meta(pid, DbCommand::LoadExtensionFunctions { extensions });
    }

    /// A run in tab `tab` was refused while the allowed extensions' functions are pending, and
    /// profile `pid` is not connected (not yet, or closed for being idle): it connects as a run
    /// would, and asks for them once connected.
    pub(super) fn connect_for_extensions(&mut self, tab: TabId, pid: ProfileId) {
        if !matches!(self.read_only_context(tab, pid).extensions, ExtensionLookup::Pending) {
            return;
        }
        let closed = self.conns.get(pid).is_none_or(|c| {
            c.meta.is_none() && c.connecting.is_none() && (c.resolved.is_none() || c.idle_closed.is_some())
        });
        if closed {
            self.connect(pid);
        }
    }

    /// The metadata session of profile `pid` answered the read of `extensions`' functions: kept
    /// with the database it said it read them in, when it is the one asked and the environment
    /// still lists those extensions; otherwise dropped, and the current list is asked for.
    pub(super) fn on_extension_functions(
        &mut self,
        pid: ProfileId,
        extensions: Vec<String>,
        result: Result<(String, datarig_core::sql::risk::ExtensionFunctions), datarig_core::driver::DbError>,
    ) {
        let current = self.rules_of(pid).read_only_extensions;
        let Some(c) = self.conns.get_mut(pid) else { return };
        let Some(r) = c.extension_functions.as_mut() else { return };
        if r.extensions != extensions || !matches!(r.state, ExtensionState::Pending) {
            return;
        }
        if extensions != current {
            // The config changed since it was asked.
            c.extension_functions = None;
            return self.ask_extension_functions(pid);
        }
        r.state = match result {
            Ok((database, functions)) => ExtensionState::Read { database, functions },
            Err(_) => ExtensionState::Failed,
        };
    }

    /// The explorer refreshed profile `pid`'s own database: a failed read of the allowed
    /// extensions' functions is asked again.
    pub(super) fn retry_extension_functions(&mut self, pid: ProfileId) {
        let c = self.conns.entry(pid);
        if c.extension_functions.as_ref().is_some_and(|r| matches!(r.state, ExtensionState::Failed)) {
            c.extension_functions = None;
        }
        self.ask_extension_functions(pid);
    }

    /// Why `statements` cannot run at all in tab `tab`, whatever the environment: `COPY … FROM
    /// STDIN` and `COPY … TO STDOUT` need the COPY protocol, which the app does not carry yet
    /// (sent, the first broke the tab's connection).
    pub(super) fn unsupported(&self, tab: TabId, statements: &[String]) -> Option<Notice> {
        let classifier = self.classifier(tab).fresh();
        let sql = statements.iter().find(|sql| classifier.clone().classify(sql).stdio)?;
        let sql = super::runlog::excerpt(sql, 60);
        Some(Notice::new(Msg::SafetyCopyStdio { sql }, Level::Error))
    }

    /// MariaDB's own executable comments are not read: statements the user wrote (a run's: the
    /// editor's, a table tab's filter text) holding the bytes `/*M!`
    /// ([`datarig_core::sql::lexer::mysql::has_m_bang`]) are refused where MariaDB may run them,
    /// under the tab's rules as they are now (a run that waited for its connection too). Values the
    /// app writes itself never hold them ([`datarig_core::query::render`] writes such a value as
    /// its bytes), and text the app writes around a result that ran already is not checked.
    pub(super) fn m_bang_refusal(&self, tab: TabId, statements: &[String]) -> Option<Notice> {
        if !statements.iter().any(|s| datarig_core::sql::lexer::mysql::has_m_bang(s)) {
            return None;
        }
        self.classifier(tab).refuses_m_bang().then(|| Notice::new(Label::EngineCommentUnsupported, Level::Error))
    }

    /// The classifier of tab `tab`, with the statements its session has prepared (none without
    /// a session): each run's statements are checked in order against a copy, so an `EXPLAIN`
    /// or `EXECUTE` sees what the run prepared before it.
    pub(super) fn classifier(&self, tab: TabId) -> Classifier {
        // A tab whose language was not pushed when its binding changed would be checked by the
        // rules of another language.
        debug_assert!(self.tabs.get(tab).is_none_or(|t| {
            let lang = self.tab_language(tab);
            t.exec.prepared.language() == lang && t.editor.language() == lang
        }));
        // Without a session, the tab's rules as they are now (the profile's server as its open
        // metadata session found it, else before any server said): never what a closed
        // session found.
        match self.tabs.get(tab) {
            Some(t) if t.exec.session.is_some() => t.exec.prepared.clone(),
            Some(_) | None => self.rules_classifier(tab),
        }
    }

    /// The statements of `statements` that ask before they run in tab `tab` under profile
    /// `pid`'s environment.
    pub(super) fn dangerous(&self, tab: TabId, pid: ProfileId, statements: &[String]) -> Vec<Dangerous> {
        let writes = self.rules_of(pid).confirm.writes();
        let mut classifier = self.classifier(tab);
        statements
            .iter()
            .enumerate()
            .filter_map(|(index, sql)| {
                let r = classifier.classify(sql);
                let why = r.confirm(writes)?;
                Some(Dangerous {
                    index,
                    why: Some(why),
                    tx: None,
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
    pub(super) fn ask_before_run(
        &mut self,
        id: TabId,
        pid: ProfileId,
        statements: Vec<String>,
        items: Vec<Dangerous>,
        read_in: Dialect,
    ) {
        self.ask_before_run_as(id, pid, statements, items, false, read_in)
    }

    /// [`App::ask_before_run`]; `as_is`: the statements are to run as they are, outside any
    /// transaction, on the tab's session as it is now.
    pub(super) fn ask_before_run_as(
        &mut self,
        id: TabId,
        pid: ProfileId,
        statements: Vec<String>,
        items: Vec<Dangerous>,
        as_is: bool,
        read_in: Dialect,
    ) {
        let Some((binding, generation)) = self.tabs.get(id).map(|t| (t.binding, t.exec.generation)) else { return };
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.key_state.clear();
        let approved = items.clone();
        self.overlays.push(Overlay::RunConfirm(RunConfirm {
            tab: id,
            profile: pid,
            binding,
            statements,
            read_in,
            items,
            run_focused: false,
            as_is,
            generation,
            approved,
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
        if self.overlays.run_confirm().is_none() {
            return;
        }
        self.run_confirm_answer(run);
        // The runs that waited behind this question, one at a time: of every connected profile
        // (one may have connected while it was open).
        let waiting: Vec<ProfileId> =
            self.conns.iter().filter(|(_, c)| c.connected && !c.pending.is_empty()).map(|(id, _)| id).collect();
        for id in waiting {
            self.send_waiting(id);
        }
    }

    /// [`App::run_confirm_answered`] for the question itself.
    fn run_confirm_answer(&mut self, run: bool) {
        let Some(c) = self.overlays.take_run_confirm() else { return };
        if !run {
            self.unstage_run(c.tab);
            let label = if c.as_is { Label::TxOutsideKept } else { Label::SafetyConfirmCancelled };
            return self.tab_status(c.tab, Notice::new(label, Level::Warning));
        }
        // Outside a transaction: only on the session it was asked for, still open (a new one
        // has no part in what failed).
        let same_session = |t: &Tab| !c.as_is || (t.exec.generation == c.generation && t.exec.session.is_some());
        match self.tabs.get(c.tab).map(|t| (t.profile, t.binding, same_session(t))) {
            Some((Some(p), b, true)) if p == c.profile && b == c.binding => {
                self.run_approved_as(c.tab, c.statements, c.as_is, c.approved, c.read_in)
            }
            Some(_) => {
                self.unstage_run(c.tab);
                self.tab_status(c.tab, Notice::new(Label::SafetyConfirmMoved, Level::Warning))
            }
            None => {}
        }
    }
}

/// `items` (the statements of `statements` that ask for their risk, in order) with what the
/// run does to the open transaction that asks (`reasons`, by statement): one item per
/// statement, in order.
pub(super) fn with_tx_reasons(
    mut items: Vec<Dangerous>,
    statements: &[String],
    reasons: Vec<(usize, super::tx::TxReason)>,
) -> Vec<Dangerous> {
    for (index, reason) in reasons {
        match items.iter_mut().find(|d| d.index == index) {
            Some(d) => d.tx = Some(reason),
            None => items.push(Dangerous {
                index,
                why: None,
                tx: Some(reason),
                no_where: None,
                explain: false,
                target: None,
                sql: statements.get(index).cloned().unwrap_or_default(),
            }),
        }
    }
    items.sort_by_key(|d| d.index);
    items
}

/// Why a statement asks, in words: what it does, and what makes it worse, then what it does to
/// the open transaction.
pub(crate) fn why_text(i18n: &I18n, d: &Dangerous) -> String {
    let mut parts = Vec::new();
    if let Some(why) = d.why {
        parts.push(risk_text(i18n, d, why));
    }
    match d.tx {
        Some(super::tx::TxReason::OutsideTx) => parts.push(i18n.label(Label::SafetyWhyOutsideTx).to_string()),
        Some(super::tx::TxReason::CommitsItself) => parts.push(i18n.label(Label::SafetyWhyCommitsItself).to_string()),
        Some(super::tx::TxReason::ImplicitCommit { statements, rows }) => {
            let rows = i18n.msg(&Msg::TxRows { count: rows }).to_string();
            parts.push(i18n.msg(&Msg::SafetyWhyImplicitCommit { count: statements, rows }).to_string());
        }
        None => {}
    }
    parts.join(" · ")
}

/// Why a statement's risk `why` asks, in words: what it does, and what makes it worse.
fn risk_text(i18n: &I18n, d: &Dangerous, why: Why) -> String {
    let what = match why {
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
        Why::Danger(Danger::AnalyzeRunsCode) => Label::SafetyWhyAnalyzeRunsCode,
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
    if let Why::Danger(Danger::DeleteAll | Danger::UpdateAll) = why {
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

/// What a refusal of a call by `name` adds ([`ReadOnlyContext::hint`]), for environment
/// `environment` (`None`: the default rules).
fn hint_msg(hint: &CallHint, name: &[String], environment: Option<String>) -> Msg {
    match (hint, environment) {
        (CallHint::NewerBuiltin, _) => Msg::SafetyHintNewerBuiltin { function: name.join(".") },
        (CallHint::NotAllowed, Some(environment)) => Msg::SafetyHintNotAllowed { environment },
        (CallHint::NotAllowed, None) => Msg::Label(Label::SafetyHintNotAllowedDefault),
        (CallHint::ExtensionsPending, _) => Msg::Label(Label::SafetyHintExtensionsPending),
        (CallHint::ExtensionsFailed, _) => Msg::Label(Label::SafetyHintExtensionsFailed),
        (CallHint::NotInstalled(list), _) => {
            Msg::SafetyHintNotInstalled { count: list.len() as u64, list: list.join(", ") }
        }
        (CallHint::SharedName, _) => Msg::Label(Label::SafetyHintSharedName),
    }
}

/// What a read-only environment says about a statement it refuses.
pub fn blocked_msg(why: &ReadOnlyBlock) -> Msg {
    let label = match why {
        ReadOnlyBlock::ReadWrite => Label::SafetyBlockedReadWrite,
        ReadOnlyBlock::Setting => Label::SafetyBlockedSetting,
        ReadOnlyBlock::Unparsed => Label::SafetyBlockedUnparsed,
        ReadOnlyBlock::TooComplex => Label::SafetyBlockedTooComplex,
        ReadOnlyBlock::ServerFile => Label::SafetyBlockedServerFile,
        ReadOnlyBlock::ServerAction => Label::SafetyBlockedServerAction,
        ReadOnlyBlock::RunsQueryText => Label::SafetyBlockedRunsQueryText,
        ReadOnlyBlock::ExecutableComment => Label::SafetyBlockedExecComment,
        ReadOnlyBlock::Unrecognized => Label::SafetyBlockedUnrecognized,
        ReadOnlyBlock::UnknownFunction(function) => {
            return Msg::SafetyBlockedUnknownFunction { function: function.clone() };
        }
        ReadOnlyBlock::ResetAll => Label::SafetyBlockedResetAll,
        ReadOnlyBlock::CustomSetting(setting) => {
            return Msg::SafetyBlockedCustomSetting { setting: setting.clone() };
        }
        ReadOnlyBlock::DynamicSql => Label::SafetyBlockedDynamicSql,
        ReadOnlyBlock::ServerCommand => Label::SafetyBlockedServerCommand,
        ReadOnlyBlock::Locks => Label::SafetyBlockedLocks,
        ReadOnlyBlock::FileAccess => Label::SafetyBlockedFileAccess,
        ReadOnlyBlock::Class(Class::Write) => Label::SafetyBlockedWrite,
        ReadOnlyBlock::Class(Class::Ddl) => Label::SafetyBlockedDdl,
        ReadOnlyBlock::Class(Class::Maintenance) => Label::SafetyBlockedMaintenance,
        ReadOnlyBlock::Class(Class::Procedural) => Label::SafetyBlockedProcedural,
        ReadOnlyBlock::Class(_) => Label::SafetyBlockedUnknown,
    };
    Msg::Label(label)
}
