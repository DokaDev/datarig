//! Per-profile password sources in the app: resolving a
//! profile's password for connecting and testing, the prompt, the keychain probe, saving the
//! profile form (moving a stored password when its source changes, after a confirmation) and
//! the `secrets.default_source` setting.
//!
//! The secrets file and the environment answer at once and are read on the UI thread. A
//! password command may run for seconds, so with an event loop it runs on a blocking task and
//! reports back as [`AppEvent::Resolved`] (connect) or inside the test-connection task. The OS
//! keychain is never called on the UI thread ([`super::keychain`]). Headless tests
//! (no event sender) run both inline.

use super::keychain::AfterSave;
use super::persist::fault_reason;
use super::*;
use crate::app::profiles::source_name;
use datarig_core::fault::{ErrorLog, Fault};
use datarig_core::secret::command::{self, CommandError};
use datarig_core::secret::file::FileError;
use datarig_core::secret::source::{self, SwitchError, Switched};
use datarig_core::secret::{Lookup, PasswordSource, Unavailable};

/// A profile's stored password as the UI thread knows it ([`App::stored_password`]).
pub(super) enum Stored {
    Known(String),
    /// In the keychain: to read on a worker.
    Keychain,
    /// The store could not be read (e.g. an insecure secrets file).
    Unreadable,
}

/// Environment variable lookup of the `env` source.
pub type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The process environment.
pub(super) fn process_env() -> EnvLookup {
    Arc::new(|k| std::env::var(k).ok())
}

/// A password in flight between tasks. `Debug` never prints it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// What the password prompt's `Enter` does.
#[derive(Debug)]
pub enum PromptPurpose {
    /// Connect to the profile.
    Connect,
    /// Test this profile (the form's or the selected one) with the typed password.
    Test(Box<ConnectionConfig>),
    /// A secret an SSH tunnel waits for: the first of `App::secret_waits`.
    Tunnel,
}

/// How a profile's password is obtained for this attempt.
pub(super) enum Plan {
    Ready(String),
    /// Run this password command (off the UI thread when there is an event loop).
    Command(String),
    /// Read the keychain entry of `account` (then `legacy`, the profile's name in a version 1
    /// config) off the UI thread.
    Keychain {
        account: String,
        legacy: Option<String>,
    },
    /// Ask for it (`prompt` source).
    Prompt,
    Failed(SourceError),
}

/// The message of a failed source; every message names the source. A fault inside is said
/// as a short reason in `i18n`'s language ([`fault_reason`]).
pub fn source_error_msg(i18n: &I18n, e: &SourceError) -> Msg {
    let reason = |f: &Fault| i18n.msg(&fault_reason(f)).to_string();
    match e {
        SourceError::Keychain(f) => Msg::SecretErrorKeychain { error: reason(f) },
        SourceError::File(FileError::Insecure { path, mode }) => {
            Msg::SecretErrorFileInsecure { path: path.display().to_string(), mode: format!("{mode:03o}") }
        }
        SourceError::File(FileError::Io(f)) => Msg::SecretErrorFile { error: reason(f) },
        SourceError::Command(c) => match c {
            CommandError::Empty => Label::SecretErrorCommandEmpty.into(),
            CommandError::Parse => Label::SecretErrorCommandParse.into(),
            CommandError::Start { program, error } => {
                Msg::SecretErrorCommandStart { program: program.clone(), error: reason(error) }
            }
            CommandError::Timeout(elapsed) => Msg::SecretErrorCommandTimeout { elapsed: *elapsed },
            CommandError::Failed { status, stderr } if stderr.is_empty() => {
                Msg::SecretErrorCommandFailedQuiet { status: status.clone() }
            }
            CommandError::Failed { status, stderr } => {
                Msg::SecretErrorCommandFailed { status: status.clone(), stderr: stderr.clone() }
            }
            CommandError::NoOutput => Label::SecretErrorCommandNoOutput.into(),
            CommandError::NotUtf8 => Label::SecretErrorCommandNotUtf8.into(),
        },
        SourceError::EnvMissing(name) => Msg::SecretErrorEnvMissing { name: name.clone() },
    }
}

