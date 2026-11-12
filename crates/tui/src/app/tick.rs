//! Timers: whether the app needs a tick, when the next one is due, and what it does.

use super::*;

impl App {
    pub fn needs_tick(&self) -> bool {
        self.any_running()
            || self.transient.is_some()
            || self.tab().completion_due.is_some()
            || self.conns.attempt().is_some()
            || self.hangul_hint.is_some()
            || self.leader_at.is_some()
            || self.quitting.is_some()
            || self.copy_flash.is_some()
            || self.pointer.tab_slide.due(self.now()).is_some()
            || self.tabs.iter().any(|t| {
                matches!(t.exec.paging, Paging::Open { in_block: false, .. }) && self.paging_timeout(t).is_some()
            })
            || self.conn_test.as_ref().is_some_and(|t| t.state == TestState::Running)
            || self.save_pending()
            || self.conns.auxes().any(|a| self.aux_idle(a))
            // The open transaction's time in the status bar (the active tab's only).
            || self.tx_tick_due(self.now()).is_some()
            || self.tx_idle_due().is_some()
            || self.idle_due(self.now()).is_some()
            || self.changes_count_due().is_some()
            || self.monitor_due().is_some()
            || self.filter_history_due().is_some()
    }

    /// Aux session `a` is open and no tab works in its database: it closes
    /// [`conn::AUX_IDLE`] after its last use.
    pub fn aux_idle(&self, a: &conn::AuxMeta) -> bool {
        a.session.is_some()
            && !self.tabs.iter().any(|t| t.profile == Some(a.profile) && self.other_database(t) == Some(&a.database))
    }

    /// When the event loop should next run [`App::on_tick`] (no timer while
    /// nothing waits for time). While something counts in tenths of a second or animates (a
    /// running statement's time, a connection attempt, a debounce, a key sequence, a flash, the
    /// quit grace): [`FINE_TICK`] from `now`, or the next frame of a copy's flash or of sliding
    /// tabs when that is sooner. Otherwise the earliest deadline: a tab's or the workspace's
    /// autosave, an open portal's idle close, the next frame of a copy's flash (its end when it
    /// cannot fade) or of sliding tabs, the next second the active tab's countdown shows, and
    /// the next second of the active tab's open transaction's time. `None`: nothing waits.
    pub fn next_tick(&self, now: Instant) -> Option<Instant> {
        if !self.needs_tick() {
            return None;
        }
        let fine = self.any_running()
            || self.transient.is_some()
            || self.tab().completion_due.is_some()
            || self.conns.attempt().is_some()
            || self.hangul_hint.is_some()
            || self.leader_at.is_some()
            || self.quitting.is_some()
            || self.conn_test.as_ref().is_some_and(|t| t.state == TestState::Running);
        // A copy's flash wants its next frame (or, when it cannot fade, its end), tabs sliding
        // after a drag theirs and the completion popup its deadline, also between fine ticks.
        let flash = [self.copy_flash_due(now), self.pointer.tab_slide.due(now), self.tab().completion_due]
            .into_iter()
            .flatten()
            .min();
        if fine {
            let tick = flash.map_or(now + FINE_TICK, |f| f.min(now + FINE_TICK));
            return Some(tick.max(now + Duration::from_millis(1)));
        }
        let mut next: Option<Instant> = self.workspace_due;
        let mut at = |t: Instant| next = Some(next.map_or(t, |n| n.min(t)));
        if let Some(due) = flash {
            at(due);
        }
        for t in self.tabs.iter() {
            if let Some(due) = t.doc.save_due {
                at(due);
            }
            if let (Paging::Open { since, in_block: false }, Some(timeout)) = (t.exec.paging, self.paging_timeout(t)) {
                let due = since + timeout;
                at(due);
                if t.id == self.tab().id {
                    // The countdown shows whole seconds rounded up: it changes when the time left
                    // crosses a whole second.
                    let left = due.saturating_duration_since(now);
                    let frac = Duration::from_nanos(u64::from(left.subsec_nanos()));
                    let step = if frac.is_zero() { Duration::from_secs(1) } else { frac };
                    at(now + step);
                }
            }
        }
        for a in self.conns.auxes().filter(|a| self.aux_idle(a)) {
            at(a.used + conn::AUX_IDLE);
        }
        if let Some(due) = self.tx_tick_due(now) {
            at(due);
        }
        if let Some(due) = self.tx_idle_due() {
            at(due);
        }
        if let Some(due) = self.idle_due(now) {
            at(due);
        }
        if let Some(due) = self.changes_count_due() {
            at(due);
        }
        // The visible Monitor tab's next read (a hidden one wakes nothing).
        if let Some(due) = self.monitor_due() {
            at(due);
        }
        // The filter history's write after its debounce.
        if let Some(due) = self.filter_history_due() {
            at(due);
        }
        // Never sooner than a fine tick, never in the past.
        next.map(|n| n.max(now + Duration::from_millis(1)))
    }

