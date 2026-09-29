//! The OS keychain off the UI thread.
//!
//! Over SSH the login keychain is often locked, and macOS may hold a keychain call while it
//! asks on the Mac's own screen, which the remote user never sees. So the app never calls the
//! keychain on its own thread: reading a password to connect or test, filling the profile form,
//! writing or removing a password and moving one when a profile's source changes all run on a
//! worker ([`App::keychain_job`]) and come back as [`AppEvent::Keychain`]. The keychain store
//! itself is [`Guarded`]: each call ends within [`App::set_keychain_timeout`] (10 s), and one
//! that did not answer is a fault of its own kind. Unknown is not absent: a keychain that did
//! not answer (or failed) never counts as "no password stored"; the password is asked for
//! instead, for this connect, and kept for the session.
//!
//! Answers are bound to what asked: a connect to the profile and its attempt's generation, a
//! test to its sequence number, the form's password to the profile the open form reads.
//!
//! The calls of one keychain account run one at a time, in the order they were asked for
//! ([`KeychainQueue`]): a profile deleted while its password is being written loses
//! the entry after the write, not before it.

use super::password::{Plan, Secret};
use super::*;
use datarig_core::fault::{ErrorLog, Fault, FaultKind, KeychainFault};
use datarig_core::secret::source::{self, SwitchError, Switched};
use datarig_core::secret::{Guarded, Unavailable};
use std::collections::HashMap;
use std::sync::{Condvar, Mutex};

/// The keychain calls waiting for their turn, per account. A job takes its place in the queue
/// of each account it touches when it is asked for (on the UI thread: a short lock, never held
/// during a call), and on its worker waits until it is first in all of them. A job waits only
/// for jobs asked for earlier, each of which ends within the keychain's limit, so none waits
/// forever.
#[derive(Clone, Default)]
pub(super) struct KeychainQueue(Arc<(Mutex<Queues>, Condvar)>);

#[derive(Default)]
struct Queues {
    next: u64,
    waiting: HashMap<String, VecDeque<u64>>,
}

/// A job's place in the queues of its accounts; dropped (also by a panic), it leaves them.
struct Turn {
    queue: KeychainQueue,
    ticket: u64,
    accounts: Vec<String>,
}

impl KeychainQueue {
    /// Take a place for a job on `accounts`.
    fn enter(&self, accounts: Vec<String>) -> Turn {
        let mut q = self.0.0.lock().unwrap_or_else(|e| e.into_inner());
        q.next += 1;
        let ticket = q.next;
        for a in &accounts {
            q.waiting.entry(a.clone()).or_default().push_back(ticket);
        }
        Turn { queue: self.clone(), ticket, accounts }
    }
}

impl Turn {
    /// Wait until every job asked for earlier on the same accounts has ended.
    fn wait(&self) {
        let (lock, turn) = &*self.queue.0;
        let q = lock.lock().unwrap_or_else(|e| e.into_inner());
        let first =
            |q: &Queues| self.accounts.iter().all(|a| q.waiting.get(a).and_then(|w| w.front()) == Some(&self.ticket));
        drop(turn.wait_while(q, |q| !first(q)).unwrap_or_else(|e| e.into_inner()));
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let (lock, turn) = &*self.queue.0;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        for a in &self.accounts {
            if let Some(w) = q.waiting.get_mut(a) {
                w.retain(|t| *t != self.ticket);
                if w.is_empty() {
                    q.waiting.remove(a);
                }
            }
        }
        turn.notify_all();
    }
}

/// A keychain call made on a worker, with its answer.
#[derive(Debug)]
pub enum KeychainDone {
    /// The stored password of profile `profile`'s connection attempt `generation`.
    Connect { profile: ProfileId, generation: u64, result: Result<Option<Secret>, Fault> },
    /// The stored password of test connection `seq`, which tests `cfg`.
    Test { seq: u64, cfg: Box<ConnectionConfig>, result: Result<Option<Secret>, Fault> },
    /// The stored password of `profile`, for the profile form that reads it.
    FormPassword { profile: ProfileId, result: Result<Option<Secret>, Fault> },
    /// `secret` was written for `profile` (named `name`) under `account`; `after` says who
    /// asked.
    Saved {
        profile: ProfileId,
        name: String,
        account: String,
        secret: Secret,
        after: AfterSave,
        result: Result<(), Fault>,
    },
    /// A password was removed (only a failure matters: the keychain is then known not to work).
    Removed(Result<bool, Fault>),
    /// Step 1 of a source change of the profile form's `profile` (the profile as it will be
    /// saved): the password written to the new store and read back.
    SwitchCopied { profile: Box<ConnectionConfig>, from: SourceKind, to: SourceKind, result: Result<bool, SwitchError> },
    /// Step 3 of the source change of profile `name`: the old copy removed.
    SwitchRemoved { name: String, from: SourceKind, to: SourceKind, copied: bool, out: Switched },
}

