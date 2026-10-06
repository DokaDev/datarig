//! Statement execution and driver events: running, paging, cancelling and turning
//! [`DbEvent`]s into results, tree and completion state.

use super::persist;
use super::runlog::StatementOutcome;
use super::*;
use datarig_core::driver::DbError;
use datarig_core::fault::{ErrorLog, FaultKind};
use datarig_core::sql::lexer::changes_schema;
use datarig_core::sql::risk;
use datarig_core::transport::{DialError, Refusal};

impl App {
    /// Send a tree or catalog request to profile `id`'s metadata session.
    pub(super) fn send_meta(&self, id: ProfileId, cmd: DbCommand) {
        if let Some(s) = self.conns.get(id).and_then(|c| c.meta.as_ref()) {
            s.send(cmd);
        }
    }

    /// Send a statement command to tab `id`'s query session.
    pub(super) fn send_tab(&self, id: TabId, cmd: DbCommand) {
        if let Some(s) = self.tabs.get(id).and_then(|t| t.exec.session.as_ref()) {
            s.send(cmd);
        }
    }

    /// Ctrl+Enter / Ctrl+E: statement under the cursor, or the Visual selection.
    pub fn execute_current(&mut self) {
        let ed = &mut self.tab_mut().editor;
        // The selection's statements, else the statement the editor highlights (found around
        // the cursor, as in the whole text); with their places, to mark them while they run.
        let (stmts, spans) = ed.run_statements();
        if stmts.is_empty() {
            self.flash(Notice::new(Label::QueryNoStatement, Level::Warning));
            return;
        }
        // Refused while the tab runs or waits: what its run took stays staged for it.
        if self.tab_busy(self.tab().id) {
            return self.flash_busy();
        }
        self.tab_mut().editor.stage_run(&stmts, spans);
        self.run(stmts);
    }

    /// Run `statements` in the active tab.
    pub fn run(&mut self, statements: Vec<String>) {
        let id = self.tab().id;
        self.run_in(id, statements);
    }

    /// Run `statements` in tab `id` (one statement at a time per tab). A tab without a
    /// connection asks for one first (quick connect); a tab whose profile is not connected
    /// connects it and runs once it is.
    pub(super) fn run_in(&mut self, id: TabId, statements: Vec<String>) {
        if self.tab_busy(id) {
            self.flash_busy();
            return;
        }
        let Some(t) = self.tabs.get(id) else { return };
        let Some(pid) = t.profile else {
            return self.open_quick(QuickPurpose::Bind { tab: id, run: Some(statements) });
        };
        // Checked before anything is queued or sent.
        if let Some(refused) = self.unsupported(&statements) {
            self.unstage_run(id);
            return self.tab_status(id, refused);
        }
        if let Some(refused) = self.read_only_refusal(id, pid, &statements) {
            self.unstage_run(id);
            return self.tab_status(id, refused);
        }
        let items = self.dangerous(id, pid, &statements);
        if !items.is_empty() {
            return self.ask_before_run(id, pid, statements, items);
        }
        self.run_approved(id, statements);
    }

    /// Run `statements` in tab `id` once the checks passed or the user confirmed them: on its
    /// session, or queued until its profile connects. The read-only check is made again right
    /// before, so nothing it refuses ever reaches a session.
    pub(super) fn run_approved(&mut self, id: TabId, statements: Vec<String>) {
        if self.tab_busy(id) {
            self.flash_busy();
            return;
        }
        let Some(pid) = self.tabs.get(id).and_then(|t| t.profile) else { return };
        if let Some(refused) = self.read_only_refusal(id, pid, &statements) {
            self.unstage_run(id);
            return self.tab_status(id, refused);
        }
        let Some(t) = self.tabs.get(id) else { return };
        if t.exec.session.is_none() && self.conns.get(pid).is_none_or(|c| c.resolved.is_none()) {
            // Bound to this tab, this profile and this tab generation.
            let q = Queued { tab: id, profile: pid, binding: t.binding, statements };
            let c = self.conns.entry(pid);
            c.pending.push(q);
            if c.connecting.is_none() {
                self.connect(pid);
            }
            return;
        }
        if t.exec.session.is_none() && !self.open_query_session(id) {
            return self.unstage_run(id);
        }
        self.query_seq += 1;
        let qid = self.query_seq;
        if let Some(t) = self.tabs.get_mut(id) {
            // The driver ends an open portal before it runs the next statement: the rows stay
            // (until this run delivers rows of its own), past them paging is closed.
            if matches!(t.exec.paging, Paging::Open { .. }) {
                t.exec.paging = Paging::Replaced;
                t.exec.rerun_ok = datarig_core::sql::risk::repeat::repeatable(t.answer_sql()).is_ok();
            }
            t.exec.want_page = None;
            t.exec.resuming = None;
            t.exec.released = false;
            t.exec.query_id = qid;
            // Marked on the editor's text when that is where the user ran them from.
            t.editor.start_run(qid, &statements);
            // The row results stay until this run delivers its first rows:
            // a run without rows (a COMMIT) leaves them on screen, from an earlier run.
            let has_rows = matches!(t.results, Results::Rows(_)) || !t.exec.steps.is_empty();
            let before = std::mem::replace(&mut t.exec.run, super::runlog::RunLog::new(&statements));
            if has_rows && t.exec.kept_log.is_none() {
                t.exec.kept_log = Some(before);
            }
            t.exec.replace_pending = true;
            t.exec.messages_scroll = 0;
            t.exec.ddl = statements.iter().any(|s| changes_schema(s));
            t.exec.explain_rolled_back = statements.iter().any(|s| risk::classify(s).rollback_matters());
            // What the run prepares and deallocates counts once the server says each statement
            // succeeded (`TabSession::succeeded`), never when it is sent.
            t.exec.unconfirmed = statements.iter().cloned().enumerate().collect();
            t.exec.running =
                Some(Running { id: qid, started: Instant::now(), fetch: false, count: false, cancelling: None });
            if t.exec.path_per_transaction {
                t.warn_session_path();
            }
            // The results pane shows from the first run on, also after the user hid it.
            t.ran = true;
            t.pane.hidden = false;
        }
        let paging = self.tabs.get(id).map(|t| self.paging_mode(t)).unwrap_or_default();
        self.send_tab(id, DbCommand::Execute { id: qid, statements, paging });
    }

    /// Open tab `id`'s query session with its profile's password (the first run, or the next
    /// one after the connection was lost). A lost metadata session reopens too.
    fn open_query_session(&mut self, id: TabId) -> bool {
        let Some(pid) = self.tabs.get(id).and_then(|t| t.profile) else { return false };
        let Some(c) = self.conns.get(pid) else { return false };
        let Some(cfg) = c.resolved.clone() else { return false };
        if c.meta.is_none() && c.connecting.is_none() {
            self.reopen_meta(pid);
        }
        let Some(t) = self.tabs.get(id) else { return false };
        // A connect of the profile since (e.g. after its tunnel was lost) made the tab idle; the
        // user's transaction that was open is still gone, and said so.
        let (lost, lost_tx) = (t.exec.state == SessionState::Lost || t.exec.lost_tx, t.exec.lost_tx);
        // A tab in another database: that database's catalog and keys come from a metadata
        // session of their own.
        if let Some(db) = self.tabs.get(id).and_then(|t| self.other_database(t)).map(str::to_string) {
            self.ensure_aux(pid, &db);
        }
        self.open_tab_session(id, &cfg);
        let Some(t) = self.tabs.get_mut(id) else { return false };
        if t.exec.session.is_none() {
            return false;
        }
        t.exec.lost_tx = false;
        if lost {
            let label = if lost_tx { Label::QueryReconnectedTx } else { Label::QueryReconnected };
            self.flash(Notice::new(label, Level::Warning));
        }
        true
    }

