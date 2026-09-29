//! Connections of the profiles: connecting with the
//! profile's password source, what happens once a profile connects (console tab, pending
//! statements, saving a typed password), disconnecting, deleting profiles and folders, opening
//! a table in its profile's tab, test connections and the profile form.

use super::*;
use crate::widgets::tree::Tree;
use datarig_core::profile::folder::FolderPath;
use datarig_core::profile::ssh::SshSettings;

impl App {
    /// Connect to profile `id` with its source's password: at once for the secrets file and
    /// environment passwords, after a keychain read or a password command (off the UI thread),
    /// or after the prompt (`prompt` source). What waits for the attempt (a console,
    /// statements) stays with it.
    pub(super) fn connect(&mut self, id: ProfileId) {
        let Some(conn) = self.profile(id).cloned() else { return };
        self.close_profile_sessions(id);
        self.conns.next_generation(id);
        self.notices.clear();
        let c = self.conns.entry(id);
        c.error = None;
        c.connected = false;
        c.connecting = None;
        c.cache_off_said = false;
        if self.driver(&conn.driver).is_none() {
            let m = Notice::new(Msg::ConnUnknownDriver { driver: conn.driver.clone() }, Level::Error);
            return self.attempt_failed(id, m);
        }
        self.show_status(Notice::new(Msg::ConnConnecting { name: conn.name.clone() }, Level::Info));
        match self.plan_password(&conn) {
            password::Plan::Ready(pw) => self.start_connect(conn, pw),
            password::Plan::Failed(e) => self.source_failed(id, e),
            password::Plan::Prompt => {
                let every = Notice::new(Label::PromptPasswordEveryTime, Level::Info);
                self.open_prompt(&conn, every, PromptPurpose::Connect);
            }
            password::Plan::Keychain { account, legacy } => self.connect_with_keychain(conn, account, legacy),
            password::Plan::Command(cmd) => {
                let started = self.now();
                let c = self.conns.entry(id);
                c.connecting = Some(Connecting {
                    name: conn.name.clone(),
                    started,
                    had_password: true,
                    keychain: false,
                    tunnel: None,
                });
                let generation = c.generation;
                let Some(tx) = self.tx.clone() else {
                    // Headless: run it here.
                    let r = datarig_core::secret::command::run(&cmd, datarig_core::secret::command::TIMEOUT);
                    return self.on_resolved(id, r.map(Secret).map_err(SourceError::Command));
                };
                tokio::task::spawn_blocking(move || {
                    let r = datarig_core::secret::command::run(&cmd, datarig_core::secret::command::TIMEOUT);
                    let result = r.map(Secret).map_err(SourceError::Command);
                    let _ = tx.send(AppEvent::Resolved { profile: id, generation, result });
                });
            }
        }
    }

    /// Hand the profile and its password to the driver: the profile's metadata session (the
    /// attempt of the current generation). Tabs open their query sessions when they first run
    /// a statement.
    pub(super) fn start_connect(&mut self, conn: ConnectionConfig, password: String) {
        if self.driver(&conn.driver).is_none() {
            return;
        }
        let mut cfg = conn.clone();
        cfg.password = password;
        // Through its SSH tunnel: opened first (or the open one, when its settings are the
        // same); the attempt goes on once it is open.
        if let Some(settings) = conn.tunnel()
            && !self.tunnel_ready(conn.id, settings)
        {
            let started = self.now();
            let c = self.conns.entry(conn.id);
            c.connecting = Some(Connecting {
                name: conn.name.clone(),
                started,
                had_password: !cfg.password.is_empty(),
                keychain: false,
                tunnel: Some(datarig_ssh::tunnel::Stage::Connecting),
            });
            return self.open_tunnel(&conn, cfg);
        }
        let started = self.now();
        let c = self.conns.entry(conn.id);
        c.connecting = Some(Connecting {
            name: conn.name.clone(),
            started,
            had_password: !cfg.password.is_empty(),
            keychain: false,
            tunnel: None,
        });
        let generation = c.generation;
        let meta =
            self.open_session(&cfg, SessionRole::Meta, EventTarget::Meta(cfg.id), generation, Default::default());
        let c = self.conns.entry(conn.id);
        c.meta = meta;
        c.resolved = Some(cfg);
    }

