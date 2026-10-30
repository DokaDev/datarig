//! Tips (`[ui] tips`): one line about a key in a place that is empty anyway, the welcome
//! panel's first steps, and a status line that names the key of an action run from a menu a few
//! times.
//!
//! **The catalog.** [`catalog`] lists every tip: where it shows ([`Place`]), what it is about
//! ([`Subject`]: an action with the context whose key it names, a command, a setting's value)
//! and its text, a catalog entry with a `{key}`, `{command}` or `{setting}` placeholder. No text
//! names a key itself: the key comes from the keymap ([`App::tip_key`]), so a
//! remapped key shows remapped. Tests check every subject against the action registry, the
//! command table and the settings.
//!
//! **Which tip.** The candidates of a place are the next first step not taken whose key works
//! there (in the empty workspace and the empty editor, while the first steps show), then its tips
//! whose action can run now, has a key where the keys go now and was never run by a key (a key
//! the user knows needs no tip), in catalog order; a tip about a command or a setting only where
//! `:` opens the command line. The tip shown is `candidates[epoch % len]`; the epoch moves only
//! when the user turns a page (another tab or pane, a run that starts or ends) and at launch (from
//! the counts of the usage file), so a screen keeps its tip while it is looked at and no timer
//! ever runs for a tip. While a popup, a dialog, a menu or the command line is open no place has
//! a tip, and the one under it is back when it closes. Each place keeps its pick and its text for
//! the page, the usage, the key map, the language and the context they were made for, so a frame
//! only looks them up.
//!
//! **Usage.** Every action run is counted by how it was asked for (a key, an action menu, the
//! command line) in memory and written to `<state>/usage.toml` ([`datarig_core::usage`]) when the
//! workspace is saved (its own cadence, quitting) and when the first steps are dismissed; never
//! per key. With tips off nothing is counted or written. A file that cannot be read keeps the
//! tips working from memory and is never written over.

use super::command::{SETTINGS, Setting};
use super::*;
use crate::keymap::keys;
use crate::keymap::{KeyChord, Target};
use datarig_core::config::Tips;
use datarig_core::i18n::Lang;
use datarig_core::usage::{self, Usage, Via};
use std::cell::RefCell;

/// Runs of an action from a menu or the command line, without its key ever used, before a status
/// line names its key (once per action).
pub const NUDGE_AFTER: u32 = 3;

/// Where a tip shows. Never over rows, a plan, a chart, a dialog, a menu or a form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Profiles, no tab open: under the keys of the empty workspace.
    NoTab = 0,
    /// A console with no text, focused: inside the editor, gone with the first character.
    EmptyEditor = 1,
    /// The results pane with nothing to show, nothing running.
    EmptyResults = 2,
    /// The results pane while a run waits for its first rows.
    Running = 3,
    /// The status bar's spare room, when no message is there and every hint fits beside it.
    StatusIdle = 4,
}

/// How many places there are.
const PLACES: usize = 5;

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

// No tip names a key the place lists right beside it (the empty workspace's key rows, the
// status bar's hints).
const TIPS: &[Tip] = &[
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
    tip("menu", Place::StatusIdle, Subject::Action(Action::OpenMenu, Ctx::Nav), |key| Msg::TipsMenu { key }),
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
    /// Moves when what decides the candidates of a place changes (an action used for the first
    /// time, a first step taken, the first steps dismissed): the kept picks are made again.
    generation: u64,
    /// Moves when the user turns a page (see the module docs).
    pub epoch: u64,
    /// What turning a page compares: focus, tab, workspace shape, running.
    page: Option<(Focus, Option<TabId>, bool, bool, bool)>,
    /// No tips this session whatever the setting, nothing counted and nothing saved: the test
    /// harness pins its screens without them.
    pub paused: bool,
    /// Each place's last pick with its text.
    shown: RefCell<[Option<Shown>; PLACES]>,
}

/// What a place shows: the next first step (by its index in [`STEPS`]) or a tip of the catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pick {
    Step(usize),
    Tip(usize),
}

/// What a pick and its text were made for: the same again finds them kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Made {
    epoch: u64,
    generation: u64,
    keymap: u64,
    lang: Lang,
    icons: bool,
    ctx: Ctx,
}

/// A place's pick, its text and its line (with the mark). There is always one: no candidate
/// keeps nothing, so an empty place looks again at the next frame.
struct Shown {
    made: Made,
    pick: Pick,
    msg: Msg,
    line: String,
}

