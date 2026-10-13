//! The one-time offer to stop the macOS keychain prompts that follow each update.
//!
//! In `direct` mode ([`KeychainAccess::Direct`]) macOS binds the "Always Allow" of a keychain
//! item to the code signature of the program that asked. Releases are not signed, so every
//! update looks like a new program and macOS asks for the login password again. The `security`
//! tool mode avoids that; this module decides when to suggest it.
//!
//! **The signal.** Each direct-mode keychain call is timed ([`Timed`], with an injectable
//! clock), and the state file ([`STATE_FILE`] in the state directory) remembers the binary
//! that last read a saved password directly ([`BinaryId`]: version, executable path, size
//! and modification time). A call most likely waited for the user's approval when
//!
//! ```text
//! (it took at least SLOW (1 s) OR it did not answer in time)
//!     AND (no binary is recorded yet OR the recorded binary is not this one)
//! ```
//!
//! ([`prompt_likely`]). A slow call by an unchanged binary (a locked keychain, a slow disk) is
//! not an update prompt, and a fast call never is. With nothing recorded yet (the first run of
//! a version that keeps the record) timing alone decides. Only a read that returned a saved
//! password records the running binary ([`Observation::trusted`]): that proves macOS lets this
//! binary read the items. Writing a new item or removing a missing one never asks, so it proves
//! nothing. A connect that reads nothing from the keychain gives no signal.
//!
//! **Limits.** A user who answers the macOS dialog in under a second is missed (the offer comes
//! at the next prompt); a keychain that is slow for another reason right after an update can
//! make the offer appear without a prompt (harmless: `Enter` keeps everything as it is).
//!
//! **Never offered** ([`should_offer`]): outside macOS, over SSH, in `security_tool` mode, with
//! the in-memory store (`DATARIG_SECRET_STORE=memory`), once answered (`y` or `n`), or when the
//! state file exists but cannot be read (unknown is not "not answered": it is not offered, and
//! the file is not written over).

use super::{Call, Guarded, KeychainSwitch, SecretStore, Unavailable};
use crate::config::KeychainAccess;
use crate::fault::{FaultKind, KeychainFault};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// A keychain call at least this long may have waited for the user.
pub const SLOW: Duration = Duration::from_secs(1);

/// The state file's name, in the state directory.
pub const STATE_FILE: &str = "keychain.toml";

/// What identifies the running binary well enough to tell an update.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryId {
    pub version: String,
    pub exe: String,
    pub size: u64,
    /// Nanoseconds since the Unix epoch.
    pub mtime_ns: u64,
}

impl BinaryId {
    /// The running executable (`version` is the binary's own), its path with symbolic links
    /// resolved (a link it was started through is not another binary). `None` when the system
    /// does not say.
    pub fn current(version: &str) -> Option<Self> {
        let raw = std::env::current_exe().ok()?;
        let exe = std::fs::canonicalize(&raw).unwrap_or(raw);
        let meta = std::fs::metadata(&exe).ok()?;
        let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        Some(Self {
            version: version.to_string(),
            exe: exe.to_string_lossy().into_owned(),
            size: meta.len(),
            mtime_ns: u64::try_from(mtime.as_nanos()).ok()?,
        })
    }
}

/// The user's answer to the offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// `y`: switched to the `security` tool.
    Accepted,
    /// `n`: keep `direct`, and do not ask again.
    Declined,
}

/// The state file ([`STATE_FILE`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Answer>,
    /// The binary that last read a saved password directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<BinaryId>,
}

/// The state file exists but could not be read or parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unreadable;

/// Read the state file: empty when there is none.
pub fn load(path: &Path) -> Result<OfferState, Unreadable> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|_| Unreadable),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(OfferState::default()),
        Err(_) => Err(Unreadable),
    }
}

/// Write the state file atomically (a temporary file renamed over it). Only the fields of
/// [`OfferState`] are written: keys it does not know are not kept.
pub fn save(path: &Path, state: &OfferState) -> std::io::Result<()> {
    let text = toml::to_string(state).map_err(std::io::Error::other)?;
    crate::fsutil::atomic_write(path, text.as_bytes())
}

