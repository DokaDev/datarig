//! Action dispatch: what every [`Action`] does, which actions are available where, and
//! persisting settings the actions change.

use super::*;

impl App {
    /// Run a named action. Every key binding and every command-line entry ends up here.
    pub fn dispatch(&mut self, a: Action) {
        // While the launch-time move waits (the busy notice), nothing runs (there may be no
        // tab yet): Ctrl+C abandons the move and quits, as `q` does there.
        if a == Action::CancelQuery && self.overlays.top().is_some_and(|o| o.kind() == OverlayKind::Busy) {
            return self.request_quit();
        }
        if !(action::spec(a).when)(self) {
            // A table tab has neither a text to save nor a connection to switch: said, not
            // silently ignored.
            let table = !self.profiles.is_empty() && !self.tabs.is_empty() && self.tab().is_table();
            match a {
                Action::ScriptSave | Action::ScriptSaveAs if table => {
                    self.flash(Notice::new(Label::TableTabNoSave, Level::Info));
                }
                Action::SetTabConnection | Action::SetTabContext if table => {
                    self.flash(Notice::new(Label::TableTabNoRebind, Level::Info))
                }
                _ => {}
            }
            return;
        }
        match a {
            Action::RunStatement => {
                if !self.layout.too_small {
                    let t = self.tab_mut();
                    t.popup = None;
                    t.completion_due = None;
                    self.execute_current();
                }
            }
            Action::CancelQuery => self.cancel(),
            Action::OpenCommands => self.toggle_commands(),
            Action::ShowCompletions => {
                if self.focus == Focus::Editor && self.tab().editor.mode == Mode::Insert {
                    self.tab_mut().completion_due = None;
                    self.update_completion(true);
                }
            }
            Action::NewProfile => self.open_form(None, false),
            Action::TestConnection => {
                let p = self.action_profile();
                self.test_in_context(p)
            }
            Action::TestCurrent => {
                let p = self.tab().profile;
                self.test_in_context(p)
            }
            Action::QuickConnect => self.open_quick(QuickPurpose::Open),
            Action::EditProfile | Action::DuplicateProfile => {
                let i = self.selected_profile().and_then(|id| self.profiles.iter().position(|p| p.id == id));
                if i.is_some() {
                    self.open_form(i, a == Action::DuplicateProfile);
                }
            }
            Action::EditCurrent => {
                let i = self.tab().profile.and_then(|id| self.profiles.iter().position(|p| p.id == id));
                if i.is_some() {
                    self.open_form(i, false);
                }
            }
            Action::Disconnect => {
                if let Some(id) = self.selected_profile() {
                    self.request_disconnect(id, false);
                }
            }
            Action::DisconnectCurrent | Action::ReconnectCurrent => {
                if let Some(id) = self.tab().profile {
                    self.request_disconnect(id, a == Action::ReconnectCurrent);
                }
            }
            Action::OpenConsole => {
                if let Some(id) = self.selected_profile() {
                    self.open_console(id, true);
                    if matches!(self.conns.state(id), NodeState::Disconnected | NodeState::Failed) {
                        self.connect(id);
                    }
                }
            }
            Action::SetTabConnection => self.request_set_connection(),
            Action::SetTabContext => self.request_set_context(None),
            Action::SetLanguage(s) => self.set_language(s),
            Action::FocusNext => {
                self.tab_mut().popup = None;
                self.cycle_focus(1);
            }
            Action::FocusPrev => {
                self.tab_mut().popup = None;
                self.cycle_focus(-1);
            }
            Action::Quit => self.request_quit(),
            Action::SetIcons(i) => self.set_icons(i),
            Action::SetDefaultSource(d) => self.set_default_source(d),
            Action::ToggleIcons => self.set_icons(if self.icons_on() { IconsSetting::Off } else { IconsSetting::On }),
            Action::Help => self.open_help(false),
            Action::HelpAll => self.open_help(true),
            // A selected range goes first, then the grid.
            // The inspector goes back to its grid; the grid drops a selected range first, then
            // goes back to the editor.
            Action::PaneBack if self.focus == Focus::Inspector => self.focus = Focus::Results,
            Action::PaneBack => {
                if self.tab_mut().grid.anchor.take().is_some() {
                    self.selecting_done();
                } else {
                    // Back to the editor; a table tab (or maximised results) has none on screen.
                    self.focus = if self.editor_shown() { Focus::Editor } else { Focus::Tree };
                }
            }
            Action::Copy(scope, f) => self.copy_scoped(scope, f),
            Action::NewTab => self.new_tab(),
            Action::CloseTab => self.request_close_tab(),
            Action::ReopenTab => self.reopen_tab(),
            Action::NextTab => self.switch_tab(|t| {
                t.cycle(1);
                true
            }),
            Action::PrevTab => self.switch_tab(|t| {
                t.cycle(-1);
                true
            }),
            Action::GotoTab(n) => self.switch_tab(|t| t.activate(usize::from(n) - 1)),
            Action::ScriptSave => self.save_current(),
            Action::ScriptSaveAs => {
                let id = self.tab().id;
                self.ask_script_name(id, false);
            }
            Action::ScriptRename => {
                if let Some(p) = self.current_script() {
                    self.open_rename_script(p, false);
                }
            }
            Action::ScriptOpen => self.open_script_chooser(),
            Action::PickKeyFile => self.open_key_picker(),
            Action::ScriptDelete => {
                if let Some(p) = self.current_script() {
                    self.request_delete_script(p);
                }
            }
            Action::OpenSettings => self.open_settings(),
            Action::ToggleDetail => self.detail.visible = !self.detail.visible,
            Action::DetailTab => self.detail.tab = self.detail.tab.other(),
            Action::Explorer(e) => self.explorer_action(e),
            Action::Grid(g) => self.grid_action(g),
            Action::Panel(p) => self.panel_action(p),
            Action::PageNext => self.page_next(),
            Action::PagePrev => self.page_prev(),
            Action::CountRows => self.count_rows(),
            Action::ResultTab(next) => self.cycle_result_tab(if next { 1 } else { -1 }),
        }
    }

