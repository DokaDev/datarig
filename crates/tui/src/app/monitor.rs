//! The server monitor: one Monitor tab per profile ([`TabKind::Monitor`]) shows what the
//! profile's server says about its load and its sessions, read by a monitor connection of the
//! tab's own (`SessionRole::Monitor`), never the tabs' query sessions or the explorer's
//! metadata session.
//!
//! The tab reads only while it is the active tab: one read ("tick") at a time, the next one
//! `interval` after the answer to the last (`[monitor] interval`, `+`/`-` in the tab), so a slow
//! server is never asked twice at once. A hidden tab sends nothing and wakes nothing; shown
//! again, it reads at once and its charts show a gap for the time it was not looking. `p`
//! pauses (the last read stays for reading), `r` reads at once. Three failed reads in a row
//! double the delay, up to a minute; one good read goes back to the setting.
//!
//! Every read is bound to the tab, its connection's generation and the read's number: an answer
//! of an older connection or read is dropped. What the account cannot see is said, with what
//! would show it, never shown as nothing.

use super::*;
use crate::widgets::monitor::state_text;
use datarig_core::config::MonitorInterval;
use datarig_core::i18n::I18n;
use datarig_core::monitor::SessionsWanted;
use datarig_core::monitor::rate::{self, History, Pending, Rate};
use datarig_core::monitor::{
    CounterKind, GaugeKind, MonitorFormat, Sample, ServerInfo, SessionKey, SessionRow, SessionState, Snapshot,
    Timestamp, Tx, Want,
};
use std::time::Duration;

/// How many of the longest running sessions the Overview keeps.
pub const TOP_MAX: usize = 10;

/// How many points each chart keeps.
pub const HISTORY: usize = 150;

/// The longest a read's statements may run on the server (a shorter interval gives them less).
const LIMIT_MAX: Duration = Duration::from_secs(5);

/// Failed reads in a row after which the delay doubles.
const BACKOFF_AFTER: u32 = 3;

/// The longest delay between two reads after failures.
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// How long past its limit a read may stay unanswered before the tab gives up on its
/// connection (the driver itself answers well before: this only guards against an answer that
/// never comes).
const ANSWER_GRACE: Duration = Duration::from_secs(10);

/// The views of a Monitor tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MonitorView {
    /// The server, its load as charts, and the longest running sessions.
    #[default]
    Overview,
    /// Every session, sorted and filtered.
    Sessions,
}

impl MonitorView {
    pub const ALL: [MonitorView; 2] = [MonitorView::Overview, MonitorView::Sessions];

    pub fn label(self) -> Label {
        match self {
            MonitorView::Overview => Label::MonitorViewOverview,
            MonitorView::Sessions => Label::MonitorViewSessions,
        }
    }
}

/// What the Monitor tab acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonitorAction {
    /// `monitor.open`: the Monitor tab of the profile in context.
    Open,
    View(MonitorView),
    /// The next view (`Tab`).
    NextView,
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    /// Stop reading (the last read stays), or read again.
    Pause,
    /// Read at once.
    ReadNow,
    /// A shorter or a longer interval for this tab.
    Faster,
    Slower,
    /// Show idle sessions too, or hide them again.
    ShowIdle,
    /// Sort by the next column; the other way round.
    Sort,
    SortReverse,
    /// Filter the sessions (typing).
    Filter,
    /// The selected session in full, or the list again.
    Details,
    /// The selected session's statement in a new console (never run).
    OpenConsole,
    /// The selected session's statement to the clipboard.
    CopyQuery,
    /// Close the details, end the filter.
    Back,
}

/// What a column of the session list sorts by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortKey {
    Id,
    User,
    Database,
    State,
    TxAge,
    /// How long the statement has run (an idle session's: since its last one started).
    #[default]
    QueryAge,
}

impl SortKey {
    pub const ALL: [SortKey; 6] =
        [SortKey::Id, SortKey::User, SortKey::Database, SortKey::State, SortKey::TxAge, SortKey::QueryAge];

    /// Longest first for ages, the alphabet's order for names.
    fn descending_first(self) -> bool {
        matches!(self, SortKey::TxAge | SortKey::QueryAge)
    }
}

/// A metric the Overview charts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Metric {
    /// A counter per second.
    Rate(CounterKind),
    /// The share of page requests the cache answered, in percent.
    CacheHit,
    /// Client sessions connected.
    Sessions,
}

/// The metrics in the order the Overview shows them; a format shows those it has.
pub const METRICS: [Metric; 18] = [
    Metric::Rate(CounterKind::Commits),
    Metric::Rate(CounterKind::Statements),
    Metric::Sessions,
    Metric::CacheHit,
    Metric::Rate(CounterKind::RowsRead),
    Metric::Rate(CounterKind::RowsWritten),
    Metric::Rate(CounterKind::PageReads),
    Metric::Rate(CounterKind::Rollbacks),
    Metric::Rate(CounterKind::ExplicitCommits),
    Metric::Rate(CounterKind::ExplicitRollbacks),
    Metric::Rate(CounterKind::BytesIn),
    Metric::Rate(CounterKind::BytesOut),
    Metric::Rate(CounterKind::Deadlocks),
    Metric::Rate(CounterKind::RowLockWaits),
    Metric::Rate(CounterKind::TempBytes),
    Metric::Rate(CounterKind::TempDiskTables),
    Metric::Rate(CounterKind::SlowQueries),
    Metric::Rate(CounterKind::AbortedConnects),
];