/// Who wrote a password to the keychain.
#[derive(Debug)]
pub enum AfterSave {
    /// A secret typed for the profile's SSH tunnel, once the tunnel opened.
    Tunnel,
    /// The prompt's "save" checkbox, once the profile connected.
    Connected,
    /// The profile form. `plaintext`: the profile had a plaintext password in the config file,
    /// which stays there until the keychain has the new one.
    Form { plaintext: bool },
}

/// `account`'s password in the keychain, then (a version 1 config) the one stored under the
/// profile's name.
pub(super) fn read_keychain(stores: &Stores, account: &str, legacy: Option<&str>) -> Result<Option<Secret>, Fault> {
    let get = |a: &str| stores.keychain.get(a).map(|p| p.map(Secret)).map_err(|Unavailable(e)| e);
    match (get(account)?, legacy) {
        (None, Some(name)) => get(name),
        (found, _) => Ok(found),
    }
}

/// The accounts [`read_keychain`] reads.
fn read_accounts(account: &str, legacy: Option<&str>) -> Vec<String> {
    std::iter::once(account).chain(legacy).map(str::to_string).collect()
}

/// The system has no keychain (or none datarig can use): nothing is stored there, so a read
/// that failed this way is "nothing stored", not unknown.
fn no_keychain(fault: &Fault) -> bool {
    fault.kind == FaultKind::Keychain(KeychainFault::NoStore)
}

/// The keychain `store` behind a guard that ends each call within `timeout`.
pub(super) fn guarded(store: Arc<dyn SecretStore>, timeout: Duration) -> Arc<dyn SecretStore> {
    Arc::new(Guarded::new(store, timeout))
}

impl App {
    /// Keychain calls end within `timeout` (tests make it short). Call it before
    /// [`App::set_secret_store`] or [`App::set_secret_stores`].
    pub fn set_keychain_timeout(&mut self, timeout: Duration) {
        self.keychain_timeout = timeout;
    }