    pub(super) fn grid_action(&mut self, g: GridAction) {
        // The Messages of the run: the movement keys scroll the list.
        if self.tab().exec.view == super::tabs::ResultView::Messages {
            let t = self.tabs.active_mut();
            let s = &mut t.exec.messages_scroll;
            match g {
                GridAction::Down => *s += 1,
                GridAction::Up => *s = s.saturating_sub(1),
                GridAction::PageDown | GridAction::HalfDown => *s += 10,
                GridAction::PageUp | GridAction::HalfUp => *s = s.saturating_sub(10),
                GridAction::Top => *s = 0,
                GridAction::Bottom => *s = usize::MAX / 2,
                _ => {}
            }
            return;
        }
        let t = self.tabs.active_mut();
        if let (GridAction::ViewCell, Results::Rows(rs)) = (g, &mut t.results) {
            t.grid.load_selected(rs);
        }
        let Results::Rows(rs) = &t.results else { return };
        let (total, ncols) = (rs.rows.len(), rs.columns.len());
        let grid = &mut t.grid;
        let page = grid.page_rows.max(1) as isize;
        // A key brings the view back to the selection a wheel scroll left.
        grid.detached = false;
        match g {
            GridAction::Down => grid.move_row(1, total),
            GridAction::Up => grid.move_row(-1, total),
            GridAction::Left => grid.move_col(-1, ncols),
            GridAction::Right => grid.move_col(1, ncols),
            GridAction::PageDown => grid.move_row(page, total),
            GridAction::PageUp => grid.move_row(-page, total),
            GridAction::HalfDown => grid.move_row(page / 2, total),
            GridAction::HalfUp => grid.move_row(-page / 2, total),
            // The first and last row of the page.
            GridAction::Top => grid.row = grid.window(total).start,
            GridAction::Bottom => grid.row = grid.window(total).end.saturating_sub(1),
            GridAction::FirstCol => grid.col = 0,
            GridAction::LastCol => grid.col = ncols.saturating_sub(1),
            GridAction::ViewCell => {
                let null = self.i18n.label(Label::ResultsNull);
                let not_read = self.i18n.label(Label::ResultsNotRead);
                if let Some((column, text)) = viewer_text(rs, grid, &null, &not_read) {
                    self.overlays.push(Overlay::CellViewer(Viewer { column, text, scroll: 0, view_h: 1 }));
                }
            }
            GridAction::CopyCell => return self.copy(copy::CopyWhat::Cell, copy::CopyFormat::Tsv, false),
            GridAction::CopyRow => return self.copy(copy::CopyWhat::Row, copy::CopyFormat::Tsv, false),
            GridAction::Select => return self.toggle_selection(false),
            GridAction::SelectRows => return self.toggle_selection(true),
        }
        // Pages are explicit: at the end of one, say which key shows the next.
        if matches!(g, GridAction::Down | GridAction::PageDown | GridAction::HalfDown | GridAction::Bottom) {
            self.paging_hint();
        }
    }