    /// Cancel the statement running in the active tab (only there).
    pub fn cancel(&mut self) {
        let id = self.tab().id;
        self.cancel_in(id);
    }

    /// Cancel what runs in tab `id`: its running statement, or the statement that waits for
    /// its connection (dropped, never run).
    pub(super) fn cancel_in(&mut self, id: TabId) {
        // Its profile's password is being read from the keychain (it may not answer): the
        // attempt ends, and what waited for it with it.
        if let Some(p) = self.tabs.get(id).and_then(|t| t.profile)
            && self.conns.get(p).and_then(|c| c.connecting.as_ref()).is_some_and(|c| c.keychain)
        {
            return self.cancel_connect(p);
        }
        if self.take_queued(id).is_some() {
            self.unstage_run(id);
            return self.tab_status(id, Notice::new(Label::QueryCancelled, Level::Warning));
        }
        // A copy that fetches every row first stops now: a page that lands after this asks
        // for nothing more, and nothing is copied.
        let copy = self.stop_fetch_copy(id);
        let now = self.now();
        let Some(t) = self.tabs.get_mut(id) else { return };
        let Some(r) = t.exec.running.as_mut() else { return };
        if r.cancelling.is_some() {
            return;
        }
        r.cancelling = Some(now);
        if let Some(s) = &t.exec.session {
            s.cancel();
        }
        if copy {
            self.tab_status(id, Notice::new(Label::CopyFetchCancelled, Level::Warning));
        }
    }

    /// Tab `id`'s session did not answer a cancel within [`CANCEL_GRACE`] (a wedged driver or
    /// connection): the run is marked cancelled all the same, so the tab is never stuck
    /// running, and the session is closed (the driver cancels what runs and closes the
    /// connection; the server rolls an open transaction back). Its late events are ignored;
    /// the next run connects again and says the transaction is gone if one was open.
    pub(super) fn cancel_unanswered(&mut self, id: TabId) {
        let generation = self.tabs.next_generation();
        let Some(t) = self.tabs.get_mut(id) else { return };
        let Some(r) = t.exec.running.take() else { return };
        if let Some(s) = t.exec.session.take() {
            s.close();
        }
        t.exec.generation = generation;
        t.exec.unsure();
        t.exec.state = SessionState::Lost;
        t.exec.lost_tx = t.exec.tx_at_risk();
        t.exec.tx_open = false;
        // The server rolls the user's transaction back when its session ends.
        t.block_ended(true);
        t.exec.paging = Paging::None;
        t.exec.resuming = None;
        t.exec.want_page = None;
        // A count or a fetch of more rows leaves the rows as they are: whether the server has
        // more is still unknown (never "complete"); row results stay (an earlier run's, or this
        // run's shown one).
        if !matches!(t.results, Results::Rows(_)) {
            t.results = Results::Cancelled;
        }
        if !r.fetch && !r.count {
            // The statement it was at is cancelled (its hint says so).
            t.exec.run.answered(StatementOutcome::Cancelled, None);
            t.run_ended(true);
        }
        self.settle_run_hints(id);
        self.tab_status(id, Notice::new(Label::QueryCancelUnanswered, Level::Warning));
    }

    /// Cancel whatever runs or waits for its connection in any tab.
    pub(super) fn cancel_all(&mut self) {
        let ids: Vec<TabId> = self.tabs.iter().map(|t| t.id).filter(|id| self.tab_busy(*id)).collect();
        for id in ids {
            self.cancel_in(id);
        }
    }

    /// Tab `id` has a statement waiting for its profile to connect.
    pub fn is_queued(&self, id: TabId) -> bool {
        self.conns.iter().any(|(_, c)| c.pending.iter().any(|q| q.tab == id))
    }

    /// Tab `id` runs a statement or has one waiting for its connection (both count
    /// as work in progress wherever the app asks before ending it).
    pub fn tab_busy(&self, id: TabId) -> bool {
        self.tabs.get(id).is_some_and(|t| t.exec.running.is_some()) || self.is_queued(id)
    }

    /// Remove the statement waiting in tab `id`, if any.
    pub(super) fn take_queued(&mut self, id: TabId) -> Option<Queued> {
        self.conns.iter_mut().find_map(|(_, c)| {
            let i = c.pending.iter().position(|q| q.tab == id)?;
            Some(c.pending.remove(i))
        })
    }

    /// Drop the statement waiting in tab `id`; the tab says why (`why` gets the name of the
    /// profile it waited for).
    pub(super) fn drop_queued(&mut self, id: TabId, why: impl FnOnce(String) -> Msg) {
        if let Some(q) = self.take_queued(id) {
            self.unstage_run(id);
            let name = self.profile(q.profile).map(|p| p.name.clone()).unwrap_or_default();
            self.tab_status(id, Notice::new(why(name), Level::Warning));
        }
    }

    /// Drop every statement waiting for profile `id` (its attempt ended); each tab says so.
    pub(super) fn drop_pending(&mut self, id: ProfileId) {
        // A DDL tab that waited reads on `r`.
        self.ddl_stop(id, None, super::tabs::DdlState::NotLoaded, true);
        let pending = self.conns.get_mut(id).map(|c| std::mem::take(&mut c.pending)).unwrap_or_default();
        let name = self.profile(id).map(|p| p.name.clone()).unwrap_or_default();
        for q in pending {
            self.unstage_run(q.tab);
            self.tab_status(q.tab, Notice::new(Msg::QueryQueuedFailed { name: name.clone() }, Level::Warning));
        }
    }

    /// Ask tab `id`'s session for the next page of its result.
    pub(super) fn fetch_page(&mut self, id: TabId) {
        let Some(t) = self.tabs.get_mut(id) else { return };
        let qid = t.exec.query_id;
        t.exec.running =
            Some(Running { id: qid, started: Instant::now(), fetch: true, count: false, cancelling: None });
        self.send_tab(id, DbCommand::FetchMore { id: qid });
    }

    /// Rows of the active tab's result could not be read from its spill file (the grid shows
    /// them as not read): said in the status bar, the detail in the error log.
    pub(crate) fn results_read_failed(&mut self, fault: &datarig_core::fault::Fault) {
        // Every frame fails the same way until the rows change: said and logged once.
        if self.status.as_ref().is_some_and(|n| matches!(n.msg, Msg::ResultsReadFailed { .. })) {
            return;
        }
        let error = self.fault_text("results.read_failed", fault);
        self.status = Some(Notice::new(Msg::ResultsReadFailed { error }, Level::Error));
    }

    /// A movement key at the last row of a page that has another after it: say which key
    /// shows it (nothing is fetched by scrolling).
    pub(super) fn paging_hint(&mut self) {
        let t = self.tab();
        let Results::Rows(rs) = &t.results else { return };
        let len = rs.rows.len();
        if !t.grid.at_page_end(len) || (t.grid.window(len).end == len && !rs.more) {
            return;
        }
        let key = self
            .keymap
            .hint_keys(Action::PageNext, Ctx::Grid, self.enhanced_keys)
            .map(|k| crate::keymap::keys::label(&k))
            .unwrap_or_default();
        self.flash(Notice::new(Msg::ResultsPageHint { key }, Level::Info));
    }