    /// Reopen the metadata session of profile `id` after it was lost (a new generation; the
    /// tree and catalog reload).
    pub(super) fn reopen_meta(&mut self, id: ProfileId) {
        let Some(cfg) = self.conns.get(id).and_then(|c| c.resolved.clone()) else { return };
        let generation = self.conns.next_generation(id);
        let meta = self.open_session(&cfg, SessionRole::Meta, EventTarget::Meta(id), generation, Default::default());
        let c = self.conns.entry(id);
        c.meta = meta;
        c.tree = Tree::new();
    }

    /// Tests: the first profile connecting in the workspace, its node open and every tab on
    /// it (a console with `text` when there is none), the editor focused, with its metadata
    /// session open through the test drivers ([`App::set_drivers`]); the test then feeds
    /// `Connected`.
    pub fn attach_workspace(&mut self, text: &str) {
        let Some(p) = self.profiles.first().cloned() else { return };
        let ids: Vec<TabId> = self.tabs.iter().map(|t| t.id).collect();
        for t in ids {
            self.tabs.bind(t, Some(p.id));
        }
        if self.tabs.is_empty() {
            self.tabs.open(TabKind::Console, Some(p.id), Editor::new(text));
        }
        let generation = self.conns.next_generation(p.id);
        let meta = self.open_session(&p, SessionRole::Meta, EventTarget::Meta(p.id), generation, Default::default());
        let started = self.now();
        let c = self.conns.entry(p.id);
        c.meta = meta;
        c.resolved = Some(p.clone());
        c.expanded = true;
        c.connecting =
            Some(Connecting { name: p.name.clone(), started, had_password: true, keychain: false, tunnel: None });
        self.reveal_profile(p.id);
        self.focus = Focus::Editor;
    }

    /// Open a new query session for tab `id` (a new generation; the old session is closed).
    pub(super) fn open_tab_session(&mut self, id: TabId, cfg: &ConnectionConfig) {
        let generation = self.tabs.next_generation();
        let Some(t) = self.tabs.get_mut(id) else { return };
        if let Some(s) = t.exec.session.take() {
            s.close();
        }
        t.exec.generation = generation;
        t.exec.prepared = Default::default();
        t.exec.unconfirmed.clear();
        t.exec.context = None;
        t.exec.path_per_transaction = false;
        let context = t.context.clone();
        let session = self.open_session(cfg, SessionRole::Query, EventTarget::Tab(id), generation, context);
        let read_only = self.session_read_only(cfg);
        if let Some(t) = self.tabs.get_mut(id) {
            t.exec.state = if session.is_some() { SessionState::Opening } else { SessionState::Idle };
            t.exec.session = session;
            t.exec.read_only = read_only;
        }
    }

    /// Ask the driver for a session whose events come back tagged with `target` and
    /// `generation`. Headless (no event channel) nothing is opened.
    pub(super) fn open_session(
        &self,
        cfg: &ConnectionConfig,
        role: SessionRole,
        target: EventTarget,
        generation: u64,
        context: datarig_core::driver::SessionContext,
    ) -> Option<Session> {
        let driver = self.driver(&cfg.driver)?;
        let (etx, mut erx) = unbounded_channel();
        match self.tx.clone() {
            Some(tx) => {
                tokio::spawn(async move {
                    while let Some(ev) = erx.recv().await {
                        if tx.send(AppEvent::Db { target, generation, ev }).is_err() {
                            break;
                        }
                    }
                });
            }
            // Headless with the built-in drivers: attempts are recorded, not made.
            None if self.drivers.is_none() => return None,
            // Headless with test drivers: the test feeds the events.
            None => drop(erx),
        }
        let opts = ConnectOptions::new(self.page_size, role, &self.instance)
            .read_only(self.session_read_only(cfg))
            .context(context)
            .statement_cache(cfg.statement_cache)
            .dialer(self.dialer_of(cfg));
        Some(driver.connect(cfg, role, opts, etx))
    }

    /// Close the query sessions of the tabs `which` selects. The tabs move to a new
    /// generation, so a late event of a closed session (e.g. `TxOpen(true)`) is dropped.
    fn close_tab_sessions(&mut self, which: impl Fn(&Tab) -> bool) {
        let generation = self.tabs.next_generation();
        for t in self.tabs.iter_mut().filter(|t| which(t)) {
            if let Some(s) = t.exec.session.take() {
                s.close();
            }
            t.exec.state = SessionState::Idle;
            t.exec.generation = generation;
        }
    }

