//! The shared SSH connections of tunnel presets: opening one for the attempts that wait, taking
//! an open one, letting it go, and its loss (see the module docs of [`super`]).

use super::*;

impl App {
    /// Open (or take) preset `preset`'s shared connection for profile `conn`'s attempt in
    /// progress; `cfg` (the profile with its resolved database password) waits for it.
    pub(super) fn open_shared(&mut self, conn: &ConnectionConfig, preset: TunnelPreset, cfg: ConnectionConfig) {
        let id = conn.id;
        // Whatever it held before (another preset, settings changed since) is let go.
        self.close_tunnel(id);
        for e in &mut self.shared.list {
            e.waiting.retain(|(p, _)| *p != id);
        }
        let generation = self.conns.entry(id).generation;
        self.conns.entry(id).tunnel_wait = Some(cfg);
        // An open one of the preset with these settings: taken as it is.
        let same = |e: &SharedTunnel| e.tunnel == preset.id && e.settings == preset.settings;
        if let Some(serial) = self.shared.list.iter().find(|e| same(e) && e.open().is_some()).map(|e| e.serial) {
            self.prune_shared();
            return self.attach_shared(id, serial);
        }
        let opening = self.shared.list.iter_mut().find(|e| same(e) && matches!(e.state, SharedState::Opening(_)));
        if let Some(e) = opening {
            e.waiting.push((id, generation));
            let stage = match e.state {
                SharedState::Opening(s) => s,
                SharedState::Open(_) => Stage::Connecting,
            };
            if let Some(k) = self.conns.entry(id).connecting.as_mut() {
                k.tunnel = Some(stage);
            }
            return self.prune_shared();
        }
        self.shared.seq += 1;
        let serial = self.shared.seq;
        self.shared.list.push(SharedTunnel {
            serial,
            tunnel: preset.id,
            settings: preset.settings.clone(),
            state: SharedState::Opening(Stage::Connecting),
            users: BTreeSet::new(),
            waiting: vec![(id, generation)],
        });
        self.prune_shared();
        if let Some(k) = self.conns.entry(id).connecting.as_mut() {
            k.tunnel = Some(Stage::Connecting);
        }
        let Some(tunnels) = self.tunnels.clone() else {
            // No tunnels in this build (headless tests without a fake): never directly.
            self.shared.remove(serial);
            self.conns.entry(id).tunnel_wait = None;
            return self.attempt_failed(id, Notice::new(Label::SshUnavailable, Level::Error));
        };
        let Some(tx) = self.tx.clone() else {
            // Headless: the test feeds the connection's events; the request is recorded.
            self.shared_tunnel_requests.push(serial);
            return;
        };
        let request =
            self.tunnel_request(preset.id.account(), preset.settings, tx.clone(), Owner::Shared(serial), None);
        let open = tunnels.open(request);
        tokio::spawn(async move {
            let ev = match open.await {
                Ok(t) => TunnelEvent::Opened(t),
                Err(e) => TunnelEvent::Failed(e),
            };
            let _ = tx.send(AppEvent::SharedTunnel { serial, ev });
        });
    }

    /// An event of the shared connection `serial`.
    pub(crate) fn on_shared_event(&mut self, serial: u64, ev: TunnelEvent) {
        let Some(e) = self.shared.get(serial) else {
            // Its attempts are over: a late connection closes at once.
            if let TunnelEvent::Opened(t) = ev {
                self.close_handle(t);
            }
            return;
        };
        let (tunnel, settings) = (e.tunnel, e.settings.clone());
        match ev {
            TunnelEvent::Stage(stage) => {
                if let Some(e) = self.shared.get_mut(serial)
                    && matches!(e.state, SharedState::Opening(_))
                {
                    e.state = SharedState::Opening(stage);
                }
                let waiting: Vec<ProfileId> = self.current_waiters(serial).into_iter().map(|(p, _)| p).collect();
                for p in &waiting {
                    if let Some(k) = self.conns.get_mut(*p).and_then(|c| c.connecting.as_mut()) {
                        k.tunnel = Some(stage);
                    }
                }
                if let Some(name) = waiting.first().and_then(|p| self.profile(*p)).map(|p| p.name.clone()) {
                    let host = settings.host.trim().to_string();
                    let stage = self.i18n.label(stage_label(stage)).to_string();
                    self.show_status(Notice::new(Msg::SshConnecting { name, host, stage }, Level::Info));
                }
            }
            // Questions nobody waits for any more are dropped (which cancels them).
            TunnelEvent::Ask(_) if !self.owner_current(Owner::Shared(serial)) => {}
            TunnelEvent::Ask(ask) => {
                let name = self.preset_name(tunnel).unwrap_or_else(|| settings.host.trim().to_string());
                self.queue_ask(Owner::Shared(serial), name, settings, ask);
            }
            TunnelEvent::Opened(handle) => self.shared_opened(serial, handle),
            TunnelEvent::Failed(e) => self.shared_failed(serial, e),
            TunnelEvent::Lost(loss) => self.shared_lost(serial, loss),
            TunnelEvent::Through(..) => {}
        }
    }

