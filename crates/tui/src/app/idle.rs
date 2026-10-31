//! Idle connections (`idle_disconnect`): a tab's query session that sat unused for the
//! profile's limit closes (the next run opens a new one), and once the whole profile sat unused
//! that long its metadata sessions and its SSH tunnel close too (the next action connects again,
//! with the password it connected with). "Used" is a run, a page fetch, an explorer read, a
//! commit: anything the app sends, and every answer of a session (so a long statement is idle
//! from its end). A tab whose session holds an open transaction is warned [`IDLE_WARN_LEAD`]
//! before it closes: the server rolls the transaction back when the session ends.
//!
//! One timer: the earliest deadline over the tabs and profiles ([`App::idle_due`]), nothing
//! while nothing is open or the limit is off. Idle time is counted by the app's clock and the
//! wall clock, whichever says more: the app's clock stands still while the machine sleeps, so a
//! session left over a night closes at the first key, answer or tick after it wakes.
//!
//! The server gets its own limit for each query session, [`SERVER_IDLE_MARGIN`] past the app's
//! (`ConnectOptions::server_idle`), for what the app cannot close itself (it was killed, the
//! machine slept): a session the server closed says so as an idle close.
//!
//! A tab's start-up statements may set that server limit themselves (PostgreSQL's
//! `idle_session_timeout`, MySQL's `wait_timeout`; `SessionPlan::idle`): it is then the tab's
//! limit in place of the profile's, the app's timer and the server's on the same value (a value
//! the app cannot read keeps the profile's limit on the app's side). Its connection stays open as
//! long as the longest limit of its open tabs, so a tab never loses its tunnel before its own
//! limit.
//!
//! The same timer rolls back a transaction opened for manual commit that sat idle for its
//! environment's `tx_idle_rollback` (idle as above: nothing running or waiting in the tab): the
//! app sends `ROLLBACK` on the tab's session, which stays open, and says so once the server
//! confirms it. A rollback not answered in time ([`rollback_wait`]), or refused, closes
//! the session instead (the server rolls back when it ends), and says that. PostgreSQL's server
//! also gets the limit with the transaction's `BEGIN`, [`TX_ROLLBACK_SERVER_MARGIN`] later
//! ([`App::tx_rule`]), so a transaction the app cannot end (it stalled, it was killed) still
//! ends there. MySQL has no such server limit for a transaction (one for the session would also
//! close sessions idle outside one): there the rule holds only while the app runs.

use super::*;
use datarig_core::policy::format_timeout;
use std::time::SystemTime;

/// The server's idle limit for a query session is the app's plus this: the app closes first and
/// says why.
pub const SERVER_IDLE_MARGIN: Duration = Duration::from_secs(60);

/// How long before an idle session with an open transaction closes, or that transaction is
/// rolled back, the app says so (at most half the limit).
pub const IDLE_WARN_LEAD: Duration = Duration::from_secs(60);

/// How long the session may take to answer the `ROLLBACK` the app sends for an idle
/// transaction before the app closes it instead (PostgreSQL's rollback only marks the
/// transaction aborted: it answers at once).
pub const TX_ROLLBACK_ANSWER_WAIT: Duration = Duration::from_secs(10);

/// [`TX_ROLLBACK_ANSWER_WAIT`] on MySQL, whose rollback undoes the transaction's rows one by one
/// and may take about as long as its writes did (closing the session would not make that
/// faster: the server still undoes them, holding the locks).
pub const TX_ROLLBACK_ANSWER_WAIT_MYSQL: Duration = Duration::from_secs(60);

/// How long tab `t`'s session may take to answer the rollback of its idle transaction.
fn rollback_wait(t: &Tab) -> Duration {
    match t.exec.prepared.language().dialect() {
        datarig_core::sql::dialect::Dialect::MySql(_) => TX_ROLLBACK_ANSWER_WAIT_MYSQL,
        _ => TX_ROLLBACK_ANSWER_WAIT,
    }
}

