//! Tunnel presets in the app: the explorer's "Tunnels" section (each preset with the state of
//! its shared connection and the profiles that name it), the tunnel form (new, edit, copy),
//! deleting one (asked first, the profiles that name it listed), testing one (its SSH login and
//! a `direct-tcpip` channel to each database address of its profiles, nothing else), and "save
//! as tunnel preset" in the profile form.
//!
//! The config is only written by an explicit action: saving a form, a rename (the profiles that
//! name the preset follow it), a delete. A deleted preset's name stays in the profiles that
//! named it, which then do not connect (never directly) until another tunnel is picked. A
//! preset's secret lives under its own account (its id), so a rename keeps it; it is removed
//! with the preset.

use super::TunnelTest;
use super::chooser::{NameInput, NamePurpose};
use super::profiles::{FormKind, ProfileForm};
use super::tunnel::Probe;
use super::*;
use datarig_core::profile::ssh::SshSettings;
use datarig_core::profile::tunnel::{self as preset, TunnelId, TunnelPreset};
use datarig_core::secret::source;

/// At most this many database addresses are tried by a preset's test.
pub const PROBE_MAX: usize = 4;

/// A secret to move once the profile form that made a preset of its own tunnel is saved: from
/// the profile's tunnel account in the store it used (`from_kind`, as saved before the form) to
/// the preset's account in the store the preset uses (`to_kind`); `None`: a source that stores
/// none.
pub(super) struct SecretMove {
    pub name: String,
    pub from_kind: Option<SourceKind>,
    pub to_kind: Option<SourceKind>,
    pub from: String,
    pub to: String,
    pub typed: Option<String>,
}

/// Whether preset `id` (named `original` when the form opened) cannot be named `n`: another
/// preset has that name, or one a person would take for it (case aside). A name the file
/// already had, and that only differs in case from another's, may stay.
pub fn name_taken(presets: &[TunnelPreset], id: TunnelId, original: Option<&str>, n: &str) -> bool {
    presets.iter().any(|p| p.id != id && (p.name == n || (preset::same_name(&p.name, n) && original != Some(n))))
}

impl App {
    /// The explorer shows the "Tunnels" section: there are presets, or profiles and a config
    /// file the app can write (a preset can be made).
    pub fn tunnels_shown(&self) -> bool {
        !self.presets.is_empty() || (!self.profiles.is_empty() && self.config_writable())
    }

    /// Changes can be written to the config file (there is one, and it was read without
    /// errors).
    pub fn config_writable(&self) -> bool {
        self.config_path.is_some() && !self.config_broken
    }

    /// Preset `id`.
    pub fn preset(&self, id: TunnelId) -> Option<&TunnelPreset> {
        self.presets.iter().find(|p| p.id == id)
    }

    /// The preset the explorer's cursor is on (its row or its error line). A profile listed
    /// under a preset is only a way to that profile (`Enter`): no preset's action runs on it.
    pub fn selected_tunnel(&self) -> Option<TunnelId> {
        match self.explorer_row()?.kind {
            explorer::RowKind::Tunnel(id) | explorer::RowKind::TunnelError(id) => Some(id),
            _ => None,
        }
    }

    /// The cursor is in the "Tunnels" section (its head, a preset, or the empty note).
    pub fn in_tunnels_section(&self) -> bool {
        matches!(
            self.explorer_row().map(|r| r.kind),
            Some(
                explorer::RowKind::TunnelsHeader
                    | explorer::RowKind::Tunnel(_)
                    | explorer::RowKind::TunnelError(_)
                    | explorer::RowKind::TunnelUser(..)
                    | explorer::RowKind::TunnelsEmpty
            )
        )
    }

    /// `user@host[:port]` of `s`, as the explorer and the profile form show a bastion.
    pub fn bastion_text(s: &SshSettings) -> String {
        let port = if s.port == 22 { String::new() } else { format!(":{}", s.port) };
        format!("{}@{}{port}", s.user.trim(), s.host.trim())
    }

    // ── the form ────────────────────────────────────────────────────────────

    /// Open the tunnel form: a new preset, preset `id` to edit, or a copy of it.
    pub(super) fn open_tunnel_form(&mut self, id: Option<TunnelId>, duplicate: bool) {
        let p = id.and_then(|id| self.preset(id)).cloned();
        let taken = |n: &str| self.presets.iter().any(|q| preset::same_name(&q.name, n));
        let copy = p.as_ref().filter(|_| duplicate).map(|p| profiles::copy_name(&p.name, taken));
        let mut f = ProfileForm::for_tunnel(p.as_ref(), copy);
        f.keychain_ok = self.keychain_ok();
        if p.is_none() {
            // A new preset's secret starts in the store a new profile's password would.
            f.ssh_source = self.default_source.resolve(f.keychain_ok);
        }
        self.clear_test();
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.overlays.push(Overlay::ProfileForm(Box::new(f)));
    }