impl App {
    /// [`source_error_msg`] in the UI language; the raw detail of a fault inside goes to the
    /// error log.
    pub(super) fn source_error(&self, e: &SourceError) -> Msg {
        let log = ErrorLog::new(self.paths.errors_log());
        match e {
            SourceError::Keychain(f) | SourceError::File(FileError::Io(f)) => log.record("secret.error", f),
            SourceError::Command(CommandError::Start { error, .. }) => log.record("secret.error.command_start", error),
            _ => {}
        }
        source_error_msg(&self.i18n, e)
    }

    /// The source name in the current language ("OS keychain", …) for messages.
    pub(super) fn source_text(&self, kind: SourceKind) -> String {
        self.i18n.label(source_name(kind)).to_string()
    }

    /// The keychain works (as far as known: the launch probe or a failed call says otherwise).
    pub fn keychain_ok(&self) -> bool {
        self.secrets.unavailable.is_none()
    }

    /// Check once at launch whether the keychain works, so a new profile does not start on a
    /// keychain that is not there (`secrets.default_source = auto`). Off the UI thread when
    /// there is an event loop (the OS may take its time).
    pub(super) fn probe_keychain(&mut self) {
        let store = self.secrets.store();
        let probe = move || store.get("profile:probe").err().map(|Unavailable(e)| e);
        match self.tx.clone() {
            Some(tx) => {
                tokio::task::spawn_blocking(move || {
                    let _ = tx.send(AppEvent::KeychainProbed(probe()));
                });
            }
            None => self.on_keychain_probed(probe()),
        }
    }

    pub(super) fn on_keychain_probed(&mut self, r: Option<Fault>) {
        let Some(reason) = r else { return };
        ErrorLog::new(self.paths.errors_log()).record("secret.unavailable", &reason);
        self.secrets.unavailable = Some(reason);
        let default = self.default_source.resolve(false);
        // A new profile form that opened before the answer came must not keep the keychain.
        if let Some(f) = self.overlays.form_mut() {
            f.keychain_ok = false;
            if f.editing.is_none() && f.source == SourceKind::Keychain {
                f.source = default;
            }
        }
    }

    /// The new-profile form, its storage preselected by `secrets.default_source`.
    pub(super) fn new_profile_form(&self) -> ProfileForm {
        let ok = self.keychain_ok();
        ProfileForm::new_with_source(self.default_source.resolve(ok), ok)
    }

    /// The password stored for `c` to fill the form or connect: typed at the prompt this
    /// session > legacy plaintext > the store. The keychain is never read here (it may not
    /// answer): [`Stored::Keychain`] says to read it on a worker.
    pub(super) fn stored_password(&mut self, c: &ConnectionConfig) -> Stored {
        let account = c.id.account();
        if let Some(p) = self.secrets.session(&account) {
            return Stored::Known(p.to_string());
        }
        if !c.password.is_empty() {
            return Stored::Known(c.password.clone());
        }
        match c.source().kind() {
            SourceKind::Keychain => Stored::Keychain,
            SourceKind::File => {
                let stores = self.secrets.stores();
                if stores.check_file().is_err() {
                    return Stored::Unreadable;
                }
                stores.file.get(&account).map_or(Stored::Unreadable, |p| Stored::Known(p.unwrap_or_default()))
            }
            _ => Stored::Known(String::new()),
        }
    }

