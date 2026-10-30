//! Tips (`[ui] tips`): one line about a key in a place that is empty anyway, the welcome
//! panel's first steps, and a status line that names the key of an action run from a menu a few
//! times.
//!
//! **The catalog.** [`catalog`] lists every tip: where it shows ([`Place`]), what it is about
//! ([`Subject`]: an action with the context whose key it names, a command, a setting's value)
//! and its text, a catalog entry with a `{key}`, `{command}` or `{setting}` placeholder. No text
//! names a key itself: the key is the one the hint line would show ([`App::key_for`]), so a
//! remapped key shows remapped. Tests check every subject against the action registry, the
//! command table and the settings.
//!
//! **Which tip.** The candidates of a place are its tips whose action can run now, has a key in
//! its context and was never run by a key (a key the user knows needs no tip), in catalog order.
//! The tip shown is `candidates[epoch % len]`; the epoch moves only when the user turns a page
//! (another tab or pane, a run that starts or ends) and at launch (from the counts of the usage
//! file), so a screen keeps its tip while it is looked at and no timer ever runs for a tip. The
//! pick of each place is kept for its epoch and the usage it was made from, so a frame does not
//! search again.
//!
//! **Usage.** How often actions ran by key, from a menu and from the command line is counted in
//! memory and written to `<state>/usage.toml` ([`datarig_core::usage`]) when the workspace is
//! saved (its own cadence, quitting) and when the first steps are dismissed; never per key. A
//! file that cannot be read keeps the tips working from memory and is never written over.

use super::command::{COMMANDS, SETTINGS, Setting};
use super::*;
use crate::keymap::keys;
use datarig_core::config::Tips;
use datarig_core::usage::{self, Usage, Via};
use std::cell::Cell;

/// A first run (no profile): the new-connection form opens at once over the welcome panel, whose
/// first steps show once it is closed. `false` would leave the welcome panel with its first steps
/// in front, its new-connection key one press away.
pub const FIRST_RUN_OPENS_FORM: bool = true;

/// Runs of an action from a menu or the command line, without its key ever used, before a status
/// line names its key (once per action).
pub const NUDGE_AFTER: u32 = 3;

/// Where a tip shows. Never over rows, a plan, a chart, a dialog, a menu or a form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Profiles, no tab open: under the keys of the empty workspace.
    NoTab,
    /// A console with no text, focused: inside the editor, gone with the first character.
    EmptyEditor,
    /// The results pane with nothing to show, nothing running.
    EmptyResults,
    /// The results pane while a run waits for its first rows.
    Running,
    /// The status bar's spare room, when no message is there and every hint fits beside it.
    StatusIdle,
}

impl Place {
    const ALL: [Place; 5] = [Place::NoTab, Place::EmptyEditor, Place::EmptyResults, Place::Running, Place::StatusIdle];

    fn index(self) -> usize {
        Place::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }
}

/// What a tip is about: the value it fills in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    /// An action and the context whose key the tip names (`{key}`).
    Action(Action, Ctx),
    /// A command of the command line, by name (`{command}`).
    Command(&'static str),
    /// A setting's value (`{setting}`: the `:set` that sets it).
    Setting(Setting),
}

/// One tip of the catalog.
pub struct Tip {
    /// Stable identifier (`tips.<id>` is its text in the catalogs).
    pub id: &'static str,
    pub place: Place,
    pub subject: Subject,
    /// The text, with the subject's key, command or setting filled in.
    pub text: fn(String) -> Msg,
}

use super::action::ExplorerAction as E;
use super::action::PanelAction;

const fn tip(id: &'static str, place: Place, subject: Subject, text: fn(String) -> Msg) -> Tip {
    Tip { id, place, subject, text }
}