    /// Tab `id`'s result has had its portal open for the policy's `paging_idle_timeout`
    /// without a fetch: ask the driver to close it (it ends the implicit transaction that held
    /// it, never the user's own). The rows stay, and the server may have more (unknown is not
    /// absent): the next page runs the statement again when it is on the allowlist.
    pub(super) fn close_idle_portal(&mut self, id: TabId) {
        let Some(t) = self.tabs.get_mut(id) else { return };
        let Some((rs, _)) = t.answer_rows_mut() else { return };
        let count = rs.rows.len() as u64;
        t.exec.paging = Paging::ClosedIdle;
        let rerun = datarig_core::sql::risk::repeat::repeatable(t.answer_sql()).is_ok();
        t.exec.rerun_ok = rerun;
        let qid = t.exec.query_id;
        self.send_tab(id, DbCommand::ClosePortal { id: qid });
        let m = if rerun {
            let key = self
                .keymap
                .hint_keys(Action::PageNext, Ctx::Grid, self.enhanced_keys)
                .map(|k| crate::keymap::keys::label(&k))
                .unwrap_or_default();
            Msg::ResultsPagingClosedRerun { count, key }
        } else {
            Msg::ResultsPagingClosed { count }
        };
        self.tab_status(id, Notice::new(m, Level::Info));
    }

    /// A driver event of the active tab's profile, as the headless tests feed them:
    /// connection, tree and catalog events belong to the profile's metadata session, statement
    /// events to the active tab's query session.
    pub fn on_db_event(&mut self, ev: DbEvent) {
        let target = match &ev {
            DbEvent::Page { .. }
            | DbEvent::Released { .. }
            | DbEvent::Done { .. }
            | DbEvent::Failed { .. }
            | DbEvent::TxOpen(_)
            | DbEvent::TxAborted(_)
            | DbEvent::Block(_)
            | DbEvent::Counted { .. }
            | DbEvent::Started { .. }
            | DbEvent::StepRows { .. }
            | DbEvent::Finished { .. } => EventTarget::Tab(self.tab().id),
            // The attempt in progress, else the active tab's profile.
            _ => match self.latest_attempt().or(self.tab().profile) {
                Some(id) => EventTarget::Meta(id),
                None => return,
            },
        };
        self.on_target_event(target, ev);
    }

    /// A driver event of the session `target` names (its generation already checked).
    pub(super) fn on_target_event(&mut self, target: EventTarget, ev: DbEvent) {
        match target {
            EventTarget::Meta(id) => self.meta_event(id, ev),
            EventTarget::Tab(id) => {
                self.tab_event(id, ev);
                self.settle_run_hints(id);
            }
            EventTarget::Aux(id) => self.aux_event(id, ev),
        }
        // A run that started or ended may show or hide a pane (a results zoom waiting for them).
        if !self.tabs.is_empty() && !self.profiles.is_empty() {
            let before = self.focus;
            self.settle_panes(false);
            // Not a move of the user's: the next input compares against where it is now.
            if self.focus != before {
                self.last_focus = self.focus;
            }
        }
        // "Quit anyway" waits for the running queries to stop.
        if self.quitting.is_some() && !self.any_running() {
            self.finish_quit();
        }
    }

    /// Whether an event of `target` with `generation` belongs to a current session.
    pub(super) fn is_current(&self, target: EventTarget, generation: u64) -> bool {
        match target {
            EventTarget::Meta(pid) => self.conns.is_current(pid, generation),
            EventTarget::Tab(id) => self.tabs.get(id).is_some_and(|t| t.exec.generation == generation),
            // An aux session's id is its generation; a closed one is gone.
            EventTarget::Aux(id) => id == generation && self.conns.aux_by_id_ref(id).is_some(),
        }
    }

    /// Open profile `id`'s metadata session in database `database` unless it has one (step
    /// 2.7.1): once the profile is connected (it needs the password the profile connected
    /// with). The profile's own database has its main metadata session.
    ///
    /// Each call is a use (the session stays while used); one closed for being
    /// idle, failed or lost opens again (what it read stays until the new one reads it again).
    pub(super) fn ensure_aux(&mut self, id: ProfileId, database: &str) {
        let now = self.now();
        let known = self.conns.aux(id, database).map(|a| (a.id, a.session.is_some()));
        if let Some((aux, _)) = known
            && let Some(a) = self.conns.aux_by_id(aux)
        {
            a.used = now;
        }
        if known.is_some_and(|(_, open)| open) {
            return;
        }
        let Some(cfg) = self.conns.get(id).filter(|c| c.connected).and_then(|c| c.resolved.clone()) else { return };
        let aux = match known {
            Some((old, _)) => match self.conns.renew_aux(old) {
                Some(aux) => aux,
                None => return,
            },
            None => self.conns.add_aux(id, database, now),
        };
        let context = datarig_core::driver::SessionContext { database: Some(database.to_string()), schema: None };
        let session = self.open_session(&cfg, SessionRole::Meta, EventTarget::Aux(aux), aux, context);
        if let Some(a) = self.conns.aux_by_id(aux) {
            a.session = session;
        }
    }

    /// An event of a metadata session in another database: its schemas, catalog and keys.
    fn aux_event(&mut self, id: u64, ev: DbEvent) {
        let text = self.event_error_text(&ev);
        // The error log gets the underlying error, not the UI's words for it (which may point
        // to the error log itself).
        let raw = match &ev {
            DbEvent::ConnectFailed { error, .. } | DbEvent::Lost { error } => error.raw().into_owned(),
            _ => String::new(),
        };
        let Some(a) = self.conns.aux_by_id(id) else { return };
        let (profile, database) = (a.profile, a.database.clone());
        if let DbEvent::Ddl { id: request, result } = ev {
            return self.ddl_answered(profile, Some(&database), request, result);
        }
        if matches!(ev, DbEvent::ConnectFailed { .. } | DbEvent::Lost { .. }) {
            self.ddl_stop(profile, Some(Some(&database)), super::tabs::DdlState::Failed(text.clone()), true);
        }
        let Some(a) = self.conns.aux_by_id(id) else { return };
        match ev {
            // It opens again on the next use; why it failed is said, and where it
            // was not read yet, what the picker and the explorer show.
            DbEvent::ConnectFailed { .. } | DbEvent::Lost { .. } => {
                if let Some(s) = a.session.take() {
                    s.close();
                }
                if !matches!(a.schemas, Some(Ok(_))) {
                    a.schemas = Some(Err(text.clone()));
                }
                a.tree.schemas_loading = false;
                let (database, profile) = (a.database.clone(), a.profile);
                // The whole reason (the explorer's note and the status bar cut it):
                // the error log, and the app's Messages, one entry per database.
                let why = if raw.is_empty() { &text } else { &raw };
                let detail = datarig_core::fault::Fault::other(format!("{database}: {why}"));
                ErrorLog::new(self.paths.errors_log()).record("aux.failed", &detail);
                let m = Notice::new(Msg::AuxFailed { database: database.clone(), error: text }, Level::Error);
                self.forget_aux_failure(&database);
                self.notices.push(m.clone());
                self.status = Some(m);
                self.use_answered(profile, super::cmdline::UseAsk::Schemas(database), false);
            }
            DbEvent::Schemas(Ok(names)) => {
                a.tree.set_schemas(names.clone());
                a.schemas = Some(Ok(names));
                // It opened: what was said about its last failure goes.
                let (database, profile) = (a.database.clone(), a.profile);
                self.forget_aux_failure(&database);
                self.use_answered(profile, super::cmdline::UseAsk::Schemas(database), true);
            }
            DbEvent::Schemas(Err(_)) => {
                a.tree.schemas_loading = false;
                a.schemas = Some(Err(text));
                let (database, profile) = (a.database.clone(), a.profile);
                self.use_answered(profile, super::cmdline::UseAsk::Schemas(database), false);
            }
            DbEvent::Objects { schema, result } => {
                a.tree.set_objects(&schema, result.map_err(|_| text));
                let (profile, database) = (a.profile, a.database.clone());
                self.revealed(profile, Some(&database), &schema);
            }
            DbEvent::Structure { schema, table, result } => {
                a.tree.set_structure(&schema, &table, result.map_err(|_| text))
            }
            DbEvent::Catalog(Ok(cat)) => a.catalog = cat,
            DbEvent::Keys(Ok(keys)) => a.keys = Keys::Loaded(keys),
            DbEvent::Keys(Err(_)) => a.keys = Keys::Failed(text),
            _ => {}
        }
    }

