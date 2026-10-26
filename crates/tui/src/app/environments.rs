//! Environments in the app: the explorer's "Environments" section (the default rules first, the
//! rules of every profile without an environment, editable but never renamed, copied or
//! deleted; then each environment with its color, badge and rules and, opened, the profiles that
//! name it), the environment form (new,
//! edit, copy, rename), deleting one (asked first, the profiles that name it listed), and the
//! presets (`dev`, `test`, `staging`, `prod`) offered while there is none. The profile form's
//! environment picker is in [`super::picker`].
//!
//! The config is only written by an explicit action: saving a form, adding the presets, a rename
//! (the profiles that name the environment follow it), a delete. A deleted environment's name
//! stays in the profiles that named it, which then do not connect (never under the default
//! rules instead) until another environment is picked.

use super::profiles::{FormKind, ProfileForm};
use super::*;
use datarig_core::policy::{self, Environment, EnvironmentId, Policy};
use datarig_core::profile::tunnel::same_name;

/// Whether environment `id` (named `original` when the form opened) cannot be named `n`:
/// another environment has that name, or one a person would take for it (case aside). A name
/// the file already had, and that only differs in case from another's, may stay.
pub fn name_taken(envs: &[Environment], id: EnvironmentId, original: Option<&str>, n: &str) -> bool {
    envs.iter().any(|e| e.id != id && (e.name == n || (same_name(&e.name, n) && original != Some(n))))
}

/// The rules of `p` in a few words: `read-only · asks before every write`, … (`standard rules`
/// when it has none of them).
pub fn rules_hint(i18n: &datarig_core::i18n::I18n, p: &Policy) -> String {
    rules_tones(i18n, p, false).into_iter().map(|(t, _)| t).collect()
}

/// How a part of an environment's rules in words is drawn ([`rules_tones`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleTone {
    /// Read-only: the read-only mark's color.
    ReadOnly,
    /// Asks before every write: the warning color.
    Warning,
    /// Holds locks while paging: the error color.
    Error,
    /// The standard rules, the separators, and words around them: dim.
    Dim,
}

impl RuleTone {
    /// The style of the tone in the current theme.
    pub fn style(self) -> ratatui::style::Style {
        let th = crate::theme::cur();
        ratatui::style::Style::new().fg(match self {
            RuleTone::ReadOnly => read_only_color(&th),
            RuleTone::Warning => th.warning,
            RuleTone::Error => th.error,
            RuleTone::Dim => th.fg_dim,
        })
    }
}

/// [`rules_hint`] in parts with the tone each is drawn in, the same wherever they are (the
/// explorer's rows, the environment list, the profile form): read-only (after the lock with
/// `icons` on), asking before every write, committing by hand (moot when read-only), holding
/// locks while paging, the standard rules and the separators.
pub fn rules_tones(i18n: &datarig_core::i18n::I18n, p: &Policy, icons: bool) -> Vec<(String, RuleTone)> {
    let mut rules: Vec<(String, RuleTone)> = Vec::new();
    if p.read_only {
        let lock = if icons { format!("{} ", crate::icons::READ_ONLY) } else { String::new() };
        rules.push((format!("{lock}{}", i18n.label(Label::EnvRuleReadOnly)), RuleTone::ReadOnly));
    }
    if p.confirm.writes() {
        rules.push((i18n.label(Label::EnvRuleConfirmWrites).to_string(), RuleTone::Warning));
    }
    // Read-only refuses every write before it is sent: no transaction ever starts.
    if p.commit.manual() && !p.read_only {
        rules.push((i18n.label(Label::EnvRuleManualCommit).to_string(), RuleTone::Warning));
    }
    if p.paging == datarig_core::driver::PagingMode::Hold {
        rules.push((i18n.label(Label::EnvRuleHold).to_string(), RuleTone::Error));
    }
    if rules.is_empty() {
        rules.push((i18n.label(Label::EnvRuleStandard).to_string(), RuleTone::Dim));
    }
    let mut out = Vec::new();
    for (i, r) in rules.into_iter().enumerate() {
        if i > 0 {
            out.push((" · ".to_string(), RuleTone::Dim));
        }
        out.push(r);
    }
    out
}

/// [`rules_tones`] in the current theme's styles.
pub fn rules_parts(i18n: &datarig_core::i18n::I18n, p: &Policy, icons: bool) -> Vec<(String, ratatui::style::Style)> {
    rules_tones(i18n, p, icons).into_iter().map(|(t, tone)| (t, tone.style())).collect()
}

