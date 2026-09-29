//! [`Guarded`]: a [`SecretStore`] whose calls end within a time limit.
//!
//! The OS keychain may hold a call for as long as it likes: macOS asks for permission or for
//! the keychain's password on the Mac's own screen, which a user connected over SSH never sees,
//! so the call never returns. Each call of a guarded store runs on a thread of its own, and one
//! that does not answer within the limit is a [`KeychainFault::NoAnswer`]. Its thread is left
//! behind (nothing can stop it); while one is still hanging, the next calls fail at once
//! instead of piling up more threads, and once it returns the store is asked again. A call
//! that did not answer is **unknown**, never "no password stored".
//!
//! The limit bounds the worker that waits; the UI never waits on its own thread either.

use super::{SecretStore, Unavailable};
use crate::fault::{Fault, FaultKind, KeychainFault};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

/// How long a keychain call may take (as a password command, [`super::command::TIMEOUT`]).
pub const KEYCHAIN_TIMEOUT: Duration = Duration::from_secs(10);

const RUNNING: u8 = 0;
const DONE: u8 = 1;
const ABANDONED: u8 = 2;

/// A store whose calls answer within `timeout` (see the module docs).
pub struct Guarded {
    inner: Arc<dyn SecretStore>,
    timeout: Duration,
    /// Calls that did not answer in time and have not returned yet.
    hanging: Arc<AtomicUsize>,
}

impl Guarded {
    pub fn new(inner: Arc<dyn SecretStore>, timeout: Duration) -> Self {
        Self { inner, timeout, hanging: Arc::new(AtomicUsize::new(0)) }
    }

    /// A call that did not answer in time is still waiting inside the store.
    pub fn hanging(&self) -> bool {
        self.hanging.load(Ordering::SeqCst) > 0
    }

    fn no_answer(&self, still: bool) -> Unavailable {
        let detail = if still {
            format!("an earlier call has not answered yet (limit {:?})", self.timeout)
        } else {
            format!("no answer within {:?}", self.timeout)
        };
        Unavailable(Fault::new(FaultKind::Keychain(KeychainFault::NoAnswer), detail))
    }

    fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&dyn SecretStore) -> Result<T, Unavailable> + Send + 'static,
    ) -> Result<T, Unavailable> {
        if self.hanging() {
            return Err(self.no_answer(true));
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let state = Arc::new(AtomicU8::new(RUNNING));
        let (inner, st, hanging) = (self.inner.clone(), state.clone(), self.hanging.clone());
        let spawned = std::thread::Builder::new().name("datarig-keychain".into()).spawn(move || {
            let r = f(inner.as_ref());
            if st.compare_exchange(RUNNING, DONE, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                let _ = tx.send(r);
            } else {
                // Given up on: it answered after all, so the store may be asked again.
                hanging.fetch_sub(1, Ordering::SeqCst);
            }
        });
        if let Err(e) = spawned {
            return Err(Unavailable(Fault::new(FaultKind::Keychain(KeychainFault::Failure), e.to_string())));
        }
        match rx.recv_timeout(self.timeout) {
            Ok(r) => r,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Counted before it is marked, so the count never goes below zero.
                self.hanging.fetch_add(1, Ordering::SeqCst);
                if state.compare_exchange(RUNNING, ABANDONED, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                    Err(self.no_answer(false))
                } else {
                    // It answered right at the limit.
                    self.hanging.fetch_sub(1, Ordering::SeqCst);
                    rx.recv().unwrap_or_else(|_| Err(self.no_answer(false)))
                }
            }
            // The call panicked.
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Unavailable(Fault::new(
                FaultKind::Keychain(KeychainFault::Failure),
                "the keychain call ended without an answer",
            ))),
        }
    }
}

impl SecretStore for Guarded {
    fn get(&self, account: &str) -> Result<Option<String>, Unavailable> {
        let account = account.to_string();
        self.call(move |s| s.get(&account))
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), Unavailable> {
        let (account, secret) = (account.to_string(), secret.to_string());
        self.call(move |s| s.set(&account, &secret))
    }

    fn delete(&self, account: &str) -> Result<bool, Unavailable> {
        let account = account.to_string();
        self.call(move |s| s.delete(&account))
    }
}