    /// Close profile `id`'s metadata session and the query sessions of its tabs (the driver
    /// closes their connections; the server rolls back open transactions).
    pub(super) fn close_profile_sessions(&mut self, id: ProfileId) {
        self.close_tab_sessions(|t| t.profile == Some(id));
        if let Some(s) = self.conns.get_mut(id).and_then(|c| c.meta.take()) {
            s.close();
        }
        self.conns.close_aux(Some(id));
    }

    /// Close every metadata session and every tab's query session.
    pub(super) fn close_sessions(&mut self) {
        self.close_tab_sessions(|_| true);
        self.conns.close_aux(None);
        for (_, c) in self.conns.iter_mut() {
            if let Some(s) = c.meta.take() {
                s.close();
            }
        }
    }

    /// `Connected` arrived for profile `id`'s attempt: expand its node if asked, save a typed
    /// password, remember it as `last_used`, open the console it waited for and run
    /// the statements that waited for it.
    pub(super) fn on_connected(&mut self, id: ProfileId) {
        let Some(name) = self.profile(id).map(|p| p.name.clone()) else { return };
        let c = self.conns.entry(id);
        c.connected = true;
        c.error = None;
        c.connecting = None;
        if std::mem::take(&mut c.expand_on_connect) {
            c.expanded = true;
        }
        let console = c.console.take();
        let pending = std::mem::take(&mut c.pending);
        let mut flash = None;
        let mut to_keychain = None;
        if let Some((_, kind)) = self.save_on_connect.take_if(|(sid, _)| *sid == id)
            && let Some(pw) = self.secrets.session(&id.account()).map(str::to_string)
        {
            // The keychain is written on a worker; it says how it went.
            if kind == SourceKind::Keychain {
                to_keychain = Some(pw);
            } else {
                flash = Some(match self.secrets.save_in(kind, &id.account(), &pw) {
                    Ok(()) => Notice::new(Msg::SecretSavedFile { name: name.clone() }, Level::Success),
                    Err(e) => Notice::new(self.source_error(&e), Level::Warning),
                });
            }
        }
        if let (Some(pw), Some(p)) = (to_keychain, self.profile(id).cloned()) {
            self.save_to_keychain(&p, pw, super::keychain::AfterSave::Connected);
        }
        if self.last_used != Some(id) {
            self.last_used = Some(id);
            if let Some(m) = self.persist() {
                flash = Some(m);
            }
        }
        match console {
            Some(focus) if self.tabs.recent_for(id).is_none() => {
                self.open_console(id, focus);
            }
            Some(true) => self.goto_profile(id),
            _ => {}
        }
        self.quick_connected(id);
        // The explorer's node is open: its databases level asks for the server's databases.
        if self.conns.get(id).is_some_and(|c| c.expanded) {
            self.ask_databases(id, false);
        }
        self.show_status(Notice::new(Msg::ConnConnected { name: name.clone() }, Level::Success));
        if let Some(m) = flash {
            self.flash(m);
        }
        // A statement runs only in the tab, on the profile and under the tab generation it was
        // queued for; a tab that moved to another connection (or was rebound to this one
        // again) meanwhile gets a notice instead.
        for q in pending {
            let current = self.tabs.get(q.tab).map(|t| (t.profile, t.binding));
            match current {
                // Checked (and confirmed) before it was queued.
                Some((profile, binding)) if q.profile == id && profile == Some(id) && binding == q.binding => {
                    self.run_approved(q.tab, q.statements);
                }
                Some(_) => {
                    self.tab_status(q.tab, Notice::new(Msg::QueryQueuedDropped { name: name.clone() }, Level::Warning))
                }
                None => {}
            }
        }
    }

    /// Profile `id`'s attempt ended without a connection (`why`): its node shows the error and
    /// what waited for it is dropped.
    pub(super) fn attempt_failed(&mut self, id: ProfileId, why: Notice) {
        self.drop_pending(id);
        let c = self.conns.entry(id);
        c.connecting = None;
        c.connected = false;
        c.expand_on_connect = false;
        c.console = None;
        c.error = Some(why);
        for t in self.tabs.iter_mut().filter(|t| t.profile == Some(id)) {
            t.exec.running = None;
        }
    }

