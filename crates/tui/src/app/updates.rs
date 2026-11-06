//! The update check and its notice. `datarig_core::update` decides; this module asks at the
//! right moment, keeps what the answer says, and tells the user.
//!
//! * At launch, from what `update.toml` already knows: the notice of a newer release (drawn
//!   with the first frame, so it never catches keys meant for the workspace; after What's new
//!   when both are due), and whether a check is due.
//! * After the first frame, a due check: the attempt is written first (a crash or a hang still
//!   counts as the day's), then the request runs on a thread of its own, never joined: quitting
//!   never waits for it, whatever it is stuck on (a name lookup that never returns).
//! * Its answer is kept; a newer release found now is said on the status line only (the notice
//!   comes at the next launch).
//! * `:update` says what is known, and never "up to date" after a check that failed.
//!
//! Nothing checks with `[updates] check = "off"`, `DATARIG_NO_UPDATE_CHECK`, `CI`, in a debug
//! build (unless a test gave its own fetcher), without a state directory, with an
//! `update.toml` that cannot be read, or in a second instance (which reads and tells only).

use super::overlay::{Buttons, Overlay, OverlayKind};
use super::{App, AppEvent, Level, Notice};
use chrono::{DateTime, Utc};
use datarig_core::config::UpdateCheck;
use datarig_core::i18n::{Label, Msg};
use datarig_core::update::install::{Install, cargo_home};
use datarig_core::update::proxy::{self, Proxy, Vars};
use datarig_core::update::{self, Answer, Cache, Decision, Off, Outcome, State, Switches, Version};
use datarig_update::{Fetch, Https};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use std::path::PathBuf;
use std::sync::Arc;

/// What the app knows of the update check this run.
#[derive(Default)]
pub struct Updates {
    /// No check and no notice this run (what measures the app in-process sets it).
    pub paused: bool,
    /// The request; tests give a fake. `None`: the real one, made when a check is due (never in
    /// a debug build).
    pub fetch: Option<Arc<dyn Fetch>>,
    /// The switches of this run (`None` before launch).
    pub switches: Option<Switches>,
    /// What launch decided.
    pub decision: Option<Decision>,
    /// A check of this run is on its way.
    pub running: bool,
    /// The notice waits for What's new to close.
    pub pending: Option<Version>,
}

/// The update notice.
#[derive(Clone, Debug)]
pub struct UpdateNotice {
    pub version: Version,
    pub running: Version,
    /// The release's one line, when known.
    pub summary: Option<String>,
    pub install: Install,
    /// The button `Enter` presses: Close, Skip this version, Turn off checks.
    pub focus: usize,
    pub buttons: Buttons,
}

/// What a press on the notice does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeAnswer {
    /// Close: told again after a week.
    Close,
    /// Not told about this version again.
    Skip,
    /// No more checks.
    TurnOff,
}

impl NoticeAnswer {
    pub const ALL: [NoticeAnswer; 3] = [NoticeAnswer::Close, NoticeAnswer::Skip, NoticeAnswer::TurnOff];
}

impl App {
    /// Now, as the wall clock says.
    fn utc_now(&self) -> DateTime<Utc> {
        (self.wall)().into()
    }

    /// The process's proxy variables (upper case first).
    fn proxy_vars(&self) -> Vars {
        let get = |upper: Option<String>, lower: Option<String>| upper.filter(|v| !v.is_empty()).or(lower);
        Vars {
            https_proxy: get((self.env)("HTTPS_PROXY"), (self.env)("https_proxy")),
            all_proxy: get((self.env)("ALL_PROXY"), (self.env)("all_proxy")),
            no_proxy: get((self.env)("NO_PROXY"), (self.env)("no_proxy")),
        }
    }

    /// How this datarig was installed.
    fn install(&self) -> Install {
        let home = (self.env)("HOME").or_else(|| (self.env)("USERPROFILE")).map(PathBuf::from);
        let cargo = cargo_home((self.env)("CARGO_HOME").map(PathBuf::from), home);
        Install::current(cargo.as_deref())
    }

    /// Launch (after What's new): the notice from what is known, and whether to check.
    pub(super) fn updates_at_launch(&mut self) {
        // With a config that has errors, the setting is not known: no check, no notice.
        if self.config_broken || self.updates.paused {
            return;
        }
        let set = |v: Option<String>| v.is_some_and(|v| !v.is_empty());
        let mut sw = Switches::new(
            self.prefs.update_check == UpdateCheck::On,
            set((self.env)("DATARIG_NO_UPDATE_CHECK")),
            set((self.env)("CI")),
            !self.read_only,
        );
        // A fetcher a test gave never reaches the network: the debug build's switch is its.
        sw.debug_build &= self.updates.fetch.is_none();
        let cache = Cache::load(&self.paths);
        let now = self.utc_now();
        self.updates.switches = Some(sw);
        self.updates.decision = Some(update::decide(now, &cache, &sw));
        // Not over the welcome panel's first steps: the next launch tells.
        if self.onboarding_shown() {
            return;
        }
        if let Some(v) = update::notice(now, &cache, &sw, &Version::running()) {
            if self.overlays.is_open(OverlayKind::WhatsNew) {
                self.updates.pending = Some(v);
            } else {
                self.open_update_notice(v, &cache);
            }
        }
    }