/// The session runs over SSH (`env` is the process environment, or a test's).
pub fn remote(env: impl Fn(&str) -> Option<String>) -> bool {
    ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"].iter().any(|k| env(k).is_some_and(|v| !v.is_empty()))
}

/// One timed direct-mode keychain call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    pub call: Call,
    pub elapsed: Duration,
    /// It did not answer within the keychain's limit ([`Guarded`]).
    pub timed_out: bool,
    /// It succeeded (an entry that is not there is a success too).
    pub ok: bool,
    /// A read that returned a saved password.
    pub found: bool,
}

impl Observation {
    /// macOS let this binary read a saved password: the binary may be recorded as trusted.
    pub fn trusted(&self) -> bool {
        self.call == Call::Get && self.ok && self.found
    }
}

/// The call most likely waited for the user's approval after an update (see the module docs).
pub fn prompt_likely(seen: Observation, recorded: Option<&BinaryId>, current: &BinaryId) -> bool {
    (seen.elapsed >= SLOW || seen.timed_out) && recorded != Some(current)
}

/// Everything [`should_offer`] decides on.
#[derive(Clone, Copy, Debug)]
pub struct Facts<'a> {
    /// Running on macOS.
    pub macos: bool,
    /// The keychain is the OS's own (not the in-memory store).
    pub os_keychain: bool,
    /// The session runs over SSH ([`remote`]).
    pub remote: bool,
    pub access: KeychainAccess,
    /// The state file; `None` when it exists but cannot be read.
    pub state: Option<&'a OfferState>,
    pub current: &'a BinaryId,
    pub seen: Observation,
}

/// Offer to switch to the `security` tool after the call `f.seen`.
pub fn should_offer(f: &Facts) -> bool {
    f.macos
        && f.os_keychain
        && !f.remote
        && f.access == KeychainAccess::Direct
        && f.state.is_some_and(|s| s.answer.is_none() && prompt_likely(f.seen, s.binary.as_ref(), f.current))
}

/// The time source of [`Timed`].
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// The calls [`Timed`] saw and the app has not looked at yet.
pub type Seen = Arc<Mutex<Vec<Observation>>>;

/// The guarded OS keychain, each direct-mode call timed into `seen`. A call refused at once
/// because an earlier one still hangs is not timed (it never reached the keychain).
pub struct Timed {
    inner: Arc<Guarded>,
    switch: KeychainSwitch,
    clock: Clock,
    seen: Seen,
}

impl Timed {
    pub fn new(inner: Arc<Guarded>, switch: KeychainSwitch, clock: Clock, seen: Seen) -> Self {
        Self { inner, switch, clock, seen }
    }

    fn time<T>(
        &self,
        call: Call,
        f: impl FnOnce(&dyn SecretStore) -> Result<T, Unavailable>,
        found: impl FnOnce(&T) -> bool,
    ) -> Result<T, Unavailable> {
        if self.switch.get() != KeychainAccess::Direct || self.inner.hanging() {
            return f(self.inner.as_ref());
        }
        let start = (self.clock)();
        let r = f(self.inner.as_ref());
        let elapsed = (self.clock)().saturating_duration_since(start);
        let timed_out = matches!(&r, Err(Unavailable(e)) if e.kind == FaultKind::Keychain(KeychainFault::NoAnswer));
        let seen = Observation { call, elapsed, timed_out, ok: r.is_ok(), found: r.as_ref().is_ok_and(found) };
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).push(seen);
        r
    }
}

impl SecretStore for Timed {
    fn get(&self, account: &str) -> Result<Option<String>, Unavailable> {
        self.time(Call::Get, |s| s.get(account), Option::is_some)
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), Unavailable> {
        self.time(Call::Set, |s| s.set(account, secret), |_| false)
    }

    fn delete(&self, account: &str) -> Result<bool, Unavailable> {
        self.time(Call::Delete, |s| s.delete(account), |_| false)
    }
}

/// The platform facts the app needs for the offer (tests pass their own).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host {
    pub macos: bool,
    /// The running binary; `None` when unknown (nothing is then offered or recorded).
    pub binary: Option<BinaryId>,
}

impl Host {
    /// This process (`version` is the binary's own).
    pub fn current(version: &str) -> Self {
        Self { macos: cfg!(target_os = "macos"), binary: BinaryId::current(version) }
    }
}

#[cfg(test)]
mod tests;