    /// Timer work (debounced auto-completion, connect timeout, which-key popup, idle paging
    /// portals). Called on every tick with the current time.
    pub fn on_tick(&mut self, now: Instant) {
        self.which_key_tick(now);
        // At the tick's own time: the clock is not read again here.
        self.monitor_sync_at(now);
        self.changes_count_tick(now);
        self.tx_idle_tick(now);
        self.idle_tick(now);
        // A timer may move what the status bar asks about (an idle transaction rolled back).
        self.sweep_status_ask();
        self.autosave_tick(now);
        self.filter_history_tick(now);
        // Aux sessions no tab works in, unused for a while, close; what they read stays.
        let idle: Vec<u64> = self
            .conns
            .auxes()
            .filter(|a| self.aux_idle(a) && now.saturating_duration_since(a.used) >= conn::AUX_IDLE)
            .map(|a| a.id)
            .collect();
        for id in idle {
            if let Some(s) = self.conns.aux_by_id(id).and_then(|a| a.session.take()) {
                s.close();
            }
        }
        let idle: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|t| t.exec.running.is_none() && t.exec.paging.due(now, self.paging_timeout(t)))
            .map(|t| t.id)
            .collect();
        for id in idle {
            self.close_idle_portal(id);
        }
        let unanswered: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|t| {
                t.exec
                    .running
                    .and_then(|r| r.cancelling)
                    .is_some_and(|at| now.saturating_duration_since(at) >= CANCEL_GRACE)
            })
            .map(|t| t.id)
            .collect();
        for id in unanswered {
            self.cancel_unanswered(id);
        }
        if self.quitting.is_some_and(|t| now.saturating_duration_since(t) >= QUIT_GRACE) {
            self.finish_quit();
        }
        if self.hangul_hint.is_some_and(|t| now.saturating_duration_since(t) >= TRANSIENT) {
            self.hangul_hint = None;
        }
        self.copy_flash_tick(now);
        self.pointer.tab_slide.tick(now);
        // An attempt opening its tunnel is not timed here (each step of the tunnel has a limit
        // of its own, which stops while the user answers a question).
        let late: Vec<ProfileId> = self
            .conns
            .connecting()
            .filter(|(_, c)| c.tunnel.is_none() && now.saturating_duration_since(c.started) >= CONNECT_TIMEOUT)
            .map(|(id, _)| id)
            .collect();
        for id in late {
            let m = Notice::new(Msg::ConnTimeout { elapsed: CONNECT_TIMEOUT }, Level::Error);
            self.abort_connect(id, m.clone());
            self.status = Some(m);
        }
        if let Some(due) = self.tab().completion_due
            && now >= due
        {
            self.tab_mut().completion_due = None;
            if self.focus == Focus::Editor && self.tab().editor.mode == Mode::Insert && !self.modal_open() {
                self.update_completion(false);
            } else if self.filter_line_focused() && !self.modal_open() {
                self.filter_complete(false, None);
            }
        }
    }
}
