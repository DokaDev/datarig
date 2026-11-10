//! The status line and the flashes and texts it shows.

use super::*;

impl App {
    /// Message for the status bar: transient > running > last status.
    /// The shorter forms of the notice the status line shows, longest first (none while it
    /// shows something else, or a notice flashed without them).
    pub fn status_shorter(&self) -> Vec<String> {
        match (&self.transient, &self.transient_shorter) {
            (Some((n, _)), Some((of, forms))) if !self.tab_grabbed() && n.msg == *of => {
                forms.iter().map(|m| self.i18n.msg(m).to_string()).collect()
            }
            _ => Vec::new(),
        }
    }

    pub fn status_line(&mut self) -> Option<(Localized, Level, bool)> {
        // A tab moved by keys: how (no timer: it stays while the move does).
        if self.tab_grabbed() {
            return Some((self.i18n.label(Label::StatusTabMove), Level::Info, false));
        }
        if let Some((_, at)) = &self.transient
            && at.elapsed() >= TRANSIENT
        {
            self.transient = None;
        }
        if let Some((m, _)) = &self.transient {
            return Some((m.render(&self.i18n), m.level, false));
        }
        // An Apply waits for the counts of its rows.
        if let Some(text) = self.apply_wait_text() {
            return Some((Localized::verbatim(text), Level::Info, true));
        }
        // Why the cell editor did not take its text, or why the value it opened with is not
        // valid, while it is open.
        let refused = (!self.tabs.is_empty()).then(|| self.tab().cell_edit.as_ref()).flatten();
        if let Some(e) = refused
            && let Some(reason) = e.error.clone()
        {
            let msg = if e.invalid { Msg::EditNotValid { reason } } else { Msg::EditRefused { reason } };
            return Some((self.i18n.msg(&msg), Level::Info, false));
        }
        // An answer waits for transactions to end (whichever tab is shown).
        if let Some(m) = self.end_pending_status() {
            return Some((m, Level::Info, true));
        }
        if let Some(r) = &self.tab().exec.running {
            let log = &self.tab().exec.run;
            let step = log.running().filter(|_| log.several() && !r.fetch);
            let text = if r.refused {
                self.i18n.label(Label::QueryCancelRefusedRunsOn)
            } else if r.stopping {
                self.i18n.label(Label::QueryCancellingServer)
            } else if r.cancelling.is_some() {
                self.i18n.label(Label::QueryCancelling)
            } else if r.apply {
                self.i18n.label(Label::EditApplying)
            } else if r.export {
                let id = self.tab().id;
                self.export_progress(id).map_or_else(|| self.i18n.label(Label::ExportRunning), |m| self.i18n.msg(&m))
            } else if r.estimate {
                self.i18n.label(Label::ResultsTitleEstimating)
            } else if r.count {
                self.i18n.label(Label::ResultsTitleCounting)
            } else if r.fetch {
                self.i18n.label(Label::QueryFetching)
            } else if let Some(i) = step {
                let (step, total) = ((i + 1).to_string(), log.len().to_string());
                self.i18n.msg(&Msg::QueryRunningStep { step, total, elapsed: r.started.elapsed() })
            } else {
                self.i18n.msg(&Msg::QueryRunning { elapsed: r.started.elapsed() })
            };
            return Some((text, if r.cancelling.is_some() { Level::Warning } else { Level::Info }, true));
        }
        self.status.as_ref().map(|m| (m.render(&self.i18n), m.level, false))
    }

    /// Text of the test-connection state (the profile form's footer).
    pub fn test_status(&self) -> Option<(Localized, Level)> {
        // The form's line says that its save waits for the keychain.
        if self.overlays.form().is_some_and(|f| f.saving) {
            return Some((self.i18n.label(Label::FormSavingKeychain), Level::Info));
        }
        let t = self.conn_test.as_ref()?;
        let m = test_msg(&self.i18n, t);
        Some((m.render(&self.i18n), m.level))
    }