/// PostgreSQL's server ends a session whose transaction opened for manual commit sat idle for
/// its `tx_idle_rollback` plus this (`idle_in_transaction_session_timeout`, for that transaction
/// only): later than the app's own rollback and its [`TX_ROLLBACK_ANSWER_WAIT`], with room for
/// the timer's and the network's delays (the server counts from its answer, before the app gets
/// it), so it acts only when the app could not.
pub const TX_ROLLBACK_SERVER_MARGIN: Duration = Duration::from_secs(30);

/// Why the rollback the app sent for an idle transaction did not end it on its session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RollbackFailed {
    /// No answer within this long ([`rollback_wait`]).
    Unanswered(Duration),
    /// The server's error.
    Refused(String),
    /// The server ended the session for the transaction's idle time itself (its message).
    ServerEnded(String),
}

/// When something was last used, by the app's clock and the wall clock.
type Used = (Instant, SystemTime);

impl App {
    /// How long profile `id`'s connection may sit unused (its environment's `idle_disconnect`,
    /// else the config's); `None`: never closed for that.
    pub fn idle_disconnect_of(&self, id: ProfileId) -> Option<Duration> {
        self.rules_of(id).idle_disconnect.unwrap_or(self.prefs.idle_disconnect)
    }

    /// How long a statement of profile `id`'s query sessions waits for a lock (its environment's
    /// `lock_wait`, else the config's); `None`: the server's setting.
    pub fn lock_wait_of(&self, id: ProfileId) -> Option<Duration> {
        self.rules_of(id).lock_wait.unwrap_or(self.prefs.lock_wait)
    }

    /// The idle rollback limit a transaction opened for manual commit in tab `t` gets when it
    /// begins (`OpenTx::rollback`): its environment's `tx_idle_rollback` now. `None`: off.
    pub(super) fn tx_rollback_limit(&self, t: &Tab) -> Option<Duration> {
        self.tab_rules(t).tx_idle_rollback
    }

    /// How long tab `t`'s open transaction may sit idle before the app rolls it back: the limit
    /// it began with ([`App::tx_rollback_limit`]). `None`: no such transaction is open, or it
    /// has no limit.
    fn tx_rollback_of(&self, t: &Tab) -> Option<Duration> {
        t.exec.tx.as_ref().filter(|_| t.exec.user_tx()).and_then(|x| x.rollback)
    }

    /// How long something used at `used` has been idle at `now`: by the app's clock or the wall
    /// clock, whichever says more.
    fn idle_for(&self, used: Used, now: Instant) -> Duration {
        let app = now.saturating_duration_since(used.0);
        let wall = self.wall_now().duration_since(used.1).unwrap_or_default();
        app.max(wall)
    }

    /// How long tab `t`'s session may sit unused: what its start-up statements set the server's
    /// idle limit to on its way (direct, or behind a pooler), else its profile's limit. `None`:
    /// never closed for that.
    pub(super) fn tab_idle_limit(&self, t: &Tab) -> Option<Duration> {
        use datarig_core::profile::session::Wait;
        let own = t.exec.idle.as_ref().map(|o| if t.exec.session_pooled { o.pooled } else { o.direct });
        match own {
            Some(Wait::Limit(d)) => Some(d),
            Some(Wait::NoLimit) => None,
            Some(Wait::Unknown) | None => self.idle_disconnect_of(t.profile?),
        }
    }

    /// Profile `id` was used now.
    pub(super) fn touch_profile(&mut self, id: ProfileId) {
        let at = (self.now(), self.wall_now());
        if let Some(c) = self.conns.get_mut(id) {
            c.used = Some(at);
        }
    }

    /// Tab `id`'s session (and its profile) was used now: a new idle period starts.
    pub(super) fn touch_tab(&mut self, id: TabId) {
        let at = (self.now(), self.wall_now());
        let Some(t) = self.tabs.get_mut(id) else { return };
        t.exec.used = Some(at);
        t.exec.idle_warned = false;
        if let Some(p) = t.profile {
            self.touch_profile(p);
        }
    }

