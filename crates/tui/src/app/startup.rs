//! Starting the app: the first frame, the launch steps, profile migration.

use super::*;

impl App {
    /// Launch with background work enabled: events of connections, tests and the password move
    /// come back through `tx`. See [`App::launch`]. Nothing here waits for the keychain: the
    /// password move runs in the background and `datarig <profile>` connects only after
    /// [`App::first_frame_drawn`].
    pub fn start(&mut self, tx: UnboundedSender<AppEvent>, startup: Startup) {
        self.tx = Some(tx);
        self.hear_late_keychain();
        self.launch(startup);
    }

    /// Startup flow: the launch-time migration (config v2, keychain entries by id,
    /// plaintext passwords; see [`migrate`]), then the workspace with the explorer on the last
    /// used profile (the new-profile form when there is none), or a connection attempt to the
    /// profile named on the command line (an unknown name is reported in the workspace).
    /// Without `start`'s sender (headless tests) everything runs at once, and connection
    /// attempts are recorded but not made.
    pub fn launch(&mut self, startup: Startup) {
        let migration = self.migration.take();
        let background = migration.is_some() && self.tx.is_some();
        if let Some(input) = migration.clone().filter(|_| !background) {
            let stores = self.secrets.stores().clone();
            let report = migrate::run(stores.keychain.as_ref(), stores.file.as_ref(), input);
            let notices = self.migrated(report);
            self.notices.extend(notices);
        }
        self.notices.append(&mut self.keymap_notices);
        if let Some(n) = self.theme_problem.clone() {
            self.notices.push(n);
        }
        if let Some(e) = &self.status
            && matches!(e.msg, Msg::ErrorConfig { .. })
        {
            self.notices.push(e.clone());
        }
        if self.config_broken {
            // The profiles are unknown, not gone: every binding is kept as it is.
            self.notices.push(Notice::new(Label::ConfigConnectionsUnavailable, Level::Warning));
        }
        self.probe_keychain();
        self.open_workspace();
        // After the workspace: a second instance does not keep it as shown.
        if let Some(n) = self.theme_unlisted_notice() {
            self.notices.push(n);
        }
        if let (true, Some(tx), Some(input)) = (background, self.tx.clone(), migration) {
            self.migrating = true;
            self.overlays.push(Overlay::Busy(Busy { title: Label::MigrateTitle, text: Label::MigrateRunning }));
            let stores = self.secrets.stores().clone();
            tokio::task::spawn_blocking(move || {
                let report = migrate::run(stores.keychain.as_ref(), stores.file.as_ref(), input);
                let _ = tx.send(AppEvent::Migrated(report));
            });
        }
        match startup {
            Startup::Profile(_) if self.tx.is_some() => {
                self.deferred_startup = Some(startup);
                self.resume_startup();
            }
            s => self.run_startup(s),
        }
        // Not with a config that has errors: the answer could not be saved (asked on the first
        // launch after the file is fixed).
        // Over the workspace, under the icons question (asked first).
        self.whats_new_at_launch();
        self.updates_at_launch();
        if self.ask_icons && self.icons == IconsSetting::Auto && !self.config_broken {
            self.overlays.push(Overlay::IconsAsk(overlay::IconsAsk::default()));
        }
    }

    /// The event loop drew its first frame: a deferred `datarig <profile>` may connect now
    /// (its keychain read cannot freeze a blank screen any more).
    pub fn first_frame_drawn(&mut self) {
        self.drawn = true;
        self.resume_startup();
        self.updates_after_first_frame();
    }

    fn resume_startup(&mut self) {
        if self.drawn
            && !self.migrating
            && let Some(s) = self.deferred_startup.take()
        {
            self.run_startup(s);
        }
    }