    /// The profile form's test lines through a tunnel: one line per stage,
    /// the SSH hop (its host key, the login and its time) then the database (its result and
    /// time); the stage that failed says why, and one that did not run says so. `None` without a
    /// tunnel (the one line of [`App::test_status`]) or while a save waits.
    pub fn test_lines(&self) -> Option<Vec<(Localized, Level)>> {
        if self.overlays.form().is_some_and(|f| f.saving) {
            return None;
        }
        let t = self.conn_test.as_ref()?;
        let tt = t.tunnel.as_ref()?;
        let host = tt.host.clone();
        let line = |m: Msg, level: Level| (self.i18n.msg(&m), level);
        let whole = || {
            let m = test_msg(&self.i18n, t);
            (m.render(&self.i18n), m.level)
        };
        let ssh = match (&t.state, tt.opened, &tt.failure) {
            (_, Some(elapsed), _) => match &tt.how {
                Some(how) => {
                    let (host_key, login) = self.opened_texts(how, &tt.settings);
                    line(Msg::TestLineSshOpened { host, host_key, login, elapsed }, Level::Success)
                }
                None => line(Msg::TestLineSshOk { host, elapsed }, Level::Success),
            },
            (TestState::Running, None, _) => {
                let stage = self
                    .i18n
                    .label(tunnel::stage_label(tt.stage.unwrap_or(datarig_ssh::tunnel::Stage::Connecting)))
                    .to_string();
                line(Msg::TestLineSshRunning { host, stage, elapsed: t.started.elapsed() }, Level::Info)
            }
            (_, None, Some(failure)) => (Localized::verbatim(failure.clone()), Level::Error),
            // Timed out, cancelled, or a source failed before the tunnel opened.
            (_, None, None) => whole(),
        };
        if t.probe {
            let probe = match (&t.state, tt.opened) {
                (TestState::Running, None) => line(Label::TestLineProbeWaiting.into(), Level::Info),
                (_, None) => line(Label::TestLineProbeNotRun.into(), Level::Warning),
                (TestState::Running, Some(_)) => line(Label::TestLineProbeRunning.into(), Level::Info),
                (TestState::Reached(v), Some(_)) if v.is_empty() => {
                    line(Label::TestLineProbeNone.into(), Level::Success)
                }
                (TestState::Reached(v), Some(_)) => match reached_failures(v) {
                    None => line(Msg::TestLineProbeOk { targets: reached_targets(v) }, Level::Success),
                    Some(error) => line(Msg::TestLineProbeFailed { error }, Level::Error),
                },
                (_, Some(_)) => whole(),
            };
            return Some(vec![ssh, probe]);
        }
        let db = match (&t.state, tt.opened) {
            (TestState::Running, None) => line(Label::TestLineDbWaiting.into(), Level::Info),
            (_, None) => line(Label::TestLineDbNotRun.into(), Level::Warning),
            (TestState::Running, Some(_)) => line(Label::TestLineDbRunning.into(), Level::Info),
            (TestState::Ok(info), Some(_)) => {
                line(Msg::TestLineDbOk { version: info.server_version.clone(), latency: info.latency }, Level::Success)
            }
            (TestState::Failed(error), Some(_)) => line(Msg::TestLineDbFailed { error: error.clone() }, Level::Error),
            (_, Some(_)) => whole(),
        };
        Some(vec![ssh, db])
    }

    /// The host key and login parts of a test's SSH line.
    fn opened_texts(
        &self,
        how: &datarig_ssh::tunnel::Opened,
        settings: &datarig_core::profile::ssh::SshSettings,
    ) -> (String, String) {
        use datarig_ssh::tunnel::Method;
        // The key's type as `ssh-keygen -l` names it (the line has little room).
        let algorithm = match how.host_key.as_str() {
            "ssh-ed25519" => "ED25519".to_string(),
            a if a.starts_with("ecdsa-sha2-") => "ECDSA".to_string(),
            "ssh-rsa" | "rsa-sha2-256" | "rsa-sha2-512" => "RSA".to_string(),
            a => a.to_string(),
        };
        let host_key = match how.trusted_now {
            true => Msg::TestHostKeyTrusted { algorithm },
            false => Msg::TestHostKeyKnown { algorithm },
        };
        let key = how.key.clone().unwrap_or_default();
        let login = match how.method {
            Method::Key => {
                // The file's name: the SSH section shows its path.
                let file = settings.key_file.as_deref().unwrap_or_default();
                let name = Path::new(file).file_name().map_or(file.into(), |n| n.to_string_lossy());
                self.i18n.msg(&Msg::TestLoginKey { file: name.into_owned(), key }).to_string()
            }
            Method::Agent => self.i18n.msg(&Msg::TestLoginAgent { key }).to_string(),
            Method::Password => self.i18n.label(Label::TestLoginPassword).to_string(),
            Method::KeyboardInteractive => self.i18n.label(Label::TestLoginAnswers).to_string(),
        };
        (self.i18n.msg(&host_key).to_string(), login)
    }

    pub(super) fn flash(&mut self, m: Notice) {
        self.transient = Some((m, Instant::now()));
    }

    /// [`App::flash`] `m`, with forms of it for a status bar too narrow for it, longest first.
    pub(super) fn flash_shorter(&mut self, m: Notice, shorter: Vec<Msg>) {
        self.transient_shorter = Some((m.msg.clone(), shorter));
        self.flash(m);
    }

    /// Show `m` in the status bar, unless the active tab's own warning is there and `m` is less
    /// severe: then the warning stays (it says what happened to that tab's work, e.g. a queued
    /// statement that was dropped) and `m` only flashes for a moment.
    pub(super) fn show_status(&mut self, m: Notice) {
        let held = |n: &Notice| n.level as u8 >= Level::Warning as u8 && n.level as u8 > m.level as u8;
        let tab_warning = self.tab().status.as_ref().filter(|t| held(t));
        let keep = match (tab_warning, &self.status) {
            (Some(t), Some(s)) => t.msg == s.msg,
            _ => false,
        };
        if keep {
            self.flash(m);
        } else {
            self.status = Some(m);
        }
    }
}
