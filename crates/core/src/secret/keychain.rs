//! [`SecretStore`] backed by the OS credential store (macOS Keychain, Linux Secret Service,
//! Windows Credential Manager) through the `keyring` crate.

use super::{SERVICE, SecretStore, Unavailable};
use crate::fault::{Fault, FaultKind, KeychainFault};

/// OS keychain via the `keyring` crate.
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    pub fn new() -> Self {
        Self::with_service(SERVICE)
    }

    pub fn with_service(service: &str) -> Self {
        Self { service: service.to_string() }
    }

    fn entry(&self, account: &str) -> Result<keyring::Entry, Unavailable> {
        keyring::Entry::new(&self.service, account).map_err(fault)
    }
}

/// A keyring error as a [`Fault`] of the kind the UI can say in words.
fn fault(e: keyring::Error) -> Unavailable {
    use keyring::Error as E;
    let kind = match &e {
        E::NoStorageAccess(_) => KeychainFault::Locked,
        E::NoDefaultStore | E::NotSupportedByStore(_) => KeychainFault::NoStore,
        E::BadEncoding(_) | E::BadDataFormat(..) | E::BadStoreFormat(_) => KeychainFault::Damaged,
        E::TooLong(..) | E::Invalid(..) => KeychainFault::Refused,
        E::Ambiguous(_) => KeychainFault::Ambiguous,
        _ => KeychainFault::Failure,
    };
    Unavailable(Fault::new(FaultKind::Keychain(kind), e.to_string()))
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, account: &str) -> Result<Option<String>, Unavailable> {
        match self.entry(account)?.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(fault(e)),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), Unavailable> {
        self.entry(account)?.set_password(secret).map_err(fault)
    }

    fn delete(&self, account: &str) -> Result<bool, Unavailable> {
        match self.entry(account)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(fault(e)),
        }
    }
}