    /// The tabs whose session may close, or whose transaction may be rolled back, for being
    /// idle: an open session, nothing running or waiting, a limit (the profile's
    /// `idle_disconnect`, or the transaction's [`App::tx_rollback_of`] when that comes first);
    /// with that limit, when it was last used, whether it holds an open transaction not warned
    /// about yet (for its transaction's limit: nor by `tx_idle_warning` in this idle period),
    /// and whether the limit is its transaction's (then the transaction is rolled back, and the
    /// session stays).
    fn idle_tabs(&self) -> Vec<(TabId, Duration, Used, bool, bool)> {
        self.tabs
            .iter()
            .filter(|t| t.exec.session.is_some() && t.profile.is_some() && !self.tab_busy(t.id))
            .filter_map(|t| {
                let unused = self.tab_idle_limit(t);
                let (limit, tx) = match (unused, self.tx_rollback_of(t)) {
                    (Some(u), Some(r)) => (u.min(r), r <= u),
                    (Some(u), None) => (u, false),
                    (None, r) => (r?, true),
                };
                let tx_warned = tx && t.exec.tx.as_ref().is_some_and(|x| x.warned);
                let warn = t.exec.tx_at_risk() && !t.exec.idle_warned && !tx_warned;
                Some((t.id, limit, t.exec.used?, warn, tx))
            })
            .collect()
    }

    /// The profiles whose sessions may close for being idle: connected, nothing of theirs
    /// running or waiting, a limit (the profile's, or a longer one of its tabs with a session
    /// open: [`App::tab_idle_limit`]); with it and when they were last used.
    fn idle_profiles(&self) -> Vec<(ProfileId, Duration, Used)> {
        self.conns
            .iter()
            .filter(|(_, c)| c.connected && c.connecting.is_none() && c.pending.is_empty())
            .filter(|(id, _)| !self.tabs.iter().any(|t| t.profile == Some(*id) && self.tab_busy(t.id)))
            .filter_map(|(id, c)| {
                let tabs = self.tabs.iter().filter(|t| t.profile == Some(id) && t.exec.session.is_some());
                let limit = tabs.fold(self.idle_disconnect_of(id), |l, t| Some(l?.max(self.tab_idle_limit(t)?)));
                Some((id, limit?, c.used?))
            })
            .collect()
    }

    /// The tabs whose idle rollback was sent and not answered: by when it must be.
    fn idle_endings(&self) -> Vec<(TabId, super::tx::IdleEnd)> {
        self.tabs.iter().filter_map(|t| Some((t.id, t.exec.ending?.idle?))).collect()
    }

    /// When the next idle close, rollback or warning is due (`None`: nothing can close for
    /// being idle).
    pub(super) fn idle_due(&self, now: Instant) -> Option<Instant> {
        let tabs = self.idle_tabs().into_iter().flat_map(|(_, limit, used, warn, _)| {
            let idle = self.idle_for(used, now);
            let close = now + limit.saturating_sub(idle);
            let warning = warn.then(|| now + limit.saturating_sub(IDLE_WARN_LEAD.min(limit / 2)).saturating_sub(idle));
            [Some(close), warning]
        });
        let profiles = self
            .idle_profiles()
            .into_iter()
            .map(|(_, limit, used)| Some(now + limit.saturating_sub(self.idle_for(used, now))));
        let endings = self.idle_endings().into_iter().map(|(_, e)| Some(e.by));
        tabs.chain(profiles).chain(endings).flatten().min()
    }

