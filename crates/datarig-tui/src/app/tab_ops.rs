//! Tab actions: open, switch, close with confirmation and
//! teardown, reopen, and the tab's connection. Each tab runs on one profile's connection; its
//! query session opens on its first statement.

use super::*;
use datarig_core::workspace::{self, UntrashError};

/// What a running statement, a statement waiting for its connection or an open transaction
/// would lose: the confirmation text, if any. `texts`: running, transaction, both, waiting.
/// A waiting statement next to other work (other tabs) is counted as running.
pub(super) fn at_risk(running: bool, queued: bool, tx: bool, texts: [Label; 4]) -> Option<Label> {
    match (running || queued, tx) {
        (false, false) => None,
        (true, false) if !running => Some(texts[3]),
        (true, false) => Some(texts[0]),
        (false, true) => Some(texts[1]),
        (true, true) => Some(texts[2]),
    }
}

impl App {
    /// `Ctrl+T`: a new, empty console tab. In the explorer, where its selected row is, as
    /// the row's menu opens it (its profile, database and schema: [`App::console_here`]);
    /// elsewhere (or on a row without a profile) on the active tab's profile, else on one picked
    /// in the quick connect list.
    pub(super) fn new_tab(&mut self) {
        if self.focus == Focus::Tree
            && let Some(row) = self.explorer_row()
            && self.console_here(&row)
        {
            return;
        }
        match self.tab().profile {
            Some(id) => {
                self.open_console(id, true);
            }
            None => self.open_quick(QuickPurpose::NewConsole),
        }
    }

    /// A new, empty console tab on profile `id` after the active one, made active; `focus`:
    /// the editor gets the focus (otherwise it stays where it is).
    pub(super) fn open_console(&mut self, id: ProfileId, focus: bool) -> TabId {
        self.leave_tab();
        let tab = self.tabs.open(TabKind::Console, Some(id), Editor::new(""));
        self.sync_tab_language(tab);
        self.entered_tab();
        if focus {
            self.focus = Focus::Editor;
        }
        tab
    }

    /// Tab `tab` runs on profile `id` in `context`, a database and schema of the profile's server
    /// (the profile's defaults: `Default`), from now on: its session, running
    /// statement and transaction end (the server rolls it back), a statement waiting for the old
    /// connection is dropped; its text and results stay. Work bound to the old binding
    /// is refused.
    pub(super) fn bind_tab_in(&mut self, tab: TabId, id: ProfileId, context: datarig_core::driver::SessionContext) {
        if self.tabs.get(tab).is_some_and(|t| t.exec.running.is_some()) {
            self.cancel_in(tab);
        }
        self.drop_queued(tab, |name| Msg::QueryQueuedDropped { name });
        let generation = self.tabs.next_generation();
        let Some(t) = self.tabs.get_mut(tab) else { return };
        if let Some(s) = t.exec.session.take() {
            s.close();
        }
        t.exec.generation = generation;
        t.exec.state = SessionState::Idle;
        // A run, a count or a fetch of more rows that ran ends here (its answer is never
        // read): said, so its announcement (or "waiting") is not the last word (one terminal
        // note per request, and per run).
        let stopped = match t.exec.running.take() {
            Some(r) if r.count => Some(Label::ResultsCountRebound),
            Some(r) if r.fetch => Some(Label::ResultsFetchRebound),
            Some(_) => {
                t.exec.run.answered(super::runlog::StatementOutcome::Cancelled, None);
                if !matches!(t.results, Results::Rows(_)) {
                    t.results = Results::Cancelled;
                }
                t.run_ended(true);
                Some(Label::QueryCancelledRebound)
            }
            None => None,
        };
        t.exec.resuming = None;
        t.exec.want_page = None;
        t.exec.tx_open = false;
        // The server rolls the user's transaction back when its session ends.
        t.block_ended(true);
        t.exec.lost_tx = false;
        t.exec.paging = Paging::None;
        t.exec.context = None;
        t.exec.path_per_transaction = false;
        if let Some(label) = stopped {
            let note = Notice::new(label, Level::Warning);
            t.exec.run.notes.push(note.clone());
            t.status = Some(note);
        }
        // The result's session is gone, so `n` is refused from now on: its hint goes too.
        if let Some(Notice { msg: Msg::ResultsPagingClosedRerun { count, .. }, level }) = t.status.clone() {
            t.status = Some(Notice::new(Msg::ResultsPagingClosed { count }, level));
        }
        self.tabs.bind_in(tab, Some(id), context);
        self.sync_tab_language(tab);
        // A run it stopped says so on its statements now.
        self.settle_run_hints(tab);
        if self.tab().id == tab {
            self.entered_tab();
        }
    }

