//! Session presets in the app: what a profile's query sessions start with
//! ([`App::session_plan_of`], checked before a profile connects and again before each query
//! session opens), how a failure of it is said, the explorer's "Session presets" section (each
//! preset with its database, how many statements and profiles, the first statement that is not
//! allowed under it, and, opened, the profiles that use it), the preset form (new, edit, copy,
//! rename), deleting one (asked first, its profiles and environments listed) and testing one.
//!
//! A profile whose plan has an error (a preset that is not there, one for the other database, a
//! statement that is not a setting or that its read-only environment refuses) does not connect:
//! nothing is asked or sent, never without the preset instead.
//!
//! The config is only written by an explicit action: saving a form, a rename (the profiles and
//! environments that name the preset follow it), a delete. A deleted preset's name stays where it
//! was named; those profiles then do not connect until another preset (or none) is picked.
//!
//! A test opens one throwaway query session on a connected profile that uses the preset (or any
//! of its database), as a tab's would open with this preset, and closes it once it connected or
//! failed: nothing else is sent, and its settings end with it.

use super::profiles::{FormKind, ProfileForm};
use super::*;
use datarig_core::driver::SessionStatement;
use datarig_core::profile::session::{
    self, Dbms, Defaults, PickedBy, SessionError, SessionPlan, SessionPreset, SessionPresetId, SessionRef, Source,
};
use datarig_core::sql::risk::{Danger, Owner, SetProblem};
use std::collections::HashMap;

/// What each preset's statements are: the first one that is not allowed (`index` from 1), and
/// whether one changes a setting the server's checks depend on (`Danger::Setting`: refused in
/// read-only environments). Made once each time the presets change, not at every frame.
#[derive(Clone, Debug, Default)]
pub struct Checks(HashMap<SessionPresetId, Check>);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Check {
    pub problem: Option<(usize, SetProblem)>,
    pub risky: bool,
}

impl Checks {
    pub fn of(presets: &[SessionPreset]) -> Self {
        let check = |p: &SessionPreset| {
            let sets: Vec<_> =
                p.sql.iter().map(|s| datarig_core::sql::risk::Classifier::session_set(p.dbms.language(), s)).collect();
            let problem = sets.iter().enumerate().find_map(|(i, r)| r.as_ref().err().map(|e| (i + 1, e.clone())));
            let risky = sets.iter().flatten().any(|s| s.risk.danger == Some(Danger::Setting));
            Check { problem, risky }
        };
        Checks(presets.iter().map(|p| (p.id, check(p))).collect())
    }

    pub fn get(&self, id: SessionPresetId) -> Option<&Check> {
        self.0.get(&id)
    }
}

/// A preset's test that runs: its session and what it tests.
pub struct SessionTest {
    pub seq: u64,
    pub session: Session,
    /// The preset's name and the profile it runs on.
    pub name: String,
    pub profile: String,
    /// The statements it starts with.
    pub statements: Vec<String>,
    /// Behind a pooler: the statements not sent (said before it connected).
    pub pooled: Option<Vec<usize>>,
}