const TIPS: &[Tip] = &[
    tip("quick_connect", Place::NoTab, Subject::Action(Action::QuickConnect, Ctx::Explorer), |key| {
        Msg::TipsQuickConnect { key }
    }),
    tip("explorer_filter", Place::NoTab, Subject::Action(Action::Explorer(E::Filter), Ctx::Explorer), |key| {
        Msg::TipsExplorerFilter { key }
    }),
    tip("settings", Place::NoTab, Subject::Action(Action::OpenSettings, Ctx::Nav), |key| Msg::TipsSettings { key }),
    tip("ddl_command", Place::NoTab, Subject::Command("ddl"), |command| Msg::TipsDdlCommand { command }),
    tip("run", Place::EmptyEditor, Subject::Action(Action::RunStatement, Ctx::Workspace), |key| Msg::TipsRun { key }),
    tip("complete", Place::EmptyEditor, Subject::Action(Action::ShowCompletions, Ctx::VimInsert), |key| {
        Msg::TipsComplete { key }
    }),
    tip("external_edit", Place::EmptyEditor, Subject::Action(Action::ExternalEdit, Ctx::Workspace), |key| {
        Msg::TipsExternalEdit { key }
    }),
    tip("save_script", Place::EmptyEditor, Subject::Action(Action::ScriptSave, Ctx::Workspace), |key| {
        Msg::TipsSaveScript { key }
    }),
    tip("format", Place::EmptyEditor, Subject::Action(Action::FormatSql, Ctx::Nav), |key| Msg::TipsFormat { key }),
    tip("explain", Place::EmptyResults, Subject::Action(Action::Explain(false), Ctx::Nav), |key| Msg::TipsExplain {
        key,
    }),
    tip("results_toggle", Place::EmptyResults, Subject::Action(Action::Panel(PanelAction::Toggle), Ctx::Nav), |key| {
        Msg::TipsResultsToggle { key }
    }),
    tip("new_console", Place::EmptyResults, Subject::Action(Action::NewTab, Ctx::Workspace), |key| {
        Msg::TipsNewConsole { key }
    }),
    tip("cancel", Place::Running, Subject::Action(Action::CancelQuery, Ctx::Workspace), |key| Msg::TipsCancel { key }),
    tip("run_elsewhere", Place::Running, Subject::Action(Action::NewTab, Ctx::Workspace), |key| {
        Msg::TipsRunElsewhere { key }
    }),
    tip("help", Place::StatusIdle, Subject::Action(Action::Help, Ctx::Workspace), |key| Msg::TipsHelp { key }),
    tip("menu", Place::StatusIdle, Subject::Action(Action::OpenMenu, Ctx::Nav), |key| Msg::TipsMenu { key }),
    tip("commands", Place::StatusIdle, Subject::Action(Action::OpenCommands, Ctx::Nav), |key| Msg::TipsCommands {
        key,
    }),
    tip("zoom", Place::StatusIdle, Subject::Action(Action::Zoom, Ctx::Nav), |key| Msg::TipsZoom { key }),
    tip("theme", Place::StatusIdle, Subject::Command("theme"), |command| Msg::TipsTheme { command }),
    tip("tips_off", Place::StatusIdle, Subject::Setting(Setting::Tips(Tips::Off)), |setting| Msg::TipsTipsOff {
        setting,
    }),
];

/// Every tip, in the order each place offers them (the docs list them from here too).
pub fn catalog() -> &'static [Tip] {
    TIPS
}

/// A first step of the welcome panel: the action, the context whose key it shows, its text.
pub struct Step {
    pub action: Action,
    pub ctx: Ctx,
    pub label: Label,
}

/// The welcome panel's first steps, in order.
pub const STEPS: [Step; 4] = [
    Step { action: Action::NewProfile, ctx: Ctx::Welcome, label: Label::OnboardingConnect },
    Step { action: Action::RunStatement, ctx: Ctx::VimNormal, label: Label::OnboardingRun },
    Step { action: Action::OpenMenu, ctx: Ctx::Nav, label: Label::OnboardingMenu },
    Step { action: Action::Help, ctx: Ctx::Nav, label: Label::OnboardingHelp },
];

/// The command line's name of a command's use in the usage file (`:theme`): command names and
/// action ids never meet.
fn command_id(name: &str) -> String {
    format!(":{name}")
}