    /// `Space c s`: pick another connection for the active tab; with a query running or a
    /// transaction open ask first.
    pub(super) fn request_set_connection(&mut self) {
        let t = self.tab();
        let id = t.id;
        let texts = [Label::TabRebindRunning, Label::TabRebindTx, Label::TabRebindRunningTx, Label::TabRebindQueued];
        match at_risk(t.exec.running.is_some(), self.is_queued(id), t.exec.tx_at_risk(), texts) {
            None => self.open_quick(QuickPurpose::Bind { tab: id, run: None }),
            Some(text) => {
                self.pending_context = None;
                self.confirm(Label::TabRebindTitle, text, Label::TabRebindKeys, ConfirmAction::SetConnection(id))
            }
        }
    }

    /// `Space c d`, `:use`: move the active tab to database and schema `to`
    /// (`None`: pick them). A running query, a statement waiting or an open transaction asks
    /// first (Cancel focused): the switch is a rebind, which ends them. Where the tab works
    /// already nothing happens, and nothing is asked.
    pub(super) fn request_set_context(&mut self, to: Option<datarig_core::driver::SessionContext>) {
        let t = self.tab();
        let id = t.id;
        if to.as_ref() == Some(&t.context) {
            return;
        }
        let texts = [Label::TabRebindRunning, Label::TabRebindTx, Label::TabRebindRunningTx, Label::TabRebindQueued];
        match at_risk(t.exec.running.is_some(), self.is_queued(id), t.exec.tx_at_risk(), texts) {
            None => match to {
                Some(ctx) => self.set_context(id, ctx),
                None => self.open_context_picker(id),
            },
            Some(text) => {
                self.pending_context = to.map(|c| (id, c));
                self.confirm(Label::TabContextTitle, text, Label::TabRebindKeys, ConfirmAction::SetContext(id))
            }
        }
    }

    /// Tab `id` works in `context` of its profile from now on (a rebind); the profile connects
    /// when it is not. Nothing happens when it is there already.
    pub(super) fn set_context(&mut self, id: TabId, context: datarig_core::driver::SessionContext) {
        let Some(p) = self.tabs.get(id).and_then(|t| t.profile) else { return };
        if self.tabs.get(id).is_some_and(|t| t.context == context) {
            return;
        }
        self.bind_tab_in(id, p, context);
        if matches!(self.conns.state(p), NodeState::Disconnected | NodeState::Failed) {
            self.connect(p);
        }
    }

    /// Switch tabs with `f` (it returns whether the active tab changed).
    pub(super) fn switch_tab(&mut self, f: impl FnOnce(&mut TabManager) -> bool) {
        let before = self.tab().id;
        self.leave_tab();
        if f(&mut self.tabs) && self.tab().id != before {
            self.entered_tab();
        }
    }

    /// The active tab is about to change: close what belongs to it on screen.
    pub(super) fn leave_tab(&mut self) {
        let t = self.tab_mut();
        t.popup = None;
        t.completion_due = None;
        let (id, save) = (t.id, t.dirty() && !t.doc.conflict && t.doc.save_due.is_some());
        self.overlays.close(OverlayKind::CellViewer);
        if save {
            self.save_tab(id);
        }
    }

    /// A tab became active: the status bar shows its last outcome, and the workspace state is
    /// written (which tabs are open, which one is active).
    pub(super) fn entered_tab(&mut self) {
        self.status = self.tab().status.clone();
        self.save_workspace();
    }