    /// `Ctrl+S` in the tunnel form: save the preset (a rename takes the profiles that name it
    /// along), then its secret: a typed one goes to its store; a store it left gives its copy
    /// to the new one (copied, then removed, on a worker for the keychain).
    pub(super) fn save_tunnel_form(&mut self) {
        let Some(form) = self.overlays.form_mut().filter(|f| !f.saving && f.is_tunnel()) else { return };
        form.attempted = true;
        let FormKind::Tunnel { id, editing } = form.kind else { return };
        let (presets, original) = (&self.presets, form.original_name.clone());
        let taken = |n: &str| name_taken(presets, id, original.as_deref(), n);
        if !form.errors(taken).is_empty() {
            return;
        }
        let Some(mut p) = form.to_preset() else { return };
        let typed = form.typed_bastion_secret();
        let old = self.presets.iter().position(|q| q.id == id).filter(|_| editing);
        let before = old.map(|i| self.presets[i].clone());
        let renamed = before.as_ref().map(|b| b.name.clone()).filter(|n| *n != p.name);
        p.origin = before.as_ref().and_then(|b| b.origin.clone());
        let (presets, profiles) = (self.presets.clone(), self.profiles.clone());
        match old {
            Some(i) => self.presets[i] = p.clone(),
            None => {
                self.presets.push(p.clone());
                self.presets.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            }
        }
        // Renamed: the profiles that named it name it still.
        if let Some(from) = &renamed {
            for c in self.profiles.iter_mut().filter(|c| c.tunnel.as_deref() == Some(from.as_str())) {
                c.tunnel = Some(p.name.clone());
            }
        }
        if let Err(fault) = self.try_persist() {
            // Nothing changed: the form stays open.
            self.presets = presets;
            self.profiles = profiles;
            let error = self.fault_text("config.save_failed", &fault);
            return self.flash(Notice::new(Msg::ConfigSaveFailed { error }, Level::Error));
        }
        for q in &mut self.presets {
            q.origin = Some(q.name.clone());
        }
        self.preset_secret_saved(before.as_ref(), &p, typed);
        self.clear_test();
        self.overlays.close(OverlayKind::ProfileForm);
        self.reveal_tunnel(p.id);
        // Profiles on its connection keep it as it was until they connect again.
        let in_use = self
            .shared
            .list
            .iter()
            .filter(|e| e.tunnel == p.id && e.open().is_some())
            .map(|e| e.users.len())
            .sum::<usize>();
        let changed = before.as_ref().is_some_and(|b| b.settings != p.settings);
        let msg = if changed && in_use > 0 {
            Notice::new(Msg::TunnelSavedInUse { name: p.name.clone(), count: in_use as u64 }, Level::Info)
        } else {
            Notice::new(Msg::TunnelSaved { name: p.name.clone() }, Level::Success)
        };
        self.flash(msg);
    }

    /// The secret of preset `p` after its form was saved (`before`: as it was).
    fn preset_secret_saved(&mut self, before: Option<&TunnelPreset>, p: &TunnelPreset, typed: Option<String>) {
        let account = p.id.account();
        let store = |s: &SshSettings| Some(s.source().kind()).filter(|k| s.auth.has_secret() && k.stores_secret());
        let (old, new) = (before.and_then(|b| store(&b.settings)), store(&p.settings));
        match (old, new) {
            // Another store: the old copy moves there (or goes, with nothing to keep it).
            (Some(from), to) if Some(from) != to => {
                self.secrets.forget(&account);
                match to {
                    Some(to) if typed.is_none() => return self.move_preset_secret_between(&p.name, &account, from, to),
                    _ => {
                        if from == SourceKind::Keychain {
                            self.remove_from_keychain(vec![account.clone()]);
                        } else if let Err(e) = self.secrets.remove_in(from, &account) {
                            let m = Notice::new(self.source_error(&e), Level::Warning);
                            self.flash(m);
                        }
                    }
                }
            }
            _ => {}
        }
        let (Some(kind), Some(text)) = (new, typed) else { return };
        self.secrets.forget(&account);
        if kind == SourceKind::Keychain {
            self.save_preset_secret_to_keychain(p.id, p.name.clone(), account, text, false);
        } else if let Err(e) = self.secrets.save_in(kind, &account, &text) {
            let m = Notice::new(self.source_error(&e), Level::Warning);
            self.flash(m);
        }
    }

