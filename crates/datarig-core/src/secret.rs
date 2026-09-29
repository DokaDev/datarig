//! Password storage.
//!
//! Passwords never go into the config file. Each profile picks a [`source`]: the OS keychain
//! (the default), the secrets file, a command, an environment variable, or a prompt on every
//! connect. [`KeyringStore`] uses the OS credential store (macOS Keychain, Linux Secret
//! Service, Windows Credential Manager) through the `keyring` crate; [`FileStore`] is
//! `secrets.toml` next to the config file; [`MemoryStore`] is the in-process implementation used
//! by tests and by development runs with `DATARIG_SECRET_STORE=memory` ([`StoreKind`]), which
//! replaces **both** the keychain and the secrets file. [`Secrets`] adds a per-session cache on
//! top and implements the fallback: when the keychain is unavailable the password is asked for
//! when connecting ("prompt") and only kept in memory.

pub mod command;
pub mod file;
mod guard;
mod keychain;
mod log;
mod memory;
pub mod source;

pub use file::FileStore;
pub use guard::{Guarded, KEYCHAIN_TIMEOUT};
pub use keychain::KeyringStore;
pub use log::{DeletionLog, Logged};
pub use memory::MemoryStore;
pub use source::{DefaultSource, Lookup, PasswordSource, SourceError, SourceKind};

use crate::fault::Fault;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Service name of every datarig keychain item. The account is `profile:<uuid>`
/// ([`crate::profile::ProfileId::account`]); version 1 configs used the profile name.
pub const SERVICE: &str = "datarig";

/// Environment variable that picks the stores of the whole process: `keychain` (the default:
/// the OS keychain and the secrets file) or `memory`. `memory` never touches the OS keychain or
/// the secrets file, so development runs cannot read or change the user's real passwords.
pub const STORE_ENV: &str = "DATARIG_SECRET_STORE";

/// Which [`SecretStore`] the process uses ([`STORE_ENV`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreKind {
    /// The OS credential store ([`KeyringStore`]).
    Keychain,
    /// An in-process store that starts empty ([`MemoryStore`]).
    Memory,
    /// `stall`, debug builds only: a keychain that never answers (as a locked one over SSH
    /// may not), for checking the app's time limit by hand. The secrets file is in memory.
    Stalled,
}