    /// Forget profile `id`'s attempt in progress; late events of it are dropped (session
    /// generation). `why` is what its node says.
    pub(super) fn abort_connect(&mut self, id: ProfileId, why: Notice) {
        if self.conns.get(id).is_some_and(|c| c.connecting.is_some()) {
            self.close_profile_sessions(id);
            self.conns.next_generation(id);
            // Its tunnel, and what the tunnel asked, go with the attempt.
            self.close_tunnel(id);
            self.drop_tunnel_asks(id);
            if let Some(c) = self.conns.get_mut(id) {
                c.tunnel_wait = None;
            }
            self.attempt_failed(id, why);
        }
    }

    /// `Esc` while profile `id` connects: the attempt ends and the node is back to "not
    /// connected".
    pub(super) fn cancel_connect(&mut self, id: ProfileId) {
        let name = self.profile(id).map(|p| p.name.clone()).unwrap_or_default();
        let m = Notice::new(Msg::ConnCancelled { name: name.clone() }, Level::Warning);
        self.abort_connect(id, m.clone());
        if let Some(c) = self.conns.get_mut(id) {
            c.error = None;
        }
        // The status bar said what the attempt waited for (the keychain, the server): it says
        // now that it was cancelled, not the wait that is over.
        let waiting = |m: &Msg| match m {
            Msg::ConnReadingKeychain { name: n } | Msg::ConnConnecting { name: n } => *n == name,
            _ => false,
        };
        if self.status.as_ref().is_some_and(|s| waiting(&s.msg)) {
            self.status = Some(m.clone());
        }
        self.flash(m);
    }

    /// `x`: disconnect profile `id` (and connect again with `reconnect`); with a query running
    /// or a transaction open in one of its tabs ask first.
    pub(super) fn request_disconnect(&mut self, id: ProfileId, reconnect: bool) {
        let tabs = || self.tabs.iter().filter(|t| t.profile == Some(id));
        let (running, tx) = (tabs().any(|t| t.exec.running.is_some()), tabs().any(|t| t.exec.tx_at_risk()));
        let queued = self.conns.get(id).is_some_and(|c| !c.pending.is_empty());
        let texts =
            [Label::DisconnectRunning, Label::DisconnectTx, Label::DisconnectRunningTx, Label::DisconnectQueued];
        match tab_ops::at_risk(running, queued, tx, texts) {
            Some(text) => self.confirm(
                Label::DisconnectTitle,
                text,
                Label::DisconnectKeys,
                ConfirmAction::Disconnect { id, reconnect },
            ),
            None => self.disconnect(id, reconnect),
        }
    }

    /// Disconnect profile `id`: cancel what runs in its tabs, close its sessions (the server
    /// rolls back open transactions) and forget its tree. Its tabs stay, with their text and
    /// results, marked as not connected.
    pub(super) fn disconnect(&mut self, id: ProfileId, reconnect: bool) {
        if self.conns.state(id) == NodeState::Disconnected && !reconnect {
            return;
        }
        let name = self.profile(id).map(|p| p.name.clone()).unwrap_or_default();
        let tabs: Vec<TabId> = self.tabs.iter().filter(|t| t.profile == Some(id)).map(|t| t.id).collect();
        let tx = self.tabs.iter().any(|t| t.profile == Some(id) && t.exec.tx_at_risk());
        let running = self.tabs.iter().any(|t| t.profile == Some(id) && t.exec.running.is_some());
        // What waits for this connection is dropped (it never runs).
        let queued = self.conns.get_mut(id).map(|c| std::mem::take(&mut c.pending)).unwrap_or_default();
        for q in &queued {
            self.tab_status(q.tab, Notice::new(Label::QueryCancelled, Level::Warning));
        }
        for t in &tabs {
            self.cancel_in(*t);
        }
        self.close_profile_sessions(id);
        self.close_tunnel(id);
        self.drop_tunnel_asks(id);
        for t in self.tabs.iter_mut().filter(|t| t.profile == Some(id)) {
            t.exec.running = None;
            t.exec.tx_open = false;
            // The server rolls the user's transaction back when its session ends.
            t.block_ended(true);
            t.exec.lost_tx = false;
            t.exec.paging = Paging::None;
        }
        self.conns.next_generation(id);
        self.conns.entry(id).reset();
        if matches!(self.explorer.cursor(), explorer::RowKind::Node(p, _)
            | explorer::RowKind::AuxNode(p, _, _)
            | explorer::RowKind::Database(p, _)
            | explorer::RowKind::DatabasesNote(p)
            | explorer::RowKind::DatabaseNote(p, _)
            | explorer::RowKind::ProfileError(p) if *p == id)
        {
            self.explorer.select_kind(explorer::RowKind::Profile(id));
        }
        let m = if tx {
            Notice::new(Msg::ConnDisconnectedTx { name }, Level::Warning)
        } else if running {
            Notice::new(Msg::ConnDisconnectedQuery { name }, Level::Warning)
        } else if !queued.is_empty() {
            Notice::new(Msg::ConnDisconnectedQueued { name }, Level::Warning)
        } else {
            Notice::new(Msg::ConnDisconnected { name }, Level::Info)
        };
        self.show_status(m.clone());
        self.flash(m);
        if reconnect {
            let c = self.conns.entry(id);
            c.expand_on_connect = true;
            self.connect(id);
        }
        if self.quitting.is_some() && !self.any_running() {
            self.finish_quit();
        }
    }

