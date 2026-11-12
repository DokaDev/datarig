//! Running statements: sending to a session, the current statement, approval and the query session.

use crate::app::*;
use datarig_core::driver::TxRule;
use datarig_core::sql::lexer::changes_schema_in;

impl App {
    /// Send a tree or catalog request to profile `id`'s metadata session.
    pub(in crate::app) fn send_meta(&mut self, id: ProfileId, cmd: DbCommand) {
        if let Some(s) = self.conns.get(id).and_then(|c| c.meta.as_ref()) {
            s.send(cmd);
            // A read asked for is a use, also before its answer comes.
            self.touch_profile(id);
        }
    }

    /// Send a statement command to tab `id`'s query session.
    pub(in crate::app) fn send_tab(&self, id: TabId, cmd: DbCommand) {
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
    pub(in crate::app) fn run_in(&mut self, id: TabId, statements: Vec<String>) {
        if self.tab_busy(id) {
            self.flash_busy();
            return;
        }
        let Some(t) = self.tabs.get(id) else { return };
        let Some(pid) = t.profile else {
            return self.open_quick(QuickPurpose::Bind { tab: id, run: Some(statements) });
        };
        // A run replaces the tab's results, and their pending changes with them: asked first.
        if self.ask_leave(crate::app::edit::Leave::Run(id)) {
            self.pending_leave = Some((id, statements));
            return;
        }
        // Checked before anything is queued or sent.
        if let Some(refused) = self.unsupported(id, &statements) {
            self.unstage_run(id);
            return self.tab_status(id, refused);
        }
        if let Some(refused) = self.read_only_refusal(id, pid, &statements) {
            self.unstage_run(id);
            return self.tab_status(id, refused);
        }
        // What the run does to the open transaction (manual commit, a statement that cannot
        // run in one, MySQL's implicit commit): refused, or asked about with the rest.
        let check = self.tx_check(id, &statements);
        if let Some(refused) = check.refused {
            self.unstage_run(id);
            return self.tab_status(id, refused);
        }
        let items =
            crate::app::safety::with_tx_reasons(self.dangerous(id, pid, &statements), &statements, check.reasons);
        if !items.is_empty() {
            return self.ask_before_run(id, pid, statements, items);
        }
        self.run_approved(id, statements);
    }

    /// Run `statements` in tab `id` once the checks passed or the user confirmed them: on its
    /// session, or queued until its profile connects. The read-only check is made again right
    /// before, so nothing it refuses ever reaches a session.
    pub(in crate::app) fn run_approved(&mut self, id: TabId, statements: Vec<String>) {
        self.run_approved_as(id, statements, false)
    }

    /// [`App::run_approved`]; `as_is`: with manual commit, every statement runs as it is (no
    /// transaction is opened for it: a procedure the user chose to run outside one).
    pub(in crate::app) fn run_approved_as(&mut self, id: TabId, statements: Vec<String>, as_is: bool) {
        if self.tab_busy(id) {
            self.flash_busy();
            return;
        }
        let Some(pid) = self.tabs.get(id).and_then(|t| t.profile) else { return };
        // A table tab's filter checked again as the policy is now (it may have waited for the
        // connection while the environment changed).
        if !self.table_pending_holds(id, &statements) {
            return self.unstage_run(id);
        }
        if let Some(refused) = self.read_only_refusal(id, pid, &statements) {
            self.unstage_run(id);
            return self.tab_status(id, refused);
        }
        let Some(t) = self.tabs.get(id) else { return };
        // Outside a transaction: only on the session it was asked for, never queued for another.
        if as_is && t.exec.session.is_none() {
            self.unstage_run(id);
            return self.tab_status(id, Notice::new(Label::SafetyConfirmMoved, Level::Warning));
        }
        // A profile whose sessions closed for being idle connects again first (its tunnel too).
        if t.exec.session.is_none()
            && self.conns.get(pid).is_none_or(|c| c.resolved.is_none() || c.idle_closed.is_some())
        {
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
        // Checked again on the session it goes to (a run that waited for its connection).
        let check = self.tx_check(id, &statements);
        if let Some(refused) = check.refused {
            self.unstage_run(id);
            return self.tab_status(id, refused);
        }
        // A table tab's read, checked again right before it is sent on the session it goes to
        // (its state as it is now, a session still opening as not known yet): one that locks
        // rows there without the tab's yes for that state asks first, and nothing is sent.
        if self.table_locks_hold(id, &statements) {
            return self.unstage_run(id);
        }
        let tx = self.tx_rule(id, &statements, as_is);
        let manual = self.manual_commit(id);
        let (now, wall) = (self.now(), self.wall_now());
        self.touch_tab(id);
        self.query_seq += 1;
        let qid = self.query_seq;
        // The cap of a table tab's query this run sends: the driver reads no row past it.
        let mut max_rows = None;
        if let Some(t) = self.tabs.get_mut(id) {
            // The driver ends an open portal before it runs the next statement: the rows stay
            // (until this run delivers rows of its own), past them paging is closed.
            if matches!(t.exec.paging, Paging::Open { .. }) {
                t.exec.paging = Paging::Replaced;
                t.exec.rerun_ok = t.exec.prepared.repeatable(t.answer_sql()).is_ok();
            }
            t.exec.want_page = None;
            t.exec.resuming = None;
            t.exec.released = false;
            t.exec.query_id = qid;
            // A table tab's query is bound to the run that sends its statement.
            if let Some(tq) = t.doc.table_query.as_mut() {
                tq.sent(qid, &statements);
                max_rows = tq.requested.as_ref().filter(|(rid, _)| *rid == qid).and_then(|(_, q)| q.limit);
            }
            // Marked on the editor's text when that is where the user ran them from.
            t.editor.start_run(qid, &statements);
            // The row results stay until this run delivers its first rows:
            // a run without rows (a COMMIT) leaves them on screen, from an earlier run.
            let has_rows = matches!(t.results, Results::Rows(_)) || !t.exec.steps.is_empty();
            let log = crate::app::runlog::RunLog {
                binding: t.binding,
                generation: t.exec.generation,
                ..crate::app::runlog::RunLog::new(&statements)
            };
            let before = std::mem::replace(&mut t.exec.run, log);
            if has_rows && t.exec.kept_log.is_none() {
                t.exec.kept_log = Some(before);
            }
            t.exec.replace_pending = true;
            t.exec.messages_scroll = 0;
            let lang = t.exec.prepared.language();
            // A statement that commits the user's open transaction before it runs (MySQL's
            // implicit commit): said in the run's Messages (with manual commit it asked).
            if t.exec.user_tx()
                && !manual
                && let Some(i) = statements.iter().position(|s| Classifier::classify_once(lang, s).implicit_commit)
            {
                let m = Notice::new(Msg::QueryImplicitCommit { step: (i + 1).to_string() }, Level::Warning);
                t.exec.run.notes.push(m);
            }
            t.exec.run.notes.extend(check.notes);
            // The statements of the run run in the open transaction from the first on.
            t.exec.tx_from = t.exec.user_tx().then_some(0);
            t.exec.sent_manual = tx != TxRule::Auto;
            // A statement runs: the open transaction is not idle.
            if let Some(open) = t.exec.tx.as_mut() {
                open.touch(now, wall);
            }
            t.exec.run_id = qid;
            t.exec.outside_offer = None;
            t.exec.ddl = statements.iter().any(|s| changes_schema_in(s, lang.dialect()));
            t.exec.explain_rolled_back =
                statements.iter().any(|s| Classifier::classify_once(lang, s).rollback_matters());
            // What the run prepares and deallocates counts once the server says each statement
            // succeeded (`TabSession::succeeded`), never when it is sent.
            t.exec.unconfirmed = statements.iter().cloned().enumerate().collect();
            t.exec.running = Some(Running {
                id: qid,
                started: now,
                fetch: false,
                count: false,
                estimate: false,
                cancelling: None,
                stopping: false,
                refused: false,
                export: false,
                apply: false,
            });
            if t.exec.path_per_transaction {
                t.warn_session_path();
            }
            // The results pane shows from the first run on, also after the user hid it.
            t.ran = true;
            t.pane.hidden = false;
        }
        let paging = self.tabs.get(id).map(|t| self.paging_mode(t)).unwrap_or_default();
        // Asked first, so its answer comes before the statement runs (a session answers in
        // order, and one that runs answers nothing else until it ends).
        self.monitor_ask_own_at_run(id);
        self.send_tab(id, DbCommand::Execute { id: qid, statements, paging, tx, max_rows });
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
        // What the session starts with is checked again (a preset may have changed since the
        // profile connected): nothing opens without it.
        if let Err(e) = self.session_plan_of(&cfg) {
            let m = self.session_error_notice(&cfg, &e);
            self.tab_status(id, m);
            return false;
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
}