    /// Every quit goes through here. With a query running, a transaction open or a
    /// tab whose text could not be saved, ask first and say what would be lost; otherwise quit
    /// at once.
    pub(super) fn request_quit(&mut self) {
        // Everything is written first; a saved query that changed on disk is not, and asks.
        self.save_workspace();
        if self.overlays.confirm().is_some_and(|c| matches!(c.action, ConfirmAction::ScriptConflict(_))) {
            return;
        }
        let running = self.tabs.iter().any(|t| t.exec.running.is_some());
        let queued = self.tabs.iter().any(|t| self.is_queued(t.id));
        let texts = [Label::QuitRunning, Label::QuitTx, Label::QuitRunningTx, Label::QuitQueued];
        let conflicts = self.conflicted(None) > 0;
        // The final flush above failed for these (a failed write, a full disk, …).
        let unsaved = self.unsaved(None) > 0;
        let text = tab_ops::at_risk(running, queued, self.any_tx_at_risk(), texts)
            .or(conflicts.then_some(Label::QuitScriptConflict))
            .or(unsaved.then_some(Label::QuitUnsaved));
        let Some(text) = text else {
            self.finish_quit();
            return;
        };
        if self.quitting.is_some() {
            return;
        }
        self.tab_mut().popup = None;
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.key_state.clear();
        let details = if unsaved { self.unsaved_error(None).into_iter().collect() } else { Vec::new() };
        self.overlays.push(Overlay::Confirm(Confirm {
            title: Label::QuitTitle,
            text: text.into(),
            details,
            keys: Label::QuitKeys,
            action: ConfirmAction::Quit,
            folder: None,
            path: None,
        }));
    }

    /// Keys of a yes/no confirmation.
    /// A switch or a replace was not confirmed: nothing of it stays pending.
    fn confirm_kept(&mut self) {
        self.overlays.close(OverlayKind::Confirm);
        self.pending_context = None;
        if let Some(t) = self.overlays.script_tree_mut() {
            t.overwrite = None;
        }
    }