    /// `d` on a profile: ask first, saying where its saved password is removed from
    /// (only for a source that stores one), how many of its tabs close and, tab by tab, what
    /// is lost: a running query, an open transaction, a statement waiting for the connection.
    pub(super) fn request_delete_profile(&mut self, id: ProfileId) {
        let Some(p) = self.profile(id) else { return };
        let name = p.name.clone();
        let mut details = Vec::new();
        match p.source().kind() {
            SourceKind::Keychain => details.push(Msg::Label(Label::ExplorerDeletePasswordKeychain)),
            SourceKind::File => details.push(Msg::Label(Label::ExplorerDeletePasswordFile)),
            SourceKind::Command | SourceKind::Env | SourceKind::Prompt => {}
        }
        let count = self.tabs.iter().filter(|t| t.profile == Some(id)).count() as u64;
        if count > 0 {
            details.push(Msg::ExplorerDeleteTabs { count });
        }
        for (i, t) in self.tabs.iter().enumerate().filter(|(_, t)| t.profile == Some(id)) {
            let tab = (i + 1).to_string();
            details.extend(match (t.exec.running.is_some(), t.exec.tx_at_risk(), self.is_queued(t.id)) {
                (true, true, _) => Some(Msg::ExplorerDeleteTabRunningTx { tab }),
                (true, false, _) => Some(Msg::ExplorerDeleteTabRunning { tab }),
                (false, true, _) => Some(Msg::ExplorerDeleteTabTx { tab }),
                (false, false, true) => Some(Msg::ExplorerDeleteTabQueued { tab }),
                (false, false, false) => None,
            });
        }
        self.tab_mut().popup = None;
        self.overlays.close(OverlayKind::Commands);
        self.overlays.push(Overlay::Confirm(Confirm {
            title: Label::ExplorerDeleteTitle,
            text: Msg::ExplorerDeleteProfile { name },
            details,
            keys: Label::ExplorerDeleteKeys,
            action: ConfirmAction::DeleteProfile(id),
            folder: None,
            path: None,
        }));
    }