/// Where the monitor connection is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MonState {
    /// No connection yet: it opens (its profile connecting first) once the tab is shown.
    Idle,
    /// Its connection was closed with its profile's (a disconnect, the idle disconnect): it
    /// opens again when the tab is shown again, or on `r`, never by itself while shown (a
    /// profile the user disconnected stays so).
    Closed,
    /// The profile is connecting: the monitor opens once it has.
    WaitingForProfile,
    /// The profile could not connect: `r` tries again.
    ProfileFailed(String),
    /// Opened, its probe not answered yet.
    Opening,
    /// Reading with this format.
    Ready(MonitorFormat),
    /// The server offers no monitor (too old, the probe failed, a driver without one): why.
    /// `r` tries again.
    Unavailable(String),
    /// The connection ended by itself: it opens again at the next read.
    Lost(String),
}

/// The session list's sort, filter and selection.
#[derive(Clone, Debug, Default)]
pub struct SessionsView {
    pub sort: SortKey,
    /// Against the column's own first order (ages longest first, names A to Z).
    pub reversed: bool,
    pub filter: TextInput,
    /// The filter has the keyboard.
    pub filtering: bool,
    /// Idle sessions are read too (`a`).
    pub show_idle: bool,
    /// The rows of the snapshot in the order shown (sorted, filtered): indexes into its rows.
    pub order: Vec<usize>,
    /// The selected row's place in `order`.
    pub cursor: usize,
    /// The selected session, kept across reads.
    pub selected: Option<SessionKey>,
    /// The first row on screen.
    pub top: usize,
    /// The selected session's details are shown.
    pub details: bool,
    /// The first line of the details on screen.
    pub details_top: usize,
}

/// Where the parts of the Monitor tab that take a click were drawn last.
#[derive(Clone, Debug, Default)]
pub struct MonitorHits {
    pub views: Vec<(Rect, MonitorView)>,
    pub headers: Vec<(Rect, SortKey)>,
    /// The rows of the session list (or of the Overview's longest running): row `top + n` at
    /// line `n`.
    pub rows: Rect,
    /// The list the rows are of.
    pub rows_view: Option<MonitorView>,
    /// The rows of the Overview's top sessions, as indexes into the snapshot's rows.
    pub top_rows: Vec<usize>,
}

/// A Monitor tab: its connection, its reads, and what it shows.
pub struct MonitorTab {
    pub session: Option<Session>,
    /// Generation of `session`: events of an older one are dropped.
    pub generation: u64,
    pub state: MonState,
    pub view: MonitorView,
    /// This tab's interval (the setting's until `+`/`-`).
    pub interval: MonitorInterval,
    pub paused: bool,
    /// `r` while paused: one read is due, the tab stays paused after it.
    pub read_once: bool,
    /// The read in flight: its number and when it was sent.
    pub request: Option<(u64, Instant)>,
    seq: u64,
    /// When the next read is due (`None`: none planned).
    pub due: Option<Instant>,
    /// Failed reads in a row.
    pub failures: u32,
    /// The latest read.
    pub snapshot: Option<Snapshot>,
    /// What the server is, as last read.
    pub server: Option<ServerInfo>,
    /// The last read failed: why (the last good read stays shown).
    pub error: Option<String>,
    /// The tab was the active one at the last look: a hidden tab reads nothing.
    pub shown: bool,
    /// Another tab was the active one since this one was last shown (not only a dialog over
    /// it).
    left: bool,
    /// The read the next rates are worked out from (`None` after a gap).
    prev: Option<Sample>,
    pending: Pending,
    /// The server info is read with the next tick.
    want_server: bool,
    /// The read in flight asks for the server info (asked again when it fails).
    asked_server: bool,
    pub history: Vec<(Metric, History)>,
    /// The Overview's longest running sessions (a statement running, or a transaction open),
    /// longest first: indexes into the latest read's rows, worked out when it came.
    pub top: Vec<usize>,
    pub list: SessionsView,
    pub hits: MonitorHits,
}

impl MonitorTab {
    pub fn new(interval: MonitorInterval) -> Self {
        Self {
            session: None,
            generation: 0,
            state: MonState::Idle,
            view: MonitorView::Overview,
            interval,
            paused: false,
            read_once: false,
            request: None,
            seq: 0,
            due: None,
            failures: 0,
            snapshot: None,
            server: None,
            error: None,
            shown: false,
            left: false,
            prev: None,
            pending: Pending::default(),
            want_server: true,
            asked_server: false,
            history: Vec::new(),
            top: Vec::new(),
            list: SessionsView::default(),
            hits: MonitorHits::default(),
        }
    }

    /// The delay before the next read: the interval, doubled for each three failures in a row,
    /// at most [`BACKOFF_MAX`] (never shorter than the interval).
    pub fn delay(&self) -> Duration {
        let base = self.interval.duration();
        let doublings = (self.failures / BACKOFF_AFTER).min(6);
        base.saturating_mul(1 << doublings).min(BACKOFF_MAX.max(base))
    }

    /// The longest each statement of a read may run on the server.
    pub fn limit(&self) -> Duration {
        self.interval.duration().min(LIMIT_MAX)
    }

