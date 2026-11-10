//! Routing driver events to the tab, the auxiliary session or the meta session they belong to.

use crate::app::*;

impl App {
    /// A driver event of the active tab's profile, as the headless tests feed them:
    /// connection, tree and catalog events belong to the profile's metadata session, statement
    /// events to the active tab's query session.
    pub fn on_db_event(&mut self, ev: DbEvent) {
        self.on_db_event_here(ev);
        self.tips_follow();
        // Rows that came in (a page while every row is fetched) grow a whole-column selection.
        self.sync_grid_summary();
        self.sweep_cell_edit();
        self.sweep_as_plan();
        self.sweep_exports();
        self.keychain_offer_check();
    }

    pub(super) fn on_db_event_here(&mut self, ev: DbEvent) {
        let target = match &ev {
            DbEvent::Page { .. }
            | DbEvent::Language(_)
            | DbEvent::Explain(_)
            | DbEvent::Edit(_)
            | DbEvent::SessionValues(_)
            | DbEvent::ValuesNotKnown { .. }
            | DbEvent::Info { .. }
            | DbEvent::Released { .. }
            | DbEvent::Stopping { .. }
            | DbEvent::CancelRefused { .. }
            | DbEvent::Done { .. }
            | DbEvent::Failed { .. }
            | DbEvent::TxOpen(_)
            | DbEvent::TxAborted(_)
            | DbEvent::Block(_)
            | DbEvent::Counted { .. }
            | DbEvent::RepeatChecked { .. }
            | DbEvent::Started { .. }
            | DbEvent::StepRows { .. }
            | DbEvent::Finished { .. }
            | DbEvent::Exported { .. }
            | DbEvent::Applied { .. } => EventTarget::Tab(self.tab().id),
            // The attempt in progress, else the active tab's profile.
            _ => match self.latest_attempt().or(self.tab().profile) {
                Some(id) => EventTarget::Meta(id),
                None => return,
            },
        };
        self.on_target_event(target, ev);
    }

    /// A driver event of the session `target` names (its generation already checked).
    pub(in crate::app) fn on_target_event(&mut self, target: EventTarget, ev: DbEvent) {
        // An answer is a use of the session (a lost one is not).
        if !matches!(ev, DbEvent::Lost { .. }) {
            match target {
                EventTarget::Meta(id) => self.touch_profile(id),
                EventTarget::Tab(id) => self.touch_tab(id),
                EventTarget::Aux(id) => {
                    if let Some(p) = self.conns.aux_by_id_ref(id).map(|a| a.profile) {
                        self.touch_profile(p);
                    }
                }
                // Its own: only while it is shown.
                EventTarget::SessionTest(_) | EventTarget::Monitor(_) => {}
            }
        }
        match target {
            EventTarget::Meta(id) => self.meta_event(id, ev),
            EventTarget::Tab(id) => {
                // What decides whether the tab's reads lock rows, before and after the event.
                let safety = |t: &crate::app::Tab| {
                    let e = &t.exec;
                    (e.generation, e.isolation, e.autocommit_off, e.tx_open, e.in_block, e.tx_epoch)
                };
                let before = self.tabs.get(id).map(safety);
                let covering = self.table_locks_covering(id);
                self.tab_event(id, ev);
                if self.tabs.get(id).map(safety) != before {
                    self.table_safety_changed(id, covering);
                }
                self.settle_run_hints(id);
            }
            EventTarget::Aux(id) => self.aux_event(id, ev),
            EventTarget::SessionTest(seq) => self.session_test_event(seq, ev),
            EventTarget::Monitor(id) => {
                self.monitor_event(id, ev);
                self.monitor_sync();
            }
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
    pub(in crate::app) fn is_current(&self, target: EventTarget, generation: u64) -> bool {
        match target {
            EventTarget::Meta(pid) => self.conns.is_current(pid, generation),
            EventTarget::Tab(id) => self.tabs.get(id).is_some_and(|t| t.exec.generation == generation),
            // An aux session's id is its generation; a closed one is gone.
            EventTarget::Aux(id) => id == generation && self.conns.aux_by_id_ref(id).is_some(),
            EventTarget::SessionTest(seq) => {
                seq == generation && self.session_test.as_ref().is_some_and(|t| t.seq == seq)
            }
            EventTarget::Monitor(id) => self.monitor_of(id).is_some_and(|m| m.generation == generation),
        }
    }

    /// The outcome of a statement in tab `id`: kept with the tab, shown now if it is active.
    pub(in crate::app) fn tab_status(&mut self, id: TabId, m: Notice) {
        if self.tab().id == id {
            self.status = Some(m.clone());
        }
        if let Some(t) = self.tabs.get_mut(id) {
            t.status = Some(m);
        }
    }
}