    /// Delete profile `id` (confirmed): what waits for its connection is dropped, its tabs
    /// close (running queries cancelled, the server rolls back open transactions), its sessions
    /// close, its stored password is removed from its own store. The notice says what was lost.
    pub(super) fn delete_profile(&mut self, id: ProfileId) {
        let Some(i) = self.profiles.iter().position(|p| p.id == id) else { return };
        // The cursor goes to the row above.
        let rows = self.explorer_rows();
        let at = rows.iter().position(|r| r.kind == explorer::RowKind::Profile(id)).unwrap_or(0);
        let tabs: Vec<TabId> = self.tabs.iter().filter(|t| t.profile == Some(id)).map(|t| t.id).collect();
        let mine = || self.tabs.iter().filter(|t| t.profile == Some(id));
        let running = mine().any(|t| t.exec.running.is_some());
        let tx = mine().any(|t| t.exec.tx_at_risk());
        let queued = self.conns.get_mut(id).map(|c| std::mem::take(&mut c.pending)).unwrap_or_default();
        for t in &tabs {
            self.cancel_in(*t);
        }
        self.close_profile_sessions(id);
        for t in &tabs {
            // Unbound first, so a last tab is replaced by a console without a connection.
            self.tabs.bind(*t, None);
            self.close_tab(*t);
        }
        self.close_tunnel(id);
        self.drop_tunnel_asks(id);
        self.conns.remove(id);
        // Deleted here, by the user: the only way a binding to a profile goes.
        self.tabs.unbind_closed(id);
        if let Some(store) = self.scripts.as_mut()
            && let Err(e) = store.unbind_profile(id)
        {
            let error = self.io_text(&e);
            self.notices.push(Notice::new(Msg::ScriptsIndexSaveFailed { error }, Level::Error));
        }
        let p = self.profiles.remove(i);
        // The database password, and the SSH tunnel's secret, each from its
        // own store.
        let mut keychain = Vec::new();
        let tunnel = p.ssh.as_ref().map(|t| (t.source().kind(), SshSettings::account(p.id)));
        for (kind, account) in std::iter::once((p.source().kind(), p.id.account())).chain(tunnel) {
            if kind == SourceKind::Keychain {
                keychain.push(account.clone());
            } else if kind.stores_secret() {
                let _ = self.secrets.remove_in(kind, &account);
            }
            self.secrets.forget(&account);
        }
        if !keychain.is_empty() {
            // On a worker. Not migrated yet: its old entry named after the profile
            // goes too.
            if p.source().kind() == SourceKind::Keychain && self.config_version < config::CONFIG_VERSION {
                keychain.push(p.name.clone());
            }
            self.remove_from_keychain(keychain);
        }
        if self.last_used == Some(p.id) {
            self.last_used = None;
        }
        let saved = self.persist();
        let rows = self.explorer_rows();
        self.explorer.select(&rows, at.saturating_sub(1).max(usize::from(rows.len() > 1)));
        let (name, count) = (p.name, tabs.len() as u64);
        let msg = match (running, tx, !queued.is_empty()) {
            _ if count == 0 => Notice::new(Msg::ProfilesDeleted { name }, Level::Success),
            (true, true, _) => Notice::new(Msg::ProfilesDeletedQueryTx { name, count }, Level::Warning),
            (true, false, _) => Notice::new(Msg::ProfilesDeletedQuery { name, count }, Level::Warning),
            (false, true, _) => Notice::new(Msg::ProfilesDeletedTx { name, count }, Level::Warning),
            (false, false, true) => Notice::new(Msg::ProfilesDeletedQueued { name, count }, Level::Warning),
            (false, false, false) => Notice::new(Msg::ProfilesDeletedTabs { name, count }, Level::Success),
        };
        self.flash(saved.unwrap_or(msg));
        if self.profiles.is_empty() {
            self.focus = Focus::Tree;
        }
    }

    /// `d` on a folder: an empty one is deleted after a confirmation; one with profiles says
    /// so.
    pub(super) fn request_delete_folder(&mut self, f: FolderPath) {
        if self.profiles.iter().any(|p| p.folder_path().is_some_and(|pf| pf.is_within(&f))) {
            return self.flash(Notice::new(Msg::ExplorerFolderNotEmpty { name: f.to_string() }, Level::Warning));
        }
        self.overlays.push(Overlay::Confirm(Confirm {
            title: Label::ExplorerDeleteTitle,
            text: Msg::ExplorerDeleteFolder { name: f.to_string() },
            details: Vec::new(),
            keys: Label::ExplorerDeleteKeys,
            action: ConfirmAction::DeleteFolder,
            folder: Some(f),
            path: None,
        }));
    }

    pub(super) fn delete_folder(&mut self, f: FolderPath) {
        self.folders.remove(&f);
        let saved = self.persist();
        let rows = self.explorer_rows();
        let i = self.explorer.index(&rows);
        self.explorer.select(&rows, i);
        self.flash(saved.unwrap_or(Notice::new(Msg::ExplorerFolderDeleted { name: f.to_string() }, Level::Success)));
    }

    /// Enter on a table or view of profile `id` in `database` (`None`: the profile's own):
    /// its own table tab on that profile and database (another database's table too, bound to that database with its default schema), which runs
    /// its query; a table already open in a tab of the profile and database goes to that tab
    /// (without running it again: `Ctrl+E` reloads it), and runs its query there only when the
    /// tab has not run since it opened or was restored (opening it is the intent
    /// that loads a new one). Never another profile's or database's tab. The focus stays in
    /// the explorer.
    pub(super) fn open_table(&mut self, id: ProfileId, database: Option<String>, schema: &str, name: &str) {
        let table = super::tabs::TableRef { schema: schema.to_string(), name: name.to_string() };
        let context = self.session_context(id, database, None);
        if let Some(t) = self.tabs.find_table(id, context.database.as_deref(), &table) {
            self.switch_tab(|m| m.position(t).is_some_and(|i| m.activate(i)));
            let not_loaded = self.tabs.get(t).is_some_and(|tab| !tab.ran) && !self.tab_busy(t);
            if not_loaded {
                self.run_in(t, vec![table.query()]);
            }
            return;
        }
        let query = table.query();
        self.leave_tab();
        let tab = self.tabs.open(TabKind::Table, Some(id), Editor::new(&query));
        if let Some(t) = self.tabs.get_mut(tab) {
            t.doc.saved = query.clone();
            t.doc.written = true;
            t.doc.table = Some(table);
            t.context = context;
        }
        self.entered_tab();
        self.run_in(tab, vec![query]);
    }