/// How an action was asked for ([`App::dispatch_from`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Key,
    Menu,
    Command,
}

impl App {
    /// Run `a` as [`App::dispatch`] does, counting it by where it came from (tips on): a tip
    /// about a key the user knows goes away, and the third run from a menu or the command line
    /// of an action with a key here names that key once.
    pub fn dispatch_from(&mut self, a: Action, source: Source) {
        let via = match source {
            Source::Key => Via::Key,
            Source::Menu => Via::Menu,
            Source::Command => Via::Command,
        };
        let spec = action::spec(a);
        if !self.tips_on() || !(spec.when)(self) {
            return self.dispatch(a);
        }
        let ctx = self.key_context();
        let said = |app: &App| (app.transient.as_ref().map(|t| t.1), app.status.as_ref().map(|s| s.msg.clone()));
        let before = (via != Via::Key).then(|| said(self));
        self.dispatch(a);
        let n = self.count_use(spec.id, via);
        // Said only when the action said nothing itself (its own message matters more; the next
        // run tries again).
        if before.is_some_and(|b| b == said(self)) {
            self.nudge(a, ctx, via, n);
        }
    }

    /// Count a use of `id` (an action id, or a command's [`command_id`]); returns the menu and
    /// command line runs so far.
    fn count_use(&mut self, id: &str, via: Via) -> u32 {
        if self.tips.usage.bump(via, id) == 1 {
            self.tips.generation += 1;
            self.steps_follow();
        }
        self.tips.dirty = true;
        self.tips.usage.count(Via::Menu, id) + self.tips.usage.count(Via::Command, id)
    }

    /// A command of the command line ran (one without an action of its own: `:theme`, `:ddl`).
    pub(super) fn count_command(&mut self, name: &str) {
        if self.tips_on() {
            self.count_use(&command_id(name), Via::Command);
        }
    }

    /// The first step about `a` was taken some other way than by running `a` (a menu opened by
    /// a right click, a run from the explorer, the keyboard help from its own key): its mark
    /// ticks.
    pub(super) fn step_taken(&mut self, a: Action) {
        let id = action::spec(a).id;
        if !self.tips_on() || self.tips.usage.onboarding_done || self.tips.usage.steps.contains(id) {
            return;
        }
        self.tips.usage.steps.insert(id.to_string());
        self.tips.generation += 1;
        self.tips.dirty = true;
        self.steps_follow();
    }

    /// Every first step taken: the first steps are done for good.
    fn steps_follow(&mut self) {
        if !self.tips.usage.onboarding_done && STEPS.iter().all(|s| self.step_done(s)) {
            self.tips.usage.onboarding_done = true;
            self.tips.generation += 1;
            self.tips.dirty = true;
        }
    }

    /// The status line naming `a`'s key in `ctx`, once per action, at the [`NUDGE_AFTER`]th run
    /// from a menu or the command line (or the next quiet one), never once its key was used.
    fn nudge(&mut self, a: Action, ctx: Ctx, via: Via, runs: u32) {
        let id = action::spec(a).id;
        let u = &self.tips.usage;
        if runs < NUDGE_AFTER || u.count(Via::Key, id) > 0 || u.nudged.contains(id) {
            return;
        }
        let Some(k) = self.keymap.tip_keys(a, ctx, self.enhanced_keys) else { return };
        self.tips.usage.nudged.insert(id.to_string());
        self.tips.dirty = true;
        let (mark, key) = (self.tip_mark(), keys::label(&k));
        let action = self.i18n.label(action::spec(a).label).to_string();
        let msg = match via {
            Via::Command => Msg::NudgeCommand { mark, key, action },
            Via::Menu | Via::Key => Msg::NudgeMenu { mark, key, action },
        };
        self.show_status(Notice::new(msg, Level::Info));
    }