    /// Database `database`'s metadata session opened (or failed again): the entry of its last
    /// failure leaves the Messages, and the status bar when it says that failure.
    fn forget_aux_failure(&mut self, database: &str) {
        let of = |n: &Notice| matches!(&n.msg, Msg::AuxFailed { database: d, .. } if d == database);
        self.notices.retain(|n| !of(n));
        if self.status.as_ref().is_some_and(of) {
            self.status = None;
        }
    }

    fn meta_event(&mut self, id: ProfileId, ev: DbEvent) {
        let Some(name) = self.profile(id).map(|p| p.name.clone()) else { return };
        let text = self.event_error_text(&ev);
        match ev {
            DbEvent::Connected => self.on_connected(id),
            // The profile's sessions are read-only per transaction only: said once it connected.
            DbEvent::ReadOnlyPerTransaction => {
                self.status = Some(Notice::new(Msg::DbReadOnlyPerTransaction { name }, Level::Info));
            }
            DbEvent::ConnectFailed { error, auth } => {
                let had_password =
                    self.conns.get(id).and_then(|c| c.connecting.as_ref()).is_none_or(|a| a.had_password);
                self.save_on_connect = None;
                // The server asked for a password and none was sent: say so plainly.
                let missing = auth && !had_password && error.raw().contains("password");
                let failed = Notice::new(Msg::ConnFailed { error: text }, Level::Error);
                let reason = if !missing {
                    failed.clone()
                } else if self.secrets.unavailable.is_some() {
                    Notice::new(Label::PromptPasswordNoKeychain, Level::Warning)
                } else {
                    Notice::new(Label::PromptPasswordMissing, Level::Warning)
                };
                // Ask again, except for a command or an environment variable: those are fixed
                // where they come from.
                let ask = self
                    .profile(id)
                    .cloned()
                    .filter(|c| auth && !matches!(c.source().kind(), SourceKind::Command | SourceKind::Env));
                // What waited for this attempt waits for the next one when the prompt asks;
                // otherwise it is dropped and its tabs say so.
                let (console, expand) = self.conns.get(id).map_or((None, false), |c| (c.console, c.expand_on_connect));
                let pending = match ask {
                    Some(_) => self.conns.get_mut(id).map(|c| std::mem::take(&mut c.pending)).unwrap_or_default(),
                    None => Vec::new(),
                };
                let ddl = if ask.is_some() { self.ddl_waiting(id) } else { Vec::new() };
                self.attempt_failed(id, reason.clone());
                self.ddl_wait_again(&ddl);
                // A tunnel stays open only for the password asked for now.
                if ask.is_none() {
                    self.close_tunnel(id);
                }
                if let Some(c) = ask {
                    let pc = self.conns.entry(id);
                    pc.console = console;
                    pc.expand_on_connect = expand;
                    pc.pending = pending;
                    self.open_prompt(&c, reason.clone(), PromptPurpose::Connect);
                }
                self.status = Some(failed);
            }
            DbEvent::Schemas(Ok(names)) => {
                self.conns.entry(id).tree.set_schemas(names);
                let own = self.own_database(id);
                self.use_answered(id, super::cmdline::UseAsk::Schemas(own), true);
            }
            DbEvent::Schemas(Err(_)) => {
                self.conns.entry(id).tree.schemas_loading = false;
                self.status = Some(Notice::new(Msg::QueryError { error: text }, Level::Error));
                let own = self.own_database(id);
                self.use_answered(id, super::cmdline::UseAsk::Schemas(own), false);
            }
            DbEvent::Objects { schema, result } => {
                self.conns.entry(id).tree.set_objects(&schema, result.map_err(|_| text));
                self.revealed(id, None, &schema);
            }
            DbEvent::Structure { schema, table, result } => {
                self.conns.entry(id).tree.set_structure(&schema, &table, result.map_err(|_| text));
            }
            DbEvent::Catalog(Ok(cat)) => {
                let c = self.conns.entry(id);
                c.catalog = cat;
                c.catalog_error = None;
            }
            DbEvent::Catalog(Err(_)) => {
                // Completion and the explorer's columns keep what was read before, if anything;
                // the columns say it could not be read.
                self.conns.entry(id).catalog_error = Some(text.clone());
                self.status = Some(Notice::new(Msg::CatalogFailed { error: text }, Level::Error));
            }
            DbEvent::Keys(Ok(keys)) => self.conns.entry(id).keys = Keys::Loaded(keys),
            // Only a query session sets its path per transaction.
            // Only query sessions keep prepared statements.
            DbEvent::ContextPerTransaction | DbEvent::StatementCacheOff => {}
            DbEvent::Databases(result) => {
                let c = self.conns.entry(id);
                let ok = result.is_ok();
                c.databases = Some(result.map_err(|_| text));
                c.databases_asked = false;
                self.use_answered(id, super::cmdline::UseAsk::Databases, ok);
            }
            DbEvent::Context { database, .. } => self.conns.entry(id).database = Some(database),
            DbEvent::Keys(Err(_)) => {
                self.conns.entry(id).keys = Keys::Failed(text.clone());
                self.status = Some(Notice::new(Msg::KeysFailed { error: text }, Level::Warning));
            }
            DbEvent::Lost { .. } => {
                // Lost with its tunnel: the tunnel's words.
                let lost = self.conns.get(id).and_then(|c| c.tunnel_lost.clone());
                let m = lost.unwrap_or_else(|| Notice::new(Msg::ConnLost { name, error: text }, Level::Error));
                let why = self.i18n.msg(&m.msg).to_string();
                self.ddl_stop(id, Some(None), super::tabs::DdlState::Failed(why), true);
                let c = self.conns.entry(id);
                c.meta = None;
                c.connected = false;
                c.error = Some(m.clone());
                self.status = Some(m);
            }
            // Statement events never come from the metadata session.
            DbEvent::Released { .. }
            | DbEvent::Page { .. }
            | DbEvent::Done { .. }
            | DbEvent::Failed { .. }
            | DbEvent::TxOpen(_)
            | DbEvent::TxAborted(_)
            | DbEvent::Block(_)
            | DbEvent::Counted { .. }
            | DbEvent::Started { .. }
            | DbEvent::StepRows { .. }
            | DbEvent::Finished { .. } => {}
            DbEvent::Ddl { id: request, result } => self.ddl_answered(id, None, request, result),
        }
    }

    /// The outcome of a statement in tab `id`: kept with the tab, shown now if it is active.
    pub(super) fn tab_status(&mut self, id: TabId, m: Notice) {
        if self.tab().id == id {
            self.status = Some(m.clone());
        }
        if let Some(t) = self.tabs.get_mut(id) {
            t.status = Some(m);
        }
    }