impl StoreKind {
    /// Read the value of [`STORE_ENV`]. Unset or empty means [`StoreKind::Keychain`]; any
    /// other unknown value is an error (a typo must not silently reach the real keychain).
    pub fn from_env(value: Option<&str>) -> Result<Self, String> {
        match value.map(str::trim) {
            None | Some("") => Ok(StoreKind::Keychain),
            Some(v) if v.eq_ignore_ascii_case("keychain") => Ok(StoreKind::Keychain),
            Some(v) if v.eq_ignore_ascii_case("memory") => Ok(StoreKind::Memory),
            Some(v) if cfg!(debug_assertions) && v.eq_ignore_ascii_case("stall") => Ok(StoreKind::Stalled),
            Some(v) => Err(format!("{STORE_ENV}={v:?}: expected \"keychain\" or \"memory\"")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            StoreKind::Keychain => "keychain",
            StoreKind::Memory => "memory",
            StoreKind::Stalled => "stall",
        }
    }

    /// A new store of this kind.
    pub fn open(self) -> Arc<dyn SecretStore> {
        match self {
            StoreKind::Keychain => Arc::new(KeyringStore::new()),
            StoreKind::Memory => Arc::new(MemoryStore::new()),
            StoreKind::Stalled => Arc::new(Stalled),
        }
    }

    /// The keychain and secrets-file stores of this kind, their deletions written to `log`
    /// ([`Logged`]). `secrets_file` is `secrets.toml` next to the config file (`None`: no
    /// config directory, so the file source is unavailable). `memory` replaces both stores.
    pub fn open_stores(self, secrets_file: Option<PathBuf>, log: Arc<DeletionLog>) -> Stores {
        let logged = |store: Arc<dyn SecretStore>, source: &str| -> Arc<dyn SecretStore> {
            Arc::new(Logged::new(store, source, log.clone()))
        };
        match self {
            StoreKind::Keychain => {
                let file: Arc<dyn SecretStore> = match &secrets_file {
                    Some(p) => Arc::new(FileStore::new(p.clone())),
                    None => Arc::new(MemoryStore::failing(Fault::new(
                        crate::fault::FaultKind::Io(std::io::ErrorKind::NotFound),
                        "no config directory for secrets.toml",
                    ))),
                };
                Stores {
                    keychain: logged(Arc::new(KeyringStore::new()), "keychain"),
                    file: logged(file, "file"),
                    file_path: secrets_file,
                }
            }
            StoreKind::Memory => Stores {
                keychain: logged(Arc::new(MemoryStore::new()), "keychain (memory)"),
                file: logged(Arc::new(MemoryStore::new()), "file (memory)"),
                file_path: None,
            },
            StoreKind::Stalled => Stores {
                keychain: Arc::new(Stalled),
                file: logged(Arc::new(MemoryStore::new()), "file (memory)"),
                file_path: None,
            },
        }
    }
}

/// A keychain that never answers ([`StoreKind::Stalled`]).
struct Stalled;

impl Stalled {
    fn wait() -> ! {
        loop {
            std::thread::park();
        }
    }
}

impl SecretStore for Stalled {
    fn get(&self, _: &str) -> Result<Option<String>, Unavailable> {
        Self::wait()
    }
    fn set(&self, _: &str, _: &str) -> Result<(), Unavailable> {
        Self::wait()
    }
    fn delete(&self, _: &str) -> Result<bool, Unavailable> {
        Self::wait()
    }
}

/// The stores behind the storing sources.
#[derive(Clone)]
pub struct Stores {
    pub keychain: Arc<dyn SecretStore>,
    pub file: Arc<dyn SecretStore>,
    /// The real secrets file, for its permission check (`None` for an in-memory file store).
    pub file_path: Option<PathBuf>,
}

impl Stores {
    /// `keychain` plus an in-memory secrets file (tests).
    pub fn with_keychain(keychain: Arc<dyn SecretStore>) -> Self {
        Self { keychain, file: Arc::new(MemoryStore::new()), file_path: None }
    }

    /// The secrets file may be used (its permissions, on Unix).
    pub fn check_file(&self) -> Result<(), file::FileError> {
        self.file_path.as_deref().map_or(Ok(()), file::check_permissions)
    }

    /// The store of a storing source.
    pub fn of(&self, kind: SourceKind) -> Option<&Arc<dyn SecretStore>> {
        match kind {
            SourceKind::Keychain => Some(&self.keychain),
            SourceKind::File => Some(&self.file),
            _ => None,
        }
    }
}

/// A store failed (why, see [`Fault`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unavailable(pub Fault);

pub trait SecretStore: Send + Sync {
    /// `Ok(None)` when there is no entry for `account`.
    fn get(&self, account: &str) -> Result<Option<String>, Unavailable>;
    fn set(&self, account: &str, secret: &str) -> Result<(), Unavailable>;
    /// Remove the entry. `Ok(true)` when an entry was removed; deleting a missing entry is not
    /// an error (`Ok(false)`).
    fn delete(&self, account: &str) -> Result<bool, Unavailable>;
}

/// Why a password could not be stored safely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// The store failed (no keychain, access denied, …).
    Unavailable(Fault),
    /// The store took the password but gave back something else when read again.
    Mismatch,
}

/// Write `secret` for `account`, then read it back. `Ok` only when the store returns exactly
/// `secret`: only then may the caller drop its other copy (e.g. the plaintext in the config
/// file). Blocking — the OS may show a permission dialog — so the UI runs it off its thread.
pub fn store_verified(store: &dyn SecretStore, account: &str, secret: &str) -> Result<(), StoreError> {
    store.set(account, secret).map_err(|Unavailable(e)| StoreError::Unavailable(e))?;
    match store.get(account) {
        Ok(Some(back)) if back == secret => Ok(()),
        Ok(_) => Err(StoreError::Mismatch),
        Err(Unavailable(e)) => Err(StoreError::Unavailable(e)),
    }
}

