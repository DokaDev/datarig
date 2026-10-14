//! `[secrets] macos_keychain_access` at run time: [`KeychainSwitch`] holds the current
//! [`KeychainAccess`], and [`SwitchedKeychain`] sends each call to the store of that mode. The app
//! changes the switch when the setting changes, so the next call already goes the new way (no
//! restart). Only the OS keychain on macOS is built as a [`SwitchedKeychain`] store
//! ([`super::StoreKind::open_stores`]); everywhere else the switch is kept but nothing reads it.

use super::{SecretStore, Unavailable};
use crate::config::KeychainAccess;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// The current [`KeychainAccess`], shared by the app (which sets it) and the store (which reads
/// it on every call). Clones share the same value.
#[derive(Clone, Debug, Default)]
pub struct KeychainSwitch(Arc<AtomicU8>);

impl KeychainSwitch {
    /// A switch set to `access`.
    pub fn new(access: KeychainAccess) -> Self {
        let s = Self::default();
        s.set(access);
        s
    }

    /// The mode the next call uses.
    pub fn get(&self) -> KeychainAccess {
        match self.0.load(Ordering::SeqCst) {
            1 => KeychainAccess::SecurityTool,
            _ => KeychainAccess::Direct,
        }
    }

    /// Use `access` from the next call on (a call already running finishes the way it began).
    pub fn set(&self, access: KeychainAccess) {
        let v = match access {
            KeychainAccess::Direct => 0,
            KeychainAccess::SecurityTool => 1,
        };
        self.0.store(v, Ordering::SeqCst);
    }
}

/// A store that sends each call to `direct` or `tool`, as the switch says at that moment. Both
/// stores reach the same keychain items (same service and account), so switching moves
/// nothing.
pub struct SwitchedKeychain {
    direct: Arc<dyn SecretStore>,
    tool: Arc<dyn SecretStore>,
    switch: KeychainSwitch,
}

impl SwitchedKeychain {
    pub fn new(direct: Arc<dyn SecretStore>, tool: Arc<dyn SecretStore>, switch: KeychainSwitch) -> Self {
        Self { direct, tool, switch }
    }

    fn now(&self) -> &dyn SecretStore {
        match self.switch.get() {
            KeychainAccess::Direct => self.direct.as_ref(),
            KeychainAccess::SecurityTool => self.tool.as_ref(),
        }
    }
}

impl SecretStore for SwitchedKeychain {
    fn get(&self, account: &str) -> Result<Option<String>, Unavailable> {
        self.now().get(account)
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), Unavailable> {
        self.now().set(account, secret)
    }

    fn delete(&self, account: &str) -> Result<bool, Unavailable> {
        self.now().delete(account)
    }
}