    /// How to get `c`'s password for connecting or testing. A missing stored password is an
    /// empty one: the server then says whether it needs one, and the prompt opens.
    pub(super) fn plan_password(&mut self, c: &ConnectionConfig) -> Plan {
        // Typed at the prompt of a `prompt` profile: for this one attempt only.
        if self.prompted.as_ref().is_some_and(|(id, _)| *id == c.id)
            && let Some((_, Secret(pw))) = self.prompted.take()
        {
            return Plan::Ready(pw);
        }
        let source = c.source();
        match source {
            PasswordSource::Keychain => match self.stored_password(c) {
                Stored::Known(pw) => Plan::Ready(pw),
                _ => Plan::keychain(c, self.config_version < config::CONFIG_VERSION),
            },
            PasswordSource::File => {
                if let Some(p) = self.secrets.session(&c.id.account()) {
                    return Plan::Ready(p.to_string());
                }
                if !c.password.is_empty() {
                    return Plan::Ready(c.password.clone());
                }
                let env = self.env.clone();
                match source::lookup(&source, &c.id.account(), self.secrets.stores(), &*env, command::TIMEOUT) {
                    Ok(Lookup::Found(p)) => Plan::Ready(p),
                    Ok(_) => Plan::Ready(String::new()),
                    Err(e) => Plan::Failed(e),
                }
            }
            PasswordSource::Command(cmd) => Plan::Command(cmd),
            PasswordSource::Env(_) => {
                let env = self.env.clone();
                match source::lookup(&source, &c.id.account(), self.secrets.stores(), &*env, command::TIMEOUT) {
                    Ok(Lookup::Found(p)) => Plan::Ready(p),
                    Ok(_) => Plan::Ready(String::new()),
                    Err(e) => Plan::Failed(e),
                }
            }
            PasswordSource::Prompt => Plan::Prompt,
        }
    }

    /// Open the password prompt for `c`. `purpose` says what `Enter` does; the checkbox saves
    /// to the profile's store (keychain when it works, secrets file), never for `prompt`. While
    /// another profile's prompt is open, a connect prompt waits for it (one at a time).
    pub(super) fn open_prompt(&mut self, c: &ConnectionConfig, error: Notice, purpose: PromptPurpose) {
        if matches!(purpose, PromptPurpose::Connect)
            && self.overlays.prompt().is_some_and(|p| {
                (p.id != c.id && matches!(p.purpose, PromptPurpose::Connect))
                    || matches!(p.purpose, PromptPurpose::Tunnel)
            })
        {
            self.prompt_queue.retain(|(id, _)| *id != c.id);
            self.prompt_queue.push_back((c.id, error));
            return;
        }
        let kind = c.source().kind();
        let testing = matches!(purpose, PromptPurpose::Test(_));
        // A keychain that did not answer or is locked may work later: saving there is offered,
        // never checked for the user.
        let (save_to, note, save) = match kind {
            _ if testing => (None, Label::PromptPasswordNotSaved, false),
            SourceKind::Keychain if self.keychain_ok() => (Some(SourceKind::Keychain), Label::PromptPasswordSave, true),
            SourceKind::Keychain if self.keychain_unsure() => {
                (Some(SourceKind::Keychain), Label::PromptPasswordSaveUnsure, false)
            }
            SourceKind::Keychain => (None, Label::PromptPasswordSaveUnavailable, false),
            SourceKind::File => (Some(SourceKind::File), Label::PromptPasswordSaveFile, true),
            _ => (None, Label::PromptPasswordNotSaved, false),
        };
        self.overlays.push(Overlay::Password(PasswordPrompt {
            id: c.id,
            profile: c.name.clone(),
            input: TextInput::default(),
            error,
            save,
            save_focus: false,
            save_to,
            note,
            purpose,
            title: None,
            field: Label::FormFieldPassword.into(),
            echo: false,
            checkbox: Default::default(),
            buttons: Default::default(),
        }));
    }

    /// The next waiting password prompt, if its profile still needs it; else a secret an SSH
    /// tunnel waits for.
    fn next_prompt(&mut self) {
        self.next_db_prompt();
        if self.overlays.prompt().is_none() {
            self.next_secret_wait();
        }
    }

    fn next_db_prompt(&mut self) {
        while let Some((id, error)) = self.prompt_queue.pop_front() {
            if let Some(c) = self.profile(id).cloned()
                && !self.conns.is_connected(id)
            {
                return self.open_prompt(&c, error, PromptPurpose::Connect);
            }
        }
    }