    /// Preset `name`'s secret moves from store `from` to store `to` (same account): copied and
    /// read back, then the old copy removed; a copy that failed keeps the old one.
    fn move_preset_secret_between(&mut self, name: &str, account: &str, from: SourceKind, to: SourceKind) {
        let (name, acc) = (name.to_string(), account.to_string());
        let work = move |s: &Stores| {
            source::switch_copy(s, from, to, &acc, None).map(|copied| {
                let mut out = if copied { source::switch_remove(s, from, to, &acc) } else { Default::default() };
                out.copied = copied;
                out
            })
        };
        if from == SourceKind::Keychain || to == SourceKind::Keychain {
            self.keychain_job(vec![account.to_string()], work, move |result| keychain::KeychainDone::SecretMoved {
                name,
                result,
            });
        } else {
            let result = work(self.secrets.stores());
            self.on_secret_moved(name, result);
        }
    }

    /// Put the explorer's cursor on preset `id`, opening the section.
    pub(super) fn reveal_tunnel(&mut self, id: TunnelId) {
        self.tunnels_expanded = true;
        self.explorer.select_kind(explorer::RowKind::Tunnel(id));
        let rows = self.explorer_rows();
        let i = self.explorer.index(&rows);
        self.explorer.select(&rows, i);
    }

    // ── delete ──────────────────────────────────────────────────────────────

    /// `d` on a preset: ask first, listing the profiles that name it (they will not connect
    /// until another tunnel is picked) and where its saved secret is removed from.
    pub(super) fn request_delete_tunnel(&mut self, id: TunnelId) {
        let Some(p) = self.preset(id).cloned() else { return };
        let mut details = Vec::new();
        let users: Vec<String> = self.preset_users(&p.name).iter().map(|c| c.name.clone()).collect();
        if !users.is_empty() {
            details.push(Msg::TunnelDeleteUsers { count: users.len() as u64, names: users.join(", ") });
        }
        if matches!(self.preset_state(id), tunnel::PresetState::Open(_)) {
            details.push(Label::TunnelDeleteOpen.into());
        }
        if p.settings.auth.has_secret() {
            match p.settings.source().kind() {
                SourceKind::Keychain => details.push(Label::TunnelDeleteSecretKeychain.into()),
                SourceKind::File => details.push(Label::TunnelDeleteSecretFile.into()),
                _ => {}
            }
        }
        self.overlays.close(OverlayKind::Commands);
        self.overlays.push(Overlay::Confirm(Confirm {
            title: Label::ExplorerDeleteTitle,
            text: Msg::TunnelDelete { name: p.name.clone() },
            details,
            keys: Label::ExplorerDeleteKeys,
            action: ConfirmAction::DeleteTunnel(id),
            folder: None,
            path: None,
        }));
    }

    /// Delete preset `id` (confirmed). The profiles that name it keep the name (they do not
    /// connect until another tunnel is picked); connections through it stay until their
    /// profiles disconnect; its saved secret is removed.
    pub(super) fn delete_tunnel(&mut self, id: TunnelId) {
        let Some(i) = self.presets.iter().position(|p| p.id == id) else { return };
        let rows = self.explorer_rows();
        let at = rows.iter().position(|r| r.kind == explorer::RowKind::Tunnel(id)).unwrap_or(0);
        let p = self.presets.remove(i);
        if let Err(fault) = self.try_persist() {
            self.presets.insert(i, p);
            let error = self.fault_text("config.save_failed", &fault);
            return self.flash(Notice::new(Msg::ConfigSaveFailed { error }, Level::Error));
        }
        self.give_up_openings(id, &p.name);
        let account = id.account();
        self.secrets.forget(&account);
        let mut warn = None;
        if p.settings.auth.has_secret() {
            match p.settings.source().kind() {
                SourceKind::Keychain => self.remove_from_keychain(vec![account]),
                SourceKind::File => {
                    if let Err(e) = self.secrets.remove_in(SourceKind::File, &account) {
                        warn = Some(Notice::new(self.source_error(&e), Level::Warning));
                    }
                }
                _ => {}
            }
        }
        self.tunnels_open.remove(&id);
        self.shared.lost.remove(&id);
        let rows = self.explorer_rows();
        self.explorer.select(&rows, at.saturating_sub(1));
        let count = self.preset_users(&p.name).len() as u64;
        let msg = match count {
            0 => Notice::new(Msg::TunnelDeleted { name: p.name }, Level::Success),
            _ => Notice::new(Msg::TunnelDeletedUsed { name: p.name, count }, Level::Warning),
        };
        self.flash(warn.unwrap_or(msg));
    }