    /// Run `work` with the stores on a worker, after the jobs asked for earlier on the same
    /// keychain `accounts`, and hand its answer to the app as `done(answer)`. Headless (no
    /// event loop) it runs here, at once.
    pub(super) fn keychain_job<T: Send + 'static>(
        &mut self,
        accounts: Vec<String>,
        work: impl FnOnce(&Stores) -> T + Send + 'static,
        done: impl FnOnce(T) -> KeychainDone + Send + 'static,
    ) {
        let stores = self.secrets.stores().clone();
        let turn = self.keychain_queue.enter(accounts);
        match self.tx.clone() {
            // A blocking task, not a thread of its own: every keychain call in it (and in each
            // job it waits for) ends within the guard's limit.
            Some(tx) => {
                tokio::task::spawn_blocking(move || {
                    turn.wait();
                    let answer = done(work(&stores));
                    drop(turn);
                    let _ = tx.send(AppEvent::Keychain(answer));
                });
            }
            // Every job before this one ran at once too: its turn is now.
            None => {
                let ev = done(work(&stores));
                drop(turn);
                self.on_keychain_done(ev);
            }
        }
    }

    /// A session over SSH (the keychain may be locked, its dialogs out of sight).
    pub(super) fn remote_session(&self) -> bool {
        crate::clipboard::in_ssh(|k| (self.env)(k))
    }

    /// The keychain failed (or did not answer): known not to work for now; the reason goes to
    /// the error log.
    pub(super) fn keychain_failed(&mut self, fault: &Fault) {
        ErrorLog::new(self.paths.errors_log()).record("secret.unavailable", fault);
        self.secrets.unavailable = Some(fault.clone());
    }

    /// The keychain answered: if it was only known not to answer, it works again.
    fn keychain_answered(&mut self) {
        if self.secrets.unavailable.as_ref().is_some_and(|f| f.kind == FaultKind::Keychain(KeychainFault::NoAnswer)) {
            self.secrets.unavailable = None;
        }
    }

    /// The keychain did not work at the last try in a way the user may fix (it did not answer,
    /// or it is locked): the prompt offers to save there, unchecked.
    pub(super) fn keychain_unsure(&self) -> bool {
        matches!(
            self.secrets.unavailable.as_ref().map(|f| &f.kind),
            Some(FaultKind::Keychain(KeychainFault::NoAnswer | KeychainFault::Locked))
        )
    }

    /// What the prompt says when the keychain could not give `fault`'s profile its password.
    fn keychain_prompt_msg(&self, fault: &Fault) -> Msg {
        // `security unlock-keychain` is macOS's.
        let remote = self.remote_session() && cfg!(target_os = "macos");
        match (&fault.kind, remote) {
            (FaultKind::Keychain(KeychainFault::NoAnswer), true) => Label::PromptPasswordKeychainNoAnswerRemote.into(),
            (FaultKind::Keychain(KeychainFault::NoAnswer), false) => Label::PromptPasswordKeychainNoAnswer.into(),
            (_, true) => Msg::PromptPasswordKeychainFailedRemote { error: self.fault_words(fault) },
            (_, false) => Msg::PromptPasswordKeychainFailed { error: self.fault_words(fault) },
        }
    }

    fn fault_words(&self, fault: &Fault) -> String {
        self.i18n.msg(&super::persist::fault_reason(fault)).to_string()
    }

    /// Connect to `conn` with the password its keychain entry holds, read on a worker; the
    /// attempt is on (`Esc` cancels it) until the answer comes.
    pub(super) fn connect_with_keychain(&mut self, conn: ConnectionConfig, account: String, legacy: Option<String>) {
        let id = conn.id;
        let started = self.now();
        let c = self.conns.entry(id);
        c.connecting =
            Some(Connecting { name: conn.name.clone(), started, had_password: true, keychain: true, tunnel: None });
        let generation = c.generation;
        self.show_status(Notice::new(Msg::ConnReadingKeychain { name: conn.name }, Level::Info));
        self.keychain_job(
            read_accounts(&account, legacy.as_deref()),
            move |s| read_keychain(s, &account, legacy.as_deref()),
            move |result| KeychainDone::Connect { profile: id, generation, result },
        );
    }

    /// Test `c` with the password its keychain entry holds, read on a worker (the test runs
    /// meanwhile; `Esc` cancels it).
    pub(super) fn test_with_keychain(&mut self, c: ConnectionConfig, account: String, legacy: Option<String>) {
        self.clear_test();
        self.test_seq += 1;
        let seq = self.test_seq;
        self.conn_test =
            Some(ConnTest { seq, started: Instant::now(), state: TestState::Running, abort: None, tunnel: None });
        self.report_test();
        let cfg = Box::new(c);
        self.keychain_job(
            read_accounts(&account, legacy.as_deref()),
            move |s| read_keychain(s, &account, legacy.as_deref()),
            move |result| KeychainDone::Test { seq, cfg, result },
        );
    }

    /// Read the password of `p` for the profile form that just opened on it; the form's field
    /// is filled when it answers (if nothing was typed there meanwhile).
    pub(super) fn read_form_password(&mut self, p: &ConnectionConfig) {
        let (profile, account) = (p.id, p.id.account());
        let legacy = (self.config_version < config::CONFIG_VERSION).then(|| p.name.clone());
        self.keychain_job(
            read_accounts(&account, legacy.as_deref()),
            move |s| read_keychain(s, &account, legacy.as_deref()),
            move |result| KeychainDone::FormPassword { profile, result },
        );
    }

    /// Write `pw` as profile `p`'s keychain password on a worker. Until it is written it is
    /// kept for this session.
    pub(super) fn save_to_keychain(&mut self, p: &ConnectionConfig, pw: String, after: AfterSave) {
        self.save_account_to_keychain(p, p.id.account(), pw, after);
    }

    /// Write `pw` under `account` of profile `p` (its database password, or its tunnel's
    /// secret) on a worker; kept for this session until it is written.
    pub(super) fn save_account_to_keychain(
        &mut self,
        p: &ConnectionConfig,
        account: String,
        pw: String,
        after: AfterSave,
    ) {
        let (profile, name) = (p.id, p.name.clone());
        self.secrets.remember(&account, &pw);
        let secret = Secret(pw);
        let written = secret.clone();
        let key = account.clone();
        self.keychain_job(
            vec![account.clone()],
            move |s| s.keychain.set(&key, &written.0).map_err(|Unavailable(e)| e),
            move |result| KeychainDone::Saved { profile, name, account, secret, after, result },
        );
    }

    /// Remove the keychain passwords of `accounts` on a worker.
    pub(super) fn remove_from_keychain(&mut self, accounts: Vec<String>) {
        self.keychain_job(
            accounts.clone(),
            move |s| {
                let mut removed = false;
                for a in &accounts {
                    removed |= s.keychain.delete(a).map_err(|Unavailable(e)| e)?;
                }
                Ok(removed)
            },
            KeychainDone::Removed,
        );
    }

    /// Change the source of the profile form's profile `p` from `from` to `to` when one of them
    /// is the keychain: step 1 (the copy) on a worker while the form waits, then the config,
    /// then step 3 (the old copy removed) on a worker ([`source::switch`]'s order).
    pub(super) fn switch_with_keychain(
        &mut self,
        p: ConnectionConfig,
        from: SourceKind,
        to: SourceKind,
        typed: String,
    ) {
        if let Some(f) = self.overlays.form_mut() {
            f.saving = true;
        }
        let account = p.id.account();
        let profile = Box::new(p);
        self.keychain_job(
            vec![account.clone()],
            move |s| source::switch_copy(s, from, to, &account, Some(typed.as_str()).filter(|t| !t.is_empty())),
            move |result| KeychainDone::SwitchCopied { profile, from, to, result },
        );
    }

    pub(super) fn on_keychain_done(&mut self, done: KeychainDone) {
        match done {
            KeychainDone::Connect { profile, generation, result } => {
                self.on_keychain_connect(profile, generation, result)
            }
            KeychainDone::Test { seq, cfg, result } => self.on_keychain_test(seq, *cfg, result),
            KeychainDone::FormPassword { profile, result } => self.on_form_password(profile, result),
            KeychainDone::Saved { profile, name, account, secret, after, result } => {
                self.on_keychain_saved(profile, name, account, secret, after, result)
            }
            KeychainDone::Removed(result) => match result {
                Ok(_) => self.keychain_answered(),
                Err(f) => self.keychain_failed(&f),
            },
            KeychainDone::SwitchCopied { profile, from, to, result } => {
                self.on_switch_copied(*profile, from, to, result)
            }
            KeychainDone::SwitchRemoved { name, from, to, copied, out } => {
                self.on_switch_removed(name, from, to, copied, out)
            }
        }
    }

    fn on_keychain_connect(&mut self, id: ProfileId, generation: u64, result: Result<Option<Secret>, Fault>) {
        // Cancelled, or another attempt since: the answer is dropped.
        if !self.conns.is_current(id, generation) || self.conns.get(id).is_none_or(|c| c.connecting.is_none()) {
            return;
        }
        let Some(conn) = self.profile(id).cloned() else { return };
        match result {
            // Nothing stored: the server says whether it needs a password (the prompt opens).
            Ok(pw) => {
                self.keychain_answered();
                self.start_connect(conn, pw.map(|s| s.0).unwrap_or_default());
            }
            // No keychain on this system: nothing can be stored there, as before.
            Err(fault) if no_keychain(&fault) => {
                self.keychain_failed(&fault);
                self.start_connect(conn, String::new());
            }
            // Unknown, not absent: ask for it (what waited for the attempt waits for the prompt).
            Err(fault) => {
                self.keychain_failed(&fault);
                self.conns.entry(id).connecting = None;
                let m = Notice::new(self.keychain_prompt_msg(&fault), Level::Warning);
                self.status = Some(m.clone());
                self.open_prompt(&conn, m, PromptPurpose::Connect);
            }
        }
    }

    fn on_keychain_test(&mut self, seq: u64, mut cfg: ConnectionConfig, result: Result<Option<Secret>, Fault>) {
        let Some(t) = self.conn_test.as_ref() else { return };
        if t.seq != seq || t.state != TestState::Running {
            return;
        }
        match result {
            Ok(pw) => {
                self.keychain_answered();
                cfg.password = pw.map(|s| s.0).unwrap_or_default();
                self.start_test(cfg);
            }
            Err(fault) if no_keychain(&fault) => {
                self.keychain_failed(&fault);
                self.start_test(cfg);
            }
            Err(fault) => {
                self.keychain_failed(&fault);
                self.clear_test();
                let m = Notice::new(self.keychain_prompt_msg(&fault), Level::Warning);
                let c = cfg.clone();
                self.open_prompt(&c, m, PromptPurpose::Test(Box::new(cfg)));
            }
        }
    }

    fn on_form_password(&mut self, profile: ProfileId, result: Result<Option<Secret>, Fault>) {
        match &result {
            Ok(_) => self.keychain_answered(),
            Err(f) => self.keychain_failed(f),
        }
        let Some(f) = self.overlays.form_mut().filter(|f| f.reading == Some(profile)) else { return };
        f.reading = None;
        // Read: the field shows it, unless something was typed there meanwhile. Not read: the
        // field stays marked unread (left empty, the stored password is kept).
        if let Ok(pw) = result
            && f.password.text().is_empty()
        {
            f.password.set(&pw.map(|s| s.0).unwrap_or_default());
            f.password_unread = false;
        }
    }

    fn on_keychain_saved(
        &mut self,
        id: ProfileId,
        name: String,
        account: String,
        secret: Secret,
        after: AfterSave,
        result: Result<(), Fault>,
    ) {
        let plaintext = matches!(after, AfterSave::Form { plaintext: true });
        // Written for a profile deleted since (its removal did not reach this entry): the
        // password does not stay behind.
        if result.is_ok() && self.profile(id).is_none() {
            self.keychain_answered();
            self.secrets.forget(&account);
            return self.remove_from_keychain(vec![account]);
        }
        let msg = match result {
            Ok(()) => {
                self.keychain_answered();
                // Stored: the session copy goes (unless another was typed meanwhile).
                if self.secrets.session(&account) == Some(secret.0.as_str()) {
                    self.secrets.forget(&account);
                }
                if plaintext && let Some(p) = self.profiles.iter_mut().find(|p| p.id == id) {
                    p.password.clear();
                    if let Some(m) = self.persist() {
                        return self.flash(m);
                    }
                }
                match after {
                    AfterSave::Connected => Some(Notice::new(Msg::SecretSaved { name }, Level::Success)),
                    AfterSave::Tunnel => Some(Notice::new(Msg::SshSecretSaved { name }, Level::Success)),
                    AfterSave::Form { .. } => None,
                }
            }
            // Not stored: remembered for this session only. A password that was already
            // plaintext in the file is replaced there rather than being lost.
            Err(fault) => {
                self.keychain_failed(&fault);
                if plaintext && let Some(p) = self.profiles.iter_mut().find(|p| p.id == id) {
                    p.password = secret.0.clone();
                    let _ = self.persist();
                }
                Some(Notice::new(self.source_error(&SourceError::Keychain(fault)), Level::Warning))
            }
        };
        if let Some(m) = msg {
            self.flash(m);
        }
    }

    fn on_switch_copied(
        &mut self,
        p: ConnectionConfig,
        from: SourceKind,
        to: SourceKind,
        result: Result<bool, SwitchError>,
    ) {
        if let Some(f) = self.overlays.form_mut().filter(|f| f.profile_id() == p.id) {
            f.saving = false;
        }
        let to_t = self.source_text(to);
        let copied = match result {
            Ok(copied) => copied,
            // Nothing was removed; the form stays open.
            Err(e) => {
                if let SwitchError::Read(SourceError::Keychain(f)) | SwitchError::Write(SourceError::Keychain(f)) = &e {
                    self.keychain_failed(f);
                }
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
        // Step 2: the config says `to` (the profile may have moved in the list meanwhile).
        let Some(i) = self.profiles.iter().position(|q| q.id == p.id) else { return };
        let old = self.profiles[i].clone();
        self.put_profile(Some(i), p.clone());
        if let Err(fault) = self.try_persist() {
            self.put_profile(Some(i), old);
            let error = self.fault_text("source.change.failed", &fault);
            return self.flash(Notice::new(Msg::SourceChangeFailed { error }, Level::Error));
        }
        self.secrets.forget(&p.id.account());
        if self.overlays.form().is_some_and(|f| f.profile_id() == p.id) {
            self.form_saved(i);
        }
        let (name, account) = (p.name.clone(), p.id.account());
        self.keychain_job(
            vec![account.clone()],
            move |s| source::switch_remove(s, from, to, &account),
            move |out| KeychainDone::SwitchRemoved { name, from, to, copied, out },
        );
    }

    fn on_switch_removed(&mut self, name: String, from: SourceKind, to: SourceKind, copied: bool, out: Switched) {
        let switched = Switched { copied, ..out };
        let msg = self.switched_msg(name, from, to, switched);
        self.flash(msg);
    }
}

impl Plan {
    /// The password is in the keychain: read it on a worker.
    pub(super) fn keychain(c: &ConnectionConfig, old_config: bool) -> Self {
        Plan::Keychain { account: c.id.account(), legacy: old_config.then(|| c.name.clone()) }
    }
}