    /// The format the connection reads with, once probed.
    pub fn format(&self) -> Option<&MonitorFormat> {
        match &self.state {
            MonState::Ready(f) => Some(f),
            _ => None,
        }
    }

    /// The history of `metric`, if the format has it.
    pub fn history_of(&self, metric: Metric) -> Option<&History> {
        self.history.iter().find(|(m, _)| *m == metric).map(|(_, h)| h)
    }

    /// The selected session's row.
    pub fn selected_row(&self) -> Option<&SessionRow> {
        let rows = &self.snapshot.as_ref()?.sessions.as_ref()?.rows;
        rows.get(*self.list.order.get(self.list.cursor)?)
    }

    /// The server's clock at the latest read of the sessions.
    pub fn sessions_time(&self) -> Option<Timestamp> {
        Some(self.snapshot.as_ref()?.sessions.as_ref()?.time)
    }

    /// The connection is gone (closed or lost): what it read stays, the next rates start
    /// again, and a new connection counts its own traffic from nothing.
    pub(super) fn drop_connection(&mut self) {
        if let Some(s) = self.session.take() {
            s.close();
        }
        // What the closed connection still sends (a probe's answer already queued) is dropped:
        // no current connection has generation 0.
        self.generation = 0;
        self.request = None;
        self.prev = None;
        self.pending = Pending::default();
        self.want_server = true;
    }

    /// The charts show a gap before the next point: what came before is not joined to it.
    fn gap(&mut self) {
        self.prev = None;
        // The own traffic still to take out belongs to the time before the gap: the read after
        // it may count it already.
        self.pending = Pending::default();
    }

    /// Take a read's answer: the rates since the read before, the charts, the list.
    fn take(&mut self, snapshot: Snapshot, i18n: &I18n) {
        let now = &snapshot.sample;
        if self.history.is_empty() {
            for m in METRICS {
                let has = match m {
                    Metric::Rate(k) => now.get(k).is_some(),
                    Metric::CacheHit => {
                        now.get(CounterKind::PageRequests).is_some() && now.get(CounterKind::PageReads).is_some()
                    }
                    Metric::Sessions => snapshot.gauges.iter().any(|g| g.kind == GaugeKind::Sessions),
                };
                if has {
                    self.history.push((m, History::new(HISTORY)));
                }
            }
        }
        let prev = self.prev.take();
        // After a gap (hidden, paused, reconnected) the first point stands alone: a gap is put
        // before it, never a line across the time nobody looked.
        let joined = prev.is_some();
        let gauge = |k: GaugeKind| snapshot.gauges.iter().find(|g| g.kind == k).map(|g| g.value as f64);
        for (m, h) in &mut self.history {
            let value = match (*m, &prev) {
                (Metric::Sessions, _) => gauge(GaugeKind::Sessions),
                (_, None) => None,
                (Metric::Rate(k), Some(before)) => match rate::rate(before, now, k, &mut self.pending) {
                    Some(Rate::PerSecond(v)) => Some(v),
                    _ => None,
                },
                (Metric::CacheHit, Some(before)) => match rate::cache_hit(before, now) {
                    Some(Ok(share)) => Some(share * 100.0),
                    _ => None,
                },
            };
            if !joined && !h.is_empty() {
                h.push(Timestamp(now.time.0 - 1), None);
            }
            h.push(now.time, value);
        }
        self.prev = Some(snapshot.sample.clone());
        if let Some(info) = &snapshot.server {
            self.server = Some(info.clone());
        }
        self.snapshot = Some(snapshot);
        self.error = None;
        self.refresh_order(i18n);
    }

    /// Sort and filter the latest read's sessions again, keeping the selected session selected
    /// when it is still there.
    pub fn refresh_order(&mut self, i18n: &I18n) {
        let Some(list) = self.snapshot.as_ref().and_then(|s| s.sessions.as_ref()) else {
            self.list.order.clear();
            self.top.clear();
            self.list.cursor = 0;
            return;
        };
        self.top = longest(&list.rows, list.time);
        let filter = self.list.filter.text().to_lowercase();
        let time = list.time;
        let mut order: Vec<usize> = (0..list.rows.len())
            .filter(|&i| filter.is_empty() || matches_filter(&list.rows[i], &filter, i18n))
            .collect();
        let key = self.list.sort;
        let descending = key.descending_first() != self.list.reversed;
        order.sort_by(|&a, &b| {
            let (ra, rb) = (&list.rows[a], &list.rows[b]);
            let o = compare(ra, rb, key, time, i18n);
            let o = if descending { o.reverse() } else { o };
            o.then(ra.key.id.cmp(&rb.key.id))
        });
        let selected = self.list.selected;
        self.list.order = order;
        let at = selected.and_then(|k| self.list.order.iter().position(|&i| list.rows[i].key == k));
        self.list.cursor = at.unwrap_or(self.list.cursor.min(self.list.order.len().saturating_sub(1)));
        self.list.selected = self.list.order.get(self.list.cursor).map(|&i| list.rows[i].key);
    }

    /// Move the selection by `delta` rows (clamped).
    fn move_cursor(&mut self, delta: isize) {
        let n = self.list.order.len();
        if n == 0 {
            return;
        }
        let at = (self.list.cursor as isize + delta).clamp(0, n as isize - 1) as usize;
        self.select(at);
    }