    /// The idle closes, rollbacks and warnings due at `now`: the sessions whose idle rollback
    /// went unanswered, the idle transactions due a rollback (before the profiles' closes: when
    /// both are due at once, after a sleep, the transaction's limit is the shorter one, so its
    /// rollback would have come first; the tab is busy then, so its profile stays), the profiles
    /// nothing used for their limit (with their tabs' sessions), then each other tab on its own.
    pub(super) fn idle_tick(&mut self, now: Instant) {
        for (id, e) in self.idle_endings() {
            if now >= e.by {
                self.idle_rollback_failed(id, e.limit, RollbackFailed::Unanswered(e.wait));
            }
        }
        for (id, limit, used, _, tx) in self.idle_tabs() {
            if tx && self.idle_for(used, now) >= limit {
                self.roll_back_idle_tab(id, limit);
            }
        }
        for (id, limit, used) in self.idle_profiles() {
            if self.idle_for(used, now) >= limit {
                self.close_idle_profile(id, limit);
            }
        }
        for (id, limit, used, warn, tx) in self.idle_tabs() {
            let idle = self.idle_for(used, now);
            if idle >= limit {
                // A transaction's limit was dealt with above.
                if !tx {
                    self.close_idle_tab(id, limit, false);
                }
            } else if warn && idle >= limit.saturating_sub(IDLE_WARN_LEAD.min(limit / 2)) {
                self.warn_idle_tab(id, limit - idle, tx);
            }
        }
    }

    /// Tab `id`'s idle session holds an open transaction and closes in `left` (`tx`: for that
    /// transaction's limit): said in its Messages, its status and the status bar (once per idle
    /// period).
    fn warn_idle_tab(&mut self, id: TabId, left: Duration, tx: bool) {
        let Some(i) = self.tabs.iter().position(|t| t.id == id) else { return };
        let (commit, rollback) = self.tx_keys();
        let (tab, time) = ((i + 1).to_string(), super::tx::fmt_clock(left));
        let m = match tx {
            true => Msg::TxIdleRollbackWarning { tab, time, commit, rollback },
            false => Msg::TxIdleCloseWarning { tab, time, commit, rollback },
        };
        let n = Notice::new(m, Level::Warning);
        if let Some(t) = self.tabs.get_mut(id) {
            t.exec.idle_warned = true;
            t.exec.run.notes.push(n.clone());
        }
        self.tab_status(id, n.clone());
        self.status = Some(n);
    }

    /// Tab `id`'s transaction opened for manual commit sat idle for its `limit`: the app sends
    /// `ROLLBACK` on its session, bound to that session and that request, to be answered within
    /// [`rollback_wait`] (the answer says the rest, `App::tab_event`). The tab stays
    /// busy meanwhile: nothing else is sent there. A session that cannot end it on request
    /// closes as before (the server rolls back).
    fn roll_back_idle_tab(&mut self, id: TabId, limit: Duration) {
        let endable = self.tabs.get(id).is_some_and(|t| {
            t.exec.user_tx() && t.exec.ending.is_none() && t.exec.session.as_ref().is_some_and(|s| s.caps.manual_commit)
        });
        if !endable {
            self.close_idle_tab(id, limit, true);
            return;
        }
        self.query_seq += 1;
        let qid = self.query_seq;
        let wait = self.tabs.get(id).map_or(TX_ROLLBACK_ANSWER_WAIT, rollback_wait);
        let idle = super::tx::IdleEnd { limit, wait, by: self.now() + wait };
        if let Some(t) = self.tabs.get_mut(id) {
            t.exec.ending = Some(super::tx::Ending { id: qid, commit: false, idle: Some(idle) });
            t.exec.last_end = None;
        }
        let time = format_timeout(Some(limit));
        self.tab_status(id, Notice::new(Msg::TxIdleRollingBack { time }, Level::Info));
        self.send_tab(id, DbCommand::EndTx { id: qid, commit: false });
    }