    pub(super) fn confirm_key(&mut self, key: KeyEvent, repeat: bool) {
        let Some((action, folder, path)) =
            self.overlays.confirm().map(|c| (c.action, c.folder.clone(), c.path.clone()))
        else {
            return;
        };
        if let ConfirmAction::ScriptConflict(id) = action {
            if !repeat {
                self.conflict_key(id, key);
            }
            return;
        }
        // `Enter` keeps whatever the answer would lose (Cancel is the default): only `y` quits,
        // deletes, disconnects, closes, switches or replaces (steps 2.7.1, 2.7.4 and 2.7.5).
        // `Enter` says yes only to a copy, which loses nothing.
        let enter_yes = matches!(action, ConfirmAction::Copy | ConfirmAction::FetchThenCopy);
        let code = if key.code == KeyCode::Enter && !enter_yes { KeyCode::Char('n') } else { key.code };
        // Kept switches and replaces leave nothing pending.
        let keeps = matches!(
            action,
            ConfirmAction::SetConnection(_)
                | ConfirmAction::SetContext(_)
                | ConfirmAction::OverwriteScript
                | ConfirmAction::CloseTab(_)
        );
        match code {
            KeyCode::Char('y') | KeyCode::Enter if !repeat => {
                self.overlays.close(OverlayKind::Confirm);
                match action {
                    ConfirmAction::Quit => self.quit_anyway(),
                    ConfirmAction::ChangeSource => self.source_change_answered(true),
                    ConfirmAction::CloseTab(id) => self.close_tab(id),
                    ConfirmAction::Disconnect { id, reconnect } => self.disconnect(id, reconnect),
                    ConfirmAction::DeleteProfile(id) => self.delete_profile(id),
                    ConfirmAction::DeleteFolder => {
                        if let Some(f) = folder {
                            self.delete_folder(f);
                        }
                    }
                    ConfirmAction::SetConnection(tab) => self.open_quick(QuickPurpose::Bind { tab, run: None }),
                    ConfirmAction::SetContext(tab) => match self.pending_context.take() {
                        Some((t, ctx)) if t == tab => self.set_context(tab, ctx),
                        _ => self.open_context_picker(tab),
                    },
                    ConfirmAction::DeleteScript => {
                        if let Some(p) = path {
                            self.delete_script(&p);
                        }
                    }
                    ConfirmAction::DeleteScriptFolder => {
                        if let Some(p) = path {
                            self.delete_script_folder(&p);
                        }
                    }
                    ConfirmAction::ScriptConflict(_) => {}
                    ConfirmAction::Copy => self.copy_confirmed(),
                    ConfirmAction::FetchThenCopy => self.fetch_then_copy(),
                    ConfirmAction::OverwriteScript => self.overwrite_confirmed(),
                    ConfirmAction::TrustHostKey => self.host_key_answered(true),
                }
            }
            KeyCode::Char('n') | KeyCode::Esc if keeps => self.confirm_kept(),
            KeyCode::Char('n') | KeyCode::Esc => {
                self.overlays.close(OverlayKind::Confirm);
                if action == ConfirmAction::TrustHostKey {
                    self.host_key_answered(false);
                }
                if action == ConfirmAction::ChangeSource {
                    self.source_change_answered(false);
                }
                if action == ConfirmAction::Copy {
                    self.pending_copy = None;
                }
                if action == ConfirmAction::FetchThenCopy {
                    self.pending_fetch_copy = None;
                }
            }
            _ => {}
        }
    }

    /// "Quit anyway": cancel the running query, then close the session — the server rolls back
    /// an open transaction when its connection closes — and quit. A query that does not stop
    /// within [`QUIT_GRACE`] is left to the server (the connection closes regardless).
    fn quit_anyway(&mut self) {
        self.cancel_all();
        if self.any_running() {
            self.quitting = Some(Instant::now());
        } else {
            // Only statements waiting for a connection: dropped, nothing to wait for.
            self.finish_quit();
        }
    }

    /// The process was asked to end (SIGTERM, SIGHUP, …): what can be written is written (a
    /// saved query that changed on disk is left alone), the sessions close (the server stops
    /// what runs and rolls back open transactions) and the app quits without asking.
    pub fn quit_on_signal(&mut self) {
        self.finish_quit();
    }

    /// The query stopped (or the grace time is over): close the session and quit.
    pub(super) fn finish_quit(&mut self) {
        self.quitting = None;
        self.save_workspace();
        self.close_sessions();
        let ids: Vec<ProfileId> = self.conns.iter().map(|(id, _)| id).collect();
        for id in ids {
            self.conns.next_generation(id);
        }
        for t in self.tabs.iter_mut() {
            t.exec.running = None;
            t.exec.tx_open = false;
            // The server rolls the user's transaction back when its session ends.
            t.block_ended(true);
        }
        self.quit = true;
    }