    /// Select row `at` of the list.
    pub fn select(&mut self, at: usize) {
        let rows = self.snapshot.as_ref().and_then(|s| s.sessions.as_ref()).map(|l| &l.rows);
        let Some(rows) = rows else { return };
        if let Some(&i) = self.list.order.get(at) {
            self.list.cursor = at;
            self.list.selected = Some(rows[i].key);
            self.list.details_top = 0;
        }
    }
}

/// The longest running client sessions of `rows` (a statement running: for how long; a
/// transaction open while idle: for how long it is open), longest first, at most [`TOP_MAX`].
fn longest(rows: &[SessionRow], time: Timestamp) -> Vec<usize> {
    use datarig_core::monitor::SessionKind;
    let long = |r: &SessionRow| match r.state {
        SessionState::Active => query_age(r, time),
        _ => tx_age(r, time),
    };
    let mut top: Vec<(Option<i64>, i64, usize)> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            !r.monitor
                && r.kind == SessionKind::Client
                && matches!(r.state, SessionState::Active | SessionState::IdleInTx | SessionState::IdleInTxAborted)
        })
        .map(|(i, r)| (long(r), r.key.id, i))
        .collect();
    let n = top.len().min(TOP_MAX);
    if n > 0 && n < top.len() {
        top.select_nth_unstable_by(n - 1, |a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        top.truncate(n);
    }
    top.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    top.into_iter().map(|(_, _, i)| i).collect()
}

/// Whether a session's user, database, application, client, state or statement contains
/// `filter` (lower case).
fn matches_filter(r: &SessionRow, filter: &str, i18n: &I18n) -> bool {
    let has = |s: &Option<String>| s.as_deref().is_some_and(|s| s.to_lowercase().contains(filter));
    has(&r.user)
        || has(&r.database)
        || has(&r.application)
        || has(&r.client)
        || state_text(i18n, r).to_lowercase().contains(filter)
        || r.query.as_ref().is_some_and(|q| q.text.to_lowercase().contains(filter))
}