    /// `:q` and `:wq`: close the active tab, or quit on the last one (vim); both ask first
    /// when something would be lost.
    pub(super) fn close_or_quit(&mut self) {
        if self.tabs.len() > 1 { self.request_close_tab() } else { self.request_quit() }
    }

    /// `Ctrl+W`: close the active tab; with a running query, an open transaction or edits
    /// that could not be written (a saved query's or a console's) ask first. A
    /// console that closes goes to the trash, where `Space t u` finds it.
    pub(super) fn request_close_tab(&mut self) {
        let id = self.tab().id;
        self.request_close(id);
    }

    /// Close tab `id` (the active one or another, from the tab list) as `Ctrl+W` closes the
    /// active one: written first, and asked first when something would be lost.
    pub(super) fn request_close(&mut self, id: TabId) {
        let Some(t) = self.tabs.get(id) else { return };
        // The tab is written first; a saved query that changed on disk asks what to do.
        if t.dirty() && !t.doc.conflict {
            self.save_tab(id);
            if self.tabs.get(id).is_some_and(|t| t.doc.conflict) {
                return;
            }
        }
        if self.conflicted(Some(id)) > 0 {
            let keys = Label::TabCloseKeys;
            return self.confirm(
                Label::TabCloseTitle,
                Label::TabCloseScriptConflict,
                keys,
                ConfirmAction::CloseTab(id),
            );
        }
        let Some(t) = self.tabs.get(id) else { return };
        if self.unsaved(Some(id)) > 0 {
            let details = self.unsaved_error(Some(id)).into_iter().collect();
            let text = if t.script().is_some() { Label::TabCloseUnsaved } else { Label::TabCloseConsoleUnsaved };
            self.confirm(Label::TabCloseTitle, text, Label::TabCloseKeys, ConfirmAction::CloseTab(id));
            if let Some(c) = self.overlays.confirm_mut() {
                c.details = details;
            }
            return;
        }
        let texts = [Label::TabCloseRunning, Label::TabCloseTx, Label::TabCloseRunningTx, Label::TabCloseQueued];
        match at_risk(t.exec.running.is_some(), self.is_queued(id), t.exec.tx_at_risk(), texts) {
            None => self.close_tab(id),
            Some(text) => self.confirm(Label::TabCloseTitle, text, Label::TabCloseKeys, ConfirmAction::CloseTab(id)),
        }
    }

    /// Close tab `id`: drop a statement waiting for its connection, cancel its running
    /// statement, then close its session (the server rolls back an open transaction), and keep
    /// it for `Space t u`. A console's text goes to the trash (its file leaves `consoles/`), so
    /// `Space t u` finds it after a restart too. Closing the last tab leaves the workspace
    /// empty (its empty state says how to open one).
    pub(super) fn close_tab(&mut self, id: TabId) {
        if self.tabs.get(id).is_some_and(|t| t.script().is_some() && t.dirty() && !t.doc.conflict) {
            self.save_tab(id);
        }
        let Some(t) = self.tabs.get(id) else { return };
        let (running, tx) = (t.exec.running.is_some(), t.exec.tx_at_risk());
        let console = (t.script().is_none() && t.is_query()).then(|| (t.doc.console_id.clone(), t.editor.text()));
        let queued = self.take_queued(id).is_some();
        if running {
            self.cancel_in(id);
        }
        let was_active = self.tab().id == id;
        if was_active {
            self.leave_tab();
        }
        if let Some(mut closed) = self.tabs.close(id)
            && let Some(s) = closed.exec.session.take()
        {
            s.close();
        }
        // A file another open tab shows stays where it is (never made, but never trusted).
        if let (Some((c, text)), Some(state)) = (console, self.state_dir())
            && !self.tabs.iter().any(|o| o.doc.console_id == c)
        {
            self.trash_closed(&state, &c, &text);
        }
        // Another tab than the active one (the tab list's `Ctrl+D`): the active tab and the
        // status line stay; only the workspace state is written.
        if was_active {
            self.entered_tab();
        } else {
            self.save_workspace();
        }
        if self.tabs.is_empty() {
            self.focus = Focus::Tree;
        }
        if tx {
            self.flash(Notice::new(Label::TabClosedTx, Level::Warning));
        } else if running {
            self.flash(Notice::new(Label::TabClosedQuery, Level::Warning));
        } else if queued {
            self.flash(Notice::new(Label::TabClosedQueued, Level::Warning));
        }
        if self.quitting.is_some() && !self.any_running() {
            self.finish_quit();
        }
        self.refresh_tab_list();
    }