    /// Tab `id`'s query session turned its statement cache off: said in the
    /// run's Messages, and in the status bar once per connection of the profile.
    fn statement_cache_off(&mut self, id: TabId, profile: Option<ProfileId>) {
        let name = profile.and_then(|p| self.profile(p)).map(|p| p.name.clone()).unwrap_or_default();
        let m = Notice::new(Msg::DbStatementCacheOff { name }, Level::Warning);
        if let Some(t) = self.tabs.get_mut(id) {
            t.exec.run.notes.push(m.clone());
        }
        if let Some(p) = profile
            && !std::mem::replace(&mut self.conns.entry(p).cache_off_said, true)
        {
            self.status = Some(m);
        }
    }

    /// An event of tab `id`'s query session.
    fn tab_event(&mut self, id: TabId, ev: DbEvent) {
        let profile = self.tabs.get(id).and_then(|t| t.profile);
        if matches!(ev, DbEvent::StatementCacheOff) {
            return self.statement_cache_off(id, profile);
        }
        // The run ends (its result, its outcome or its failure), or a transaction ends.
        let current = self.tabs.get(id).map_or(0, |t| t.exec.query_id);
        let run_ended = matches!(&ev, DbEvent::Page { id: q, columns: Some(_), .. } | DbEvent::Done { id: q, .. }
            | DbEvent::Failed { id: q, .. } if *q == current);
        let tx_ended = matches!(ev, DbEvent::TxOpen(false));
        // Rows that land for a result of this run: the answer's (`None`) or a statement's
        // before it. Read inside the user's transaction, the result says so.
        let rows_for: Option<Option<usize>> = match &ev {
            DbEvent::Page { id: q, .. } if *q == current => Some(None),
            DbEvent::StepRows { id: q, index, .. } if *q == current => Some(Some(*index)),
            _ => None,
        };
        let connecting = profile.is_some_and(|p| self.conns.state(p) == NodeState::Connecting);
        let null = self.i18n.label(Label::ResultsNull);
        let now = self.now();
        let text = self.event_error_text(&ev);
        // A statement run again for the user that the allowlist refused (the server's side of
        // it, asked right before): said as a refusal, with the reason.
        let released = self.tabs.get(id).is_some_and(|t| t.exec.paging == Paging::Released);
        let refused = match &ev {
            DbEvent::Failed { error: DbError::NotRepeatable(r), .. } => {
                let why = self.i18n.msg(&super::pages::why(r)).to_string();
                let m = if released { Msg::ResultsPageRefusedNoHold { why } } else { Msg::ResultsPageRefused { why } };
                Some(Notice::new(m, Level::Warning))
            }
            _ => None,
        };
        // A statement that cannot run in a transaction, refused only because the tab's schema
        // is set in one behind a pooler: said why, and how to run it.
        let needs_no_tx = matches!(&ev, DbEvent::Failed { error: DbError::NeedsNoTransaction(_), .. }).then(|| {
            let t = self.tabs.get(id);
            let own = profile.map(|p| self.own_database(p)).unwrap_or_default();
            t.and_then(|t| t.context.database.clone()).unwrap_or(own)
        });
        // Where the result's rows may spill, and how much.
        let (spill, limits) =
            (self.spill.clone(), self.tabs.get(id).map(|t| self.result_limits(t)).unwrap_or_default());
        // Fetching stops (the spill file is full or failed): the portal is closed after this.
        let mut stop: Option<Notice> = None;
        // A fetch of more rows failed or was cancelled.
        let mut fetch_failed = false;
        // The portal of this result to close after the event (a resumed statement's rows that
        // are not added, or stopped at the spill limit).
        let mut close_portal: Option<u64> = None;
        let page_size = self.page_size;
        let origin = self.tabs.get(id).and_then(Self::origin_now);
        let Some(t) = self.tabs.get_mut(id) else { return };
        // A page with more rows keeps (or opens) the portal and restarts its idle count; any
        // other outcome of this session means no portal is open. A portal read inside the
        // user's own transaction is never closed for being idle.
        let in_block = t.exec.in_block;
        // A first page the driver did not hold (`DbEvent::Released` came before it): nothing is
        // open past it.
        let released = t.exec.released && matches!(ev, DbEvent::Page { columns: Some(_), .. });
        let paging = |more: bool| match (more, released) {
            (false, _) => Paging::None,
            (true, true) => Paging::Released,
            (true, false) => Paging::Open { since: now, in_block },
        };
        let notice = match ev {
            DbEvent::Connected => {
                t.exec.state = SessionState::Ready;
                None
            }
            // Said by the profile's metadata session.
            DbEvent::ReadOnlyPerTransaction => None,
            // Handled first (`statement_cache_off`).
            DbEvent::StatementCacheOff => None,
            // The server dropped the schema's startup option (a pooler): the driver sets the
            // path in each transaction instead. Said in the run's Messages.
            DbEvent::ContextPerTransaction => {
                let schema = t.context.schema.clone().unwrap_or_default();
                let m = Notice::new(Msg::ContextPerTransaction { schema }, Level::Info);
                t.exec.run.notes.push(m.clone());
                t.exec.path_per_transaction = true;
                // The run that opened the session was sent before this was known.
                if t.exec.running.is_some() {
                    t.warn_session_path();
                }
                Some(m)
            }
            // While the profile connects, its metadata session reports the same failure.
            DbEvent::ConnectFailed { .. } if connecting => None,
            DbEvent::ConnectFailed { .. } => {
                if let Some(s) = t.exec.session.take() {
                    s.close();
                }
                t.exec.state = SessionState::Idle;
                t.exec.running = None;
                t.exec.unsure();
                t.exec.paging = Paging::None;
                if !matches!(t.results, Results::Rows(_)) {
                    t.results = Results::Error(text.clone());
                }
                t.exec.replace_pending = false;
                t.exec.view = super::tabs::ResultView::Rows;
                Some(Notice::new(Msg::ConnFailed { error: text }, Level::Error))
            }
            DbEvent::Schemas(_)
            | DbEvent::Objects { .. }
            | DbEvent::Catalog(_)
            | DbEvent::Keys(_)
            | DbEvent::Structure { .. }
            | DbEvent::Ddl { .. }
            | DbEvent::Databases(_) => None,
            // Where the session works, as the server says: a chosen schema it does not list
            // does not exist there or cannot be used (said, never taken as fine).
            // The run's Messages keep it (a status alone is replaced by the run's outcome at
            // once), and the editor's first line marks the schema.
            DbEvent::Context { database, schemas } => {
                let missing = t.context.schema.clone().filter(|s| !schemas.contains(s));
                t.exec.context = Some((database.clone(), schemas));
                let m =
                    missing.map(|schema| Notice::new(Msg::ContextSchemaMissing { schema, database }, Level::Warning));
                if let Some(m) = &m {
                    t.exec.run.notes.push(m.clone());
                }
                m
            }
            DbEvent::TxOpen(open) => {
                t.exec.tx_open = open;
                t.exec.tx_aborted &= open;
                None
            }
            DbEvent::TxAborted(aborted) => {
                t.exec.tx_aborted = aborted;
                None
            }
            // The coming first page of the running query is not held: past it the statement can
            // only run again.
            DbEvent::Released { id: qid } => {
                if qid == t.exec.query_id {
                    t.exec.released = true;
                }
                None
            }
            // The user's own transaction: results read in it are labelled,
            // and say how it ended once it did. `Block` comes before the `TxOpen` of the same
            // change, so an aborted block is still known here.
            DbEvent::Block(true) => {
                t.block_began();
                None
            }
            DbEvent::Block(false) => {
                let rolled_back = t.ending_rolled_back();
                if let Some(i) = t.ending_statement().filter(|_| rolled_back) {
                    t.exec.run.rolled_back(i);
                }
                t.block_ended(rolled_back);
                None
            }
            // A count the user asked for.
            DbEvent::Counted { id: qid, result, snapshot } => {
                return self.on_counted(id, qid, result, snapshot);
            }
            DbEvent::Lost { .. } => {
                t.exec.session = None;
                t.exec.unsure();
                t.exec.state = SessionState::Lost;
                t.exec.lost_tx = t.exec.tx_at_risk();
                t.exec.running = None;
                t.exec.tx_open = false;
                // The server rolls the user's transaction back when its session ends.
                t.block_ended(true);
                t.exec.paging = Paging::None;
                Some(Notice::new(Msg::QueryLost { error: text }, Level::Warning))
            }
            DbEvent::Page { id: qid, .. }
            | DbEvent::Done { id: qid, .. }
            | DbEvent::Failed { id: qid, .. }
            | DbEvent::Started { id: qid, .. }
            | DbEvent::StepRows { id: qid, .. }
            | DbEvent::Finished { id: qid, .. }
                if qid != t.exec.query_id =>
            {
                None
            }
            // A run of several statements: which one runs, and what each before the last did.
            DbEvent::Started { index, .. } => {
                t.exec.run.started(index);
                None
            }
            // The rows of a statement before the last: kept as its own result, up to its limit
            // (past it the statement still runs to its end; the rest of its rows are dropped).
            DbEvent::StepRows { index, columns, rows, more, .. } => {
                if columns.is_some() && t.exec.replace_pending {
                    t.drop_rows();
                }
                let count = match (columns, t.exec.steps.get_mut(&index)) {
                    (Some(cols), _) => {
                        if let Some((plan, json)) = super::plan::plan_of(&cols, &rows, more) {
                            t.exec.plan = Some(super::plan::PlanTab::new(plan, t.exec.query_id, index, &json));
                        }
                        let store = datarig_core::results::RowStore::new(rows, spill, limits);
                        let rs = ResultSet::new(cols, store, more, &null);
                        let n = rs.rows.len();
                        let grid = GridState::paged(page_size, t.grid.page_rows);
                        t.exec.steps.insert(index, super::tabs::Parked { rs, grid });
                        n
                    }
                    (None, Some(p)) => {
                        let rs = &mut p.rs;
                        if rs.more {
                            rs.more = match rs.rows.append(rows) {
                                Ok(a) => more && !a.capped,
                                Err(_) => false,
                            };
                        }
                        rs.rows.len()
                    }
                    (None, None) => 0,
                };
                t.exec.run.step_rows(index, count as u64, more);
                None
            }
            DbEvent::Finished { index, outcome, elapsed, .. } => {
                t.exec.succeeded(Some(index));
                t.exec.run.finished(index, &outcome, elapsed);
                None
            }
            ev @ (DbEvent::Page { .. } | DbEvent::Done { .. } | DbEvent::Failed { .. })
                if self.transient.as_ref().is_some_and(|(m, _)| {
                    matches!(m.msg, Msg::QueryBusy { .. } | Msg::Label(Label::QueryNoStatement))
                }) =>
            {
                // A finished query makes a busy/no-statement flash obsolete; show the result instead.
                self.transient = None;
                return self.tab_event(id, ev);
            }
            // The statement ran again past its closed portal: its next page
            // goes after the rows the result has, when its columns are the same.
            DbEvent::Page { columns: Some(cols), rows, more, elapsed, .. } if t.exec.resuming.is_some() => {
                let r = t.exec.resuming.take();
                t.exec.running = None;
                t.exec.released = false;
                let same = r.as_ref().is_some_and(|r| {
                    r.columns.len() == cols.len()
                        && r.columns.iter().zip(&cols).all(|((n, ty), c)| *n == c.name && *ty == c.type_name)
                });
                // What was announced when it was sent stays in the status bar with its rows.
                let announced = t.exec.run.notes.last().cloned();
                let answer = t.answer_rows_mut();
                match (answer, r) {
                    (Some((rs, grid)), Some(r)) if same => {
                        match rs.rows.append(rows) {
                            Ok(a) if a.capped => {
                                let count = rs.rows.len() as u64;
                                let limit = fmt_bytes(limits.cap.unwrap_or(0));
                                stop = Some(Notice::new(Msg::ResultsSpillCapped { count, limit }, Level::Warning));
                            }
                            Ok(_) => rs.more = more,
                            Err(fault) => {
                                let error = self.i18n.msg(&persist::fault_reason(&fault)).to_string();
                                stop = Some(Notice::new(Msg::ResultsSpillFailed { error }, Level::Error));
                            }
                        }
                        if stop.is_some() {
                            rs.more = false;
                        }
                        if (r.page * page_size) < rs.rows.len() {
                            grid.set_page(r.page);
                        }
                        let (n, more) = (rs.rows.len(), rs.more);
                        t.exec.paging = if stop.is_some() { Paging::Stopped } else { paging(more) };
                        if stop.is_some() {
                            close_portal = Some(t.exec.query_id);
                        }
                        stop.take().or(announced).or(Some(rows_msg(n, more, elapsed)))
                    }
                    _ => {
                        // Not the result it was: nothing is added, and its portal goes (one
                        // not held is gone already).
                        if more {
                            t.exec.paging = Paging::None;
                            if !released {
                                close_portal = Some(t.exec.query_id);
                            }
                        }
                        Some(Notice::new(Label::ResultsPageColumnsChanged, Level::Warning))
                    }
                }
            }
            // It ended without rows: nothing is added; the portal stays closed.
            DbEvent::Done { .. } | DbEvent::Failed { .. } if t.exec.resuming.is_some() => {
                t.exec.resuming = None;
                t.exec.running = None;
                t.exec.released = false;
                // Refused: the next page no longer offers to run it again.
                t.exec.rerun_ok &= refused.is_none();
                let m = if let Some(m) = refused {
                    m
                } else if text.is_empty() {
                    Notice::new(Label::ResultsPageColumnsChanged, Level::Warning)
                } else {
                    Notice::new(Msg::QueryError { error: text.clone() }, Level::Error)
                };
                t.exec.run.notes.push(m.clone());
                Some(m)
            }
            DbEvent::Page { columns: Some(cols), rows, more, elapsed, .. } => {
                t.exec.succeeded(None);
                t.exec.running = None;
                t.exec.released = false;
                let count = rows.len();
                // A plan (`EXPLAIN (FORMAT JSON)`): shown as one; its rows stay a result tab.
                let plan = if t.is_table() { None } else { super::plan::plan_of(&cols, &rows, more) };
                let store = datarig_core::results::RowStore::new(rows, spill, limits);
                // The run's answer: its own result tab, shown.
                if t.exec.replace_pending {
                    t.drop_rows();
                }
                t.exec.paging = paging(more);
                t.park_shown();
                let page_rows = t.grid.page_rows;
                t.results = Results::Rows(ResultSet::new(cols, store, more, &null));
                t.grid = GridState::paged(page_size, page_rows);
                t.exec.origin = origin;
                t.exec.shown = t.exec.run.len().checked_sub(1);
                t.exec.view = super::tabs::ResultView::Rows;
                if let Some((plan, json)) = plan {
                    let (query, index) = (t.exec.query_id, t.exec.shown.unwrap_or(0));
                    t.exec.plan = Some(super::plan::PlanTab::new(plan, query, index, &json));
                    t.exec.view = super::tabs::ResultView::Plan;
                }
                t.exec.run.answered(StatementOutcome::Rows { count: count as u64, more }, Some(elapsed));
                // Not held: the next page runs the statement again when the allowlist lets it;
                // otherwise the first page is all there is, and the user learns why and what to do.
                let refusal = if released && more { risk::repeat::repeatable(t.answer_sql()).err() } else { None };
                if released && more {
                    t.exec.rerun_ok = refusal.is_none();
                }
                if let Some(r) = refusal {
                    let m = Notice::new(
                        super::pages::first_page_only(&self.i18n, t.answer_sql(), &r, count as u64),
                        Level::Warning,
                    );
                    t.exec.run.notes.push(m.clone());
                    t.exec.explain_rolled_back = false;
                    Some(m)
                } else {
                    Some(if std::mem::take(&mut t.exec.explain_rolled_back) {
                        Notice::new(Label::SafetyExplainRolledBack, Level::Info)
                    } else {
                        // The warning would stay behind the grid, in Messages.
                        t.session_path_note().unwrap_or_else(|| rows_msg(count, more, elapsed))
                    })
                }
            }
            DbEvent::Page { columns: None, rows, more, elapsed, .. } => {
                t.exec.running = None;
                t.exec.paging = paging(more);
                let want = t.exec.want_page.take();
                let answer = t.answer_rows_mut();
                let m = match answer {
                    Some((rs, grid)) => {
                        match rs.rows.append(rows) {
                            Ok(a) if a.capped => {
                                let count = rs.rows.len() as u64;
                                let limit = fmt_bytes(limits.cap.unwrap_or(0));
                                stop = Some(Notice::new(Msg::ResultsSpillCapped { count, limit }, Level::Warning));
                            }
                            Ok(_) => rs.more = more,
                            Err(fault) => {
                                let log = ErrorLog::new(self.paths.errors_log());
                                log.record("results.spill_failed", &fault);
                                let error = self.i18n.msg(&persist::fault_reason(&fault)).to_string();
                                stop = Some(Notice::new(Msg::ResultsSpillFailed { error }, Level::Error));
                            }
                        }
                        if stop.is_some() {
                            rs.more = false;
                        }
                        let (n, more) = (rs.rows.len(), rs.more);
                        // The page asked for, once its rows are there.
                        if let Some(p) = want.filter(|p| p * page_size < n) {
                            grid.set_page(p);
                        }
                        if stop.is_some() {
                            t.exec.paging = Paging::Stopped;
                        }
                        t.exec.run.rows_fetched(n as u64, more);
                        Some(rows_msg(n, more, elapsed))
                    }
                    None => None,
                };
                if let Some(m) = m {
                    self.tab_status(id, m);
                }
                if let Some(n) = stop {
                    let qid = self.tabs.get(id).map_or(0, |t| t.exec.query_id);
                    self.send_tab(id, DbCommand::ClosePortal { id: qid });
                    self.tab_status(id, n);
                    // A copy of every row takes the rows fetched (and says so).
                    self.fetch_copy_page(id);
                    return;
                }
                self.fetch_copy_page(id);
                None
            }
            DbEvent::Done { outcome, elapsed, .. } => {
                t.exec.succeeded(None);
                t.exec.running = None;
                t.exec.released = false;
                // Rows an earlier run left on screen keep their paging state (closed).
                if t.exec.kept_log.is_none() {
                    t.exec.paging = Paging::None;
                }
                let done = match &outcome {
                    Outcome::Affected(n) => StatementOutcome::Affected(*n),
                    Outcome::Command(tag) => StatementOutcome::Command(tag.clone()),
                };
                t.exec.run.answered(done, Some(elapsed));
                let m = Notice::new(
                    match outcome {
                        Outcome::Affected(count) => Msg::QueryDoneAffected { count, elapsed },
                        Outcome::Command(tag) => Msg::QueryDoneCommand { tag, elapsed },
                    },
                    Level::Success,
                );
                // Row results stay where they are (this run's, or an earlier run's when this
                // one returned none); the last result tab of this run is shown, else Messages.
                if !matches!(t.results, Results::Rows(_)) {
                    t.results = Results::Message(m.clone());
                }
                t.run_ended(false);
                Some(if std::mem::take(&mut t.exec.explain_rolled_back) {
                    Notice::new(Label::SafetyExplainRolledBack, Level::Info)
                } else {
                    t.session_path_note().unwrap_or(m)
                })
            }
            DbEvent::Failed { cancelled, .. } => {
                // The failed statement and the ones after it (never run) are unknown.
                t.exec.unsure();
                t.exec.released = false;
                let was_fetch = t.exec.running.is_some_and(|r| r.fetch);
                fetch_failed = was_fetch;
                t.exec.running = None;
                if was_fetch {
                    // The portal is gone, the rows fetched stay, and whether the server has
                    // more is unknown (not absent): the next page goes the way of a closed
                    // portal (run again when the allowlist lets it, else refused), and the
                    // result never claims to be complete.
                    t.exec.paging = Paging::Interrupted;
                    t.exec.rerun_ok = risk::repeat::repeatable(t.answer_sql()).is_ok();
                } else if t.exec.kept_log.is_none() {
                    // Rows an earlier run left on screen keep their paging state (closed).
                    t.exec.paging = Paging::None;
                }
                // A run of several statements cancelled between two of them: no statement was
                // running, the next one never started.
                let between =
                    (!was_fetch && cancelled && t.exec.run.several()).then(|| t.exec.run.cancelled_between()).flatten();
                // Which statement of the run it was (a failed fetch is the last one's rows).
                let step = if was_fetch || between.is_some() {
                    None
                } else {
                    let outcome =
                        if cancelled { StatementOutcome::Cancelled } else { StatementOutcome::Failed(text.clone()) };
                    t.exec.run.answered(outcome, None).filter(|_| t.exec.run.several())
                };
                let total = t.exec.run.len();
                let m = match (between, step) {
                    (Some(i), _) if i + 1 == total => {
                        let sql = super::runlog::excerpt(&t.exec.run.statements[i].sql, 40);
                        let (step, total) = ((i + 1).to_string(), total.to_string());
                        Notice::new(Msg::QueryCancelledBeforeLast { step, total, sql }, Level::Warning)
                    }
                    (Some(i), _) => {
                        let (from, to, total) = ((i + 1).to_string(), total.to_string(), total.to_string());
                        Notice::new(Msg::QueryCancelledBefore { from, to, total }, Level::Warning)
                    }
                    (None, Some(i)) => {
                        let (step, total) = ((i + 1).to_string(), t.exec.run.len().to_string());
                        let sql = super::runlog::excerpt(&t.exec.run.statements[i].sql, 40);
                        if cancelled {
                            Notice::new(Msg::QueryStepCancelled { step, total, sql }, Level::Warning)
                        } else {
                            Notice::new(Msg::QueryStepFailed { step, total, sql, error: text.clone() }, Level::Error)
                        }
                    }
                    (None, None) if cancelled => Notice::new(Label::QueryCancelled, Level::Warning),
                    (None, None) => match needs_no_tx.clone() {
                        Some(database) => {
                            let error = text.clone();
                            Notice::new(Msg::ContextNoTransactionError { error, database }, Level::Error)
                        }
                        None => Notice::new(Msg::QueryError { error: text.clone() }, Level::Error),
                    },
                };
                if let Some(database) = needs_no_tx {
                    t.exec.run.notes.push(Notice::new(Msg::ContextNoTransaction { database }, Level::Warning));
                }
                // Row results stay (a failed fetch keeps `more` as it was); Messages says what
                // failed.
                if !matches!(t.results, Results::Rows(_)) {
                    t.results = if cancelled { Results::Cancelled } else { Results::Error(text) };
                }
                if !was_fetch {
                    t.run_ended(true);
                }
                Some(m)
            }
        };
        if let Some(m) = notice {
            self.tab_status(id, m);
        }
        if let Some(qid) = close_portal {
            self.send_tab(id, DbCommand::ClosePortal { id: qid });
        }
        if let (Some(target), Some(t)) = (rows_for, self.tabs.get_mut(id)) {
            let (in_block, epoch) = (t.exec.in_block, t.exec.tx_epoch);
            let rs = match target {
                None => t.answer_rows_mut().map(|(rs, _)| rs),
                Some(i) => t.rows_of_mut(i).map(|(rs, _)| rs),
            };
            if let Some(rs) = rs {
                rs.mark_read(in_block, epoch);
            }
        }
        if fetch_failed {
            self.fetch_copy_failed(id);
        }
        self.keys_after_ddl(id, profile, run_ended, tx_ended);
    }