/// Two sessions by `key`, a missing value last in the column's own order.
fn compare(a: &SessionRow, b: &SessionRow, key: SortKey, time: Timestamp, i18n: &I18n) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let text = |x: &Option<String>, y: &Option<String>| match (x, y) {
        (Some(x), Some(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    match key {
        SortKey::Id => a.key.id.cmp(&b.key.id),
        SortKey::User => text(&a.user, &b.user),
        SortKey::Database => text(&a.database, &b.database),
        SortKey::State => state_text(i18n, a).cmp(&state_text(i18n, b)),
        SortKey::TxAge => tx_age(a, time).cmp(&tx_age(b, time)),
        SortKey::QueryAge => query_age(a, time).cmp(&query_age(b, time)),
    }
}

/// How long the session's transaction has been open, in microseconds on the server's clock.
pub fn tx_age(r: &SessionRow, time: Timestamp) -> Option<i64> {
    match r.tx {
        Tx::Since(t) => time.since(t),
        _ => None,
    }
}

/// How long the session's statement has run (an idle one's last: since it started).
pub fn query_age(r: &SessionRow, time: Timestamp) -> Option<i64> {
    r.query_started.and_then(|t| time.since(t))
}

impl App {
    /// The Monitor tab of tab `id`, to read.
    pub fn monitor_of(&self, id: TabId) -> Option<&MonitorTab> {
        self.tabs.get(id).and_then(|t| t.doc.monitor.as_ref())
    }

    fn monitor_mut(&mut self, id: TabId) -> Option<&mut MonitorTab> {
        self.tabs.get_mut(id).and_then(|t| t.doc.monitor.as_mut())
    }

    /// The Monitor tab of tab `id` with the app's words (sorting and filtering by a state
    /// read them).
    fn monitor_words(&mut self, id: TabId) -> Option<(&mut MonitorTab, &I18n)> {
        let i18n = &self.i18n;
        self.tabs.get_mut(id).and_then(|t| t.doc.monitor.as_mut()).map(|m| (m, i18n))
    }

    /// The Monitor tab of tab `id`, for the frame that draws it (where its rows and parts are).
    pub(crate) fn monitor_mut_for_draw(&mut self, id: TabId) -> Option<&mut MonitorTab> {
        self.monitor_mut(id)
    }

    /// The active tab is a Monitor tab (drawn in the place of the editor and results).
    pub fn monitor_shown(&self) -> bool {
        !self.tabs.is_empty() && !self.profiles.is_empty() && self.tab().is_monitor()
    }

    /// Nothing covers the active tab: a Monitor tab there is on screen and reads (a modal
    /// dialog over it stops the reads, as a hidden tab's).
    fn monitor_visible(&self) -> bool {
        !self.profiles.is_empty() && !self.overlays.modal() && self.zoomed() != Some(Focus::Tree)
    }

    /// The Monitor tab's filter has the keyboard.
    pub fn monitor_filtering(&self) -> bool {
        self.monitor_shown() && self.monitor_of(self.tab().id).is_some_and(|m| m.list.filtering)
    }

    /// Profile `id`'s driver monitors its servers (`Capabilities::monitor`).
    pub(super) fn monitor_on(&self, id: ProfileId) -> bool {
        self.profile(id).and_then(|p| self.driver(&p.driver)).is_some_and(|d| d.capabilities().monitor)
    }

    /// `monitor.open` / `:monitor [profile]`: the Monitor tab of profile `id`; an open one
    /// becomes active, else a new one opens. It reads once it is shown.
    pub(super) fn open_monitor(&mut self, id: ProfileId) {
        if !self.monitor_on(id) {
            return self.flash(Notice::new(Label::MonitorUnsupported, Level::Info));
        }
        let open = self.tabs.iter().find(|t| t.is_monitor() && t.profile == Some(id)).map(|t| t.id);
        if let Some(t) = open {
            self.switch_tab(|m| m.position(t).is_some_and(|i| m.activate(i)));
        } else {
            self.leave_tab();
            let tab = self.tabs.open(TabKind::Monitor, Some(id), Editor::read_only(""));
            let interval = self.prefs.monitor_interval;
            if let Some(t) = self.tabs.get_mut(tab) {
                t.doc.monitor = Some(MonitorTab::new(interval));
                t.doc.written = true;
            }
            self.entered_tab();
        }
        self.focus = Focus::Results;
        self.fix_focus();
        self.monitor_sync();
    }

    /// The profile `monitor.open` and `:monitor` mean: the explorer's when it has the focus,
    /// else the active tab's.
    pub(super) fn monitor_profile(&self) -> Option<ProfileId> {
        match self.focus {
            Focus::Tree => self.selected_profile(),
            _ if self.tabs.is_empty() => None,
            _ => self.tab().profile,
        }
    }

    /// `:monitor [profile]`.
    pub(super) fn monitor_command(&mut self, name: &str) -> Result<(), Notice> {
        let id = match name.trim() {
            "" => self.monitor_profile().ok_or(Notice::new(Label::MonitorNoProfile, Level::Error))?,
            name => {
                let p = self.profiles.iter().find(|p| p.name == name);
                p.map(|p| p.id)
                    .ok_or_else(|| Notice::new(Msg::MonitorNoSuchProfile { name: name.into() }, Level::Error))?
            }
        };
        self.open_monitor(id);
        Ok(())
    }

    /// Bring every Monitor tab in line with what is on screen: the active one reads (its
    /// profile connected and its connection opened first), the others read nothing. Called
    /// after input, events and ticks.
    pub(super) fn monitor_sync(&mut self) {
        let now = self.now();
        self.monitor_sync_at(now);
    }

    /// [`App::monitor_sync`] at `now` (a timer tick's time).
    pub(super) fn monitor_sync_at(&mut self, now: Instant) {
        let active = (!self.tabs.is_empty()).then(|| self.tab().id);
        let ids: Vec<TabId> = self.tabs.iter().filter(|t| t.is_monitor()).map(|t| t.id).collect();
        for id in ids {
            let visible = Some(id) == active && self.monitor_visible();
            let Some(m) = self.monitor_mut(id) else { continue };
            // Another tab became the active one (not a dialog over this one).
            if Some(id) != active {
                m.left = true;
            }
            if !visible {
                if m.shown {
                    // Nothing is read while hidden; the time it was not looking is a gap.
                    m.shown = false;
                    m.due = None;
                    m.gap();
                }
                continue;
            }
            if !m.shown {
                m.shown = true;
                m.due = Some(now);
                // An answer that came while hidden is no base for the next rate: the time
                // nobody looked is a gap.
                m.gap();
                // Shown again after another tab, a closed connection opens as on a first show;
                // after a dialog (a disconnect it confirmed) it stays closed until `r`.
                if m.state == MonState::Closed && m.left {
                    m.state = MonState::Idle;
                }
                m.left = false;
            }
            self.monitor_step(id, now);
        }
    }

    /// The visible Monitor tab `id`: connect its profile, open its connection, or send the
    /// read that is due.
    fn monitor_step(&mut self, id: TabId, now: Instant) {
        let Some(profile) = self.tabs.get(id).and_then(|t| t.profile) else { return };
        let Some(m) = self.monitor_of(id) else { return };
        // An answer that never came: the connection is given up on.
        if let Some((_, sent)) = m.request
            && now >= sent + m.limit() + ANSWER_GRACE
        {
            let why = self.i18n.label(Label::MonitorNoAnswer).to_string();
            if let Some(m) = self.monitor_mut(id) {
                m.drop_connection();
                m.failures += 1;
                m.state = MonState::Lost(why);
                m.due = Some(now + m.delay());
            }
            return;
        }
        match m.state.clone() {
            MonState::ProfileFailed(_) | MonState::Unavailable(_) | MonState::Opening | MonState::Closed => return,
            MonState::WaitingForProfile if !self.conns.is_connected(profile) => return,
            MonState::Lost(_) if m.due.is_some_and(|d| d > now) => return,
            // Lost with its profile: the profile is not connected again by itself.
            MonState::Lost(_) if !self.conns.is_connected(profile) => {
                if let Some(m) = self.monitor_mut(id) {
                    m.state = MonState::Closed;
                }
                return;
            }
            _ => {}
        }
        if !self.conns.is_connected(profile) {
            if self.conns.state(profile) != NodeState::Connecting {
                self.connect(profile);
            }
            // The attempt may have failed at once (said by `monitor_profile_failed`).
            if let Some(m) = self.monitor_mut(id).filter(|m| !matches!(m.state, MonState::ProfileFailed(_))) {
                m.state = MonState::WaitingForProfile;
            }
            return;
        }
        let Some(m) = self.monitor_of(id) else { return };
        if m.session.is_none() {
            return self.monitor_open(id, profile);
        }
        let ready = m.format().is_some();
        let due = m.due.is_some_and(|d| d <= now);
        if ready && due && (!m.paused || m.read_once) && m.request.is_none() {
            self.monitor_read(id, now);
        }
    }

    /// Open tab `id`'s monitor connection on profile `profile` (connected).
    fn monitor_open(&mut self, id: TabId, profile: ProfileId) {
        let Some(cfg) = self.conns.get(profile).filter(|c| c.connected).and_then(|c| c.resolved.clone()) else {
            return;
        };
        let generation = self.tabs.next_generation();
        let session = self.open_session(
            &cfg,
            SessionRole::Monitor,
            EventTarget::Monitor(id),
            generation,
            datarig_core::driver::SessionContext::default(),
        );
        let why = self.i18n.label(Label::MonitorNoSession).to_string();
        let Some(m) = self.monitor_mut(id) else { return };
        m.drop_connection();
        m.generation = generation;
        match session {
            Some(s) => {
                m.session = Some(s);
                m.state = MonState::Opening;
            }
            None => m.state = MonState::Unavailable(why),
        }
    }

    /// Send tab `id`'s next read.
    fn monitor_read(&mut self, id: TabId, now: Instant) {
        let Some(m) = self.monitor_mut(id) else { return };
        let Some(session) = m.session.as_ref() else { return };
        m.seq += 1;
        let request = m.seq;
        let want = Want {
            sessions: Some(SessionsWanted { idle: m.list.show_idle }),
            server: std::mem::take(&mut m.want_server),
            limit: m.limit(),
        };
        m.asked_server = want.server;
        session.send(DbCommand::MonitorTick { request, want });
        m.request = Some((request, now));
        m.due = None;
        m.read_once = false;
    }

    /// An event of tab `id`'s monitor connection (its generation checked).
    pub(super) fn monitor_event(&mut self, id: TabId, ev: DbEvent) {
        let now = self.now();
        let text = self.event_error_text(&ev);
        let visible = !self.tabs.is_empty() && self.tab().id == id;
        // Read while the user looks at it: a use of the profile (a hidden tab's late answer is
        // not, so the idle disconnect still comes).
        if visible && let Some(p) = self.tabs.get(id).and_then(|t| t.profile) {
            self.touch_profile(p);
        }
        let error_text = match &ev {
            DbEvent::Monitor(Err(e)) | DbEvent::MonitorSnapshot { result: Err(e), .. } => Some(self.db_error_text(e)),
            _ => None,
        };
        let Some((m, i18n)) = self.monitor_words(id) else { return };
        match ev {
            DbEvent::Monitor(Ok(format)) => {
                m.state = MonState::Ready(format);
                m.failures = 0;
                m.due = Some(now);
            }
            DbEvent::Monitor(Err(_)) => {
                m.drop_connection();
                m.state = MonState::Unavailable(error_text.unwrap_or_default());
            }
            DbEvent::MonitorSnapshot { request, result } => {
                if m.request.map(|(r, _)| r) != Some(request) {
                    return;
                }
                m.request = None;
                match result {
                    Ok(snapshot) => {
                        m.failures = 0;
                        m.take(snapshot, i18n);
                    }
                    Err(_) => {
                        m.failures += 1;
                        m.error = error_text;
                        // What the server is was not read: asked again with the next read.
                        m.want_server |= m.asked_server;
                        // The next rates are not worked out across the failed read's time.
                        m.gap();
                    }
                }
                m.due = Some(now + m.delay());
            }
            DbEvent::ConnectFailed { .. } | DbEvent::Lost { .. } => {
                m.drop_connection();
                m.failures += 1;
                m.state = MonState::Lost(text);
                m.due = Some(now + m.delay());
            }
            _ => {}
        }
    }

    /// Profile `id` connected: its Monitor tab on screen opens its connection.
    pub(super) fn monitor_connected(&mut self, id: ProfileId) {
        for t in self.tabs.iter_mut().filter(|t| t.profile == Some(id)) {
            if let Some(m) = t.doc.monitor.as_mut()
                && matches!(m.state, MonState::WaitingForProfile | MonState::ProfileFailed(_) | MonState::Closed)
            {
                m.state = MonState::Idle;
            }
        }
        self.monitor_sync();
    }

    /// Profile `id`'s attempt to connect failed (`why`): its Monitor tabs wait for `r`.
    pub(super) fn monitor_profile_failed(&mut self, id: ProfileId, why: &str) {
        for t in self.tabs.iter_mut().filter(|t| t.profile == Some(id)) {
            if let Some(m) = t.doc.monitor.as_mut()
                && matches!(m.state, MonState::WaitingForProfile | MonState::Idle)
            {
                m.state = MonState::ProfileFailed(why.to_string());
            }
        }
    }

    /// Close the monitor connections of the tabs `which` takes (what they read stays).
    pub(super) fn close_monitors(&mut self, which: &impl Fn(&Tab) -> bool) {
        for t in self.tabs.iter_mut().filter(|t| which(t)) {
            if let Some(m) = t.doc.monitor.as_mut() {
                m.drop_connection();
                if !matches!(m.state, MonState::ProfileFailed(_)) {
                    m.state = MonState::Closed;
                }
            }
        }
    }

    /// When the visible Monitor tab next wants the event loop: its next read, or the time its
    /// read in flight is given up on. `None`: nothing waits (paused, hidden, no connection).
    pub fn monitor_due(&self) -> Option<Instant> {
        if !self.monitor_shown() || !self.monitor_visible() {
            return None;
        }
        let m = self.monitor_of(self.tab().id)?;
        if let Some((_, sent)) = m.request {
            return Some(sent + m.limit() + ANSWER_GRACE);
        }
        match m.state {
            MonState::Ready(_) if !m.paused || m.read_once => m.due,
            MonState::Lost(_) => m.due,
            _ => None,
        }
    }

    /// A Monitor tab action.
    pub(super) fn monitor_action(&mut self, a: MonitorAction) {
        if a == MonitorAction::Open {
            return match self.monitor_profile() {
                Some(id) => self.open_monitor(id),
                None => self.flash(Notice::new(Label::MonitorNoProfile, Level::Info)),
            };
        }
        if !self.monitor_shown() {
            return;
        }
        let id = self.tab().id;
        let page = (self.monitor_of(id).map_or(0, |m| m.hits.rows.height) as isize).max(1);
        let now = self.now();
        let mut say = None;
        let Some((m, i18n)) = self.monitor_words(id) else { return };
        match a {
            MonitorAction::Open => {}
            MonitorAction::View(v) => {
                m.view = v;
                m.list.details = false;
            }
            MonitorAction::NextView => {
                let i = MonitorView::ALL.iter().position(|v| *v == m.view).unwrap_or(0);
                m.view = MonitorView::ALL[(i + 1) % MonitorView::ALL.len()];
                m.list.details = false;
            }
            MonitorAction::Up if m.list.details => m.list.details_top = m.list.details_top.saturating_sub(1),
            MonitorAction::Down if m.list.details => m.list.details_top += 1,
            MonitorAction::Up => m.move_cursor(-1),
            MonitorAction::Down => m.move_cursor(1),
            MonitorAction::PageUp => m.move_cursor(-page),
            MonitorAction::PageDown => m.move_cursor(page),
            MonitorAction::First => m.move_cursor(isize::MIN / 2),
            MonitorAction::Last => m.move_cursor(isize::MAX / 2),
            MonitorAction::Pause => {
                m.paused = !m.paused;
                // The time paused is a gap in the charts.
                m.gap();
                m.read_once = false;
                if !m.paused {
                    m.due = Some(now);
                }
            }
            MonitorAction::ReadNow => {
                m.want_server = true;
                match &m.state {
                    // Tried again from the start.
                    MonState::ProfileFailed(_) | MonState::Unavailable(_) | MonState::Lost(_) | MonState::Closed => {
                        m.drop_connection();
                        m.state = MonState::Idle;
                        m.failures = 0;
                    }
                    _ => {}
                }
                m.due = Some(now);
                // A paused tab reads once (once its connection is ready, when it opens again
                // first) and stays paused; the read joins no line.
                if m.paused {
                    m.gap();
                    m.read_once = true;
                }
            }
            MonitorAction::Faster | MonitorAction::Slower => {
                m.interval = m.interval.step(a == MonitorAction::Slower);
                let next = m.interval.as_str().to_string();
                if m.request.is_none() && m.due.is_some() {
                    m.due = Some(now + m.delay());
                }
                say = Some(Notice::new(Msg::MonitorInterval { interval: next }, Level::Info));
            }
            MonitorAction::ShowIdle => {
                m.list.show_idle = !m.list.show_idle;
                say = Some(Notice::new(
                    if m.list.show_idle { Label::MonitorIdleShown } else { Label::MonitorIdleHidden },
                    Level::Info,
                ));
                if m.request.is_none() && m.format().is_some() {
                    m.due = Some(now);
                }
            }
            MonitorAction::Sort | MonitorAction::SortReverse => {
                if a == MonitorAction::Sort {
                    let i = SortKey::ALL.iter().position(|k| *k == m.list.sort).unwrap_or(0);
                    m.list.sort = SortKey::ALL[(i + 1) % SortKey::ALL.len()];
                    m.list.reversed = false;
                } else {
                    m.list.reversed = !m.list.reversed;
                }
                m.refresh_order(i18n);
            }
            MonitorAction::Filter => {
                m.view = MonitorView::Sessions;
                m.list.details = false;
                m.list.filtering = true;
            }
            MonitorAction::Details => {
                if m.selected_row().is_some() || m.list.details {
                    m.list.details = !m.list.details;
                    m.list.details_top = 0;
                }
            }
            MonitorAction::Back => {
                if m.list.filtering {
                    m.list.filtering = false;
                } else if m.list.details {
                    m.list.details = false;
                } else if !m.list.filter.text().is_empty() {
                    m.list.filter = TextInput::default();
                    m.refresh_order(i18n);
                }
            }
            MonitorAction::OpenConsole => return self.monitor_to_console(),
            MonitorAction::CopyQuery => return self.monitor_copy_query(),
        }
        if let Some(n) = say {
            self.flash(n);
        }
        self.monitor_sync();
    }

    /// A key the Monitor tab's filter takes as typing.
    pub(super) fn monitor_filter_key(&mut self, key: KeyEvent) {
        if !self.monitor_shown() {
            return;
        }
        let id = self.tab().id;
        let changed = self.monitor_mut(id).map(|m| m.list.filter.handle_key(&key));
        if changed == Some(crate::widgets::text_input::InputResult::Changed) {
            self.monitor_filter_changed();
        }
    }

    /// A paste into the Monitor tab's filter: its lines as one line.
    pub(super) fn monitor_filter_paste(&mut self, text: &str) {
        let id = self.tab().id;
        let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if let Some(m) = self.monitor_mut(id) {
            m.list.filter.insert_str(&line);
        }
        self.monitor_filter_changed();
    }

    /// The text the filter took: the list follows it.
    pub(super) fn monitor_filter_changed(&mut self) {
        if !self.monitor_shown() {
            return;
        }
        let id = self.tab().id;
        if let Some((m, i18n)) = self.monitor_words(id) {
            m.list.cursor = 0;
            m.list.top = 0;
            m.list.selected = None;
            m.refresh_order(i18n);
        }
    }

    /// The selected session's statement, as the list holds it: `None` when there is none.
    fn monitor_query(&self) -> Option<(String, Option<String>)> {
        let r = self.monitor_of(self.tab().id)?.selected_row()?;
        let q = r.query.as_ref().filter(|q| !q.text.trim().is_empty())?;
        Some((q.text.clone(), r.database.clone()))
    }

    /// `o`: the selected session's statement in a new console of the profile, in that
    /// session's database, never run; the console's first note says what is not known of the
    /// other session (its search path and settings).
    fn monitor_to_console(&mut self) {
        let Some((text, database)) = self.monitor_query() else {
            return self.flash(Notice::new(Label::MonitorNoQuery, Level::Info));
        };
        let Some(id) = self.tab().profile else { return };
        let shortened = self.monitor_of(self.tab().id).and_then(|m| m.selected_row()).and_then(|r| r.query.as_ref());
        let shortened = shortened.is_some_and(|q| q.shortened || q.maybe_cut_by_server);
        use datarig_core::sql::dialect::{Dialect, Language};
        let mysql = matches!(self.profile_language(Some(id)), Language::Sql(Dialect::MySql(_)));
        let context = match mysql {
            true => self.session_context(id, None, database.clone()),
            false => self.session_context(id, database.clone(), None),
        };
        self.console_with(&text, Some(id), context);
        let database = database.unwrap_or_default();
        let note = match shortened {
            true => Msg::MonitorOpenedCut { database },
            false => Msg::MonitorOpened { database },
        };
        self.flash(Notice::new(note, Level::Info));
    }

    /// `y`: the selected session's statement to the clipboard (the copy settings apply).
    fn monitor_copy_query(&mut self) {
        let Some((text, _)) = self.monitor_query() else {
            return self.flash(Notice::new(Label::MonitorNoQuery, Level::Info));
        };
        let done = match self.deliver(&text) {
            Ok(m) => Notice::new(Msg::MonitorCopied { method: self.method_text(m) }, Level::Success),
            Err(msg) => Notice::new(msg, Level::Error),
        };
        self.show_status(done.clone());
        self.flash(done);
    }

    /// A click in the Monitor tab at (`x`, `y`): a view, a column to sort by, a row.
    pub(super) fn monitor_click(&mut self, x: u16, y: u16) -> bool {
        if !self.monitor_shown() {
            return false;
        }
        let id = self.tab().id;
        let Some((m, i18n)) = self.monitor_words(id) else { return false };
        let inside = |r: &Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        if let Some(v) = m.hits.views.iter().find(|(r, _)| inside(r)).map(|(_, v)| *v) {
            m.view = v;
            m.list.details = false;
            return true;
        }
        if let Some(k) = m.hits.headers.iter().find(|(r, _)| inside(r)).map(|(_, k)| *k) {
            if m.list.sort == k {
                m.list.reversed = !m.list.reversed;
            } else {
                m.list.sort = k;
                m.list.reversed = false;
            }
            m.refresh_order(i18n);
            return true;
        }
        if inside(&m.hits.rows) {
            let n = (y - m.hits.rows.y) as usize;
            match m.hits.rows_view {
                Some(MonitorView::Sessions) => m.select(m.list.top + n),
                Some(MonitorView::Overview) => {
                    // A row of the longest running: that session in the list.
                    if let Some(&i) = m.hits.top_rows.get(n) {
                        let key = m.snapshot.as_ref().and_then(|s| s.sessions.as_ref()).map(|l| l.rows[i].key);
                        m.list.selected = key;
                        m.list.filter = TextInput::default();
                        m.view = MonitorView::Sessions;
                        m.refresh_order(i18n);
                    }
                }
                None => {}
            }
            return true;
        }
        true
    }

    /// The wheel over the Monitor tab: the list (or the details) by three rows.
    pub(super) fn monitor_wheel(&mut self, down: bool) {
        let a = if down { MonitorAction::Down } else { MonitorAction::Up };
        for _ in 0..3 {
            self.monitor_action(a);
        }
    }
}