/// The read-only badge of the status bar, the editor's connection line and the run
/// confirmation: `READ-ONLY`, after the lock with icons on.
pub fn read_only_badge(app: &App) -> String {
    let words = app.i18n.label(Label::StatusReadOnly);
    if app.icons_on() { format!("{} {words}", crate::icons::READ_ONLY) } else { words.to_string() }
}

/// The color of the read-only mark (the lock, the rule's words): the theme's read-only mark.
pub fn read_only_color(th: &crate::theme::Theme) -> ratatui::style::Color {
    th.read_only_mark.fg.unwrap_or(th.accent)
}

impl App {
    /// The environments, in the file's order (new ones last).
    pub fn environments(&self) -> &[Environment] {
        self.policies.environments()
    }

    /// The explorer shows the "Environments" section: there are environments, or profiles and a
    /// config file the app can write (one can be made).
    pub fn environments_shown(&self) -> bool {
        !self.environments().is_empty() || (!self.profiles.is_empty() && self.config_writable())
    }

    /// Environment `id`.
    pub fn environment(&self, id: EnvironmentId) -> Option<&Environment> {
        self.environments().iter().find(|e| e.id == id)
    }

    /// The environment profile `id` names, when there is one of that name.
    pub fn profile_environment(&self, id: ProfileId) -> Option<&Environment> {
        let name = self.profile(id)?.policy.as_deref()?;
        self.policies.environment(name)
    }

    /// The environment the explorer's cursor is on. A profile listed under one is only a way to
    /// that profile (`Enter`): no environment's action runs on it.
    pub fn selected_environment(&self) -> Option<EnvironmentId> {
        match self.explorer_row()?.kind {
            explorer::RowKind::Environment(id) => Some(id),
            _ => None,
        }
    }

    /// The cursor is in the "Environments" section (its head, an environment, a profile under
    /// one, or the presets' row).
    pub fn in_environments_section(&self) -> bool {
        matches!(
            self.explorer_row().map(|r| r.kind),
            Some(
                explorer::RowKind::EnvironmentsHeader
                    | explorer::RowKind::DefaultRules
                    | explorer::RowKind::Environment(_)
                    | explorer::RowKind::EnvironmentUser(..)
                    | explorer::RowKind::DefaultRulesUser(_)
                    | explorer::RowKind::EnvironmentPresets
            )
        )
    }

    /// The profiles without an environment (also those that name `default`): the default
    /// rules' profiles.
    pub fn default_rules_users(&self) -> Vec<&ConnectionConfig> {
        self.profiles.iter().filter(|c| c.policy.as_deref().is_none_or(|p| p == policy::DEFAULT)).collect()
    }

    /// The explorer's cursor is on the default rules' row.
    pub fn default_rules_selected(&self) -> bool {
        self.explorer_row().is_some_and(|r| r.kind == explorer::RowKind::DefaultRules)
    }

    /// Rename, copy or delete on the default rules' row: they are not an environment.
    pub(super) fn default_rules_fixed(&mut self) {
        self.flash(Notice::new(Label::EnvDefaultFixed, Level::Info));
    }

    /// Open the default rules' form.
    pub(super) fn open_default_rules_form(&mut self) {
        if self.form_open_already() {
            return;
        }
        let mut f = ProfileForm::for_default_rules(self.policies.default_rules());
        f.session_presets = self.session_presets.clone();
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.overlays.push(Overlay::ProfileForm(Box::new(f)));
    }

    /// `Ctrl+S` in the default rules' form: saved (`[policy.default]`, only what changed);
    /// nothing changes when the file cannot be written (the form stays open).
    fn save_default_rules_form(&mut self) {
        let Some(form) = self.overlays.form_mut().filter(|f| !f.saving && f.is_default_rules()) else { return };
        form.attempted = true;
        if !form.errors(|_| false).is_empty() {
            return;
        }
        let Some(rules) = form.to_rules() else { return };
        let before = self.policies.clone();
        self.policies.insert(policy::DEFAULT, rules);
        if let Err(fault) = self.try_persist() {
            self.policies = before;
            let error = self.fault_text("config.save_failed", &fault);
            return self.flash(Notice::new(Msg::ConfigSaveFailed { error }, Level::Error));
        }
        self.overlays.close(OverlayKind::ProfileForm);
        if !self.commit_rule_changed(&before) {
            self.flash(Notice::new(Label::EnvDefaultSaved, Level::Success));
        }
    }

    /// The profiles that name environment `name`.
    pub fn environment_users(&self, name: &str) -> Vec<&ConnectionConfig> {
        self.profiles.iter().filter(|c| c.policy.as_deref() == Some(name)).collect()
    }

    /// The rules of `p` in a few words ([`rules_hint`]).
    pub fn rules_hint(&self, p: &Policy) -> String {
        rules_hint(&self.i18n, p)
    }