    /// `Esc` in the password prompt: nothing connects; what waited for that attempt is
    /// dropped, the profile's node is back to "not connected" and the status bar says the
    /// connection was cancelled.
    pub(super) fn prompt_closed(&mut self) {
        let Some(p) = self.overlays.take_prompt() else { return };
        if matches!(p.purpose, PromptPurpose::Tunnel) {
            self.tunnel_prompt_answered(None);
            return self.next_prompt();
        }
        if matches!(p.purpose, PromptPurpose::Connect) {
            // Late events of the attempt that asked (an auth failure) are dropped.
            self.conns.next_generation(p.id);
            let c = self.conns.entry(p.id);
            c.console = None;
            c.expand_on_connect = false;
            c.connecting = None;
            c.error = None;
            self.drop_pending(p.id);
            self.save_on_connect = self.save_on_connect.take().filter(|(id, _)| *id != p.id);
            self.status = Some(Notice::new(Msg::ConnCancelled { name: p.profile.clone() }, Level::Warning));
        }
        self.next_prompt();
    }

    /// `Enter` in the password prompt.
    pub(super) fn prompt_entered(&mut self) {
        let Some(p) = self.overlays.take_prompt() else { return };
        let (id, pw, save, save_to) = (p.id, p.input.text().to_string(), p.save, p.save_to);
        match p.purpose {
            PromptPurpose::Tunnel => self.tunnel_prompt_answered(Some((pw, save, save_to))),
            PromptPurpose::Test(mut cfg) => {
                cfg.password = pw;
                self.start_test(*cfg);
            }
            PromptPurpose::Connect => {
                match save_to {
                    // A profile that stores its password: kept for this session, written to
                    // its store only once it connects.
                    Some(kind) => {
                        self.secrets.remember(&id.account(), &pw);
                        self.save_on_connect = save.then_some((id, kind));
                    }
                    None if self.profiles.iter().any(|c| c.id == id && c.source().kind() == SourceKind::Prompt) => {
                        self.prompted = Some((id, Secret(pw)));
                    }
                    // Keychain unavailable: this session only.
                    None => self.secrets.remember(&id.account(), &pw),
                }
                self.connect(id);
                self.conns.entry(id).prompted = true;
            }
        }
        self.next_prompt();
    }

    /// A password command finished for profile `id`'s attempt in progress.
    pub(super) fn on_resolved(&mut self, id: ProfileId, result: Result<Secret, SourceError>) {
        if self.conns.get(id).is_none_or(|c| c.connecting.is_none()) {
            return;
        }
        let Some(conn) = self.profile(id).cloned() else { return };
        match result {
            Ok(Secret(pw)) => self.start_connect(conn, pw),
            Err(e) => self.source_failed(id, e),
        }
    }

    /// The password source of profile `id` failed: nothing was sent to the server. The
    /// profile's node and the status bar say which source failed and why.
    pub(super) fn source_failed(&mut self, id: ProfileId, e: SourceError) {
        self.close_profile_sessions(id);
        let m = Notice::new(self.source_error(&e), Level::Error);
        self.attempt_failed(id, m.clone());
        self.status = Some(m);
    }

    pub(super) fn on_test_source(&mut self, seq: u64, error: SourceError) {
        let Some(t) = self.conn_test.as_mut() else { return };
        if t.seq != seq || t.state != TestState::Running {
            return;
        }
        t.abort = None;
        t.state = TestState::Source(error);
        self.report_test();
    }

    /// Test `c` with its source's password: stored and environment passwords at once, a command
    /// inside the test task, `prompt` after asking.
    pub(super) fn test_with_source(&mut self, mut c: ConnectionConfig) {
        match self.plan_password(&c) {
            Plan::Ready(pw) => {
                c.password = pw;
                self.start_test(c);
            }
            Plan::Command(cmd) => self.start_test_with_command(c, cmd),
            Plan::Keychain { account, legacy } => self.test_with_keychain(c, account, legacy),
            Plan::Prompt => {
                let every = Notice::new(Label::PromptPasswordEveryTime, Level::Info);
                self.open_prompt(&c.clone(), every, PromptPurpose::Test(Box::new(c)));
            }
            Plan::Failed(e) => {
                self.start_test(c);
                if let Some(t) = self.conn_test.as_mut() {
                    t.state = TestState::Source(e);
                }
                self.report_test();
            }
        }
    }

