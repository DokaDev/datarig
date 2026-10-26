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
//! The same timer rolls back a transaction opened for manual commit that sat idle for its
//! environment's `tx_idle_rollback` where the server does not keep that limit itself (MySQL;
//! PostgreSQL's gets it with its `BEGIN`, [`App::tx_rule`]): the app closes the tab's session and
//! the server rolls the transaction back. Only while the app runs: nothing is set on the server
//! for it, since a server limit would also close sessions idle outside a transaction.

use super::*;
use datarig_core::policy::format_timeout;
use datarig_core::sql::dialect::Dialect;
use std::time::SystemTime;

/// The server's idle limit for a query session is the app's plus this: the app closes first and
/// says why.
pub const SERVER_IDLE_MARGIN: Duration = Duration::from_secs(60);

/// How long before an idle session with an open transaction closes the app says so (at most
/// half the limit).
pub const IDLE_WARN_LEAD: Duration = Duration::from_secs(60);

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
    /// begins (`OpenTx::rollback`): its environment's `tx_idle_rollback` now, on MySQL
    /// (PostgreSQL's server keeps that limit itself). `None`: off, or not the app's to keep.
    pub(super) fn tx_rollback_limit(&self, t: &Tab) -> Option<Duration> {
        let mysql = matches!(t.exec.prepared.language().dialect(), Dialect::MySql(_));
        mysql.then(|| self.tab_rules(t).tx_idle_rollback).flatten()
    }

    /// How long tab `t`'s open transaction may sit idle before the app closes its session (the
    /// server rolls the transaction back): the limit it began with ([`App::tx_rollback_limit`]).
    /// `None`: no such transaction is open, or it has no limit.
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

    /// The tabs whose session may close for being idle: an open session, nothing running or
    /// waiting, a limit (the profile's `idle_disconnect`, or the transaction's
    /// [`App::tx_rollback_of`] when that comes first); with that limit, when it was last used,
    /// whether it holds an open transaction not warned about yet (for its transaction's limit:
    /// nor by `tx_idle_warning` in this idle period), and whether it closes for its
    /// transaction's limit.
    fn idle_tabs(&self) -> Vec<(TabId, Duration, Used, bool, bool)> {
        self.tabs
            .iter()
            .filter(|t| t.exec.session.is_some() && !self.tab_busy(t.id))
            .filter_map(|t| {
                let unused = self.idle_disconnect_of(t.profile?);
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
    /// running or waiting, a limit; with it and when they were last used.
    fn idle_profiles(&self) -> Vec<(ProfileId, Duration, Used)> {
        self.conns
            .iter()
            .filter(|(_, c)| c.connected && c.connecting.is_none() && c.pending.is_empty())
            .filter(|(id, _)| !self.tabs.iter().any(|t| t.profile == Some(*id) && self.tab_busy(t.id)))
            .filter_map(|(id, c)| Some((id, self.idle_disconnect_of(id)?, c.used?)))
            .collect()
    }

    /// When the next idle close or warning is due (`None`: nothing can close for being idle).
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
        tabs.chain(profiles).flatten().min()
    }

    /// The idle closes and warnings due at `now`: the profiles nothing used for their limit
    /// (with their tabs' sessions), then each other tab on its own.
    pub(super) fn idle_tick(&mut self, now: Instant) {
        for (id, limit, used) in self.idle_profiles() {
            if self.idle_for(used, now) >= limit {
                self.close_idle_profile(id, limit);
            }
        }
        for (id, limit, used, warn, tx) in self.idle_tabs() {
            let idle = self.idle_for(used, now);
            if idle >= limit {
                self.close_idle_tab(id, limit, tx);
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

    /// Close tab `id`'s session after it sat idle for `limit` (the server rolls back an open
    /// transaction; `tx`: the limit was that transaction's, [`App::tx_rollback_of`]): said in
    /// its Messages and its status (and the status bar when a transaction went with it). The
    /// next run opens a new session. Whether a transaction went with it.
    fn close_idle_tab(&mut self, id: TabId, limit: Duration, tx: bool) -> bool {
        let had_tx = self.tabs.get(id).is_some_and(|t| t.exec.tx_at_risk());
        self.close_tab_sessions(|t| t.id == id);
        let time = format_timeout(Some(limit));
        let n = match had_tx {
            true if tx => Notice::new(Msg::QueryTxIdleClosed { time }, Level::Warning),
            true => Notice::new(Msg::QueryIdleClosedTx { time }, Level::Warning),
            false => Notice::new(Msg::QueryIdleClosed { time }, Level::Info),
        };
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
        if had_tx {
            self.status = Some(n);
        }
        had_tx
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
