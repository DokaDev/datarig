//! The one-time offer to stop the macOS keychain prompts that follow each update
//! ([`datarig_core::secret::offer`] has the signal and when it is never offered).
//!
//! The OS keychain's direct-mode calls are timed on their workers; after each event the app
//! looks at the calls timed since ([`App::keychain_offer_check`]): it decides whether one most
//! likely waited for the user, and records the running binary after each that read a saved
//! password (the proof that macOS trusts it). The
//! offer never holds up a connect: it waits until no connection attempt is in progress and no
//! dialog is open (a dialog open meanwhile only defers it, for this session), then asks once.
//! `y` switches to the `security` tool as the setting does and records "accepted"; `n` records
//! "declined"; `Enter`/`Esc` keep everything as it is and record nothing (offered again at a
//! later prompt).

use super::overlay::{Confirm, ConfirmAction, Overlay};
use super::*;
use datarig_core::fault::{ErrorLog, Fault};
use datarig_core::secret::Guarded;
use datarig_core::secret::offer::{self, Answer, Facts, Host, OfferState, Timed};

impl App {
    /// What the keychain offer needs to know of the system (the binary passes
    /// [`Host::current`]; without this call it is never offered). Call it before
    /// [`App::set_secret_stores`].
    pub fn set_keychain_host(&mut self, host: Host) {
        self.keychain_host = Some(host);
    }

    /// The guarded keychain of `stores`, timed when it is the OS's own on macOS.
    pub(super) fn timed_keychain(&self, guard: Arc<Guarded>, stores: &Stores) -> Arc<dyn SecretStore> {
        match &self.keychain_host {
            Some(h) if h.macos && stores.os_keychain => {
                let seen = self.keychain_seen.clone();
                Arc::new(Timed::new(guard, stores.access.clone(), self.clock.clone(), seen))
            }
            _ => guard,
        }
    }

    /// After each event: look at the keychain calls timed since, then ask if one most likely
    /// waited for the user and nothing is in the way.
    pub(super) fn keychain_offer_check(&mut self) {
        let seen = std::mem::take(&mut *self.keychain_seen.lock().unwrap_or_else(|e| e.into_inner()));
        if !seen.is_empty() {
            self.keychain_calls_seen(seen);
        }
        if self.keychain_offer_due && !self.overlays.modal() && self.conns.connecting().next().is_none() {
            self.keychain_offer_due = false;
            self.open_keychain_offer();
        }
    }

    /// The offer's host, its running binary and the state file, when all are known.
    fn keychain_offer_parts(&self) -> Option<(&Host, offer::BinaryId, std::path::PathBuf)> {
        let host = self.keychain_host.as_ref()?;
        Some((host, host.binary.clone()?, self.paths.keychain_offer_file()?))
    }

    fn keychain_calls_seen(&mut self, seen: Vec<offer::Observation>) {
        let Some((host, current, file)) = self.keychain_offer_parts() else { return };
        // Unreadable: neither offered nor written over.
        let Ok(mut state) = offer::load(&file) else { return };
        let (macos, remote) = (host.macos, offer::remote(|k| (self.env)(k)));
        let mut changed = false;
        for s in seen {
            let facts = Facts {
                macos,
                os_keychain: true,
                remote,
                access: self.prefs.keychain_access,
                state: Some(&state),
                current: &current,
                seen: s,
            };
            if offer::should_offer(&facts) {
                self.keychain_offer_due = true;
            }
            if s.trusted() && state.binary.as_ref() != Some(&current) {
                state.binary = Some(current.clone());
                changed = true;
            }
        }
        // A state file that cannot be written is logged once; the session stops trying.
        if changed
            && !self.keychain_state_unwritable
            && let Err(e) = offer::save(&file, &state)
        {
            self.keychain_state_unwritable = true;
            ErrorLog::new(self.paths.errors_log()).record("keychain.offer", &Fault::io_at(&e, &file));
        }
    }

    /// Ask, unless it was answered or the mode changed since it was due.
    fn open_keychain_offer(&mut self) {
        let Some((_, _, file)) = self.keychain_offer_parts() else { return };
        let open = offer::load(&file).is_ok_and(|s| s.answer.is_none());
        if !open || self.prefs.keychain_access != KeychainAccess::Direct {
            return;
        }
        self.key_state.clear();
        self.idle_event = false;
        self.overlays.push(Overlay::Confirm(Confirm {
            title: Label::KeychainAccessTitle,
            text: Label::KeychainOfferText.into(),
            details: Vec::new(),
            keys: Label::KeychainOfferKeys,
            action: ConfirmAction::KeychainOffer,
            folder: None,
            path: None,
            tx: None,
            buttons: Default::default(),
        }));
    }

    /// A key of the offer: `y` switches, `n` declines, `Esc`/`Enter` keep (nothing recorded).
    /// Keys typed as it came up are not taken (it opens by itself).
    pub(super) fn keychain_offer_key(&mut self, code: KeyCode) {
        let shown = self.overlays.confirm().and_then(|c| c.buttons.press.shown_at);
        if shown.is_none_or(|t| self.now().saturating_duration_since(t) < super::overlay::ARM_DELAY) {
            return;
        }
        match code {
            KeyCode::Char('y') => {
                self.overlays.close(OverlayKind::Confirm);
                // The config is written as the setting writes it; only once it is saved is the
                // answer recorded.
                if let Some(error) = self.apply_keychain_access(KeychainAccess::SecurityTool) {
                    return self.flash(error);
                }
                self.record_keychain_answer(Answer::Accepted);
                self.flash(Notice::new(Label::KeychainOfferSwitched, Level::Success));
            }
            KeyCode::Char('n') => {
                self.overlays.close(OverlayKind::Confirm);
                self.record_keychain_answer(Answer::Declined);
            }
            KeyCode::Esc | KeyCode::Enter => self.overlays.close(OverlayKind::Confirm),
            _ => {}
        }
    }

    /// Record `answer` in the state file, unless the file cannot be read (it is not written
    /// over then).
    fn record_keychain_answer(&mut self, answer: Answer) {
        let Some((_, _, file)) = self.keychain_offer_parts() else { return };
        let Ok(state) = offer::load(&file) else { return };
        let state = OfferState { answer: Some(answer), ..state };
        if let Err(e) = offer::save(&file, &state) {
            ErrorLog::new(self.paths.errors_log()).record("keychain.offer", &Fault::io_at(&e, &file));
        }
    }
}