    // ── saving the form ─────────────────────────────────────────────────────

    /// `Ctrl+S` in the form. A profile whose stored password would move or be removed (its
    /// source changes from the keychain or the secrets file) asks first.
    pub(super) fn save_form(&mut self) {
        if self.overlays.form().is_some_and(ProfileForm::is_tunnel) {
            return self.save_tunnel_form();
        }
        let Some(form) = self.overlays.form_mut().filter(|f| !f.saving) else { return };
        form.attempted = true;
        let editing = form.editing;
        let profiles = &self.profiles;
        let taken = |n: &str| profiles.iter().enumerate().any(|(i, p)| p.name == n && Some(i) != editing);
        if !form.errors(taken).is_empty() || form.dsn_problem.is_some() {
            return;
        }
        // One move at a time: the database password's (a change of its storage), then (another
        // save) the tunnel secret's into a preset made of the profile's own tunnel.
        if form.made_preset().is_some() && form.original_source().is_some_and(|from| from != form.source) {
            return self.flash(Notice::new(Label::TunnelSaveAsWithSourceChange, Level::Warning));
        }
        // A preset of the same name made since the form opened ("save as tunnel preset").
        if let Some(name) = form.made_preset().map(|p| p.name)
            && self.presets.iter().any(|p| datarig_core::profile::tunnel::same_name(&p.name, &name))
        {
            return self.flash(Notice::new(Msg::TunnelNameTaken { name }, Level::Warning));
        }
        // The tunnel's secret is written (or moved out of a store it left) once the profile is
        // saved; made into a preset, it moves to the preset (`apply_form`).
        let (id, typed, new) = (form.profile_id(), form.ssh_typed_secret(), form.to_profile().ssh);
        let old = editing.and_then(|i| self.profiles.get(i)).and_then(|p| p.ssh.clone());
        self.pending_tunnel_secret =
            (!form.new_preset_picked()).then_some(super::tunnel::FormSecret { id, old, new, typed });
        let Some(form) = self.overlays.form() else { return };
        let (from, to) = (form.original_source(), form.source);
        match from {
            Some(from) if from != to && from.stores_secret() => {
                let name = form.to_profile().name;
                let (from_t, to_t) = (self.source_text(from), self.source_text(to));
                let text = if to.stores_secret() {
                    Msg::SourceChangeMove { name, from: from_t, to: to_t }
                } else {
                    Msg::SourceChangeRemove { name, from: from_t, to: to_t }
                };
                self.pending_save = true;
                self.overlays.push(Overlay::Confirm(Confirm {
                    title: Label::SourceChangeTitle,
                    text,
                    details: Vec::new(),
                    keys: Label::SourceChangeKeys,
                    action: ConfirmAction::ChangeSource,
                    folder: None,
                    path: None,
                    buttons: Default::default(),
                }));
            }
            _ => self.apply_form(),
        }
    }

    /// The source change was confirmed (`yes`) or not.
    pub(super) fn source_change_answered(&mut self, yes: bool) {
        if std::mem::take(&mut self.pending_save) && yes {
            self.apply_form();
        } else {
            self.pending_tunnel_secret = None;
        }
    }