    /// The color an environment's badge is drawn in: its own, else a neutral one (the theme's
    /// muted text color; the terminal's light gray where that is no RGB color, as the terminal
    /// theme's dark gray would not take the chip's dark text).
    pub fn badge_color(e: &Environment) -> ratatui::style::Color {
        use ratatui::style::Color;
        match e.display_color() {
            Some(pc) => crate::theme::profile_color(pc),
            None => match crate::theme::cur().fg_muted {
                c @ Color::Rgb(..) => c,
                _ => Color::Gray,
            },
        }
    }

    /// An environment's badge as every place draws it (explorer, picker, forms, tab bar, status
    /// bar): ` PROD ` as a chip of its color ([`crate::theme::chip`]), drawn as is, never on the
    /// background of the row it is on. `None` without a badge.
    pub fn badge_chip(e: &Environment) -> Option<(String, ratatui::style::Style)> {
        e.badge.as_ref().map(|b| (format!(" {b} "), crate::theme::chip(Self::badge_color(e))))
    }

    /// The color of profile `id`'s name ([`crate::theme::Theme::name_style`]): its
    /// environment's, if that has one.
    pub fn name_env_color(&self, id: ProfileId) -> Option<ratatui::style::Color> {
        self.profile_environment(id).and_then(Environment::display_color).map(crate::theme::profile_color)
    }

    /// The style of profile `id`'s name: its environment's color, bold while connected, muted
    /// while not.
    pub fn profile_name_style(&self, id: ProfileId) -> ratatui::style::Style {
        let connected = self.conns.state(id) == NodeState::Connected;
        crate::theme::cur().name_style(self.name_env_color(id), connected)
    }

    // ── the form ────────────────────────────────────────────────────────────

    /// Open the environment form: a new environment, environment `id` to edit, or a copy of it.
    pub(super) fn open_environment_form(&mut self, id: Option<EnvironmentId>, duplicate: bool) {
        if self.form_open_already() {
            return;
        }
        let e = id.and_then(|id| self.environment(id)).cloned();
        let taken = |n: &str| self.environments().iter().any(|q| same_name(&q.name, n));
        let copy = e.as_ref().filter(|_| duplicate).map(|e| profiles::copy_name(&e.name, taken));
        let mut f = ProfileForm::for_environment(e.as_ref(), copy);
        f.session_presets = self.session_presets.clone();
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.overlays.push(Overlay::ProfileForm(Box::new(f)));
    }

    /// `Ctrl+S` in the environment form: save the environment (a rename takes the profiles that
    /// name it along). Nothing changes when the file cannot be written: the form stays open.
    pub(super) fn save_environment_form(&mut self) {
        if self.overlays.form().is_some_and(ProfileForm::is_default_rules) {
            return self.save_default_rules_form();
        }
        let Some(form) = self.overlays.form_mut().filter(|f| !f.saving && f.is_environment()) else { return };
        form.attempted = true;
        let FormKind::Environment { id, editing } = form.kind else { return };
        let (envs, original) = (self.policies.environments(), form.original_name.clone());
        let taken = |n: &str| name_taken(envs, id, original.as_deref(), n);
        if !form.errors(taken).is_empty() {
            return;
        }
        let Some(mut e) = form.to_environment() else { return };
        let old = self.environments().iter().position(|q| q.id == id).filter(|_| editing);
        let before = old.map(|i| self.environments()[i].clone());
        let renamed = before.as_ref().map(|b| b.name.clone()).filter(|n| *n != e.name);
        e.origin = before.as_ref().and_then(|b| b.origin.clone());
        let (policies, profiles) = (self.policies.clone(), self.profiles.clone());
        match old {
            Some(i) => self.policies.environments_mut()[i] = e.clone(),
            None => self.policies.environments_mut().push(e.clone()),
        }
        // Renamed: the profiles that named it name it still, and so do the copies their open
        // connections were made of (the sessions they open from now on take their rules).
        let moved = match &renamed {
            Some(from) => self.rename_environment_in_profiles(from, &e.name),
            None => Vec::new(),
        };
        if let Err(fault) = self.try_persist() {
            for id in moved {
                if let Some(r) = self.conns.get_mut(id).and_then(|c| c.resolved.as_mut()) {
                    r.policy = renamed.clone();
                }
            }
            self.policies = policies;
            self.profiles = profiles;
            let error = self.fault_text("config.save_failed", &fault);
            return self.flash(Notice::new(Msg::ConfigSaveFailed { error }, Level::Error));
        }
        // Back in the profile form that asked for it, the explorer stays where it was.
        if !self.close_form(Some(e.name.clone())) {
            self.reveal_environment(e.id);
        }
        if !self.commit_rule_changed(&policies) {
            self.flash(Notice::new(Msg::EnvironmentSaved { name: e.name }, Level::Success));
        }
    }