    /// Read the usage file (launch). A file that cannot be read leaves the usage empty for this
    /// session and is never written over; the error log says so. No file and connections or a
    /// workspace already there: someone who used an earlier version, for whom the first steps
    /// are done. A first launch writes the file at once, so the next one knows even after a
    /// crash. A file whose presence cannot be checked counts as unreadable.
    pub(super) fn load_usage(&mut self) {
        let Some(path) = self.paths.usage_file() else { return };
        let existed = match path.try_exists() {
            Ok(e) => e,
            Err(e) => {
                self.tips.unreadable = true;
                datarig_core::fault::ErrorLog::new(self.paths.errors_log())
                    .record("usage.unreadable", &datarig_core::fault::Fault::io(&e));
                return;
            }
        };
        match usage::load(&path) {
            Ok(u) => self.tips.usage = u,
            Err(usage::Unreadable) => {
                self.tips.unreadable = true;
                let e = std::io::Error::new(std::io::ErrorKind::InvalidData, path.display().to_string());
                datarig_core::fault::ErrorLog::new(self.paths.errors_log())
                    .record("usage.unreadable", &datarig_core::fault::Fault::io(&e));
            }
        }
        if !existed {
            // A workspace that cannot be checked is not taken as an earlier user's.
            let workspace = self
                .paths
                .state
                .as_ref()
                .is_some_and(|s| matches!(s.join(datarig_core::workspace::FILE).try_exists(), Ok(true)));
            self.tips.usage.onboarding_done = !self.profiles.is_empty() || workspace;
            self.tips.dirty = true;
            self.save_usage();
        }
        // A launch turns a page: where the counts are now decides.
        self.tips.epoch = self.tips.usage.total();
        self.tips.generation += 1;
    }