    /// Save the form: the profile, and its password in its store. A changed source goes through
    /// [`source::switch`] (new copy written and checked, config saved, then the old copy
    /// removed); on failure the form stays open and nothing is removed.
    fn apply_form(&mut self) {
        let Some(form) = self.overlays.form() else { return };
        let editing = form.editing;
        let mut p = form.to_profile();
        // "Save as tunnel preset": the preset is saved with the profile (one write), then the
        // profile's tunnel secret moves to it, from the store it is in as saved (the form may
        // have changed it) to the one the preset keeps it in.
        let made = form.made_preset();
        let store = |s: &datarig_core::profile::ssh::SshSettings| {
            Some(s.source().kind()).filter(|k| s.auth.has_secret() && k.stores_secret())
        };
        let secret_move = made.as_ref().map(|t| super::presets::SecretMove {
            name: t.name.clone(),
            from_kind: editing.and_then(|i| self.profiles[i].ssh.as_ref()).and_then(store),
            to_kind: store(&t.settings),
            from: datarig_core::profile::ssh::SshSettings::account(p.id),
            to: t.id.account(),
            typed: form.typed_bastion_secret(),
        });
        let pw = form.password.text().to_string();
        let keep_unread = pw.is_empty() && form.password_unread;
        let from = form.original_source().unwrap_or(form.source);
        let to = form.source;
        let account = p.id.account();
        if editing.is_some() && from != to {
            // Refused with a preset to make (`save_form`): never both in one save.
            if made.is_some() {
                return;
            }
            return self.apply_source_change(editing, p, from, to, &pw);
        }
        let mut warn = None;
        // The keychain's part runs on a worker once the profile is saved.
        let mut keychain: Option<Option<(String, bool)>> = None;
        if to == SourceKind::Keychain && !keep_unread {
            let plaintext = editing.map(|i| self.profiles[i].password.clone()).filter(|p| !p.is_empty());
            if pw.is_empty() {
                keychain = self.keychain_ok().then_some(None);
            } else {
                // A plaintext password in the file stays there until the keychain has this one.
                if let Some(old) = &plaintext {
                    p.password = old.clone();
                }
                keychain = Some(Some((pw.clone(), plaintext.is_some())));
            }
        } else if to.stores_secret() && !keep_unread {
            let had_plaintext = editing.is_some_and(|i| !self.profiles[i].password.is_empty());
            if pw.is_empty() {
                if to != SourceKind::Keychain || self.keychain_ok() {
                    let _ = self.secrets.remove_in(to, &account);
                }
            } else if let Err(e) = self.secrets.save_in(to, &account, &pw) {
                let e = self.source_error(&e);
                // Not stored: remembered for this session only. A password that was already
                // plaintext in the file stays there rather than being lost.
                if had_plaintext {
                    p.password = pw.clone();
                }
                warn = Some(e);
            }
        } else if keep_unread && editing.is_some_and(|i| !self.profiles[i].password.is_empty()) {
            p.password = editing.map(|i| self.profiles[i].password.clone()).unwrap_or_default();
        }
        let before = editing.map(|i| self.profiles[i].clone());
        if let Some(t) = made.clone() {
            self.presets.push(t);
            self.presets.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        }
        let idx = self.put_profile(editing, p.clone());
        let saved = match &made {
            None => self.persist(),
            // The preset and the profile naming it go together, or not at all: the file keeps
            // the profile's own tunnel (and its secret stays where it is); the form stays open.
            Some(t) => match self.try_persist() {
                Ok(n) => n,
                Err(fault) => {
                    self.presets.retain(|q| q.id != t.id);
                    match before {
                        Some(b) => {
                            let id = b.id;
                            self.profiles[idx] = b;
                            self.sync_profile_tabs(id);
                        }
                        None => {
                            self.profiles.remove(idx);
                        }
                    }
                    let error = self.fault_text("config.save_failed", &fault);
                    return self.flash(Notice::new(Msg::ConfigSaveFailed { error }, Level::Error));
                }
            },
        };
        self.form_saved(idx);
        let msg = match (warn, saved) {
            (Some(e), _) => Notice::new(e, Level::Warning),
            (None, Some(m)) => m,
            (None, None) => Notice::new(Msg::ProfilesSaved { name: p.name.clone() }, Level::Success),
        };
        self.flash(msg);
        if let Some(m) = secret_move {
            self.move_secret_to_preset(m);
        }
        match keychain {
            Some(Some((pw, plaintext))) => self.save_to_keychain(&p, pw, AfterSave::Form { plaintext }),
            Some(None) => {
                self.secrets.forget(&account);
                self.remove_from_keychain(vec![account]);
            }
            None => {}
        }
    }