    // ── test ────────────────────────────────────────────────────────────────

    /// Test preset `id` as saved (its stored secret).
    pub(super) fn test_tunnel(&mut self, id: TunnelId) {
        let Some(p) = self.preset(id).cloned() else { return };
        self.start_preset_test(&p, None);
    }

    /// "Test" in the tunnel form: the form's settings, with a secret typed there.
    pub(super) fn test_tunnel_form(&mut self) {
        let Some(f) = self.overlays.form_mut().filter(|f| f.is_tunnel()) else { return };
        let was = f.attempted;
        f.attempted = true;
        if let Some((field, _)) = f.ssh_errors().into_iter().next() {
            f.focus_field(field);
            self.clear_test();
            return self.flash(Notice::new(Label::TestFieldsMissing, Level::Warning));
        }
        f.attempted = was;
        let typed = f.typed_bastion_secret();
        let Some(mut p) = f.to_preset() else { return };
        // A preset being renamed is still used by the profiles of its old name.
        if let Some(old) = f.original_name.clone() {
            p.name = old;
        }
        self.start_preset_test(&p, typed);
    }

    /// Test preset `p`: a throwaway SSH connection of its settings (the stages show, its
    /// questions are asked as a connect's, nothing typed is saved), then one `direct-tcpip`
    /// channel to each database address of the profiles that name it (at most [`PROBE_MAX`]),
    /// opened and closed again: nothing is sent to a database.
    pub(super) fn start_preset_test(&mut self, p: &TunnelPreset, typed: Option<String>) {
        self.clear_test();
        self.test_seq += 1;
        let seq = self.test_seq;
        let mut targets: Vec<(String, u16)> = Vec::new();
        for c in self.preset_users(&p.name) {
            let (addr, _, _) = c.endpoint();
            let target = match addr.rsplit_once(':') {
                Some((h, port)) if c.dsn.is_none() => {
                    (h.trim_matches(['[', ']']).to_string(), port.parse().unwrap_or(c.port))
                }
                _ => (c.host.clone(), c.port),
            };
            if !targets.contains(&target) && targets.len() < PROBE_MAX {
                targets.push(target);
            }
        }
        let settings = p.settings.clone();
        let tunnel = Some(TunnelTest {
            host: settings.host.trim().to_string(),
            name: p.name.clone(),
            settings: settings.clone(),
            stage: None,
            opened: None,
            how: None,
            failure: None,
        });
        let mut state = TestState::Running;
        let abort = match (self.tunnels.clone(), self.tx.clone()) {
            (None, _) => {
                state = TestState::Failed(self.i18n.label(Label::SshUnavailable).to_string());
                None
            }
            (Some(tunnels), Some(tx)) => {
                let request =
                    self.tunnel_request(p.id.account(), settings, tx.clone(), tunnel::Owner::Test(seq), typed);
                let open = tunnels.open(request);
                let task = tokio::spawn(async move {
                    let start = Instant::now();
                    let t = match open.await {
                        Ok(t) => t,
                        Err(e) => {
                            let _ = tx.send(AppEvent::TestTunnel { seq, ev: tunnel::TunnelEvent::Failed(e) });
                            return;
                        }
                    };
                    let _ = tx.send(AppEvent::TestTunnel {
                        seq,
                        ev: tunnel::TunnelEvent::Through(start.elapsed(), t.opened()),
                    });
                    let mut probes = Vec::new();
                    for (host, port) in targets {
                        let at = Instant::now();
                        let result = match tokio::time::timeout(TEST_TIMEOUT, t.dial(&host, port)).await {
                            // Opened, then closed at once: nothing is sent through it.
                            Ok(Ok(stream)) => {
                                drop(stream);
                                Ok(at.elapsed())
                            }
                            Ok(Err(e)) => Err(e),
                            Err(_) => Err(datarig_core::transport::DialError::Timeout(TEST_TIMEOUT)),
                        };
                        probes.push(Probe { host, port, result });
                    }
                    t.close().await;
                    let _ = tx.send(AppEvent::TestProbe { seq, probes });
                });
                Some(task.abort_handle())
            }
            // Headless: recorded; the test feeds the events.
            (Some(_), None) => {
                self.test_tunnel_requests.push(seq);
                None
            }
        };
        self.conn_test = Some(ConnTest { seq, started: Instant::now(), state, abort, tunnel, probe: true });
        self.report_test();
    }