    fn run_startup(&mut self, startup: Startup) {
        self.focus = Focus::Tree;
        // A hidden explorer stays hidden: the restored tab has the focus (its connection
        // starts with the first key, as when the user moves there).
        if self.explorer.hidden && !self.tabs.is_empty() && !self.profiles.is_empty() {
            self.focus =
                [Focus::Editor, Focus::Results].into_iter().find(|f| self.focusable(*f)).unwrap_or(Focus::Tree);
            self.last_focus = self.focus;
        }
        let last = self.last_used.filter(|id| self.profile(*id).is_some()).or(self.profiles_sorted_first());
        // The cursor on that profile; on the Connections head when the section was left closed
        // (it stays closed).
        match last {
            Some(id) if self.connections_expanded => self.reveal_profile(id),
            Some(_) => self.explorer.select_kind(explorer::RowKind::ConnectionsHeader),
            None => {}
        }
        match startup {
            // Nothing to connect to: the welcome panel's first steps when they show (its
            // new-connection key one press away), else the new-profile form over it.
            Startup::Normal if self.profiles.is_empty() && !self.config_broken && !self.onboarding_shown() => {
                self.open_form(None, false)
            }
            Startup::Normal => {}
            Startup::Profile(name) => match self.profiles.iter().find(|p| p.name == name).map(|p| p.id) {
                Some(id) => {
                    self.reveal_profile(id);
                    let c = self.conns.entry(id);
                    c.expand_on_connect = true;
                    c.console = Some(false);
                    let notices = std::mem::take(&mut self.notices);
                    self.connect(id);
                    self.notices = notices;
                }
                None => self.notices.push(Notice::new(Msg::ProfileUnknown { name }, Level::Error)),
            },
        }
    }

    /// The first profile in explorer order.
    fn profiles_sorted_first(&self) -> Option<ProfileId> {
        self.explorer_rows().into_iter().find_map(|r| match r.kind {
            explorer::RowKind::Profile(id) => Some(id),
            _ => None,
        })
    }

    /// Apply the migration's outcome: passwords that moved leave the profiles, the config
    /// version is updated, and the notices to show are returned. Config saves requested while
    /// it ran happen now.
    pub(crate) fn migrated(&mut self, r: migrate::Report) -> Vec<Notice> {
        self.migrating = false;
        let mut out = Vec::new();
        for p in self.profiles.iter_mut().filter(|p| r.moved.contains(&p.id)) {
            p.password.clear();
        }
        self.config_version = r.version;
        if let Some(e) = r.save_failed {
            let error = self.fault_text("migrate.save_failed", &e);
            out.push(Notice::new(Msg::MigrateSaveFailed { error }, Level::Warning));
        }
        if let Some(e) = r.final_save_failed {
            let error = self.fault_text("config.save_failed", &e);
            out.push(Notice::new(Msg::ConfigSaveFailed { error }, Level::Error));
        }
        if let Some(bak) = &r.backup
            && r.version >= config::CONFIG_VERSION
        {
            out.push(Notice::new(Msg::MigrateDone { path: bak.display().to_string() }, Level::Info));
        }
        match &r.keychain {
            Keychain::Unavailable(e) => {
                self.secrets.unavailable = Some(e.clone());
                // Once per machine: the passwords simply stay in the file.
                let plaintext = self.profiles.iter().any(|p| !p.password.is_empty());
                if plaintext && self.paths.first_time("keychain_unavailable") {
                    let error = self.fault_text("secret.migrate_failed", e);
                    out.push(Notice::new(Msg::SecretMigrateFailed { error }, Level::Warning));
                }
            }
            Keychain::Partial => {
                let error = r
                    .failures
                    .iter()
                    .map(|(n, e)| format!("{n}: {}", self.fault_text("migrate.partial", e)))
                    .collect::<Vec<_>>()
                    .join("; ");
                out.push(Notice::new(Msg::MigratePartial { error }, Level::Warning));
            }
            Keychain::Done => {}
        }
        if !r.conflicts.is_empty() {
            let mut names = r.conflicts.clone();
            names.dedup();
            out.push(Notice::new(Msg::MigrateConflict { names: names.join(", ") }, Level::Warning));
        }
        if !r.moved.is_empty() {
            out.push(Notice::new(Msg::SecretMigrated { count: r.moved.len() as u64 }, Level::Success));
        }
        if std::mem::take(&mut self.persist_deferred)
            && let Some(m) = self.persist()
        {
            out.push(m);
        }
        out
    }

    /// The background migration reported back: close its notice, show the result in the
    /// workspace and the status bar, and go on with a deferred `datarig <profile>`.
    pub(super) fn on_migrated(&mut self, r: migrate::Report) {
        self.overlays.close(OverlayKind::Busy);
        let notices = self.migrated(r);
        if let Some(m) = notices.iter().max_by_key(|n| n.level as u8).cloned() {
            self.flash(m);
        }
        for (i, m) in notices.into_iter().enumerate() {
            self.notices.insert(i, m);
        }
        self.resume_startup();
    }
}