    /// The profiles naming environment `from` name `to` instead, and so do the copies their
    /// connections were opened with (`ProfileConn::resolved`); the profiles whose copy changed.
    fn rename_environment_in_profiles(&mut self, from: &str, to: &str) -> Vec<ProfileId> {
        for c in self.profiles.iter_mut().filter(|c| c.policy.as_deref() == Some(from)) {
            c.policy = Some(to.to_string());
        }
        let mut moved = Vec::new();
        for (id, c) in self.conns.iter_mut() {
            if let Some(r) = c.resolved.as_mut().filter(|r| r.policy.as_deref() == Some(from)) {
                r.policy = Some(to.to_string());
                moved.push(id);
            }
        }
        moved
    }

    /// Put the explorer's cursor on environment `id`, opening the section.
    pub(super) fn reveal_environment(&mut self, id: EnvironmentId) {
        self.environments_expanded = true;
        self.explorer.select_kind(explorer::RowKind::Environment(id));
        let rows = self.explorer_rows();
        let i = self.explorer.index(&rows);
        self.explorer.select(&rows, i);
    }

    // ── presets ─────────────────────────────────────────────────────────────

    /// The presets' row of the empty section: `dev`, `test`, `staging` and `prod` are added in
    /// one write (none when the file cannot be written).
    pub(super) fn add_environment_presets(&mut self) {
        if !self.environments().is_empty() {
            return;
        }
        if !self.config_writable() {
            return self.flash(Notice::new(Label::ConfigReadonly, Level::Warning));
        }
        let presets = policy::presets();
        let first = presets[0].id;
        let names = presets.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ");
        self.policies.environments_mut().extend(presets);
        if let Err(fault) = self.try_persist() {
            self.policies.environments_mut().clear();
            let error = self.fault_text("config.save_failed", &fault);
            return self.flash(Notice::new(Msg::ConfigSaveFailed { error }, Level::Error));
        }
        self.reveal_environment(first);
        self.flash(Notice::new(Msg::EnvironmentPresetsAdded { names }, Level::Success));
    }

    // ── delete ──────────────────────────────────────────────────────────────

    /// `d` on an environment: ask first, listing the profiles that name it (they will not
    /// connect until another environment is picked).
    pub(super) fn request_delete_environment(&mut self, id: EnvironmentId) {
        let Some(e) = self.environment(id).cloned() else { return };
        let mut details = Vec::new();
        let users: Vec<String> = self.environment_users(&e.name).iter().map(|c| c.name.clone()).collect();
        if !users.is_empty() {
            details.push(Msg::EnvironmentDeleteUsers { count: users.len() as u64, names: users.join(", ") });
        }
        if self.environment_users(&e.name).iter().any(|c| self.conns.is_connected(c.id)) {
            details.push(Label::EnvironmentDeleteConnected.into());
        }
        self.overlays.close(OverlayKind::Commands);
        self.overlays.push(Overlay::Confirm(Confirm {
            title: Label::ExplorerDeleteTitle,
            text: Msg::EnvironmentDelete { name: e.name.clone() },
            details,
            keys: Label::ExplorerDeleteKeys,
            action: ConfirmAction::DeleteEnvironment(id),
            folder: None,
            path: None,
            tx: None,
            buttons: Default::default(),
        }));
    }

    /// Delete environment `id` (confirmed). The profiles that name it keep the name: they do not
    /// connect (and a connected one runs nothing) until another environment is picked.
    pub(super) fn delete_environment(&mut self, id: EnvironmentId) {
        let Some(i) = self.environments().iter().position(|e| e.id == id) else { return };
        let rows = self.explorer_rows();
        let at = rows.iter().position(|r| r.kind == explorer::RowKind::Environment(id)).unwrap_or(0);
        let e = self.policies.environments_mut().remove(i);
        if let Err(fault) = self.try_persist() {
            self.policies.environments_mut().insert(i, e);
            let error = self.fault_text("config.save_failed", &fault);
            return self.flash(Notice::new(Msg::ConfigSaveFailed { error }, Level::Error));
        }
        self.environments_open.remove(&id);
        let rows = self.explorer_rows();
        self.explorer.select(&rows, at.saturating_sub(1));
        let count = self.environment_users(&e.name).len() as u64;
        let msg = match count {
            0 => Notice::new(Msg::EnvironmentDeleted { name: e.name }, Level::Success),
            _ => Notice::new(Msg::EnvironmentDeletedUsed { name: e.name, count }, Level::Warning),
        };
        self.flash(msg);
    }
}