    /// Write what was counted to the usage file (with the workspace, and when the first steps
    /// are dismissed). Nothing with tips off, without a state directory this instance may write,
    /// or when the file could not be read.
    pub(super) fn save_usage(&mut self) {
        if !self.tips.dirty || self.tips.unreadable || !self.tips_on() {
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
    /// or ended, the first profile or tab) moves the epoch, so the next tip shows there. A run,
    /// however it started, is the run's first step.
    pub(super) fn tips_follow(&mut self) {
        let tab = (!self.tabs.is_empty()).then(|| self.tab().id);
        let running = !self.tabs.is_empty() && self.tab().exec.running.is_some();
        let page = Some((self.focus, tab, self.profiles.is_empty(), self.tabs.is_empty(), running));
        if self.tips.page != page {
            if self.tips.page.is_some() {
                self.tips.epoch = self.tips.epoch.wrapping_add(1);
            }
            self.tips.page = page;
            if running {
                self.step_taken(Action::RunStatement);
            }
        }
    }

    /// The tip of `place` now, if any (tips off, or a popup, a dialog, a menu or the command line
    /// open: none).
    pub fn tip(&self, place: Place) -> Option<Msg> {
        self.shown(place, |s| s.msg.clone())
    }

    /// The tip of `place` as a line: its mark (the glyph, or the catalog's `Tip:` with icons off;
    /// a first step's own mark) and its text.
    pub fn tip_line(&self, place: Place) -> Option<String> {
        self.shown(place, |s| s.line.clone())
    }

    /// `f` of the pick of `place`: the kept one when it was made for what is here now, else a
    /// new one, kept (no candidate: nothing kept).
    fn shown<T>(&self, place: Place, f: impl FnOnce(&Shown) -> T) -> Option<T> {
        // Under a popup, a dialog, a menu or the command line: no tip, and nothing kept.
        if !self.tips_on() || self.overlays.top().is_some() {
            return None;
        }
        // The welcome panel has its first steps.
        if place == Place::StatusIdle && self.profiles.is_empty() && self.onboarding_shown() {
            return None;
        }
        let made = Made {
            epoch: self.tips.epoch,
            generation: self.tips.generation,
            keymap: self.keymap.id(),
            lang: self.i18n.lang,
            icons: self.icons_on(),
            ctx: self.key_context(),
        };
        let mut shown = self.tips.shown.borrow_mut();
        let slot = &mut shown[place as usize];
        // A tip whose action can no longer run (on the same page) is picked again.
        let kept = slot.as_ref().is_some_and(|s| {
            s.made == made
                && match s.pick {
                    Pick::Tip(i) => match TIPS[i].subject {
                        Subject::Action(a, _) => (action::spec(a).when)(self),
                        _ => true,
                    },
                    Pick::Step(_) => true,
                }
        });
        if !kept {
            *slot = None;
            let c = self.candidates(place, made.ctx);
            if c.is_empty() {
                return None;
            }
            let pick = c[(made.epoch % c.len() as u64) as usize];
            let (msg, mark) = match pick {
                Pick::Step(i) => {
                    (self.step_msg(&STEPS[i], made.ctx), crate::icons::step(false, made.icons).to_string())
                }
                Pick::Tip(i) => (self.tip_msg(&TIPS[i]), self.tip_mark()),
            };
            let line = format!("{mark} {}", self.i18n.msg(&msg));
            *slot = Some(Shown { made, pick, msg, line });
        }
        slot.as_ref().map(f)
    }

    /// The candidates of `place` with the keys going to `ctx`: the next first step, then the
    /// open tips (not one about that step's action).
    fn candidates(&self, place: Place, ctx: Ctx) -> Vec<Pick> {
        let step = matches!(place, Place::NoTab | Place::EmptyEditor).then(|| self.next_step(ctx)).flatten();
        let about_step =
            |t: &Tip| step.is_some_and(|s| matches!(t.subject, Subject::Action(a, _) if a == STEPS[s].action));
        let tips = (0..TIPS.len())
            .filter(|&i| TIPS[i].place == place && !about_step(&TIPS[i]) && self.tip_open(&TIPS[i], ctx));
        step.map(Pick::Step).into_iter().chain(tips.map(Pick::Tip)).collect()
    }

    /// The mark of a tip: the glyph, or the catalog's `Tip:` with icons off.
    fn tip_mark(&self) -> String {
        match self.icons_on() {
            true => crate::icons::TIP.to_string(),
            false => self.i18n.label(Label::TipMark).to_string(),
        }
    }

    /// `t` can be shown with the keys going to `ctx`: its action can run, its context is in
    /// `ctx`'s chain (the key it names works here) and has a key for it that was never used; its
    /// command was never run and its setting is not set so already, and `:` opens the command
    /// line here.
    fn tip_open(&self, t: &Tip, ctx: Ctx) -> bool {
        let u = &self.tips.usage;
        match t.subject {
            Subject::Action(a, at) => {
                let spec = action::spec(a);
                (spec.when)(self)
                    && ctx.chain().contains(&at)
                    && self.keymap.hint_keys(a, at, self.enhanced_keys).is_some()
                    && u.count(Via::Key, spec.id) == 0
            }
            Subject::Command(name) => self.colon_opens_commands(ctx) && u.count(Via::Command, &command_id(name)) == 0,
            Subject::Setting(s) => self.colon_opens_commands(ctx) && !self.setting_is(s),
        }
    }

    /// `:` opens the command line in `ctx` (not in a dialog, not while typing text).
    fn colon_opens_commands(&self, ctx: Ctx) -> bool {
        let (target, _) = self.keymap.resolve_seq(ctx, &[KeyChord::char(':')]);
        target == Some(Target::Action(Action::OpenCommands))
    }

    /// `t`'s text with its subject filled in.
    pub fn tip_msg(&self, t: &Tip) -> Msg {
        (t.text)(self.subject_text(t.subject))
    }

    /// What a subject fills in: the key ([`App::tip_key`]), `:name`, or `:set key=value`.
    pub fn subject_text(&self, s: Subject) -> String {
        match s {
            Subject::Action(a, ctx) => self.tip_key(a, ctx),
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

    /// Step `s` was taken: its action ran (by key, menu or command line) or the step was taken
    /// some other way ([`App::step_taken`]); the first connection also once there is one.
    pub fn step_done(&self, s: &Step) -> bool {
        let id = action::spec(s.action).id;
        let u = &self.tips.usage;
        (s.action == Action::NewProfile && !self.profiles.is_empty())
            || u.steps.contains(id)
            || [Via::Key, Via::Menu, Via::Command].into_iter().any(|v| u.count(v, id) > 0)
    }

    /// The first step not taken that can be taken with a key where the keys go (`ctx`): no run
    /// step without a tab, no key that does something else here.
    fn next_step(&self, ctx: Ctx) -> Option<usize> {
        if !self.onboarding_shown() {
            return None;
        }
        STEPS.iter().position(|s| {
            !self.step_done(s)
                && (action::spec(s.action).when)(self)
                && self.keymap.hint_keys(s.action, ctx, self.enhanced_keys).is_some()
        })
    }

    /// Step `s` as the tip of a place whose keys go to `ctx`: its key there, and the key that
    /// hides the steps where there is one.
    fn step_msg(&self, s: &Step, ctx: Ctx) -> Msg {
        let label = |a: Action| self.keymap.tip_keys(a, ctx, self.enhanced_keys).map(|k| keys::label(&k));
        let (step, key) = (self.i18n.label(s.label).to_string(), label(s.action).unwrap_or_default());
        match label(Action::DismissOnboarding) {
            Some(dismiss) => Msg::OnboardingNext { step, key, dismiss },
            None => Msg::OnboardingNextStep { step, key },
        }
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

#[cfg(test)]
mod tests;