    /// After DDL run in tab `id` the key cache of its profile may be out of date: read it again
    /// once the run ends, or, inside a transaction (the metadata session cannot see its
    /// changes yet), once the transaction ends.
    fn keys_after_ddl(&mut self, id: TabId, profile: Option<ProfileId>, run_ended: bool, tx_ended: bool) {
        let Some(t) = self.tabs.get_mut(id) else { return };
        let mut reload = false;
        if run_ended && std::mem::take(&mut t.exec.ddl) {
            if t.exec.tx_open {
                t.exec.ddl_in_tx = true;
            } else {
                reload = true;
            }
        }
        if tx_ended && std::mem::take(&mut t.exec.ddl_in_tx) {
            reload = true;
        }
        if reload && let Some(p) = profile {
            self.reload_keys(p);
        }
    }
}

/// A size as the settings write it: `1 GB`, `512 MB`, `64 KB`, `1000 B`.
pub(crate) fn fmt_bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 30 && n.is_multiple_of(1 << 30) => format!("{} GB", n >> 30),
        n if n >= 1 << 30 => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 && n.is_multiple_of(1 << 20) => format!("{} MB", n >> 20),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 && n.is_multiple_of(1 << 10) => format!("{} KB", n >> 10),
        n => format!("{n} B"),
    }
}