/// What the tips know and keep between frames.
#[derive(Default)]
pub struct TipState {
    pub usage: Usage,
    /// The usage file could not be read: nothing is written to it this session.
    unreadable: bool,
    /// Counted since the last write.
    dirty: bool,
    /// Moves when what decides the candidates of a place changes (a key used for the first time,
    /// the first steps dismissed): the kept picks are made again.
    generation: u64,
    /// Moves when the user turns a page (see the module docs).
    pub epoch: u64,
    /// What turning a page compares: focus, tab, workspace shape, running.
    page: Option<(Focus, Option<TabId>, bool, bool, bool)>,
    /// No tips this session whatever the setting, and nothing of it saved: the test harness pins
    /// its screens without them.
    pub paused: bool,
    /// Each place's last pick.
    picks: Cell<[Option<Pick>; Place::ALL.len()]>,
}

/// A place's pick: the epoch and generation it was made for, and the catalog index (none: no
/// candidate).
type Pick = (u64, u64, Option<usize>);

/// How an action was asked for ([`App::dispatch_from`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Key,
    Menu,
    Command,
    /// A click, the keyboard help, a row of the explorer: not counted.
    Other,
}

impl App {
    /// Run `a` as [`App::dispatch`] does, counting it by where it came from: a tip about a key
    /// the user knows goes away, and the third run from a menu or the command line of an action
    /// with a key here names that key once.
    pub fn dispatch_from(&mut self, a: Action, source: Source) {
        let via = match source {
            Source::Key => Via::Key,
            Source::Menu => Via::Menu,
            Source::Command => Via::Command,
            Source::Other => return self.dispatch(a),
        };
        let spec = action::spec(a);
        let ctx = self.key_context();
        let counted = (spec.when)(self) && self.counts(a, via);
        let status = (self.transient.as_ref().map(|t| t.1), self.status.as_ref().map(|s| s.msg.clone()));
        self.dispatch(a);
        if !counted {
            return;
        }
        let n = self.count_use(spec.id, via);
        // Said only when the action said nothing itself (its own message matters more; the next
        // run tries again).
        let quiet = (self.transient.as_ref().map(|t| t.1), self.status.as_ref().map(|s| s.msg.clone())) == status;
        if via != Via::Key && quiet && self.tips_on() {
            self.nudge(a, ctx, n);
        }
    }

    /// Whether a run of `a` `via` is counted: tip and first-step actions always; others when not
    /// by key (a key run is counted once a menu or command run was, for the nudge).
    fn counts(&self, a: Action, via: Via) -> bool {
        let id = action::spec(a).id;
        via != Via::Key
            || STEPS.iter().any(|s| s.action == a)
            || TIPS.iter().any(|t| matches!(t.subject, Subject::Action(x, _) if x == a))
            || self.tips.usage.count(Via::Menu, id) > 0
            || self.tips.usage.count(Via::Command, id) > 0
    }

    /// Count a use of `id` (an action id, or a command's [`command_id`]); returns the menu and
    /// command line runs so far.
    fn count_use(&mut self, id: &str, via: Via) -> u32 {
        if self.tips.usage.bump(via, id) == 1 {
            self.tips.generation += 1;
        }
        self.tips.dirty = true;
        if !self.tips.usage.onboarding_done && STEPS.iter().all(|s| self.step_done(s)) {
            self.tips.usage.onboarding_done = true;
            self.tips.generation += 1;
        }
        self.tips.usage.count(Via::Menu, id) + self.tips.usage.count(Via::Command, id)
    }

    /// A command of the command line ran (one without an action of its own: `:theme`, `:ddl`).
    pub(super) fn count_command(&mut self, name: &str) {
        self.count_use(&command_id(name), Via::Command);
    }

    /// The status line naming `a`'s key in `ctx`, once per action, at the [`NUDGE_AFTER`]th run
    /// from a menu or the command line (or the next quiet one), never once its key was used.
    fn nudge(&mut self, a: Action, ctx: Ctx, runs: u32) {
        let id = action::spec(a).id;
        let u = &self.tips.usage;
        if runs < NUDGE_AFTER || u.count(Via::Key, id) > 0 || u.nudged.contains(id) {
            return;
        }
        let Some(k) = self.keymap.hint_keys(a, ctx, self.enhanced_keys) else { return };
        self.tips.usage.nudged.insert(id.to_string());
        self.tips.dirty = true;
        let action = self.i18n.label(action::spec(a).label).to_string();
        self.show_status(Notice::new(Msg::NudgeKey { key: keys::label(&k), action }, Level::Info));
    }