    /// The name of preset `id`, if it is still there.
    pub(crate) fn preset_name(&self, id: TunnelId) -> Option<String> {
        self.presets.iter().find(|p| p.id == id).map(|p| p.name.clone())
    }

    /// The attempts that wait for the shared connection `serial` and are still current.
    fn current_waiters(&self, serial: u64) -> Vec<(ProfileId, u64)> {
        let Some(e) = self.shared.get(serial) else { return Vec::new() };
        e.waiting.iter().copied().filter(|w| self.waits_for_tunnel(*w)).collect()
    }

    /// The names of the profiles waiting for the shared connection `serial`.
    pub(super) fn shared_waiters(&self, serial: u64) -> Vec<String> {
        self.current_waiters(serial).into_iter().filter_map(|(p, _)| self.profile(p).map(|p| p.name.clone())).collect()
    }

    fn shared_opened(&mut self, serial: u64, handle: Arc<dyn OpenTunnel>) {
        let waiting = self.current_waiters(serial);
        let Some(e) = self.shared.get_mut(serial).filter(|e| matches!(e.state, SharedState::Opening(_))) else {
            return self.close_handle(handle);
        };
        if waiting.is_empty() {
            self.shared.remove(serial);
            self.shared_saves.remove(&serial);
            return self.close_handle(handle);
        }
        e.state = SharedState::Open(handle.clone());
        e.waiting.clear();
        let tunnel = e.tunnel;
        self.shared.lost.remove(&tunnel);
        if let Some(tx) = self.tx.clone() {
            let lost = handle.lost();
            tokio::spawn(async move {
                let loss = lost.await;
                let _ = tx.send(AppEvent::SharedTunnel { serial, ev: TunnelEvent::Lost(loss) });
            });
        }
        self.save_shared_secret(serial);
        for (p, _) in waiting {
            self.attach_shared(p, serial);
        }
        // None of them took it (each was edited meanwhile): nobody needs it.
        if self.shared.get(serial).is_some_and(|e| e.users.is_empty())
            && let Some(SharedTunnel { state: SharedState::Open(h), .. }) = self.shared.remove(serial)
        {
            self.close_handle(h);
        }
    }

    /// Profile `id`'s attempt takes the open shared connection `serial` and goes on, if the
    /// profile still names that preset with those settings (it may have been edited, or the
    /// preset changed or deleted, while it opened); otherwise the attempt ends.
    fn attach_shared(&mut self, id: ProfileId, serial: u64) {
        let Some((tunnel, settings)) = self.shared.get(serial).map(|e| (e.tunnel, e.settings.clone())) else { return };
        let still = self.profile(id).is_some_and(
            |p| matches!(self.route_of(p), Ok(Route::Preset(t)) if t.id == tunnel && t.settings == settings),
        );
        if !still {
            let name = self.profile(id).map(|p| p.name.clone()).unwrap_or_default();
            if let Some(c) = self.conns.get_mut(id) {
                c.tunnel_wait = None;
            }
            if let Some(e) = self.shared.get_mut(serial) {
                e.waiting.retain(|(p, _)| *p != id);
            }
            self.attempt_failed(id, Notice::new(Msg::ConnCancelled { name }, Level::Warning));
            return self.prune_shared();
        }
        let Some(e) = self.shared.get_mut(serial) else { return };
        let Some(handle) = e.open().cloned() else { return };
        e.users.insert(id);
        let (settings, tunnel) = (e.settings.clone(), e.tunnel);
        let now = self.now();
        let c = self.conns.entry(id);
        c.tunnel = Some(ProfileTunnel { handle, settings, shared: Some((serial, tunnel)) });
        c.tunnel_lost = None;
        let cfg = c.tunnel_wait.take();
        if let Some(k) = c.connecting.as_mut() {
            // The database's own connect timeout starts now.
            k.tunnel = None;
            k.started = now;
        }
        if let Some(cfg) = cfg {
            let pw = cfg.password.clone();
            self.start_connect(cfg, pw);
        }
    }

    fn shared_failed(&mut self, serial: u64, e: TunnelFailure) {
        let waiting = self.current_waiters(serial);
        let Some(entry) = self.shared.remove(serial) else { return };
        self.shared_saves.remove(&serial);
        let name = self.preset_name(entry.tunnel).unwrap_or_default();
        let m = self.tunnel_failure_notice(&e, &name);
        for (p, _) in &waiting {
            if let Some(c) = self.conns.get_mut(*p) {
                c.tunnel_wait = None;
            }
            self.attempt_failed(*p, m.clone());
        }
        if !waiting.is_empty() {
            self.status = Some(m);
        }
    }