    /// The console `id` of a tab just closed, with `text`, goes to the trash. Text nobody
    /// wrote (an empty console) is not kept; its file just goes. When the
    /// trash cannot take it the file stays (the next launch recovers it) and the user is told.
    fn trash_closed(&mut self, state: &Path, id: &str, text: &str) {
        let result = if persist::unwritten(text) {
            workspace::remove_console(state, id).map(|()| None)
        } else {
            workspace::trash_console(state, id, text).map(|t| Some(t.name))
        };
        match result {
            Ok(trashed) => {
                if let Some(c) = self.tabs.last_closed_mut().filter(|c| c.console_id == id) {
                    c.trashed = trashed;
                }
            }
            Err(e) => {
                let error = self.io_text(&e);
                self.flash(Notice::new(Msg::TabTrashFailed { error }, Level::Warning));
            }
        }
    }

    /// `Space t u`: bring the most recently closed tab back (no session until it runs).
    /// A saved query comes back with its file's text (the tab it is open in, if it was opened
    /// again meanwhile); one whose file is gone comes back as a console with the closed text.
    /// Once the tabs closed in this run are used up, the newest console in the trash comes
    /// back (from earlier runs too); a trashed file that cannot be read is skipped (and said
    /// so) for the next one, and stays in the trash.
    pub(super) fn reopen_tab(&mut self) {
        let Some(c) = self.tabs.take_closed() else {
            let newest = self.state_dir().map(|s| workspace::list_trash(&s));
            return match newest {
                Some(Ok(list)) if !list.is_empty() => self.untrash_newest(&list),
                Some(Err(e)) => {
                    let error = self.io_text(&e);
                    self.flash(Notice::new(Msg::TabTrashUnreadable { error }, Level::Error));
                }
                _ => self.flash(Notice::new(Label::TabReopenNone, Level::Info)),
            };
        };
        self.bring_back(c);
    }

    /// Closed tab `serial` (the tab list's pick) comes back as `Space t u` brings the newest.
    pub(super) fn reopen_closed(&mut self, serial: u64) {
        if let Some(c) = self.tabs.take_closed_serial(serial) {
            self.bring_back(c);
        }
    }