    pub(super) fn clear_test(&mut self) {
        if let Some(t) = self.conn_test.take() {
            if let Some(a) = t.abort {
                a.abort();
            }
            self.drop_asks(|o| o == tunnel::Owner::Test(t.seq));
        }
    }

    pub(super) fn cancel_test(&mut self) -> bool {
        match self.conn_test.as_mut() {
            Some(t) if t.state == TestState::Running => {
                if let Some(a) = t.abort.take() {
                    a.abort();
                }
                t.state = TestState::Cancelled;
                let seq = t.seq;
                self.drop_asks(|o| o == tunnel::Owner::Test(seq));
                true
            }
            _ => false,
        }
    }

    pub(super) fn start_test(&mut self, cfg: ConnectionConfig) {
        // Through its SSH tunnel (a throwaway one: the form may hold settings not saved yet).
        if cfg.tunnel().is_some() {
            return self.start_tunnel_test(cfg, None);
        }
        self.clear_test();
        self.test_seq += 1;
        let seq = self.test_seq;
        let mut state = TestState::Running;
        let abort = match (self.driver(&cfg.driver), self.tx.clone()) {
            (None, _) => {
                state = TestState::Failed(
                    self.i18n.msg(&Msg::ConnUnknownDriver { driver: cfg.driver.clone() }).to_string(),
                );
                None
            }
            (Some(d), Some(tx)) => {
                let fut = d.ping(&cfg, TEST_TIMEOUT, None);
                Some(
                    tokio::spawn(async move {
                        let result = fut.await;
                        let _ = tx.send(AppEvent::Ping { seq, result });
                    })
                    .abort_handle(),
                )
            }
            // Headless tests inject the result through `on_app_event`.
            (Some(_), None) => None,
        };
        self.conn_test = Some(ConnTest { seq, started: Instant::now(), state, abort, tunnel: None });
        self.report_test();
    }

    /// Outside the profile form the test's state goes to the status bar (the form shows it in
    /// its footer).
    pub(super) fn report_test(&mut self) {
        if self.overlays.is_open(OverlayKind::ProfileForm) {
            return;
        }
        if let Some(t) = &self.conn_test {
            let m = test_msg(&self.i18n, t);
            self.flash(m);
        }
    }

    /// "Test connection": the form being edited (its source and password), else `profile`
    /// (the result goes to the status bar). The password comes through the profile's source.
    pub(super) fn test_in_context(&mut self, profile: Option<ProfileId>) {
        enum Target {
            Form(ConnectionConfig, Option<String>),
            Profile(ConnectionConfig),
        }
        // The form's fields a connection needs are checked as Save checks them: a missing
        // database host (or user, port, a tunnel field) is marked and nothing is tried.
        if let Some(f) = self.overlays.form_mut() {
            let was = f.attempted;
            f.attempted = true;
            let missing = f.errors(|_| false).into_iter().find(|(field, _)| *field != super::profiles::Field::Name);
            match missing {
                Some((field, _)) => {
                    f.focus_field(field);
                    self.clear_test();
                    return self.flash(Notice::new(Label::TestFieldsMissing, Level::Warning));
                }
                None => f.attempted = was,
            }
        }
        let target = match self.overlays.form() {
            Some(f) => {
                // The typed password of a storing source, unless it could not be loaded.
                let typed = (f.source.stores_secret() && !(f.password.text().is_empty() && f.password_unread))
                    .then(|| f.password.text().to_string());
                Some(Target::Form(f.to_profile(), typed))
            }
            None => profile.and_then(|id| self.profile(id)).cloned().map(Target::Profile),
        };
        match target {
            Some(Target::Form(mut c, Some(pw))) => {
                c.password = pw;
                self.start_test(c);
            }
            Some(Target::Form(c, None) | Target::Profile(c)) => self.test_with_source(c),
            None => {}
        }
    }