    /// The shared connection `serial` ended: every profile on it ends with it (each connects
    /// again on its next use), and the preset's row says why.
    fn shared_lost(&mut self, serial: u64, loss: Loss) {
        let Some(e) = self.shared.remove(serial) else { return };
        let mine = |c: &ProfileConn| c.tunnel.as_ref().and_then(|t| t.shared).map(|(s, _)| s) == Some(serial);
        let users: Vec<ProfileId> = e.users.iter().copied().filter(|p| self.conns.get(*p).is_some_and(mine)).collect();
        for p in &users {
            if let Some(c) = self.conns.get_mut(*p) {
                c.tunnel = None;
            }
        }
        if loss == Loss::ClosedByApp {
            return;
        }
        let m = self.loss_notice(&e.settings.host, &loss);
        self.shared.lost.insert(e.tunnel, m.clone());
        for p in users {
            self.lost_through(p, m.clone());
        }
    }

    /// Preset `tunnel` was deleted: its connections still opening are given up (their questions
    /// dropped, a late connection closed), and the attempts waiting for them end saying the
    /// preset is gone. Open ones stay with the profiles on them until they disconnect.
    pub(crate) fn give_up_openings(&mut self, tunnel: TunnelId, name: &str) {
        let openings: Vec<u64> = self
            .shared
            .list
            .iter()
            .filter(|e| e.tunnel == tunnel && matches!(e.state, SharedState::Opening(_)))
            .map(|e| e.serial)
            .collect();
        for serial in openings {
            let waiting = self.current_waiters(serial);
            self.shared.remove(serial);
            self.shared_saves.remove(&serial);
            self.drop_asks(|o| o == Owner::Shared(serial));
            for (p, _) in waiting {
                let Some(profile) = self.profile(p).map(|c| c.name.clone()) else { continue };
                if let Some(c) = self.conns.get_mut(p) {
                    c.tunnel_wait = None;
                }
                let m = self.route_error_notice(&profile, &RouteError::NotFound(name.to_string()));
                self.attempt_failed(p, m);
            }
        }
    }

    /// Profile `id` lets go of the shared connection `serial`; the last one closes it.
    pub(super) fn release_shared(&mut self, serial: u64, id: ProfileId) {
        let Some(e) = self.shared.get_mut(serial) else { return };
        e.users.remove(&id);
        if !e.users.is_empty() {
            return;
        }
        if let Some(e) = self.shared.remove(serial)
            && let SharedState::Open(h) = e.state
        {
            self.close_handle(h);
        }
    }

    /// Profile `id`'s attempt no longer waits for a shared connection; an opening that nobody
    /// waits for any more is given up (its questions dropped, a late connection closed).
    pub(super) fn leave_shared_waits(&mut self, id: ProfileId) {
        for e in &mut self.shared.list {
            e.waiting.retain(|(p, _)| *p != id);
        }
        self.prune_shared();
    }

    /// Give up the openings no current attempt waits for.
    fn prune_shared(&mut self) {
        let gone: Vec<u64> = self
            .shared
            .list
            .iter()
            .filter(|e| matches!(e.state, SharedState::Opening(_)))
            .filter(|e| !e.waiting.iter().any(|w| self.waits_for_tunnel(*w)))
            .map(|e| e.serial)
            .collect();
        for serial in gone {
            self.shared.remove(serial);
            self.shared_saves.remove(&serial);
            self.drop_asks(|o| o == Owner::Shared(serial));
        }
    }

    /// Where preset `id`'s connection is.
    pub fn preset_state(&self, id: TunnelId) -> PresetState {
        let mine = || self.shared.list.iter().filter(move |e| e.tunnel == id);
        if let Some(n) = mine().filter(|e| e.open().is_some()).map(|e| e.users.len()).max() {
            return PresetState::Open(n);
        }
        if mine().any(|e| self.owner_current(Owner::Shared(e.serial))) {
            return PresetState::Opening;
        }
        if self.shared.lost.contains_key(&id) { PresetState::Dead } else { PresetState::Closed }
    }

    /// The profiles that name preset `name`.
    pub fn preset_users(&self, name: &str) -> Vec<&ConnectionConfig> {
        let mut users: Vec<&ConnectionConfig> =
            self.profiles.iter().filter(|p| p.tunnel.as_deref() == Some(name)).collect();
        users.sort_by_key(|p| p.name.to_lowercase());
        users
    }

    /// Write a secret typed for the shared connection `serial` to the preset's store, now that
    /// it opened.
    fn save_shared_secret(&mut self, serial: u64) {
        let Some((kind, text)) = self.shared_saves.remove(&serial) else { return };
        let Some(tunnel) = self.shared.get(serial).map(|e| e.tunnel) else { return };
        let Some(name) = self.preset_name(tunnel) else { return };
        let account = tunnel.account();
        if kind == SourceKind::Keychain {
            self.save_preset_secret_to_keychain(tunnel, name, account, text, true);
        } else {
            let m = match self.secrets.save_in(kind, &account, &text) {
                Ok(()) => Notice::new(Msg::TunnelSecretSavedFile { name }, Level::Success),
                Err(e) => Notice::new(self.source_error(&e), Level::Warning),
            };
            self.flash(m);
        }
    }
}