/// Stores + session cache + fallback bookkeeping. The methods without a source kind work on
/// the keychain.
pub struct Secrets {
    stores: Stores,
    session: HashMap<String, String>,
    /// Set after the keychain failed once; the UI then says passwords are asked at connect time.
    pub unavailable: Option<Fault>,
}

impl Secrets {
    /// `store` as the keychain and an in-memory secrets file.
    pub fn new(store: Arc<dyn SecretStore>) -> Self {
        Self::with_stores(Stores::with_keychain(store))
    }

    pub fn with_stores(stores: Stores) -> Self {
        Self { stores, session: HashMap::new(), unavailable: None }
    }

    /// The keychain store (for work that runs off the UI thread).
    pub fn store(&self) -> Arc<dyn SecretStore> {
        self.stores.keychain.clone()
    }

    /// Every store (for work that runs off the UI thread).
    pub fn stores(&self) -> &Stores {
        &self.stores
    }

    fn note<T>(&mut self, kind: SourceKind, r: Result<T, Unavailable>) -> Result<T, SourceError> {
        r.map_err(|Unavailable(e)| {
            if kind == SourceKind::Keychain {
                self.unavailable = Some(e.clone());
                SourceError::Keychain(e)
            } else {
                SourceError::File(file::FileError::Io(e))
            }
        })
    }

    /// Session cache first (a password typed at the prompt), then the keychain.
    pub fn resolve(&mut self, account: &str) -> Option<String> {
        if let Some(p) = self.session.get(account) {
            return Some(p.clone());
        }
        let r = self.stores.keychain.get(account);
        self.note(SourceKind::Keychain, r).ok().flatten()
    }

    /// Password typed at the prompt during this session, if any.
    pub fn session(&self, account: &str) -> Option<&str> {
        self.session.get(account).map(String::as_str)
    }

    /// Keep a password for this session only (prompt fallback).
    pub fn remember(&mut self, account: &str, secret: &str) {
        self.session.insert(account.to_string(), secret.to_string());
    }

    /// Drop the session copy (a `prompt` profile asks again on the next connect).
    pub fn forget(&mut self, account: &str) {
        self.session.remove(account);
    }

    /// Persist in the keychain. On failure the password is still remembered for the session.
    pub fn save(&mut self, account: &str, secret: &str) -> Result<(), SourceError> {
        self.save_in(SourceKind::Keychain, account, secret)
    }

    /// Persist in the store of `kind` (keychain or secrets file). On failure the password is
    /// still remembered for the session.
    pub fn save_in(&mut self, kind: SourceKind, account: &str, secret: &str) -> Result<(), SourceError> {
        let r = match (kind, self.stores.of(kind)) {
            (SourceKind::File, Some(s)) => match self.stores.check_file() {
                Ok(()) => self.note(kind, s.set(account, secret)),
                Err(e) => Err(SourceError::File(e)),
            },
            (_, Some(s)) => self.note(kind, s.set(account, secret)),
            (_, None) => Ok(()),
        };
        match r {
            Ok(()) => {
                self.session.remove(account);
                Ok(())
            }
            Err(e) => {
                self.remember(account, secret);
                Err(e)
            }
        }
    }

    /// Remove from the keychain (and the session cache).
    pub fn remove(&mut self, account: &str) -> Result<(), SourceError> {
        self.remove_in(SourceKind::Keychain, account)
    }

    /// Remove from the store of `kind` (and the session cache).
    pub fn remove_in(&mut self, kind: SourceKind, account: &str) -> Result<(), SourceError> {
        self.session.remove(account);
        let Some(store) = self.stores.of(kind) else { return Ok(()) };
        let r = store.delete(account).map(|_| ());
        self.note(kind, r)
    }
}

#[cfg(test)]
mod tests;