    /// A test whose password comes from a command: the command runs inside the test task
    /// (headless: here), then the server is asked.
    pub(super) fn start_test_with_command(&mut self, cfg: ConnectionConfig, cmd: String) {
        use datarig_core::secret::command;
        if cfg.tunnel().is_some() && self.tx.is_some() {
            return self.start_tunnel_test(cfg, Some(cmd));
        }
        let Some(tx) = self.tx.clone() else {
            match command::run(&cmd, command::TIMEOUT) {
                Ok(pw) => self.start_test(ConnectionConfig { password: pw, ..cfg }),
                Err(e) => {
                    self.start_test(cfg);
                    if let Some(t) = self.conn_test.as_mut() {
                        t.state = TestState::Source(SourceError::Command(e));
                    }
                }
            }
            return;
        };
        self.clear_test();
        self.test_seq += 1;
        let seq = self.test_seq;
        let Some(d) = self.driver(&cfg.driver) else {
            return self.start_test(cfg);
        };
        let task = tokio::spawn(async move {
            let r = tokio::task::spawn_blocking(move || command::run(&cmd, command::TIMEOUT)).await;
            match r {
                Ok(Ok(pw)) => {
                    let result = d.ping(&ConnectionConfig { password: pw, ..cfg }, TEST_TIMEOUT, None).await;
                    let _ = tx.send(AppEvent::Ping { seq, result });
                }
                Ok(Err(e)) => {
                    let _ = tx.send(AppEvent::TestSource { seq, error: SourceError::Command(e) });
                }
                Err(_) => {}
            }
        });
        self.conn_test = Some(ConnTest {
            seq,
            started: Instant::now(),
            state: TestState::Running,
            abort: Some(task.abort_handle()),
            tunnel: None,
        });
    }

    pub(super) fn on_ping(&mut self, seq: u64, result: Result<PingInfo, PingError>) {
        let failed = match &result {
            Err(PingError::Failed(e)) => self.db_error_text(e),
            _ => String::new(),
        };
        let Some(t) = self.conn_test.as_mut() else { return };
        if t.seq != seq || t.state != TestState::Running {
            return;
        }
        t.abort = None;
        t.state = match result {
            Ok(info) => TestState::Ok(info),
            Err(PingError::Timeout(d)) => TestState::Timeout(d),
            Err(PingError::Failed(_)) => TestState::Failed(failed),
        };
        self.report_test();
    }

    /// Open the profile form: a new profile (in the explorer's folder), or profile `idx` to
    /// edit or duplicate.
    pub(super) fn open_form(&mut self, idx: Option<usize>, duplicate: bool) {
        let keychain_ok = self.keychain_ok();
        let form = match idx.and_then(|i| self.profiles.get(i).cloned().map(|p| (i, p))) {
            Some((i, p)) => {
                // Only a stored password fills the form (never run a command or read the
                // environment just to open it). The keychain's is read on a worker and fills
                // the field when it answers.
                let stored = self.stored_password(&p);
                let pw = match &stored {
                    password::Stored::Known(pw) => pw.clone(),
                    _ => String::new(),
                };
                let mut f = if duplicate {
                    let mut copy = p.clone();
                    copy.id = ProfileId::new();
                    copy.name = copy_name(&p.name, |n| self.profiles.iter().any(|q| q.name == n));
                    ProfileForm::from_profile(&copy, pw, None)
                } else {
                    ProfileForm::from_profile(&p, pw, Some(i))
                };
                f.keychain_ok = keychain_ok;
                f.password_unread = !matches!(stored, password::Stored::Known(_));
                if matches!(stored, password::Stored::Keychain) {
                    f.reading = Some(p.id);
                }
                f
            }
            None => {
                let mut f = self.new_profile_form();
                if self.focus == Focus::Tree {
                    f.set_folder(self.selected_folder().map(|p| p.to_string()));
                }
                f
            }
        };
        let mut form = form;
        form.folders = self.folders.iter().map(ToString::to_string).collect();
        for (i, (name, _)) in crate::app::profiles::DRIVERS.iter().enumerate() {
            form.drivers_enabled[i] = self.driver(name).is_some();
        }
        self.clear_test();
        let t = self.tab_mut();
        t.popup = None;
        t.completion_due = None;
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        let read = form.reading.and_then(|id| self.profile(id).cloned());
        self.overlays.push(Overlay::ProfileForm(Box::new(form)));
        if let Some(p) = read {
            self.read_form_password(&p);
        }
    }
}