    /// `on` and `off` are saved at once; `auto` (not decided yet) asks the question again,
    /// whose answer is saved.
    pub(super) fn set_icons(&mut self, icons: IconsSetting) {
        let msg = match icons {
            IconsSetting::On => Label::IconsSwitchedOn,
            IconsSetting::Off => Label::IconsSwitchedOff,
            IconsSetting::Auto => return self.overlays.push(Overlay::IconsAsk(super::overlay::IconsAsk::default())),
        };
        self.icons = icons;
        let saved = self.persist();
        self.flash(saved.unwrap_or(Notice::new(msg, Level::Info)));
    }

    /// Keys of the icons question: `y` yes, `n` no, `Enter` what has the focus (No at first),
    /// `Tab`/arrows/`h`/`l` move the focus, `Esc` ask again later (nothing saved).
    /// Held keys do nothing.
    pub(super) fn icons_ask_key(&mut self, key: KeyEvent, repeat: bool) {
        let Some(q) = self.overlays.icons_ask_mut().filter(|_| !repeat) else { return };
        match key.code {
            KeyCode::Char('y') => self.icons_answered(true),
            KeyCode::Char('n') => self.icons_answered(false),
            // Not answered: the setting stays as it is (text marks while undecided), and the
            // next launch asks again.
            KeyCode::Esc => self.overlays.close(OverlayKind::IconsAsk),
            KeyCode::Enter => {
                let yes = q.yes_focused;
                self.icons_answered(yes);
            }
            KeyCode::Tab | KeyCode::BackTab => q.yes_focused = !q.yes_focused,
            KeyCode::Left | KeyCode::Char('h') => q.yes_focused = true,
            KeyCode::Right | KeyCode::Char('l') => q.yes_focused = false,
            _ => {}
        }
    }

    /// The icons question was answered: saved as `on` or `off`.
    fn icons_answered(&mut self, yes: bool) {
        self.overlays.close(OverlayKind::IconsAsk);
        self.set_icons(if yes { IconsSetting::On } else { IconsSetting::Off });
    }

    pub(super) fn set_language(&mut self, s: LangSetting) {
        self.lang_setting = s;
        self.i18n.set_lang(match s {
            LangSetting::En => Lang::En,
            LangSetting::Ko => Lang::Ko,
            LangSetting::Auto => self.auto_lang,
        });
        let m = self.persist();
        self.flash(m.unwrap_or(Notice::new(Label::LangSwitched, Level::Info)));
    }

    /// Save the settings, profiles, folders and `last_used` to the config file. Plaintext passwords that
    /// could not be moved to the keychain at launch are written back unchanged. Returns a
    /// message worth showing, if any.
    pub(super) fn persist(&mut self) -> Option<Notice> {
        match self.try_persist() {
            Ok(notice) => notice,
            Err(f) => {
                let error = self.fault_text("config.save_failed", &f);
                Some(Notice::new(Msg::ConfigSaveFailed { error }, Level::Error))
            }
        }
    }

    /// [`App::persist`], with the reason a write failed as a fault.
    pub(super) fn try_persist(&mut self) -> Result<Option<Notice>, datarig_core::fault::Fault> {
        // The launch-time password move rewrites the file when it finishes; save then.
        if self.migrating {
            self.persist_deferred = true;
            return Ok(None);
        }
        let Some(path) = self.config_path.clone() else {
            return Ok(self.config_broken.then(|| Notice::new(Label::ConfigReadonly, Level::Warning)));
        };
        let profiles = Profiles { connections: &self.profiles, folders: &self.folders, last_used: self.last_used };
        let settings = Settings {
            version: self.config_version,
            language: self.lang_setting.as_str(),
            icons: self.icons,
            theme: &self.theme_name,
            default_source: self.default_source,
            prefs: self.prefs,
        };
        config::save(&path, settings, Some(profiles))?;
        for p in &mut self.profiles {
            p.origin = Some(p.name.clone());
        }
        Ok(None)
    }
}