    /// Closed tab `c`, taken off the list, comes back and becomes active.
    fn bring_back(&mut self, mut c: tabs::ClosedTab) {
        let mut written = false;
        if let (Some(name), Some(state)) = (c.trashed.clone(), self.state_dir()) {
            // Its file comes back out of the trash (under a new id); the text is the same.
            match workspace::untrash(&state, &name) {
                Ok((id, text)) => {
                    c.console_id = id;
                    c.text = text;
                    written = true;
                }
                Err(UntrashError::Unreadable(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    let msg = self.untrash_error(&e);
                    self.flash(Notice::new(msg, Level::Warning));
                }
            }
        }
        if let Some(path) = c.script.clone() {
            if let Some(t) = self.tabs.find_script(&path) {
                self.switch_tab(|m| m.position(t).is_some_and(|i| m.activate(i)));
                self.focus = Focus::Editor;
                return;
            }
            match self.scripts.as_ref().map(|s| s.read(&path)) {
                Some(Ok((text, stamp))) => {
                    c.text = text.clone();
                    self.leave_tab();
                    let id = self.tabs.reopen_closed(c);
                    self.sync_tab_language(id);
                    if let Some(t) = self.tabs.get_mut(id) {
                        t.kind = TabKind::Script;
                        t.doc.saved = text;
                        t.doc.written = true;
                        t.doc.stamp = Some(stamp);
                    }
                    self.entered_tab();
                    self.focus = Focus::Editor;
                    return;
                }
                _ => {
                    c.script = None;
                    c.kind = TabKind::Console;
                    let name = path.clone();
                    self.flash(Notice::new(Msg::ScriptsReopenedAsConsole { name }, Level::Warning));
                }
            }
        }
        self.leave_tab();
        let id = self.tabs.reopen_closed(c);
        self.sync_tab_language(id);
        if let Some(t) = self.tabs.get_mut(id).filter(|_| written) {
            t.doc.saved = t.editor.text();
            t.doc.written = true;
        }
        self.entered_tab();
        self.focus = Focus::Editor;
    }

    /// `Space t u` past this run's closed tabs: the newest file of the trash (`list`, newest
    /// first) that can be read comes back; the ones before it that cannot be read are
    /// skipped, and said so.
    fn untrash_newest(&mut self, list: &[workspace::Trashed]) {
        let mut skipped: Option<(u64, std::io::Error)> = None;
        for t in list {
            match self.take_from_trash(&t.name) {
                Ok(()) => break,
                Err(UntrashError::Unreadable(e)) => {
                    skipped = Some(skipped.map_or((1, e), |(n, first)| (n + 1, first)));
                }
                Err(e) => {
                    let msg = self.untrash_error(&e);
                    return self.flash(Notice::new(msg, Level::Error));
                }
            }
        }
        if let Some((count, e)) = skipped {
            let error = self.io_text(&e);
            self.flash(Notice::new(Msg::TabTrashFilesSkipped { count, error }, Level::Warning));
        }
    }

    /// Bring trashed console `name` back as a new tab (`:recover`).
    pub(super) fn untrash(&mut self, name: &str) {
        if let Err(e) = self.take_from_trash(name) {
            let msg = self.untrash_error(&e);
            self.flash(Notice::new(msg, Level::Error));
        }
    }

    /// Trashed console `name` as a new tab.
    fn take_from_trash(&mut self, name: &str) -> Result<(), UntrashError> {
        let Some(state) = self.state_dir() else { return Ok(()) };
        let (id, text) = workspace::untrash(&state, name)?;
        // Closed in this run: it comes back under its own number when that is free, as
        // `Space t u` brings it.
        let no = self.tabs.closed().find(|c| c.trashed.as_deref() == Some(name)).map(|c| c.console_no);
        self.tabs.forget_trashed(name);
        self.leave_tab();
        let tab = self.tabs.open(TabKind::Console, None, Editor::new(&text));
        self.sync_tab_language(tab);
        let taken = |n: u32| self.tabs.iter().any(|t| t.id != tab && t.doc.console_no == n);
        if let Some(n) = no.filter(|n| *n != 0 && !taken(*n))
            && let Some(t) = self.tabs.get_mut(tab)
        {
            t.doc.console_no = n;
        }
        if let Some(t) = self.tabs.get_mut(tab) {
            t.doc.console_id = id;
            t.doc.saved = text;
            t.doc.written = true;
        }
        self.entered_tab();
        self.focus = Focus::Editor;
        Ok(())
    }

    /// Why a trashed console did not come back, in words.
    fn untrash_error(&self, e: &UntrashError) -> Msg {
        match e {
            UntrashError::Unreadable(e) => Msg::TabUntrashUnreadable { error: self.io_text(e) },
            UntrashError::Io(e) => Msg::TabUntrashFailed { error: self.io_text(e) },
        }
    }

    /// Ask a yes/no question.
    pub(super) fn confirm(&mut self, title: Label, text: Label, keys: Label, action: ConfirmAction) {
        self.tab_mut().popup = None;
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.key_state.clear();
        self.overlays.push(Overlay::Confirm(Confirm {
            title,
            text: text.into(),
            details: Vec::new(),
            keys,
            action,
            folder: None,
            path: None,
            buttons: Default::default(),
        }));
    }
}