    /// The first frame is drawn: a due check starts.
    pub(super) fn updates_after_first_frame(&mut self) {
        if self.updates.decision != Some(Decision::Due) || self.updates.running {
            return;
        }
        self.updates.decision = Some(Decision::NotDue);
        let Some(tx) = self.tx.clone() else { return };
        let now = self.utc_now();
        // The attempt is kept before anything is sent; if it cannot be, nothing is sent (the
        // interval could not be kept).
        let Some(state) = self.change_update_state(|s| s.claim(now)) else { return };
        // A proxy that cannot be used: no request at all, never a direct one.
        let proxy = proxy::for_host(&self.proxy_vars(), update::HOST);
        if proxy == Proxy::Unsupported {
            return self.update_answered(Answer::Failed(Outcome::Proxy));
        }
        let fetch = match self.updates.fetch.clone() {
            Some(f) => f,
            None => match Https::new(&proxy) {
                Some(h) => Arc::new(h) as Arc<dyn Fetch>,
                None => return self.update_answered(Answer::Failed(Outcome::Proxy)),
            },
        };
        self.updates.running = true;
        let etag = state.etag;
        // Detached: never joined, so quitting does not wait for it.
        let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let answer = fetch.fetch(etag.as_deref());
            let _ = tx.send(AppEvent::UpdateChecked(answer));
        });
        if spawned.is_err() {
            self.updates.running = false;
        }
    }

    /// The check answered.
    pub(super) fn update_answered(&mut self, answer: Answer) {
        self.updates.running = false;
        let found = match &answer {
            Answer::Release(r, _) => Some(r.version),
            _ => None,
        };
        let Some(state) = self.change_update_state(|s| s.record(answer)) else { return };
        if let Some(v) = found
            && v > Version::running()
            && !v.is_rc()
            && state.skipped != Some(v)
        {
            self.flash(Notice::new(Msg::UpdateFound { version: v.to_string() }, Level::Info));
        }
    }

    /// Change `update.toml` with `change`, read again first (another part of this run may have
    /// written it). `None`, nothing written, when it cannot be read or written (or this instance
    /// may not write it).
    fn change_update_state(&mut self, change: impl FnOnce(&mut State)) -> Option<State> {
        if self.read_only {
            return None;
        }
        let Cache::Known(path, state) = Cache::load(&self.paths) else { return None };
        let mut state = *state;
        change(&mut state);
        match update::save(&path, &state) {
            Ok(()) => Some(state),
            Err(e) => {
                datarig_core::fault::ErrorLog::new(self.paths.errors_log())
                    .record("update.save", &datarig_core::fault::Fault::io(&e));
                None
            }
        }
    }

    fn open_update_notice(&mut self, version: Version, cache: &Cache) {
        let summary = cache.state().filter(|s| s.latest == Some(version)).and_then(|s| s.summary.clone());
        let notice = UpdateNotice {
            version,
            running: Version::running(),
            summary,
            install: self.install(),
            focus: 0,
            buttons: Buttons::default(),
        };
        self.overlays.close(OverlayKind::UpdateNotice);
        self.overlays.push(Overlay::UpdateNotice(notice));
    }

    /// What's new closed: the notice that waited for it.
    pub(super) fn whats_new_closed(&mut self) {
        if let Some(v) = self.updates.pending.take() {
            let cache = Cache::load(&self.paths);
            self.open_update_notice(v, &cache);
        }
    }

    /// Keys of the notice: `Enter` presses the focused button (Close at first), `Esc` and `q`
    /// close, `s` skips this version, `o` turns the checks off; `Tab`, the arrows and `h`/`l`
    /// move the focus.
    pub(super) fn update_notice_key(&mut self, key: KeyEvent, repeat: bool) {
        let Some(n) = self.overlays.update_notice_mut() else { return };
        match key.code {
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => n.focus = (n.focus + 1) % 3,
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => n.focus = (n.focus + 2) % 3,
            _ if repeat => {}
            KeyCode::Enter => {
                let a = NoticeAnswer::ALL[n.focus];
                self.update_notice_answer(a);
            }
            KeyCode::Esc | KeyCode::Char('q') => self.update_notice_answer(NoticeAnswer::Close),
            KeyCode::Char('s') => self.update_notice_answer(NoticeAnswer::Skip),
            KeyCode::Char('o') => self.update_notice_answer(NoticeAnswer::TurnOff),
            _ => {}
        }
    }

    /// The notice was answered with `a`.
    pub(super) fn update_notice_answer(&mut self, a: NoticeAnswer) {
        let Some(n) = self.overlays.update_notice() else { return };
        let version = n.version;
        self.overlays.close(OverlayKind::UpdateNotice);
        let now = self.utc_now();
        match a {
            NoticeAnswer::Close => {
                self.change_update_state(|s| s.remind(version, now));
            }
            NoticeAnswer::Skip => {
                // Said only once kept; else why it was not (it would come back at the next launch).
                let msg = match self.change_update_state(|s| s.skip(version)) {
                    Some(_) => Notice::new(Msg::UpdateSkipped { version: version.to_string() }, Level::Info),
                    None if self.read_only => Notice::new(Label::UpdateNotSkippedSecond, Level::Warning),
                    None => {
                        let path = self.paths.update_file().map(|p| p.display().to_string()).unwrap_or_default();
                        Notice::new(Msg::UpdateNotSkipped { path }, Level::Warning)
                    }
                };
                self.flash(msg);
            }
            NoticeAnswer::TurnOff => {
                self.prefs.update_check = UpdateCheck::Off;
                // What saving said (it could not be saved, say) is shown in place of the
                // confirmation: off only for this run must not read as off for good.
                let saved = self.persist();
                self.flash(saved.unwrap_or_else(|| Notice::new(Label::UpdateChecksOff, Level::Info)));
            }
        }
    }

    /// `:update`: the notice when a newer release is known, else what is known, in words.
    pub(super) fn show_update_status(&mut self) {
        let cache = Cache::load(&self.paths);
        let running = Version::running();
        if let Some(latest) = cache.state().and_then(|s| s.latest).filter(|l| *l > running && !l.is_rc()) {
            return self.open_update_notice(latest, &cache);
        }
        let msg = self.update_status(&cache, &running);
        self.flash(Notice::new(msg, Level::Info));
    }

    /// What is known of the update check, in words.
    pub fn update_status(&self, cache: &Cache, running: &Version) -> Msg {
        let mut sw = self.updates.switches.unwrap_or_else(|| Switches::new(true, false, false, !self.read_only));
        // The setting as it is now (`:set` or the settings screen may have changed it).
        sw.config_on = self.prefs.update_check == UpdateCheck::On;
        let off = match update::decide(self.utc_now(), cache, &sw) {
            Decision::Off(o) => Some(o),
            _ => None,
        };
        let label = |l: Label| Msg::Label(l);
        match off {
            Some(Off::Env) => return label(Label::UpdateStatusOffEnv),
            Some(Off::Config) => return label(Label::UpdateStatusOffConfig),
            Some(Off::Ci) => return label(Label::UpdateStatusOffCi),
            Some(Off::DebugBuild) => return label(Label::UpdateStatusOffDebug),
            Some(Off::NoStateDir) => return label(Label::UpdateStatusNoStateDir),
            Some(Off::Unreadable) => {
                let path = self.paths.update_file().map(|p| p.display().to_string()).unwrap_or_default();
                return Msg::UpdateStatusUnreadable { path };
            }
            // A second instance reads what the other one learnt.
            Some(Off::SecondInstance) | None => {}
        }
        let Some(s) = cache.state() else { return label(Label::UpdateStatusNotYet) };
        let Some(at) = s.last_attempt else { return label(Label::UpdateStatusNotYet) };
        let age = self.update_age(at);
        let why = match s.result {
            Some(Outcome::Ok | Outcome::NotModified) => {
                return Msg::UpdateStatusUpToDate { version: running.to_string(), age };
            }
            None if self.updates.running => return label(Label::UpdateStatusChecking),
            None => Label::UpdateWhyUnfinished,
            Some(Outcome::Unreachable) => Label::UpdateWhyUnreachable,
            Some(Outcome::TimedOut) => Label::UpdateWhyTimedOut,
            Some(Outcome::Tls) => Label::UpdateWhyTls,
            Some(Outcome::Proxy) => Label::UpdateWhyProxy,
            Some(Outcome::RateLimited) => Label::UpdateWhyRateLimited,
            Some(Outcome::BadAnswer) => Label::UpdateWhyBadAnswer,
            Some(Outcome::Other) => Label::UpdateWhyOther,
        };
        Msg::UpdateStatusNotChecked { why: self.i18n.label(why).to_string(), age }
    }

    /// How long ago `at` was, in words.
    fn update_age(&self, at: DateTime<Utc>) -> String {
        let millis = (self.utc_now() - at).num_milliseconds().max(0) as u128;
        self.age_text(millis)
    }
}