    /// The rollback the app sent for tab `id`'s transaction idle for `limit` did not end it
    /// (`why`): its session closes instead (the server rolls back when it sees it end), said in
    /// its Messages, its status and the status bar.
    pub(super) fn idle_rollback_failed(&mut self, id: TabId, limit: Duration, why: RollbackFailed) {
        let time = format_timeout(Some(limit));
        let m = match why {
            RollbackFailed::Refused(error) => Msg::QueryTxIdleRollbackFailed { time, error },
            RollbackFailed::ServerEnded(error) => Msg::TxIdleTimeout { error },
            RollbackFailed::Unanswered(wait) => {
                Msg::QueryTxIdleRollbackUnanswered { time, wait: format_timeout(Some(wait)) }
            }
        };
        self.close_idle_tab_saying(id, Notice::new(m, Level::Warning), true);
    }

    /// Close tab `id`'s session after it sat idle for `limit` (the server rolls back an open
    /// transaction; `tx`: the limit was that transaction's, [`App::tx_rollback_of`], and its
    /// session cannot roll it back on request): said in its Messages and its status (and the
    /// status bar when a transaction went with it). The next run opens a new session. Whether a
    /// transaction went with it.
    fn close_idle_tab(&mut self, id: TabId, limit: Duration, tx: bool) -> bool {
        let had_tx = self.tabs.get(id).is_some_and(|t| t.exec.tx_at_risk());
        let time = format_timeout(Some(limit));
        let n = match had_tx {
            true if tx => Notice::new(Msg::QueryTxIdleClosed { time }, Level::Warning),
            true => Notice::new(Msg::QueryIdleClosedTx { time }, Level::Warning),
            false => Notice::new(Msg::QueryIdleClosed { time }, Level::Info),
        };
        self.close_idle_tab_saying(id, n, had_tx);
        had_tx
    }

    /// Close tab `id`'s session for being idle, saying `n` in its Messages and its status (and
    /// the status bar: `loud`). The next run opens a new session and says nothing more of it.
    fn close_idle_tab_saying(&mut self, id: TabId, n: Notice, loud: bool) {
        self.close_tab_sessions(|t| t.id == id);
        if let Some(t) = self.tabs.get_mut(id) {
            t.tx_reset();
            // Said now: the next run does not say it again.
            t.exec.lost_tx = false;
            t.exec.paging = Paging::None;
            t.exec.used = None;
            t.exec.idle_warned = false;
            t.exec.run.notes.push(n.clone());
        }
        self.tab_status(id, n.clone());
        if loud {
            self.status = Some(n);
        }
    }

    /// Close profile `id`'s sessions and its SSH tunnel (a shared one once no other profile
    /// uses it) after nothing used them for `limit`. What it read (the catalog, the keys) and the
    /// password it connected with stay: the next action connects again without asking, and the
    /// explorer says why it is closed.
    fn close_idle_profile(&mut self, id: ProfileId, limit: Duration) {
        let name = self.profile(id).map(|p| p.name.clone()).unwrap_or_default();
        let tabs: Vec<TabId> =
            self.tabs.iter().filter(|t| t.profile == Some(id) && t.exec.session.is_some()).map(|t| t.id).collect();
        let mut had_tx = false;
        for t in tabs {
            had_tx |= self.close_idle_tab(t, limit, false);
        }
        self.close_profile_sessions(id);
        self.close_tunnel(id);
        self.drop_tunnel_asks(id);
        let c = self.conns.entry(id);
        let resolved = c.resolved.take();
        let catalog = std::mem::take(&mut c.catalog);
        let keys = std::mem::replace(&mut c.keys, super::conn::Keys::Unknown);
        let (expanded, language) = (c.expanded, c.language);
        self.conns.next_generation(id);
        let time = format_timeout(Some(limit));
        let m = match had_tx {
            true => Notice::new(Msg::ConnIdleClosedTx { name, time }, Level::Warning),
            false => Notice::new(Msg::ConnIdleClosed { name, time }, Level::Info),
        };
        let c = self.conns.entry(id);
        c.reset();
        (c.resolved, c.catalog, c.keys, c.language) = (resolved, catalog, keys, language);
        c.expand_on_connect = expanded;
        c.idle_closed = Some(m.clone());
        self.cursor_off_profile(id);
        self.show_status(m);
    }
}