    /// Read the usage file (launch). A file that cannot be read leaves the usage empty for this
    /// session and is never written over; the error log says so.
    pub(super) fn load_usage(&mut self) {
        let Some(path) = self.paths.usage_file() else { return };
        match usage::load(&path) {
            Ok(u) => self.tips.usage = u,
            Err(usage::Unreadable) => {
                self.tips.unreadable = true;
                let e = std::io::Error::new(std::io::ErrorKind::InvalidData, path.display().to_string());
                datarig_core::fault::ErrorLog::new(self.paths.errors_log())
                    .record("usage.unreadable", &datarig_core::fault::Fault::io(&e));
            }
        }
        // A launch turns a page: where the counts are now decides.
        self.tips.epoch = self.tips.usage.total();
        self.tips.generation += 1;
    }

    /// Write what was counted to the usage file (with the workspace, and when the first steps
    /// are dismissed). Nothing without a state directory this instance may write, or when the
    /// file could not be read.
    pub(super) fn save_usage(&mut self) {
        if !self.tips.dirty || self.tips.unreadable {
            return;
        }
        let Some(path) = self.state_dir().map(|s| s.join(usage::FILE)) else { return };
        match usage::save(&path, &self.tips.usage) {
            Ok(()) => self.tips.dirty = false,
            // Damaged since launch: left alone from now on.
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => self.tips.unreadable = true,
            Err(e) => {
                datarig_core::fault::ErrorLog::new(self.paths.errors_log())
                    .record("usage.save_failed", &datarig_core::fault::Fault::io_at(&e, &path));
                // Not tried again until something new is counted.
                self.tips.dirty = false;
            }
        }
    }

    /// After an input or a database event: a new page (another tab or pane, a run that started
    /// or ended, the first profile or tab) moves the epoch, so the next tip shows there.
    pub(super) fn tips_follow(&mut self) {
        let tab = (!self.tabs.is_empty()).then(|| self.tab().id);
        let running = !self.tabs.is_empty() && self.tab().exec.running.is_some();
        let page = Some((self.focus, tab, self.profiles.is_empty(), self.tabs.is_empty(), running));
        if self.tips.page != page {
            if self.tips.page.is_some() {
                self.tips.epoch = self.tips.epoch.wrapping_add(1);
            }
            self.tips.page = page;
        }
    }

    /// The tip of `place` now, if any (tips off: none). Where the first steps are not done, the
    /// next of them comes first in the empty workspace and the empty editor.
    pub fn tip(&self, place: Place) -> Option<Msg> {
        if !self.tips_on() {
            return None;
        }
        if matches!(place, Place::NoTab | Place::EmptyEditor)
            && let Some(m) = self.next_step()
        {
            return Some(m);
        }
        // The welcome panel has its first steps.
        if place == Place::StatusIdle && self.profiles.is_empty() && self.onboarding_shown() {
            return None;
        }
        let mut picks = self.tips.picks.get();
        let slot = &mut picks[place.index()];
        let kept = match *slot {
            Some((e, g, i)) if e == self.tips.epoch && g == self.tips.generation => Some(i),
            _ => None,
        };
        // A pick no longer open (the mode or what can run changed on the same page) is made
        // again.
        let i = match kept {
            Some(i) if i.is_none_or(|i| self.tip_open(&TIPS[i])) => i,
            _ => {
                let c: Vec<usize> =
                    (0..TIPS.len()).filter(|&i| TIPS[i].place == place && self.tip_open(&TIPS[i])).collect();
                let i = (!c.is_empty()).then(|| c[(self.tips.epoch % c.len() as u64) as usize]);
                *slot = Some((self.tips.epoch, self.tips.generation, i));
                self.tips.picks.set(picks);
                i
            }
        };
        Some(self.tip_msg(&TIPS[i?]))
    }

    /// The tip of `place` as a line: its mark (the glyph, or the catalog's `Tip:` with icons off;
    /// a first step's own mark) and its text.
    pub fn tip_line(&self, place: Place) -> Option<String> {
        let m = self.tip(place)?;
        let on = self.icons_on();
        let mark = match m {
            Msg::OnboardingNext { .. } => crate::icons::step(false, on).to_string(),
            _ if on => crate::icons::TIP.to_string(),
            _ => self.i18n.label(Label::TipMark).to_string(),
        };
        Some(format!("{mark} {}", self.i18n.msg(&m)))
    }