fn rows_msg(count: usize, more: bool, elapsed: Duration) -> Notice {
    let count = count as u64;
    let m = if more { Msg::QueryMoreRows { count, elapsed } } else { Msg::QueryDoneRows { count, elapsed } };
    Notice::new(m, Level::Success)
}

impl App {
    /// The text of the failure an event carries (empty for other events): see
    /// [`App::db_error_text`].
    fn event_error_text(&self, ev: &DbEvent) -> String {
        match ev {
            DbEvent::ConnectFailed { error, .. } | DbEvent::Failed { error, .. } | DbEvent::Lost { error } => {
                self.db_error_text(error)
            }
            // The version the key metadata needs is the structure's too, in its own words.
            DbEvent::Structure { result: Err(DbError::ServerTooOld), .. } => {
                self.i18n.label(Label::TreeStructureTooOld).to_string()
            }
            DbEvent::Structure { result: Err(DbError::Locked), .. } => {
                self.i18n.label(Label::TreeStructureLocked).to_string()
            }
            DbEvent::Schemas(Err(error))
            | DbEvent::Objects { result: Err(error), .. }
            | DbEvent::Structure { result: Err(error), .. }
            | DbEvent::Catalog(Err(error))
            | DbEvent::Keys(Err(error)) => self.db_error_text(error),
            _ => String::new(),
        }
    }