    /// What test `seq` of a preset reached through it.
    pub(super) fn on_test_probe(&mut self, seq: u64, probes: Vec<Probe>) {
        if !self.conn_test.as_ref().is_some_and(|t| t.seq == seq && t.state == TestState::Running) {
            return;
        }
        let reached = probes
            .iter()
            .map(|p| {
                let host = if p.host.contains(':') { format!("[{}]", p.host) } else { p.host.clone() };
                (format!("{host}:{}", p.port), p.result.clone().map_err(|e| self.dial_error_text(&e)))
            })
            .collect();
        if let Some(t) = self.conn_test.as_mut() {
            t.abort = None;
            t.state = TestState::Reached(reached);
        }
        self.report_test();
    }

    // ── save as tunnel preset ───────────────────────────────────────────────

    /// `Ctrl+B` in the profile form's own tunnel: name the preset its bastion fields become when
    /// the form is saved. The fields must be complete first.
    pub(super) fn open_save_as_tunnel(&mut self) {
        if !self.config_writable() {
            return self.flash(Notice::new(Label::ConfigReadonly, Level::Warning));
        }
        let Some(f) = self.overlays.form_mut().filter(|f| !f.is_tunnel() && f.ssh_enabled) else { return };
        let was = f.attempted;
        f.attempted = true;
        if let Some((field, _)) = f.ssh_errors().into_iter().next() {
            f.focus_field(field);
            return self.flash(Notice::new(Label::TunnelSaveAsFieldsMissing, Level::Warning));
        }
        f.attempted = was;
        let host = f.ssh_host.text().trim().to_string();
        let taken = |n: &str| self.presets.iter().any(|p| preset::same_name(&p.name, n));
        // The bastion's first name (`bastion` of `bastion.example.com`); an address names nothing.
        let ip = host.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok();
        let base = host.split('.').next().filter(|h| !h.is_empty() && !ip).unwrap_or("tunnel").to_string();
        let name = if taken(&base) { profiles::copy_name(&base, taken) } else { base };
        self.overlays.push(Overlay::NameInput(NameInput {
            title: Label::TunnelSaveAsTitle.into(),
            input: TextInput::new(&name),
            error: None,
            purpose: NamePurpose::SaveAsTunnel,
        }));
    }

    /// `Enter` in the name of "save as tunnel preset": checked, then the profile form picks the
    /// preset it will make.
    pub(super) fn save_as_tunnel_named(&mut self, name: &str) -> Result<(), Label> {
        match preset::name_problem(name) {
            Some(preset::NameProblem::Empty) => return Err(Label::ValidateFolderEmpty),
            Some(_) => return Err(Label::ValidateTunnelName),
            None => {}
        }
        if self.presets.iter().any(|p| preset::same_name(&p.name, name)) {
            return Err(Label::ValidateTunnelTaken);
        }
        let Some(f) = self.overlays.form_mut() else { return Ok(()) };
        f.save_as_preset(name);
        Ok(())
    }

    /// The profile form made preset `p` of the profile's own tunnel and was saved: the
    /// profile's tunnel secret moves to the preset (copied and read back, then the old copy
    /// removed; a typed one is written instead). Unknown is not absent: a store that cannot be
    /// read keeps the old copy, and the preset asks for its secret at its first connect.
    pub(super) fn move_secret_to_preset(&mut self, m: SecretMove) {
        // The session's copy follows (a secret typed in the form is the newer one).
        let session = m.typed.clone().or_else(|| self.secrets.session(&m.from).map(str::to_string));
        if let Some(pw) = session.filter(|_| m.to_kind.is_some()) {
            self.secrets.remember(&m.to, &pw);
        }
        self.secrets.forget(&m.from);
        let SecretMove { name, from_kind, to_kind, from, to, typed } = m;
        let keychain = from_kind == Some(SourceKind::Keychain) || to_kind == Some(SourceKind::Keychain);
        let accounts = vec![from.clone(), to.clone()];
        let work = move |s: &Stores| {
            let copied = match to_kind {
                Some(k) => source::rekey_copy(s, from_kind, &from, k, &to, typed.as_deref())?,
                None => false,
            };
            let mut out = from_kind.map(|k| source::rekey_remove(s, k, &from)).unwrap_or_default();
            out.copied = copied;
            Ok(out)
        };
        if keychain {
            self.keychain_job(accounts, work, move |result| keychain::KeychainDone::SecretMoved { name, result });
        } else {
            let result = work(self.secrets.stores());
            self.on_secret_moved(name, result);
        }
    }
}