    /// `t` can be shown now: its action can run, its context is where the keys go now (the key
    /// it names works here) and has a key for it that was never used; its command was never
    /// run; its setting is not set so already.
    fn tip_open(&self, t: &Tip) -> bool {
        let u = &self.tips.usage;
        match t.subject {
            Subject::Action(a, ctx) => {
                let spec = action::spec(a);
                (spec.when)(self)
                    && self.key_context().chain().contains(&ctx)
                    && self.keymap.hint_keys(a, ctx, self.enhanced_keys).is_some()
                    && u.count(Via::Key, spec.id) == 0
            }
            Subject::Command(name) => u.count(Via::Command, &command_id(name)) == 0,
            Subject::Setting(s) => !self.setting_is(s),
        }
    }

    /// `t`'s text with its subject filled in.
    pub fn tip_msg(&self, t: &Tip) -> Msg {
        (t.text)(self.subject_text(t.subject))
    }

    /// What a subject fills in: the key ([`App::key_for`]), `:name`, or `:set key=value`.
    pub fn subject_text(&self, s: Subject) -> String {
        match s {
            Subject::Action(a, ctx) => self.key_for(a, ctx),
            Subject::Command(name) => format!(":{name}"),
            Subject::Setting(s) => match setting_assignment(s) {
                Some((key, value)) => format!(":set {key}={value}"),
                None => String::new(),
            },
        }
    }

    /// The setting has value `s` now.
    fn setting_is(&self, s: Setting) -> bool {
        SETTINGS.iter().enumerate().any(|(k, spec)| {
            let values = spec.values.fixed();
            values.iter().position(|v| v.1 == s).is_some_and(|i| self.setting_value(k) == Some(i))
        })
    }

    /// The first steps show: tips on, not dismissed, not all done.
    pub fn onboarding_shown(&self) -> bool {
        self.tips_on() && !self.tips.usage.onboarding_done && !STEPS.iter().all(|s| self.step_done(s))
    }

    /// `[ui] tips` is on (and the session does not pause them).
    fn tips_on(&self) -> bool {
        self.prefs.tips == Tips::On && !self.tips.paused
    }

    /// Step `s` was taken: its action ran (by key, menu or command line); the first connection
    /// also once there is one.
    pub fn step_done(&self, s: &Step) -> bool {
        let id = action::spec(s.action).id;
        let u = &self.tips.usage;
        (s.action == Action::NewProfile && !self.profiles.is_empty())
            || [Via::Key, Via::Menu, Via::Command].into_iter().any(|v| u.count(v, id) > 0)
    }

    /// The first step not taken, as the tip of the empty workspace or editor.
    fn next_step(&self) -> Option<Msg> {
        if !self.onboarding_shown() {
            return None;
        }
        let s = STEPS.iter().find(|s| !self.step_done(s))?;
        Some(Msg::OnboardingNext {
            step: self.i18n.label(s.label).to_string(),
            key: self.key_for(s.action, s.ctx),
            dismiss: self.key_for(Action::DismissOnboarding, Ctx::Explorer),
        })
    }

    /// Hide the first steps for good (saved at once).
    pub(super) fn dismiss_onboarding(&mut self) {
        self.tips.usage.onboarding_done = true;
        self.tips.generation += 1;
        self.tips.dirty = true;
        self.save_usage();
    }
}

/// The settings key and value name that set `s` (`ui.tips`, `off`).
pub fn setting_assignment(s: Setting) -> Option<(&'static str, &'static str)> {
    SETTINGS.iter().find_map(|spec| spec.values.fixed().iter().find(|v| v.1 == s).map(|v| (spec.key, v.0)))
}

/// The first steps are shown (the action that hides them can run).
pub(super) fn onboarding_open(a: &App) -> bool {
    a.onboarding_shown()
}

/// Whether `name` is a command of the command line.
pub fn command_exists(name: &str) -> bool {
    COMMANDS.iter().any(|c| c.name == name)
}

#[cfg(test)]
mod tests;