/// Whether preset `id` (named `original` when the form opened) cannot be named `n`: another
/// preset has that name, or one a person would take for it (case aside). A name the file already
/// had, and that only differs in case from another's, may stay.
pub fn name_taken(presets: &[SessionPreset], id: SessionPresetId, original: Option<&str>, n: &str) -> bool {
    presets.iter().any(|p| p.id != id && (p.name == n || (session::same_name(&p.name, n) && original != Some(n))))
}

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

    /// Where an item of a plan comes from, in words.
    pub(super) fn session_source_text(&self, source: &Source) -> String {
        let m = match source {
            Source::Global => return self.i18n.label(Label::SessionSourceGlobal).to_string(),
            Source::DefaultRules => return self.i18n.label(Label::SessionSourceDefaultRules).to_string(),
            Source::Environment(e) => Msg::SessionSourceEnvironment { environment: e.clone() },
            Source::Preset { name, by: PickedBy::Profile } => Msg::SessionSourceProfilePreset { preset: name.clone() },
            Source::Preset { name, by: PickedBy::Environment(Some(e)) } => {
                Msg::SessionSourceEnvironmentPreset { preset: name.clone(), environment: e.clone() }
            }
            Source::Preset { name, by: PickedBy::Environment(None) } => {
                Msg::SessionSourceDefaultPreset { preset: name.clone() }
            }
            Source::Profile => return self.i18n.label(Label::SessionSourceProfile).to_string(),
        };
        self.i18n.msg(&m).to_string()
    }

    /// "statement `index` of `source`", in words.
    pub(super) fn statement_of(&self, source: &Source, index: usize) -> String {
        let m = Msg::SessionStatementOf { index: index.to_string(), source: self.session_source_text(source) };
        self.i18n.msg(&m).to_string()
    }

    /// Why a statement cannot be a start-up setting, in words.
    pub fn set_problem_text(&self, p: &SetProblem) -> String {
        let label = match p {
            SetProblem::NotOne => Label::SetNotOne,
            SetProblem::Unread => Label::SetUnread,
            SetProblem::NotSet => Label::SetNotSet,
            SetProblem::Local => Label::SetLocal,
            SetProblem::Writes => Label::SetWrites,
            SetProblem::Danger(_) => Label::SetDanger,
            SetProblem::ModeDependent => Label::SetModeDependent,
            SetProblem::TransactionOnly { name } => {
                return self.i18n.msg(&Msg::SetTransactionOnly { setting: name.clone() }).to_string();
            }
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

    /// What profile `cfg` says when its plan has error `e`.
    pub(super) fn session_error_notice(&self, cfg: &ConnectionConfig, e: &SessionError) -> Notice {
        let name = cfg.name.clone();
        let sql = |s: &str| super::runlog::excerpt(s, 60);
        let m = match e {
            SessionError::NotFound { preset, by: PickedBy::Profile } => {
                Msg::SessionNotFoundProfile { name, preset: preset.clone() }
            }
            SessionError::NotFound { preset, by: PickedBy::Environment(env) } => {
                let dbms = Dbms::of_driver(&cfg.driver).map_or_else(String::new, |d| d.display().to_string());
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
        // A preset of the same name but for case: the one meant, most likely (names are matched
        // exactly).
        let like = match e {
            SessionError::NotFound { preset, .. } => {
                self.session_presets.iter().find(|p| session::same_name(&p.name, preset)).map(|p| p.name.clone())
            }
            _ => None,
        };
        if let Some(like) = like {
            let text = self.i18n.msg(&m).to_string();
            return Notice::new(Msg::SessionDidYouMean { text, like }, Level::Error);
        }
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

impl App {
    /// `:session`: what the explorer's (else the active tab's) profile's query sessions start
    /// with ([`SessionView`]).
    ///
    /// [`SessionView`]: super::overlay::SessionView
    pub(super) fn session_view(&mut self) {
        let Some(cfg) = self.action_profile().and_then(|p| self.profile(p)).cloned() else {
            return self.flash(Notice::new(Label::SessionViewNoProfile, Level::Info));
        };
        self.open_session_view(&cfg, false, None);
    }

    /// The profile form's values, saved or not: what its sessions would start with.
    pub(super) fn form_session_view(&mut self) {
        let Some(f) = self.overlays.form().filter(|f| f.kind == FormKind::Profile) else { return };
        let cfg = f.to_test_profile();
        // A value of the plan that differs from the saved profile's (or a new profile): unsaved.
        let saved = f.editing.and_then(|i| self.profiles.get(i));
        let unsaved = saved.is_none_or(|p| {
            (&p.name, Dbms::of_driver(&p.driver), &p.policy, &p.session, &p.session_sql)
                != (&cfg.name, Dbms::of_driver(&cfg.driver), &cfg.policy, &cfg.session, &cfg.session_sql)
        });
        // The environment the form makes when it is saved has its rules there only.
        let made = f.made_environment().map(|e| e.policy.clone());
        self.open_session_view(&cfg, unsaved, made);
    }

    /// The session view of `cfg` (`unsaved`: a form's values the file does not have yet; `made`:
    /// the rules of the environment that form makes), made now: it does not follow later
    /// changes. An environment that is not defined says what the connect refuses with.
    fn open_session_view(&mut self, cfg: &ConnectionConfig, unsaved: bool, made: Option<datarig_core::policy::Policy>) {
        use super::overlay::{SessionLine, Tone};
        use datarig_core::profile::session::{Intent, Planned};
        let label = |l: Label| self.i18n.label(l).to_string();
        let env = cfg.policy.as_deref().filter(|n| *n != datarig_core::policy::DEFAULT);
        let rules = self.policies.get(cfg.policy.as_deref()).or_else(|e| made.ok_or(e));
        let title = match (unsaved, cfg.name.is_empty()) {
            (false, _) => self.i18n.msg(&Msg::SessionViewTitle { name: cfg.name.clone() }),
            (true, false) => self.i18n.msg(&Msg::SessionViewTitleUnsaved { name: cfg.name.clone() }),
            (true, true) => self.i18n.label(Label::SessionViewTitleNew),
        };
        // The profile, its database, its environment.
        let dbms = Dbms::of_driver(&cfg.driver).map_or(cfg.driver.clone(), |d| d.display().to_string());
        let place = self.session_source_text(&match env {
            Some(e) => Source::Environment(e.to_string()),
            None => Source::DefaultRules,
        });
        let who = [cfg.name.as_str(), &dbms, &place].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>();
        let mut head = vec![SessionLine::Text(who.join(" · "), Tone::Strong)];
        if rules.as_ref().is_ok_and(|r| r.read_only) {
            head.push(SessionLine::Text(label(Label::SessionViewReadOnly), Tone::Warning));
        }
        let mut lines = Vec::new();
        let defaults = Defaults { lock_wait: self.prefs.lock_wait, idle_disconnect: self.prefs.idle_disconnect };
        let plan = rules.map(|r| session::plan(cfg, &r, defaults, &self.session_presets));
        let foot = match plan {
            Err(e) => {
                let text = self.unknown_environment_notice(&cfg.name, &e).render(&self.i18n).to_string();
                lines.push(SessionLine::Text(text, Tone::Error));
                SessionLine::Text(label(Label::SessionViewNoConnect), Tone::Warning)
            }
            Ok(Err(e)) => {
                let text = self.session_error_notice(cfg, &e).render(&self.i18n).to_string();
                lines.push(SessionLine::Text(text, Tone::Error));
                SessionLine::Text(label(Label::SessionViewNoConnect), Tone::Warning)
            }
            Ok(Ok(plan)) => {
                let mut from: Option<&Source> = None;
                for i in &plan.items {
                    // A blank line after the app's limits.
                    if !matches!(i.what, Planned::Intent(_)) && matches!(lines.last(), Some(SessionLine::Intent { .. }))
                    {
                        lines.push(SessionLine::Text(String::new(), Tone::Muted));
                    }
                    match &i.what {
                        Planned::Intent(x) => {
                            let (what, d, off) = match *x {
                                Intent::LockWait(d) => (Label::FormFieldLockWait, d, Label::SessionViewServerSetting),
                                Intent::IdleDisconnect(d) => {
                                    (Label::FormFieldIdleDisconnect, d, Label::SessionViewNever)
                                }
                                Intent::TxIdleRollback(d) => {
                                    (Label::FormFieldTxIdleRollback, d, Label::SessionViewNever)
                                }
                            };
                            let value = d.map_or_else(|| label(off), |d| datarig_core::policy::format_timeout(Some(d)));
                            let source = self.session_source_text(&i.source);
                            lines.push(SessionLine::Intent { what: label(what), value, source });
                        }
                        Planned::Sql(sql) => {
                            if from != Some(&i.source) {
                                let source = self.session_source_text(&i.source);
                                lines.push(SessionLine::Source(
                                    self.i18n.msg(&Msg::SessionViewFrom { source }).to_string(),
                                ));
                                from = Some(&i.source);
                            }
                            lines.push(SessionLine::Statement(sql.clone()));
                            if let Some(p) = &i.pooled {
                                lines.push(SessionLine::Pooled(p.clone()));
                            }
                        }
                        Planned::Skipped { preset } => {
                            let by = PickedBy::Environment(env.map(str::to_string));
                            let source = self.session_source_text(&Source::Preset { name: preset.clone(), by });
                            let text = self.i18n.msg(&Msg::SessionViewSkipped { source }).to_string();
                            lines.push(SessionLine::Text(text, Tone::Muted));
                        }
                    }
                }
                if plan.statements().is_empty() {
                    if matches!(lines.last(), Some(SessionLine::Intent { .. })) {
                        lines.push(SessionLine::Text(String::new(), Tone::Muted));
                    }
                    lines.push(SessionLine::Text(label(Label::SessionViewNoStatements), Tone::Muted));
                }
                SessionLine::Text(label(Label::SessionViewOrder), Tone::Muted)
            }
        };
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.key_state.clear();
        let v = super::overlay::SessionView {
            title,
            head,
            lines,
            foot,
            scroll: 0,
            max_scroll: 0,
            buttons: Default::default(),
        };
        self.overlays.push(Overlay::SessionView(v));
    }

    /// Keys of the session view: `Esc`, `q` and `Enter` close it, `j`/`k` and the arrows
    /// scroll. Held keys only scroll.
    pub(super) fn session_view_key(&mut self, key: KeyEvent, repeat: bool) {
        let Some(s) = self.overlays.session_view_mut() else { return };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => s.scroll = (s.scroll + 1).min(s.max_scroll),
            KeyCode::Char('k') | KeyCode::Up => s.scroll = s.scroll.saturating_sub(1),
            _ if repeat => {}
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.overlays.close(OverlayKind::SessionView),
            _ => {}
        }
    }
}

impl App {
    /// The explorer shows the "Session presets" section: there are presets, or profiles and a
    /// config file the app can write (one can be made).
    pub fn sessions_shown(&self) -> bool {
        !self.session_presets.is_empty() || (!self.profiles.is_empty() && self.config_writable())
    }

    /// Session preset `id`.
    pub fn session_preset(&self, id: SessionPresetId) -> Option<&SessionPreset> {
        self.session_presets.iter().find(|p| p.id == id)
    }

    /// What preset `id`'s statements are ([`Checks`]).
    pub fn session_check(&self, id: SessionPresetId) -> Check {
        self.session_checks.get(id).cloned().unwrap_or_default()
    }

    /// The preset the explorer's cursor is on (its row or its problem's line). A profile listed
    /// under one is only a way to that profile (`Enter`).
    pub fn selected_session(&self) -> Option<SessionPresetId> {
        match self.explorer_row()?.kind {
            explorer::RowKind::Session(id) | explorer::RowKind::SessionError(id) => Some(id),
            _ => None,
        }
    }

    /// The cursor is in the "Session presets" section.
    pub fn in_sessions_section(&self) -> bool {
        matches!(
            self.explorer_row().map(|r| r.kind),
            Some(
                explorer::RowKind::SessionsHeader
                    | explorer::RowKind::Session(_)
                    | explorer::RowKind::SessionError(_)
                    | explorer::RowKind::SessionUser(..)
                    | explorer::RowKind::SessionsEmpty
            )
        )
    }

    /// The preset profile `c` starts its sessions with by name: its own, else the one its
    /// environment maps for its database (`None`: none, or none it can name).
    pub fn preset_of_profile(&self, c: &ConnectionConfig) -> Option<String> {
        match &c.session {
            SessionRef::Named(n) => Some(n.clone()),
            SessionRef::Off => None,
            SessionRef::Inherit => {
                let dbms = Dbms::of_driver(&c.driver)?;
                self.policies.get(c.policy.as_deref()).ok()?.session.get(dbms).map(str::to_string)
            }
        }
    }

    /// The profiles that start their sessions with preset `name` (their own or their
    /// environment's).
    pub fn session_users(&self, name: &str) -> Vec<&ConnectionConfig> {
        self.profiles.iter().filter(|c| self.preset_of_profile(c).as_deref() == Some(name)).collect()
    }

    /// The environments that map preset `name` for a database (the default rules as `default`).
    pub fn session_environments(&self, name: &str) -> Vec<String> {
        let maps = |m: &datarig_core::policy::SessionMap| m.iter().any(|(_, n)| n == name);
        let mut out: Vec<String> = Vec::new();
        if maps(&self.policies.default_rules().session) {
            out.push(datarig_core::policy::DEFAULT.to_string());
        }
        out.extend(self.environments().iter().filter(|e| maps(&e.policy.session)).map(|e| e.name.clone()));
        out
    }

    /// The "Session presets" section: the head, then the presets by name (with a filter, the
    /// ones whose name or statements match), each with the first statement that is not allowed
    /// and, opened, the profiles that use it.
    pub(super) fn push_sessions(&self, rows: &mut Vec<explorer::Row>) {
        use explorer::{Row, RowKind};
        if !self.sessions_shown() {
            return;
        }
        let q = self.explorer.filter.text().trim().to_lowercase();
        let filtering = !q.is_empty();
        let matches =
            |p: &SessionPreset| std::iter::once(&p.name).chain(p.sql.iter()).any(|t| t.to_lowercase().contains(&q));
        if filtering && !self.session_presets.iter().any(matches) {
            return;
        }
        rows.push(Row::new(RowKind::SessionsHeader, 0));
        if !self.sessions_expanded && !filtering {
            return;
        }
        if self.session_presets.is_empty() {
            rows.push(Row::new(RowKind::SessionsEmpty, 1));
            return;
        }
        for p in self.session_presets.iter().filter(|p| !filtering || matches(p)) {
            rows.push(Row { hit: filtering, ..Row::new(RowKind::Session(p.id), 1) });
            if self.session_check(p.id).problem.is_some() {
                rows.push(Row::new(RowKind::SessionError(p.id), 2));
            }
            if self.sessions_open.contains(&p.id) {
                for c in self.session_users(&p.name) {
                    rows.push(Row::new(RowKind::SessionUser(p.id, c.id), 2));
                }
            }
        }
    }

    /// Put the explorer's cursor on preset `id`, opening the section.
    pub(super) fn reveal_session(&mut self, id: SessionPresetId) {
        self.sessions_expanded = true;
        self.explorer.select_kind(explorer::RowKind::Session(id));
        let rows = self.explorer_rows();
        let i = self.explorer.index(&rows);
        self.explorer.select(&rows, i);
    }

    // ── the form ────────────────────────────────────────────────────────────

    /// Open the session preset form: a new preset, preset `id` to edit, or a copy of it.
    pub(super) fn open_session_form(&mut self, id: Option<SessionPresetId>, duplicate: bool) {
        if self.form_open_already() {
            return;
        }
        let p = id.and_then(|id| self.session_preset(id)).cloned();
        let taken = |n: &str| self.session_presets.iter().any(|q| session::same_name(&q.name, n));
        let copy = p.as_ref().filter(|_| duplicate).map(|p| profiles::copy_name(&p.name, taken));
        let f = ProfileForm::for_session(p.as_ref(), copy);
        self.clear_test();
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.overlays.push(Overlay::ProfileForm(Box::new(f)));
    }

    /// `Ctrl+S` in the session preset form: save the preset (a rename takes the profiles and
    /// environments that name it along). A field that is not valid gets the focus; nothing
    /// changes when the file cannot be written (the form stays open).
    pub(super) fn save_session_form(&mut self) {
        let Some(form) = self.overlays.form_mut().filter(|f| !f.saving && f.is_session()) else { return };
        form.attempted = true;
        let FormKind::Session { id, editing } = form.kind else { return };
        let (presets, original) = (&self.session_presets, form.original_name.clone());
        let taken = |n: &str| name_taken(presets, id, original.as_deref(), n);
        if let Some((field, _)) = form.errors(taken).into_iter().next() {
            form.focus_field(field);
            return;
        }
        let Some(mut p) = form.to_session_preset() else { return };
        let old = self.session_presets.iter().position(|q| q.id == id).filter(|_| editing);
        let before = old.map(|i| self.session_presets[i].clone());
        let renamed = before.as_ref().map(|b| b.name.clone()).filter(|n| *n != p.name);
        p.origin = before.as_ref().and_then(|b| b.origin.clone());
        let undo = (self.session_presets.clone(), self.profiles.clone(), self.policies.clone());
        match old {
            Some(i) => self.session_presets[i] = p.clone(),
            None => {
                self.session_presets.push(p.clone());
                self.session_presets.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            }
        }
        let moved = match &renamed {
            Some(from) => self.rename_session_preset(from, &p.name),
            None => Vec::new(),
        };
        if let Err(fault) = self.try_persist() {
            for id in moved {
                if let Some(r) = self.conns.get_mut(id).and_then(|c| c.resolved.as_mut()) {
                    r.session = SessionRef::Named(renamed.clone().unwrap_or_default());
                }
            }
            (self.session_presets, self.profiles, self.policies) = undo;
            let error = self.fault_text("config.save_failed", &fault);
            return self.flash(Notice::new(Msg::ConfigSaveFailed { error }, Level::Error));
        }
        self.session_checks = Checks::of(&self.session_presets);
        self.clear_test();
        // Back in the form that asked for it, the explorer stays where it was.
        if !self.close_form(Some(p.name.clone())) {
            self.reveal_session(p.id);
        }
        self.flash(Notice::new(Msg::SessionSaved { name: p.name }, Level::Success));
    }

    /// The profiles naming preset `from` name `to` instead, and so do the environments that map
    /// it and the copies the profiles' connections were opened with; the profiles whose copy
    /// changed.
    fn rename_session_preset(&mut self, from: &str, to: &str) -> Vec<ProfileId> {
        let named = |r: &SessionRef| matches!(r, SessionRef::Named(n) if n == from);
        for c in self.profiles.iter_mut().filter(|c| named(&c.session)) {
            c.session = SessionRef::Named(to.to_string());
        }
        let follow = |m: &mut datarig_core::policy::SessionMap| {
            for d in Dbms::ALL {
                if m.get(d) == Some(from) {
                    m.set(d, Some(to.to_string()));
                }
            }
        };
        let mut rules = self.policies.default_rules().clone();
        follow(&mut rules.session);
        self.policies.insert(datarig_core::policy::DEFAULT, rules);
        for e in self.policies.environments_mut() {
            follow(&mut e.policy.session);
        }
        // A form put aside under this one (it opened from that form's list) that names it.
        if let Some(back) = self.overlays.form_mut().and_then(|f| f.back_to.as_mut()) {
            if named(&back.session_ref) {
                back.session_ref = SessionRef::Named(to.to_string());
            }
            follow(&mut back.env_session);
        }
        let mut moved = Vec::new();
        for (id, c) in self.conns.iter_mut() {
            if let Some(r) = c.resolved.as_mut().filter(|r| named(&r.session)) {
                r.session = SessionRef::Named(to.to_string());
                moved.push(id);
            }
        }
        moved
    }

    // ── delete ──────────────────────────────────────────────────────────────

    /// `d` on a preset: ask first, listing the profiles and environments that use it.
    pub(super) fn request_delete_session(&mut self, id: SessionPresetId) {
        let Some(p) = self.session_preset(id).cloned() else { return };
        let mut details = Vec::new();
        let users: Vec<String> = self.session_users(&p.name).iter().map(|c| c.name.clone()).collect();
        if !users.is_empty() {
            details.push(Msg::SessionDeleteUsers { count: users.len() as u64, names: users.join(", ") });
        }
        let envs = self.session_environments(&p.name);
        if !envs.is_empty() {
            details.push(Msg::SessionDeleteEnvironments { names: envs.join(", ") });
        }
        self.overlays.close(OverlayKind::Commands);
        self.overlays.push(Overlay::Confirm(Confirm {
            title: Label::ExplorerDeleteTitle,
            text: Msg::SessionDelete { name: p.name.clone() },
            details,
            keys: Label::ExplorerDeleteKeys,
            action: ConfirmAction::DeleteSession(id),
            folder: None,
            path: None,
            tx: None,
            buttons: Default::default(),
        }));
    }

    /// Delete preset `id` (confirmed). The profiles and environments that name it keep the name:
    /// those profiles do not connect until another preset (or none) is picked.
    pub(super) fn delete_session(&mut self, id: SessionPresetId) {
        let Some(i) = self.session_presets.iter().position(|p| p.id == id) else { return };
        let rows = self.explorer_rows();
        let at = rows.iter().position(|r| r.kind == explorer::RowKind::Session(id)).unwrap_or(0);
        let p = self.session_presets.remove(i);
        if let Err(fault) = self.try_persist() {
            self.session_presets.insert(i, p);
            let error = self.fault_text("config.save_failed", &fault);
            return self.flash(Notice::new(Msg::ConfigSaveFailed { error }, Level::Error));
        }
        self.session_checks = Checks::of(&self.session_presets);
        self.sessions_open.remove(&id);
        let rows = self.explorer_rows();
        self.explorer.select(&rows, at.saturating_sub(1));
        let count = self.session_users(&p.name).len() as u64;
        let msg = match count {
            0 => Notice::new(Msg::SessionDeleted { name: p.name }, Level::Success),
            _ => Notice::new(Msg::SessionDeletedUsed { name: p.name, count }, Level::Warning),
        };
        self.flash(msg);
    }

    // ── test ────────────────────────────────────────────────────────────────

    /// Test preset `id` as saved.
    pub(super) fn test_session(&mut self, id: SessionPresetId) {
        let Some(p) = self.session_preset(id).cloned() else { return };
        self.start_session_test(p);
    }

    /// "Test" in the session preset form: the form's preset (a statement that is not allowed is
    /// marked, and nothing is tried).
    pub(super) fn test_session_form(&mut self) {
        let Some(f) = self.overlays.form_mut().filter(|f| f.is_session()) else { return };
        if f.session_problem().is_some() {
            f.focus_field(profiles::Field::SessionSql);
            self.clear_test();
            return self.flash(Notice::new(Label::TestFieldsMissing, Level::Warning));
        }
        let Some(mut p) = f.to_session_preset() else { return };
        // A preset being renamed is still used under its old name.
        if let Some(old) = f.original_name.clone() {
            p.name = old;
        }
        self.start_session_test(p);
    }

    /// The profiles a test of `p` may run on: those that use it, else those of its database.
    fn session_test_candidates(&self, p: &SessionPreset) -> Vec<ProfileId> {
        let users: Vec<ProfileId> = self.session_users(&p.name).iter().map(|c| c.id).collect();
        if !users.is_empty() {
            return users;
        }
        self.profiles.iter().filter(|c| Dbms::of_driver(&c.driver) == Some(p.dbms)).map(|c| c.id).collect()
    }

    /// Test `p` on a connected profile that may use it: the one there is, else the user picks.
    fn start_session_test(&mut self, p: SessionPreset) {
        self.clear_test();
        let candidates = self.session_test_candidates(&p);
        if candidates.is_empty() {
            let dbms = p.dbms.display().to_string();
            return self.session_test_said(Msg::TestSessionNoProfile { dbms }, Level::Warning);
        }
        let connected: Vec<ProfileId> = candidates
            .iter()
            .copied()
            .filter(|id| self.conns.get(*id).is_some_and(|c| c.connected && c.resolved.is_some()))
            .collect();
        match connected.as_slice() {
            [] => {
                let names: Vec<String> =
                    candidates.iter().filter_map(|id| self.profile(*id)).map(|c| c.name.clone()).collect();
                let m = Msg::TestSessionNotConnected { names: names.join(", ") };
                self.session_test_said(m, Level::Warning);
            }
            [one] => self.run_session_test(p, *one),
            several => {
                let entries = several
                    .iter()
                    .filter_map(|id| self.profile(*id).map(|c| (id, c)))
                    .map(|(id, c)| {
                        let (addr, db, _) = c.endpoint();
                        crate::widgets::list_picker::Entry::new(*id, c.name.clone(), format!("{addr} {db}"))
                    })
                    .collect();
                let list = crate::widgets::list_picker::ListPicker::new(entries, None, None);
                self.overlays.push(Overlay::Picker(super::picker::Picker::SessionTest { preset: Box::new(p), list }));
            }
        }
    }

    /// A test that tries nothing says `m`: in the form's test line when a form is open, else in
    /// the status bar.
    fn session_test_said(&mut self, m: Msg, level: Level) {
        self.clear_test();
        self.test_seq += 1;
        let state = TestState::Applied(m, level);
        self.conn_test = Some(ConnTest {
            seq: self.test_seq,
            started: Instant::now(),
            state,
            abort: None,
            tunnel: None,
            probe: false,
        });
        self.report_test();
    }

    /// Test `p` on connected profile `pid`: a throwaway query session of that profile's connection
    /// that starts with `p` alone (the profile's own statements left out), as its tab's would,
    /// then closed.
    pub(super) fn run_session_test(&mut self, p: SessionPreset, pid: ProfileId) {
        self.clear_test();
        let Some(mut cfg) = self.conns.get(pid).and_then(|c| c.resolved.clone()) else { return };
        cfg.session = SessionRef::Named(p.name.clone());
        cfg.session_sql.clear();
        let mut presets: Vec<SessionPreset> = self.session_presets.iter().filter(|q| q.id != p.id).cloned().collect();
        presets.retain(|q| q.name != p.name);
        presets.push(p.clone());
        let rules = self.policies.get(cfg.policy.as_deref()).unwrap_or_else(|_| self.rules_of(pid));
        let defaults = Defaults { lock_wait: self.prefs.lock_wait, idle_disconnect: self.prefs.idle_disconnect };
        let statements: Vec<SessionStatement> = match session::plan(&cfg, &rules, defaults, &presets) {
            Ok(plan) => plan.statements(),
            Err(e) => {
                let text = self.session_error_notice(&cfg, &e).render(&self.i18n).to_string();
                return self.test_refused(text);
            }
        };
        self.test_seq += 1;
        let seq = self.test_seq;
        let sql = statements.iter().map(|s| s.sql.clone()).collect();
        let target = EventTarget::SessionTest(seq);
        let session = self.open_session_as(&cfg, SessionRole::Query, target, seq, Default::default(), statements);
        let state = match session {
            Some(_) => TestState::Running,
            None => {
                TestState::Failed(self.i18n.msg(&Msg::ConnUnknownDriver { driver: cfg.driver.clone() }).to_string())
            }
        };
        if let Some(session) = session {
            self.session_test = Some(SessionTest {
                seq,
                session,
                name: p.name,
                profile: cfg.name.clone(),
                statements: sql,
                pooled: None,
            });
        }
        self.conn_test =
            Some(ConnTest { seq, started: Instant::now(), state, abort: None, tunnel: None, probe: false });
        self.report_test();
    }

    /// An event of test `seq`'s session: said once it connected (its statements all ran) or
    /// failed, and the session closed then.
    pub(super) fn session_test_event(&mut self, seq: u64, ev: DbEvent) {
        let Some(t) = self.session_test.as_mut().filter(|t| t.seq == seq) else { return };
        let (m, level) = match ev {
            DbEvent::SessionPerTransaction { refused } => {
                t.pooled = Some(refused);
                return;
            }
            DbEvent::Connected => {
                let count = t.statements.len() as u64;
                match &t.pooled {
                    None => {
                        (Msg::TestSessionOk { name: t.name.clone(), profile: t.profile.clone(), count }, Level::Success)
                    }
                    Some(refused) if refused.is_empty() => {
                        (Msg::TestSessionPooled { name: t.name.clone(), profile: t.profile.clone() }, Level::Success)
                    }
                    Some(refused) => {
                        let sql: Vec<String> = refused
                            .iter()
                            .filter_map(|i| t.statements.get(*i))
                            .map(|s| super::runlog::excerpt(s, 40))
                            .collect();
                        let count = refused.len() as u64;
                        (
                            Msg::SessionPooledRefused { name: t.profile.clone(), count, sql: sql.join("; ") },
                            Level::Warning,
                        )
                    }
                }
            }
            DbEvent::ConnectFailed { error, .. } => {
                (Msg::TestSessionFailed { error: self.db_error_text(&error) }, Level::Error)
            }
            _ => return,
        };
        if let Some(t) = self.session_test.take() {
            t.session.close();
        }
        if let Some(c) = self.conn_test.as_mut().filter(|c| c.seq == seq) {
            c.state = TestState::Applied(m, level);
        }
        self.report_test();
    }
}
