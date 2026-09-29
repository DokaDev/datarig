//! In-process [`SecretStore`] (tests, and machines without a keychain in tests).

use super::{SecretStore, Unavailable};
use crate::fault::{Fault, FaultKind, KeychainFault};
use std::collections::HashMap;
use std::sync::Mutex;

/// In-memory store for tests. `MemoryStore::unavailable` behaves like a machine without a
/// keychain (every call fails).
#[derive(Default)]
pub struct MemoryStore {
    map: Mutex<HashMap<String, String>>,
    unavailable: Option<Fault>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// A machine without a keychain (`reason` is the detail).
    pub fn unavailable(reason: &str) -> Self {
        Self::failing(Fault::new(FaultKind::Keychain(KeychainFault::NoStore), reason))
    }

    /// Every call fails with `fault`.
    pub fn failing(fault: Fault) -> Self {
        Self { map: Mutex::default(), unavailable: Some(fault) }
    }

    fn check(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, String>>, Unavailable> {
        if let Some(r) = &self.unavailable {
            return Err(Unavailable(r.clone()));
        }
        self.map.lock().map_err(|e| Unavailable(Fault::other(e.to_string())))
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, account: &str) -> Result<Option<String>, Unavailable> {
        Ok(self.check()?.get(account).cloned())
    }
    fn set(&self, account: &str, secret: &str) -> Result<(), Unavailable> {
        self.check()?.insert(account.to_string(), secret.to_string());
        Ok(())
    }
    fn delete(&self, account: &str) -> Result<bool, Unavailable> {
        Ok(self.check()?.remove(account).is_some())
    }
}