    fn apply_source_change(
        &mut self,
        editing: Option<usize>,
        p: ConnectionConfig,
        from: SourceKind,
        to: SourceKind,
        pw: &str,
    ) {
        let Some(i) = editing else { return };
        if from == SourceKind::Keychain || to == SourceKind::Keychain {
            return self.switch_with_keychain(p, from, to, pw.to_string());
        }
        let old = self.profiles[i].clone();
        let stores = self.secrets.stores().clone();
        let typed = Some(pw).filter(|t| !t.is_empty());
        let result = source::switch(&stores, from, to, &p.id.account(), typed, || {
            self.put_profile(editing, p.clone());
            match self.try_persist() {
                Err(fault) => {
                    self.put_profile(editing, old.clone());
                    Err(fault)
                }
                Ok(_) => Ok(()),
            }
        });
        let to_t = self.source_text(to);
        let msg = match result {
            Ok(done) => self.switched_msg(p.name.clone(), from, to, done),
            Err(e) => {
                let error = match e {
                    SwitchError::Read(e) | SwitchError::Write(e) => {
                        let m = self.source_error(&e);
                        self.i18n.msg(&m).to_string()
                    }
                    SwitchError::Mismatch(_) => {
                        return self.flash(Notice::new(Msg::SourceChangeMismatch { to: to_t }, Level::Error));
                    }
                    SwitchError::Config(f) => self.fault_text("source.change.failed", &f),
                };
                return self.flash(Notice::new(Msg::SourceChangeFailed { error }, Level::Error));
            }
        };
        self.secrets.forget(&p.id.account());
        self.form_saved(i);
        self.flash(msg);
    }

    /// What a finished source change of profile `name` says.
    pub(super) fn switched_msg(&self, name: String, from: SourceKind, to: SourceKind, done: Switched) -> Notice {
        let (from_t, to_t) = (self.source_text(from), self.source_text(to));
        match done {
            Switched { remove_failed: Some(e), .. } => {
                let error = self.source_error(&e);
                let error = self.i18n.msg(&error).to_string();
                Notice::new(Msg::SourceChangeRemoveFailed { name, from: from_t, error }, Level::Warning)
            }
            Switched { copied: true, .. } => Notice::new(Msg::SourceChangeMoved { name, to: to_t }, Level::Success),
            Switched { removed: true, .. } => {
                Notice::new(Msg::SourceChangeRemoved { name, from: from_t }, Level::Success)
            }
            _ => Notice::new(Msg::ProfilesSaved { name }, Level::Success),
        }
    }

    /// Put the saved profile in the list (and in the active connection); its index.
    pub(super) fn put_profile(&mut self, editing: Option<usize>, p: ConnectionConfig) -> usize {
        let idx = match editing {
            Some(i) => {
                self.profiles[i] = p.clone();
                i
            }
            None => {
                self.profiles.push(p.clone());
                self.profiles.len() - 1
            }
        };
        if let Some(f) = p.folder_path() {
            self.folders.insert(&f);
        }
        self.sync_profile_tabs(p.id);
        idx
    }

    /// The tabs on profile `id` follow its driver's language (its driver may have changed).
    pub(super) fn sync_profile_tabs(&mut self, id: ProfileId) {
        let tabs: Vec<TabId> = self.tabs.iter().filter(|t| t.profile == Some(id)).map(|t| t.id).collect();
        for t in tabs {
            self.sync_tab_language(t);
        }
    }

    /// Close the form; the explorer's cursor goes to the saved profile.
    pub(super) fn form_saved(&mut self, idx: usize) {
        if let Some(s) = self.pending_tunnel_secret.take() {
            self.apply_tunnel_secret(s);
        }
        self.clear_test();
        self.overlays.close(OverlayKind::ProfileForm);
        if let Some(id) = self.profiles.get(idx).map(|p| p.id) {
            self.reveal_profile(id);
        }
    }

    // ── settings ────────────────────────────────────────────────────────────

    pub(super) fn set_default_source(&mut self, d: DefaultSource) {
        self.default_source = d;
        let saved = self.persist();
        let source = match d {
            DefaultSource::Auto => self.i18n.label(Label::ActionSecretsDefaultAuto).to_string(),
            DefaultSource::Kind(k) => self.source_text(k),
        };
        self.flash(saved.unwrap_or(Notice::new(Msg::SecretsDefaultSet { source }, Level::Info)));
    }
}
