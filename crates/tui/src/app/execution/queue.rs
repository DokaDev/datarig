//! Cancelling runs and the queue of statements waiting for a tab's session.

use crate::app::runlog::StatementOutcome;
use crate::app::*;

impl App {
    /// Cancel the statement running in the active tab (only there).
    pub fn cancel(&mut self) {
        let id = self.tab().id;
        self.cancel_in(id);
    }

    /// Cancel what runs in tab `id`: its running statement, or the statement that waits for
    /// its connection (dropped, never run).
    pub(in crate::app) fn cancel_in(&mut self, id: TabId) {
        // A wait for the server's answer to view a text plan as a plan ends (said once).
        if self.waiting_as_plan(id) {
            self.end_as_plan_wait();
        }
        // Its profile's password is being read from the keychain (it may not answer): the
        // attempt ends, and what waited for it with it.
        if let Some(p) = self.tabs.get(id).and_then(|t| t.profile)
            && self.conns.get(p).and_then(|c| c.connecting.as_ref()).is_some_and(|c| c.keychain)
        {
            return self.cancel_connect(p);
        }
        // A table tab's query waiting for its table's keys never runs (a late answer finds it
        // gone).
        if self.table_keys_cancel(id) {
            return self.tab_status(id, Notice::new(Label::QueryCancelled, Level::Warning));
        }
        if self.take_queued(id).is_some() {
            self.unstage_run(id);
            return self.tab_status(id, Notice::new(Label::QueryCancelled, Level::Warning));
        }
        // A copy that fetches every row first stops now: a page that lands after this asks
        // for nothing more, and nothing is copied.
        let copy = self.stop_fetch_copy(id);
        // So does an export: the one whose statement runs, or one writing fetched rows while
        // nothing else runs (a statement the user runs meanwhile is not its).
        if self.tabs.get(id).is_some_and(|t| t.exec.running.is_none_or(|r| r.export)) {
            self.stop_export(id);
        }
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
    pub(in crate::app) fn cancel_unanswered(&mut self, id: TabId) {
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
        t.tx_reset();
        t.exec.paging = Paging::None;
        t.exec.resuming = None;
        t.exec.want_page = None;
        // A count or a fetch of more rows leaves the rows as they are: whether the server has
        // more is still unknown (never "complete"); row results stay (an earlier run's, or this
        // run's shown one).
        if !matches!(t.results, Results::Rows(_)) {
            t.results = Results::Cancelled;
        }
        if !r.fetch && !r.count && !r.export && !r.apply {
            // The statement it was at is cancelled (its hint says so).
            t.exec.run.answered(StatementOutcome::Cancelled, None);
            t.run_ended(true);
        }
        self.settle_run_hints(id);
        self.tab_status(id, Notice::new(Label::QueryCancelUnanswered, Level::Warning));
    }

    /// Cancel whatever runs or waits for its connection in any tab.
    pub(in crate::app) fn cancel_all(&mut self) {
        let ids: Vec<TabId> = self.tabs.iter().map(|t| t.id).filter(|id| self.tab_busy(*id)).collect();
        for id in ids {
            self.cancel_in(id);
        }
    }

    /// Tab `id` has a statement waiting for its profile to connect, or a table tab's query
    /// waiting for its table's keys.
    pub fn is_queued(&self, id: TabId) -> bool {
        self.conns.iter().any(|(_, c)| c.pending.iter().any(|q| q.tab == id)) || self.table_keys_waiting(id)
    }

    /// Tab `id` runs a statement, has one waiting for its connection, or waits for the answer
    /// to a commit or rollback (each counts as work in progress wherever the app asks before
    /// ending it).
    pub fn tab_busy(&self, id: TabId) -> bool {
        self.tabs.get(id).is_some_and(|t| t.exec.running.is_some() || t.exec.ending.is_some()) || self.is_queued(id)
    }

    /// Remove the statement waiting in tab `id`, if any.
    pub(in crate::app) fn take_queued(&mut self, id: TabId) -> Option<Queued> {
        self.conns.iter_mut().find_map(|(_, c)| {
            let i = c.pending.iter().position(|q| q.tab == id)?;
            Some(c.pending.remove(i))
        })
    }

    /// Drop the statement waiting in tab `id`; the tab says why (`why` gets the name of the
    /// profile it waited for).
    pub(in crate::app) fn drop_queued(&mut self, id: TabId, why: impl FnOnce(String) -> Msg) {
        if let Some(q) = self.take_queued(id) {
            self.unstage_run(id);
            let name = self.profile(q.profile).map(|p| p.name.clone()).unwrap_or_default();
            self.tab_status(id, Notice::new(why(name), Level::Warning));
        }
    }

    /// Drop every statement waiting for profile `id` (its attempt ended); each tab says so.
    pub(in crate::app) fn drop_pending(&mut self, id: ProfileId) {
        // A DDL tab that waited reads on `r`.
        self.ddl_stop(id, None, crate::app::tabs::DdlState::NotLoaded, true);
        let pending = self.conns.get_mut(id).map(|c| std::mem::take(&mut c.pending)).unwrap_or_default();
        let name = self.profile(id).map(|p| p.name.clone()).unwrap_or_default();
        // So do the table tabs' queries waiting for their keys, in any database of it.
        self.table_keys_dropped(id, &name);
        for q in pending {
            self.unstage_run(q.tab);
            self.tab_status(q.tab, Notice::new(Msg::QueryQueuedFailed { name: name.clone() }, Level::Warning));
        }
    }
}