    /// A database failure as the UI says it: the server's own message as it is (it is data),
    /// anything else in words, with its raw detail in the error log.
    pub(super) fn db_error_text(&self, e: &DbError) -> String {
        let label = match e {
            DbError::Server(s) | DbError::NeedsNoTransaction(s) => return s.clone(),
            DbError::NoAnswer(d) => return self.i18n.msg(&Msg::DbNoAnswer { elapsed: *d }).to_string(),
            DbError::RowsNotRead(n) => return self.i18n.msg(&Msg::DbRowsNotRead { count: *n }).to_string(),
            DbError::Closed => Label::DbClosed,
            DbError::NotSupported => Label::DbNotSupported,
            DbError::ServerTooOld => Label::DbServerTooOld,
            DbError::SchemaChanged => Label::DbSchemaChanged,
            DbError::NoResult => Label::DbNoResult,
            DbError::Cancelled => Label::QueryCancelled,
            DbError::ReadWriteRefused => Label::DbReadWriteRefused,
            DbError::Locked => Label::DbLocked,
            DbError::NotFound => Label::DbNotFound,
            DbError::NotRepeatable(r) => return self.i18n.msg(&super::pages::why(r)).to_string(),
            DbError::Settings(f) => {
                ErrorLog::new(self.paths.errors_log()).record("db.settings", f);
                Label::DbSettings
            }
            DbError::Transport(e) => return self.dial_error_text(e),
            DbError::Connection(f) => {
                ErrorLog::new(self.paths.errors_log()).record("db.connection", f);
                network_reason(&f.kind)
            }
        };
        self.i18n.label(label).to_string()
    }

    /// A failure of the transport a session reaches its server through (a tunnel),
    /// in words; the far end's message or the fault's detail goes to the error log.
    pub(super) fn dial_error_text(&self, e: &DialError) -> String {
        let log = |f: &datarig_core::fault::Fault| ErrorLog::new(self.paths.errors_log()).record("db.transport", f);
        let msg = match e {
            DialError::NotOpen => return self.i18n.label(Label::DialNotOpen).to_string(),
            DialError::Timeout(d) => Msg::DialTimeout { elapsed: *d },
            DialError::Refused { host, port, reason, detail } => {
                let target = format!("{host}:{port}");
                // The far end's message may be empty: the target and the reason say what it was.
                let text = format!(
                    "{target} refused ({reason:?}){}",
                    if detail.is_empty() { String::new() } else { format!(": {detail}") }
                );
                log(&datarig_core::fault::Fault::other(text));
                match reason {
                    Refusal::Prohibited => Msg::DialProhibited { target },
                    Refusal::Unreachable => Msg::DialUnreachable { target },
                    Refusal::Other => Msg::DialRefused { target },
                }
            }
            // Reaching the far end failed: worded (and logged) as a connection failure is.
            DialError::Failed(f) => return self.db_error_text(&DbError::Connection(f.clone())),
        };
        self.i18n.msg(&msg).to_string()
    }
}

/// A network failure (connecting to a server or a bastion) in words: refused, cut, timed out,
/// unreachable, a name that does not resolve; anything else is a connection failure whose
/// detail is in `errors.log`. Never a file system wording.
pub(super) fn network_reason(kind: &FaultKind) -> Label {
    use std::io::ErrorKind as K;
    match kind {
        FaultKind::HostNotFound => Label::DbHostNotFound,
        FaultKind::Io(K::ConnectionRefused) => Label::DbRefused,
        FaultKind::Io(
            K::ConnectionReset | K::ConnectionAborted | K::BrokenPipe | K::UnexpectedEof | K::NotConnected,
        ) => Label::DbCut,
        FaultKind::Io(K::TimedOut) => Label::DbTimedOut,
        FaultKind::Io(K::HostUnreachable | K::NetworkUnreachable | K::NetworkDown | K::AddrNotAvailable) => {
            Label::DbUnreachable
        }
        _ => Label::DbConnection,
    }
}
